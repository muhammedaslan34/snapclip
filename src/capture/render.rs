//! Renders the selection plus annotations into the final image (CPU, tiny-skia).

use ab_glyph::{Font, FontRef, ScaleFont};
use egui::{Color32, Pos2, Rect, Vec2};
use image::RgbaImage;
use tiny_skia::{
    FillRule, LineCap, LineJoin, Mask, Paint, Path, PathBuilder, Pixmap, Stroke, Transform,
};

use super::annot::{
    Annot, Effects, SelShape, Selection, Shape, arrow_geometry, contrast_text, marker_color, marker_width,
    step_radius,
};
use super::text;

pub fn export(
    shot: &RgbaImage,
    effects: Effects<&RgbaImage>,
    sel: &Selection,
    annots: &[Annot],
) -> Option<RgbaImage> {
    let (iw, ih) = shot.dimensions();
    let x0 = sel.rect.min.x.floor().clamp(0.0, iw as f32) as u32;
    let y0 = sel.rect.min.y.floor().clamp(0.0, ih as f32) as u32;
    let x1 = sel.rect.max.x.ceil().clamp(0.0, iw as f32) as u32;
    let y1 = sel.rect.max.y.ceil().clamp(0.0, ih as f32) as u32;
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let (w, h) = (x1 - x0, y1 - y0);
    let mut pixmap = Pixmap::new(w, h)?;
    copy_region(&mut pixmap, shot, x0, y0, Rect::from_min_max(Pos2::ZERO, Pos2::new(w as f32, h as f32)));

    let offset = Vec2::new(x0 as f32, y0 as f32);
    let ts = Transform::from_translate(-offset.x, -offset.y);
    let font = text::font();
    for annot in annots {
        draw(&mut pixmap, annot, ts, offset, effects, font);
    }

    if sel.shape != SelShape::Rect {
        let path = polygon_path(&sel.polygon())?;
        let mut mask = Mask::new(w, h)?;
        mask.fill_path(&path, FillRule::Winding, true, ts);
        pixmap.apply_mask(&mask);
    }

    let mut out = RgbaImage::new(w, h);
    for (dst, src) in out.pixels_mut().zip(pixmap.pixels()) {
        let c = src.demultiply();
        *dst = image::Rgba([c.red(), c.green(), c.blue(), c.alpha()]);
    }
    Some(out)
}

/// Copies `area` (in pixmap coordinates) from `src`, where the pixmap starts at `(x0, y0)` in `src`.
fn copy_region(pixmap: &mut Pixmap, src: &RgbaImage, x0: u32, y0: u32, area: Rect) {
    let (pw, ph) = (pixmap.width(), pixmap.height());
    let (sw, sh) = src.dimensions();
    let ax0 = area.min.x.floor().max(0.0) as u32;
    let ay0 = area.min.y.floor().max(0.0) as u32;
    let ax1 = (area.max.x.ceil().max(0.0) as u32).min(pw);
    let ay1 = (area.max.y.ceil().max(0.0) as u32).min(ph);
    let data = pixmap.data_mut();
    for y in ay0..ay1 {
        for x in ax0..ax1 {
            let (sx, sy) = (x0 + x, y0 + y);
            if sx >= sw || sy >= sh {
                continue;
            }
            let [r, g, b, a] = src.get_pixel(sx, sy).0;
            let i = ((y * pw + x) * 4) as usize;
            let pm = |c: u8| ((c as u16 * a as u16 + 127) / 255) as u8;
            data[i..i + 4].copy_from_slice(&[pm(r), pm(g), pm(b), a]);
        }
    }
}

fn paint_for(color: Color32) -> Paint<'static> {
    let [r, g, b, a] = color.to_srgba_unmultiplied();
    let mut paint = Paint::default();
    paint.set_color_rgba8(r, g, b, a);
    paint.anti_alias = true;
    paint
}

fn stroke(width: f32) -> Stroke {
    Stroke {
        width,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Default::default()
    }
}

fn polyline(points: &[Pos2]) -> Option<Path> {
    let (first, rest) = points.split_first()?;
    let mut pb = PathBuilder::new();
    pb.move_to(first.x, first.y);
    for p in rest {
        pb.line_to(p.x, p.y);
    }
    pb.finish()
}

