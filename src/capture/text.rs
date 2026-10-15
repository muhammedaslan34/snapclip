//! Text measurement, line wrapping and right-to-left handling shared by the screen
//! and the exported image, so a text box looks and wraps the same in both.

use std::borrow::Cow;
use std::sync::OnceLock;

use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use egui::Vec2;

/// Same font egui uses for proportional text, so the export matches the screen.
pub fn font() -> Option<&'static FontRef<'static>> {
    static FONT: OnceLock<Option<FontRef<'static>>> = OnceLock::new();
    FONT.get_or_init(|| FontRef::try_from_slice(epaint_default_fonts::UBUNTU_LIGHT).ok())
        .as_ref()
}

/// A system font with Arabic letters (including their joined forms), which egui's
/// built-in fonts lack. Also registered with egui, see `theme::apply`.
pub fn fallback_font_data() -> Option<&'static [u8]> {
    static DATA: OnceLock<Option<&'static [u8]>> = OnceLock::new();
    *DATA.get_or_init(|| {
        let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
        let candidates = [
            format!("{windows}\\Fonts\\segoeui.ttf"),
            format!("{windows}\\Fonts\\tahoma.ttf"),
            format!("{windows}\\Fonts\\arial.ttf"),
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf".into(),
            "/usr/share/fonts/TTF/DejaVuSans.ttf".into(),
            "/usr/share/fonts/dejavu/DejaVuSans.ttf".into(),
            "/usr/share/fonts/truetype/noto/NotoSansArabic-Regular.ttf".into(),
        ];
        candidates.iter().find_map(|path| {
            let bytes = std::fs::read(path).ok()?;
            // Needs the joined letter forms, not just the basic letters.
            let font = FontRef::try_from_slice(&bytes).ok()?;
            if font.glyph_id('\u{FE91}').0 == 0 {
                return None;
            }
            log::debug!("fallback font: {path}");
            Some(&*Box::leak(bytes.into_boxed_slice()))
        })
    })
}

fn fallback_font() -> Option<&'static FontRef<'static>> {
    static FONT: OnceLock<Option<FontRef<'static>>> = OnceLock::new();
    FONT.get_or_init(|| FontRef::try_from_slice(fallback_font_data()?).ok())
        .as_ref()
}

/// The font that draws `c`: the main font, or the fallback for letters it lacks.
pub fn font_for(c: char) -> Option<&'static FontRef<'static>> {
    let main = font()?;
    if main.glyph_id(c).0 != 0 || c.is_whitespace() {
        return Some(main);
    }
    Some(fallback_font().filter(|f| f.glyph_id(c).0 != 0).unwrap_or(main))
}

/// ab_glyph scales by `ascent - descent`; egui sizes fonts by em. Convert em → ab_glyph scale.
pub fn px_scale(font: &FontRef<'_>, em_size: f32) -> PxScale {
    let upem = font.units_per_em().unwrap_or(1000.0);
    PxScale::from(em_size * font.height_unscaled() / upem)
}

/// Height of one line of glyphs, without the gap between lines.
pub fn glyph_height(size: f32) -> f32 {
    font().map_or(size, |f| f.as_scaled(px_scale(f, size)).height())
}

/// Distance between the tops of two consecutive lines.
pub fn line_height(size: f32) -> f32 {
    font().map_or(size * 1.2, |f| {
        let scaled = f.as_scaled(px_scale(f, size));
        scaled.height() + scaled.line_gap()
    })
}

/// Width of a single line of text as given (already in display order), including kerning.
pub fn width(text: &str, size: f32) -> f32 {
    if font().is_none() {
        return text.chars().count() as f32 * size * 0.5;
    }
    let mut prev: Option<(&FontRef<'_>, ab_glyph::GlyphId)> = None;
    let mut x = 0.0;
    for ch in text.chars() {
        let Some(font) = font_for(ch) else { continue };
        let scaled = font.as_scaled(px_scale(font, size));
        let id = scaled.glyph_id(ch);
        if let Some((prev_font, prev_id)) = prev {
            if std::ptr::eq(prev_font, font) {
                x += scaled.kern(prev_id, id);
            }
        }
        x += scaled.h_advance(id);
        prev = Some((font, id));
    }
    x
}

fn is_rtl(c: char) -> bool {
    matches!(c, '\u{0590}'..='\u{08FF}' | '\u{FB1D}'..='\u{FDFF}' | '\u{FE70}'..='\u{FEFF}')
}

/// One line in display order: Arabic letters joined into their connected forms and
/// right-to-left runs reversed. egui does neither, so text is drawn from this.
pub fn visual(line: &str) -> Cow<'_, str> {
    if !line.chars().any(is_rtl) {
        return Cow::Borrowed(line);
    }
    let joined = ar_reshaper::ArabicReshaper::default().reshape(line);
    Cow::Owned(reorder(&joined))
}

