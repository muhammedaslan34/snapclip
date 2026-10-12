//! Renders the selection plus annotations into the final image (CPU, tiny-skia).

use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use egui::{Color32, Pos2, Rect, Vec2};
use image::RgbaImage;
use tiny_skia::{
    FillRule, LineCap, LineJoin, Mask, Paint, Path, PathBuilder, Pixmap, Stroke, Transform,
};

use super::annot::{
    Annot, SelShape, Selection, Shape, arrow_geometry, contrast_text, marker_color, marker_width,
    step_radius,
};

/// Same font egui uses for proportional text, so the export matches the screen.
fn font() -> Option<FontRef<'static>> {
    FontRef::try_from_slice(epaint_default_fonts::UBUNTU_LIGHT).ok()
}

pub fn export(
    shot: &RgbaImage,
    pixelated: Option<&RgbaImage>,
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
    let font = font();
    for annot in annots {
        draw(&mut pixmap, annot, ts, offset, pixelated, font.as_ref());
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
    pixelated: Option<&RgbaImage>,
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
        Shape::Text { pos, text } => {
            if let Some(font) = font {
                draw_text(pm, font, text, *pos - offset, annot.font_size, annot.color);
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
                let (tw, th) = measure_text(font, &label, size);
                let top_left = *center - offset - Vec2::new(tw / 2.0, th / 2.0);
                draw_text(pm, font, &label, top_left, size, contrast_text(annot.color));
            }
        }
        Shape::Pixelate(r) => {
            if let Some(pixelated) = pixelated {
                copy_region(pm, pixelated, offset.x as u32, offset.y as u32, r.translate(-offset));
            }
        }
    }
}

/// ab_glyph scales by `ascent - descent`; egui sizes fonts by em. Convert em → ab_glyph scale.
fn px_scale(font: &FontRef<'_>, em_size: f32) -> PxScale {
    let upem = font.units_per_em().unwrap_or(1000.0);
    PxScale::from(em_size * font.height_unscaled() / upem)
}

fn measure_text(font: &FontRef<'_>, text: &str, size: f32) -> (f32, f32) {
    let scaled = font.as_scaled(px_scale(font, size));
    let width = text.chars().map(|c| scaled.h_advance(scaled.glyph_id(c))).sum();
    (width, scaled.height())
}

fn draw_text(pm: &mut Pixmap, font: &FontRef<'_>, text: &str, origin: Pos2, size: f32, color: Color32) {
    let scale = px_scale(font, size);
    let scaled = font.as_scaled(scale);
    let line_height = scaled.height() + scaled.line_gap();
    let [cr, cg, cb, ca] = color.to_srgba_unmultiplied();
    let (pw, ph) = (pm.width() as i32, pm.height() as i32);
    let data = pm.data_mut();

    for (line_no, line) in text.split('\n').enumerate() {
        let baseline = origin.y + scaled.ascent() + line_no as f32 * line_height;
        let mut x = origin.x;
        let mut prev = None;
        for ch in line.chars() {
            let id = scaled.glyph_id(ch);
            if let Some(prev) = prev {
                x += scaled.kern(prev, id);
            }
            prev = Some(id);
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let out = export(&shot(), None, &sel, &annots).unwrap();
        assert_eq!(out.dimensions(), (50, 40));
        assert_eq!(out.get_pixel(25, 20).0, [255, 0, 0, 255]);
        assert_eq!(out.get_pixel(25, 5).0, [255, 255, 255, 255]);
    }

    #[test]
    fn ellipse_selection_is_transparent_outside() {
        let sel = Selection::rect(Rect::from_min_max(Pos2::ZERO, Pos2::new(100.0, 80.0)), SelShape::Ellipse);
        let out = export(&shot(), None, &sel, &[]).unwrap();
        assert_eq!(out.get_pixel(0, 0).0[3], 0);
        assert_eq!(out.get_pixel(50, 40).0[3], 255);
    }

    #[test]
    fn text_and_steps_render_pixels() {
        let sel = Selection::rect(Rect::from_min_max(Pos2::ZERO, Pos2::new(100.0, 80.0)), SelShape::Rect);
        let annots = [
            annot(Shape::Text { pos: Pos2::new(5.0, 5.0), text: "Hi".into() }),
            annot(Shape::Step { center: Pos2::new(70.0, 50.0), number: 3 }),
        ];
        let out = export(&shot(), None, &sel, &annots).unwrap();
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
    fn pixelate_averages_blocks() {
        let img = RgbaImage::from_fn(4, 4, |x, _| image::Rgba([if x < 2 { 0 } else { 200 }, 0, 0, 255]));
        let out = pixelate(&img, 4);
        assert!(out.pixels().all(|p| p.0 == [100, 0, 0, 255]));
    }
}
