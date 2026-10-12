//! The app icon, drawn in code so no image assets are needed.

use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Rect, Stroke, Transform};

/// Returns straight (non-premultiplied) RGBA pixels of a square icon.
pub fn rgba(size: u32) -> Vec<u8> {
    let mut pixmap = Pixmap::new(size, size).expect("icon size must be non-zero");
    let s = size as f32;

    let mut bg = Paint::default();
    bg.set_color_rgba8(37, 99, 235, 255);
    bg.anti_alias = true;
    let r = s * 0.22;
    if let Some(path) = rounded_rect(0.0, 0.0, s, s, r) {
        pixmap.fill_path(&path, &bg, FillRule::Winding, Transform::identity(), None);
    }

    // Crop-mark corners.
    let mut white = Paint::default();
    white.set_color_rgba8(255, 255, 255, 255);
    white.anti_alias = true;
    let stroke = Stroke {
        width: s * 0.08,
        line_cap: tiny_skia::LineCap::Round,
        ..Default::default()
    };
    let (a, b, len) = (s * 0.22, s * 0.78, s * 0.18);
    let mut pb = PathBuilder::new();
    for (x, y, dx, dy) in [
        (a, a, 1.0, 1.0),
        (b, a, -1.0, 1.0),
        (a, b, 1.0, -1.0),
        (b, b, -1.0, -1.0),
    ] {
        pb.move_to(x + dx * len, y);
        pb.line_to(x, y);
        pb.line_to(x, y + dy * len);
    }
    if let Some(path) = pb.finish() {
        pixmap.stroke_path(&path, &white, &stroke, Transform::identity(), None);
    }

    // Lens dot in the middle.
    if let Some(path) = PathBuilder::from_circle(s / 2.0, s / 2.0, s * 0.12) {
        pixmap.fill_path(&path, &white, FillRule::Winding, Transform::identity(), None);
    }

    pixmap
        .pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect()
}

fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    Rect::from_xywh(x, y, w, h)?;
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.quad_to(x + w, y, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.quad_to(x + w, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.quad_to(x, y + h, x, y + h - r);
    pb.line_to(x, y + r);
    pb.quad_to(x, y, x + r, y);
    pb.close();
    pb.finish()
}

pub fn tray_icon() -> Option<tray_icon::Icon> {
    tray_icon::Icon::from_rgba(rgba(32), 32, 32).ok()
}

pub fn window_icon() -> Option<winit::window::Icon> {
    winit::window::Icon::from_rgba(rgba(64), 64, 64).ok()
}