/// Like [`visual`], but keeps one output character per input character (no
/// ligatures), so a text field's cursor positions still line up while typing.
pub fn visual_same_length(line: &str) -> String {
    if !line.chars().any(is_rtl) {
        return line.to_owned();
    }
    let config = ar_reshaper::ReshaperConfig {
        support_ligatures: false,
        delete_harakat: false,
        support_zwj: false,
        ..Default::default()
    };
    let joined = ar_reshaper::ArabicReshaper::new(config).reshape(line);
    let shown = reorder(&joined);
    if shown.chars().count() == line.chars().count() { shown } else { line.to_owned() }
}

fn reorder(text: &str) -> String {
    let info = unicode_bidi::ParagraphBidiInfo::new(text, None);
    info.reorder_line(0..text.len()).into_owned()
}

fn line_width(line: &str, size: f32) -> f32 {
    width(&visual(line), size)
}

/// Splits `text` into lines (in typing order; draw each through [`visual`]): at
/// newlines, and between words when `wrap` (a box width) is set. A word wider than
/// the box is broken between characters.
pub fn lines(text: &str, size: f32, wrap: Option<f32>) -> Vec<String> {
    let mut out = Vec::new();
    for paragraph in text.split('\n') {
        let Some(max) = wrap else {
            out.push(paragraph.to_owned());
            continue;
        };
        let mut line = String::new();
        for word in paragraph.split_inclusive(' ') {
            let candidate = format!("{line}{word}");
            if line.is_empty() || line_width(candidate.trim_end(), size) <= max {
                line = candidate;
            } else {
                out.push(line.trim_end().to_owned());
                line = word.to_owned();
            }
            while line_width(line.trim_end(), size) > max && line.trim_end().chars().count() > 1 {
                let mut head = String::new();
                let mut rest = line.len();
                for (i, c) in line.char_indices() {
                    head.push(c);
                    if head.chars().count() > 1 && line_width(&head, size) > max {
                        head.pop();
                        rest = i;
                        break;
                    }
                }
                out.push(head);
                line = line[rest..].trim_start().to_owned();
            }
        }
        out.push(line.trim_end().to_owned());
    }
    out
}

/// Size of the laid-out text. A text box is always as wide as its box.
pub fn block_size(text: &str, size: f32, wrap: Option<f32>) -> Vec2 {
    let lines = lines(text, size, wrap);
    let widest = lines.iter().map(|l| line_width(l, size)).fold(0.0, f32::max);
    let height = (lines.len().max(1) - 1) as f32 * line_height(size) + glyph_height(size);
    Vec2::new(wrap.unwrap_or(widest), height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_between_words_and_keeps_newlines() {
        let one_word = width("hello", 20.0);
        let wrapped = lines("hello world foo\nbar", 20.0, Some(one_word * 1.5));
        assert_eq!(wrapped, ["hello", "world", "foo", "bar"]);
        assert_eq!(lines("hello world", 20.0, None), ["hello world"]);
    }

    #[test]
    fn breaks_words_wider_than_the_box() {
        let wrapped = lines("abcdefghij", 20.0, Some(width("abcd", 20.0)));
        assert!(wrapped.len() >= 3, "{wrapped:?}");
        assert_eq!(wrapped.concat(), "abcdefghij");
        assert!(wrapped.iter().all(|l| width(l, 20.0) <= width("abcd", 20.0) + 0.5));
    }

    #[test]
    fn arabic_is_joined_and_right_to_left() {
        // "بب": the first beh (initial form) is drawn on the right, so it comes last.
        assert_eq!(visual("\u{0628}\u{0628}"), "\u{FE90}\u{FE91}");
        assert_eq!(visual("abc"), "abc");
        // Mixed text keeps the Latin word on the left.
        assert!(visual("hi \u{0628}\u{0628}").starts_with("hi "));
        assert_eq!(visual_same_length("\u{0644}\u{0627}").chars().count(), 2, "no lam-alef ligature");
    }

    #[test]
    fn arabic_letters_come_from_a_font_that_has_them() {
        if fallback_font_data().is_none() {
            return; // No Arabic system font on this machine.
        }
        for c in visual("\u{0645}\u{0631}\u{062D}\u{0628}\u{0627}").chars() {
            assert_ne!(font_for(c).unwrap().glyph_id(c).0, 0, "no glyph for U+{:04X}", c as u32);
        }
    }

    #[test]
    fn a_text_box_is_as_wide_as_its_box() {
        assert_eq!(block_size("hi", 20.0, Some(300.0)).x, 300.0);
        let two_lines = block_size("a\nb", 20.0, None).y;
        assert!(two_lines > block_size("a", 20.0, None).y * 1.8);
    }
}