fn polygon_path(points: &[Pos2]) -> Option<Path> {
    let (first, rest) = points.split_first()?;
    let mut pb = PathBuilder::new();
    pb.move_to(first.x, first.y);
    for p in rest {
        pb.line_to(p.x, p.y);
    }
    pb.close();
    pb.finish()
}

fn sk_rect(r: Rect) -> Option<tiny_skia::Rect> {
    tiny_skia::Rect::from_ltrb(r.min.x, r.min.y, r.max.x, r.max.y)
}

fn draw(
    pm: &mut Pixmap,
    annot: &Annot,
    ts: Transform,
    offset: Vec2,
    effects: Effects<&RgbaImage>,
    font: Option<&FontRef<'_>>,
) {
    let paint = paint_for(annot.color);
    match &annot.shape {
        Shape::Pen(pts) => {
            if let Some(path) = polyline(pts) {
                pm.stroke_path(&path, &paint, &stroke(annot.width), ts, None);
            }
        }
        Shape::Marker(pts) => {
            if let Some(path) = polyline(pts) {
                let paint = paint_for(marker_color(annot.color));
                pm.stroke_path(&path, &paint, &stroke(marker_width(annot.width)), ts, None);
            }
        }
        Shape::Line(a, b) => {
            if let Some(path) = polyline(&[*a, *b]) {
                pm.stroke_path(&path, &paint, &stroke(annot.width), ts, None);
            }
        }
        Shape::Arrow(a, b) => {
            if let Some((shaft_end, head)) = arrow_geometry(*a, *b, annot.width) {
                if let Some(path) = polyline(&[*a, shaft_end]) {
                    pm.stroke_path(&path, &paint, &stroke(annot.width), ts, None);
                }
                if let Some(path) = polygon_path(&head) {
                    pm.fill_path(&path, &paint, FillRule::Winding, ts, None);
                }
            }
        }
        Shape::Rect(r) => {
            if let Some(rect) = sk_rect(*r) {
                let path = PathBuilder::from_rect(rect);
                if annot.filled {
                    pm.fill_path(&path, &paint, FillRule::Winding, ts, None);
                } else {
                    let s = Stroke {
                        line_join: LineJoin::Miter,
                        ..stroke(annot.width)
                    };
                    pm.stroke_path(&path, &paint, &s, ts, None);
                }
            }
        }
        Shape::Ellipse(r) => {
            if let Some(path) = sk_rect(*r).and_then(PathBuilder::from_oval) {
                if annot.filled {
                    pm.fill_path(&path, &paint, FillRule::Winding, ts, None);
                } else {
                    pm.stroke_path(&path, &paint, &stroke(annot.width), ts, None);
                }
            }
        }
        Shape::Text { pos, text, box_width } => {
            if let Some(font) = font {
                let lines = text::lines(text, annot.font_size, *box_width);
                draw_text(pm, font, &lines, *pos - offset, annot.font_size, annot.color);
            }
        }
        Shape::Step { center, number } => {
            let r = step_radius(annot.width);
            if let Some(path) = PathBuilder::from_circle(center.x, center.y, r) {
                pm.fill_path(&path, &paint, FillRule::Winding, ts, None);
            }
            if let Some(font) = font {
                let label = number.to_string();
                let size = r * 1.15;
                let (tw, th) = (text::width(&label, size), text::glyph_height(size));
                let top_left = *center - offset - Vec2::new(tw / 2.0, th / 2.0);
                draw_text(pm, font, &[label], top_left, size, contrast_text(annot.color));
            }
        }
        Shape::Pixelate(r) | Shape::Blur(r) => {
            let source = match annot.shape {
                Shape::Blur(_) => effects.blurred,
                _ => effects.pixelated,
            };
            if let Some(source) = source {
                copy_region(pm, source, offset.x as u32, offset.y as u32, r.translate(-offset));
            }
        }
    }
}

/// Draws `lines` (in typing order) top-down from `origin`, each in display order.
/// `main` sets the baseline; letters it lacks (e.g. Arabic) come from the fallback font.
fn draw_text(pm: &mut Pixmap, main: &FontRef<'_>, lines: &[String], origin: Pos2, size: f32, color: Color32) {
    let ascent = main.as_scaled(text::px_scale(main, size)).ascent();
    let line_height = text::line_height(size);
    let [cr, cg, cb, ca] = color.to_srgba_unmultiplied();
    let (pw, ph) = (pm.width() as i32, pm.height() as i32);
    let data = pm.data_mut();

    for (line_no, line) in lines.iter().enumerate() {
        let baseline = origin.y + ascent + line_no as f32 * line_height;
        let mut x = origin.x;
        let mut prev: Option<(&FontRef<'_>, ab_glyph::GlyphId)> = None;
        for ch in text::visual(line).chars() {
            let Some(font) = text::font_for(ch) else { continue };
            let scale = text::px_scale(font, size);
            let scaled = font.as_scaled(scale);
            let id = scaled.glyph_id(ch);
            if let Some((prev_font, prev_id)) = prev {
                if std::ptr::eq(prev_font, font) {
                    x += scaled.kern(prev_id, id);
                }
            }
            prev = Some((font, id));
            let glyph = id.with_scale_and_position(scale, ab_glyph::point(x, baseline));
            x += scaled.h_advance(id);
            let Some(outline) = font.outline_glyph(glyph) else {
                continue;
            };
            let bounds = outline.px_bounds();
            outline.draw(|gx, gy, coverage| {
                let px = bounds.min.x as i32 + gx as i32;
                let py = bounds.min.y as i32 + gy as i32;
                if px < 0 || py < 0 || px >= pw || py >= ph {
                    return;
                }
                let a = coverage.clamp(0.0, 1.0) * ca as f32 / 255.0;
                let i = ((py * pw + px) * 4) as usize;
                for (k, c) in [cr, cg, cb, 255].into_iter().enumerate() {
                    let dst = data[i + k] as f32;
                    data[i + k] = (c as f32 * a + dst * (1.0 - a)).round() as u8;
                }
            });
        }
    }
}

/// A copy of `img` made of `block`×`block` single-color squares.
pub fn pixelate(img: &RgbaImage, block: u32) -> RgbaImage {
    let (w, h) = img.dimensions();
    let mut out = RgbaImage::new(w, h);
    for by in (0..h).step_by(block as usize) {
        for bx in (0..w).step_by(block as usize) {
            let (bw, bh) = (block.min(w - bx), block.min(h - by));
            let mut sum = [0u32; 4];
            for y in by..by + bh {
                for x in bx..bx + bw {
                    let p = img.get_pixel(x, y).0;
                    for k in 0..4 {
                        sum[k] += p[k] as u32;
                    }
                }
            }
            let n = bw * bh;
            let avg = image::Rgba(sum.map(|s| (s / n) as u8));
            for y in by..by + bh {
                for x in bx..bx + bw {
                    out.put_pixel(x, y, avg);
                }
            }
        }
    }
    out
}

/// A blurred copy of `img`, strong enough to make text unreadable.
pub fn blur(img: &RgbaImage) -> RgbaImage {
    let sigma = (img.width().max(img.height()) as f32 / 250.0).max(6.0);
    image::imageops::fast_blur(img, sigma)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NO_EFFECTS: Effects<&RgbaImage> = Effects { pixelated: None, blurred: None };

    fn shot() -> RgbaImage {
        RgbaImage::from_pixel(100, 80, image::Rgba([255, 255, 255, 255]))
    }

    fn annot(shape: Shape) -> Annot {
        Annot {
            shape,
            color: Color32::from_rgb(255, 0, 0),
            width: 4.0,
            filled: false,
            font_size: 20.0,
        }
    }

    #[test]
    fn crops_to_selection_and_draws_annotations() {
        let sel = Selection::rect(Rect::from_min_max(Pos2::new(10.0, 10.0), Pos2::new(60.0, 50.0)), SelShape::Rect);
        let annots = [annot(Shape::Line(Pos2::new(10.0, 30.0), Pos2::new(60.0, 30.0)))];
        let out = export(&shot(), NO_EFFECTS, &sel, &annots).unwrap();
        assert_eq!(out.dimensions(), (50, 40));
        assert_eq!(out.get_pixel(25, 20).0, [255, 0, 0, 255]);
        assert_eq!(out.get_pixel(25, 5).0, [255, 255, 255, 255]);
    }

    #[test]
    fn ellipse_selection_is_transparent_outside() {
        let sel = Selection::rect(Rect::from_min_max(Pos2::ZERO, Pos2::new(100.0, 80.0)), SelShape::Ellipse);
        let out = export(&shot(), NO_EFFECTS, &sel, &[]).unwrap();
        assert_eq!(out.get_pixel(0, 0).0[3], 0);
        assert_eq!(out.get_pixel(50, 40).0[3], 255);
    }

    #[test]
    fn text_and_steps_render_pixels() {
        let sel = Selection::rect(Rect::from_min_max(Pos2::ZERO, Pos2::new(100.0, 80.0)), SelShape::Rect);
        let annots = [
            annot(Shape::Text { pos: Pos2::new(5.0, 5.0), text: "Hi".into(), box_width: None }),
            annot(Shape::Step { center: Pos2::new(70.0, 50.0), number: 3 }),
        ];
        let out = export(&shot(), NO_EFFECTS, &sel, &annots).unwrap();
        let red = |x0: u32, y0: u32, x1: u32, y1: u32| {
            (x0..x1).flat_map(|x| (y0..y1).map(move |y| (x, y))).filter(|&(x, y)| {
                let p = out.get_pixel(x, y).0;
                p[0] > 200 && p[1] < 120
            }).count()
        };
        assert!(red(0, 0, 40, 35) > 20, "text should draw red pixels");
        assert!(red(55, 35, 85, 65) > 100, "step marker should draw a red circle");
    }

    #[test]
    fn blur_reveals_blurred_copy_inside_its_rect() {
        let img = RgbaImage::from_fn(100, 80, |x, _| {
            image::Rgba(if x % 2 == 0 { [0, 0, 0, 255] } else { [255, 255, 255, 255] })
        });
        let blurred = blur(&img);
        let sel = Selection::rect(Rect::from_min_max(Pos2::ZERO, Pos2::new(100.0, 80.0)), SelShape::Rect);
        let annots = [annot(Shape::Blur(Rect::from_min_max(Pos2::new(20.0, 20.0), Pos2::new(60.0, 60.0))))];
        let effects = Effects { pixelated: None, blurred: Some(&blurred) };
        let out = export(&img, effects, &sel, &annots).unwrap();
        let gray = out.get_pixel(40, 40).0[0];
        assert!((90..=165).contains(&gray), "stripes inside should average out, got {gray}");
        assert_eq!(out.get_pixel(10, 10).0, img.get_pixel(10, 10).0, "outside stays sharp");
    }

    #[test]
    fn text_box_wraps_in_the_export() {
        let sel = Selection::rect(Rect::from_min_max(Pos2::ZERO, Pos2::new(100.0, 80.0)), SelShape::Rect);
        let words = "aa aa aa".to_owned();
        let box_width = text::width("aa", 20.0) * 1.5;
        let annots = [annot(Shape::Text { pos: Pos2::new(2.0, 2.0), text: words, box_width: Some(box_width) })];
        let out = export(&shot(), NO_EFFECTS, &sel, &annots).unwrap();
        let red_rows: Vec<u32> = (0..80).filter(|&y| (0..100).any(|x| out.get_pixel(x, y).0[1] < 120)).collect();
        let span = (red_rows.last().unwrap() - red_rows[0]) as f32;
        assert!(span > text::line_height(20.0) * 2.0, "three wrapped lines, got a {span} px tall block");
        assert!((box_width.ceil() as u32 + 4..100).all(|x| (0..80).all(|y| out.get_pixel(x, y).0[1] > 200)));
    }

    #[test]
    fn pixelate_averages_blocks() {
        let img = RgbaImage::from_fn(4, 4, |x, _| image::Rgba([if x < 2 { 0 } else { 200 }, 0, 0, 255]));
        let out = pixelate(&img, 4);
        assert!(out.pixels().all(|p| p.0 == [100, 0, 0, 255]));
    }
}
