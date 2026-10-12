//! Selection and annotation model, plus on-screen drawing with egui.
//!
//! Everything is stored in screenshot pixel coordinates so the same data can be
//! drawn on screen (via [`Mapper`]) and rendered into the final image.

use egui::{Align2, Color32, FontId, Pos2, Rect, Shape as EShape, Stroke, StrokeKind, Vec2, pos2};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelShape {
    Rect,
    Ellipse,
    Free,
}

#[derive(Clone, Debug)]
pub struct Selection {
    pub shape: SelShape,
    pub rect: Rect,
    /// Freehand outline, normalized to `0..=1` inside `rect` so resizing scales it.
    pub outline: Vec<Pos2>,
}

impl Selection {
    pub fn rect(rect: Rect, shape: SelShape) -> Self {
        Self {
            shape,
            rect,
            outline: Vec::new(),
        }
    }

    /// Builds a freehand selection from an outline in image coordinates.
    pub fn free(points: &[Pos2]) -> Option<Self> {
        let rect = Rect::from_points(points).round_ui_px();
        if points.len() < 3 || rect.width() < 3.0 || rect.height() < 3.0 {
            return None;
        }
        let outline = points
            .iter()
            .map(|p| pos2((p.x - rect.min.x) / rect.width(), (p.y - rect.min.y) / rect.height()))
            .collect();
        Some(Self {
            shape: SelShape::Free,
            rect,
            outline,
        })
    }

    /// The outline as a polygon in image coordinates.
    pub fn polygon(&self) -> Vec<Pos2> {
        let r = self.rect;
        match self.shape {
            SelShape::Rect => vec![r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()],
            SelShape::Ellipse => {
                let n = 96;
                (0..n)
                    .map(|i| {
                        let t = i as f32 / n as f32 * std::f32::consts::TAU;
                        r.center() + Vec2::new(t.cos() * r.width() / 2.0, t.sin() * r.height() / 2.0)
                    })
                    .collect()
            }
            SelShape::Free => self
                .outline
                .iter()
                .map(|p| pos2(r.min.x + p.x * r.width(), r.min.y + p.y * r.height()))
                .collect(),
        }
    }
}

trait RoundPx {
    fn round_ui_px(self) -> Self;
}

impl RoundPx for Rect {
    fn round_ui_px(self) -> Self {
        Rect::from_min_max(self.min.round(), self.max.round())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Adjust,
    Pen,
    Marker,
    Line,
    Arrow,
    Rect,
    Ellipse,
    Text,
    Step,
    Pixelate,
}

impl Tool {
    pub const ALL: [Tool; 10] = [
        Tool::Adjust,
        Tool::Pen,
        Tool::Marker,
        Tool::Line,
        Tool::Arrow,
        Tool::Rect,
        Tool::Ellipse,
        Tool::Text,
        Tool::Step,
        Tool::Pixelate,
    ];

    pub fn icon(self) -> &'static str {
        match self {
            Tool::Adjust => "⛶",
            Tool::Pen => "✏",
            Tool::Marker => "🖊",
            Tool::Line => "📏",
            Tool::Arrow => "⬈",
            Tool::Rect => "⬜",
            Tool::Ellipse => "⭕",
            Tool::Text => "🔤",
            Tool::Step => "🔢",
            Tool::Pixelate => "🔲",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Tool::Adjust => "Move / resize selection",
            Tool::Pen => "Pen",
            Tool::Marker => "Highlighter",
            Tool::Line => "Line",
            Tool::Arrow => "Arrow",
            Tool::Rect => "Rectangle",
            Tool::Ellipse => "Ellipse",
            Tool::Text => "Text",
            Tool::Step => "Numbered step",
            Tool::Pixelate => "Pixelate / hide",
        }
    }

    pub fn act(self) -> crate::shortcuts::Act {
        use crate::shortcuts::Act;
        match self {
            Tool::Adjust => Act::ToolAdjust,
            Tool::Pen => Act::ToolPen,
            Tool::Marker => Act::ToolMarker,
            Tool::Line => Act::ToolLine,
            Tool::Arrow => Act::ToolArrow,
            Tool::Rect => Act::ToolRect,
            Tool::Ellipse => Act::ToolEllipse,
            Tool::Text => Act::ToolText,
            Tool::Step => Act::ToolStep,
            Tool::Pixelate => Act::ToolPixelate,
        }
    }

    pub fn supports_fill(self) -> bool {
        matches!(self, Tool::Rect | Tool::Ellipse)
    }
}

#[derive(Clone, Debug)]
pub enum Shape {
    Pen(Vec<Pos2>),
    Marker(Vec<Pos2>),
    Line(Pos2, Pos2),
    Arrow(Pos2, Pos2),
    Rect(Rect),
    Ellipse(Rect),
    Text { pos: Pos2, text: String },
    Step { center: Pos2, number: u32 },
    Pixelate(Rect),
}

#[derive(Clone, Debug)]
pub struct Annot {
    pub shape: Shape,
    pub color: Color32,
    pub width: f32,
    pub filled: bool,
    pub font_size: f32,
}

impl Annot {
    /// True if the annotation is too small to be intentional (e.g. a click).
    pub fn is_degenerate(&self) -> bool {
        match &self.shape {
            Shape::Pen(pts) | Shape::Marker(pts) => pts.len() < 2,
            Shape::Line(a, b) | Shape::Arrow(a, b) => a.distance(*b) < 2.0,
            Shape::Rect(r) | Shape::Ellipse(r) | Shape::Pixelate(r) => {
                r.width() < 2.0 || r.height() < 2.0
            }
            Shape::Text { text, .. } => text.trim().is_empty(),
            Shape::Step { .. } => false,
        }
    }
}

pub fn marker_color(color: Color32) -> Color32 {
    let [r, g, b, _] = color.to_srgba_unmultiplied();
    Color32::from_rgba_unmultiplied(r, g, b, 100)
}

pub fn marker_width(width: f32) -> f32 {
    width * 3.0 + 8.0
}

pub fn step_radius(width: f32) -> f32 {
    12.0 + width * 2.0
}

/// Black or white, whichever reads better on `color`.
pub fn contrast_text(color: Color32) -> Color32 {
    let [r, g, b, _] = color.to_srgba_unmultiplied();
    let luma = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
    if luma > 160.0 {
        Color32::BLACK
    } else {
        Color32::WHITE
    }
}

/// Returns the end of the arrow shaft and the arrow-head triangle.
pub fn arrow_geometry(a: Pos2, b: Pos2, width: f32) -> Option<(Pos2, [Pos2; 3])> {
    let len = a.distance(b);
    if len < 1.0 {
        return None;
    }
    let dir = (b - a) / len;
    let head_len = (width * 3.0 + 10.0).min(len);
    let half = head_len * 0.55;
    let base = b - dir * head_len;
    let perp = Vec2::new(-dir.y, dir.x);
    Some((base + dir * 1.0, [b, base + perp * half, base - perp * half]))
}

/// Constrains `b` so the segment `a`→`b` snaps to 45° steps.
pub fn snap_angle(a: Pos2, b: Pos2) -> Pos2 {
    let v = b - a;
    let step = std::f32::consts::FRAC_PI_4;
    let angle = (v.y.atan2(v.x) / step).round() * step;
    a + Vec2::angled(angle) * v.length()
}

/// Constrains a drag rectangle to a square.
pub fn square(a: Pos2, b: Pos2) -> Pos2 {
    let v = b - a;
    let side = v.x.abs().max(v.y.abs());
    a + Vec2::new(side * v.x.signum(), side * v.y.signum())
}

/// Maps between screenshot pixels and egui points.
#[derive(Clone, Copy)]
pub struct Mapper {
    pub origin: Pos2,
    pub ppp: f32,
}

impl Mapper {
    pub fn to_screen(self, p: Pos2) -> Pos2 {
        self.origin + p.to_vec2() / self.ppp
    }

    pub fn to_img(self, p: Pos2) -> Pos2 {
        ((p - self.origin) * self.ppp).to_pos2()
    }

    pub fn len(self, px: f32) -> f32 {
        px / self.ppp
    }

    pub fn rect(self, r: Rect) -> Rect {
        Rect::from_min_max(self.to_screen(r.min), self.to_screen(r.max))
    }
}

/// Draws one annotation on screen.
pub fn paint(
    painter: &egui::Painter,
    annot: &Annot,
    map: &Mapper,
    pixelated: Option<egui::TextureId>,
    img_size: Vec2,
) {
    let stroke = Stroke::new(map.len(annot.width), annot.color);
    match &annot.shape {
        Shape::Pen(pts) => {
            let pts: Vec<Pos2> = pts.iter().map(|p| map.to_screen(*p)).collect();
            painter.add(EShape::line(pts, stroke));
        }
        Shape::Marker(pts) => {
            let pts: Vec<Pos2> = pts.iter().map(|p| map.to_screen(*p)).collect();
            let stroke = Stroke::new(
                map.len(marker_width(annot.width)),
                marker_color(annot.color),
            );
            painter.add(EShape::line(pts, stroke));
        }
        Shape::Line(a, b) => {
            painter.line_segment([map.to_screen(*a), map.to_screen(*b)], stroke);
        }
        Shape::Arrow(a, b) => {
            if let Some((shaft_end, head)) = arrow_geometry(*a, *b, annot.width) {
                painter.line_segment([map.to_screen(*a), map.to_screen(shaft_end)], stroke);
                painter.add(EShape::convex_polygon(
                    head.iter().map(|p| map.to_screen(*p)).collect(),
                    annot.color,
                    Stroke::NONE,
                ));
            }
        }
        Shape::Rect(r) => {
            let r = map.rect(*r);
            if annot.filled {
                painter.rect_filled(r, 0.0, annot.color);
            } else {
                painter.rect_stroke(r, 0.0, stroke, StrokeKind::Middle);
            }
        }
        Shape::Ellipse(r) => {
            let r = map.rect(*r);
            if annot.filled {
                painter.add(EShape::ellipse_filled(r.center(), r.size() / 2.0, annot.color));
            } else {
                painter.add(EShape::ellipse_stroke(r.center(), r.size() / 2.0, stroke));
            }
        }
        Shape::Text { pos, text } => {
            painter.text(
                map.to_screen(*pos),
                Align2::LEFT_TOP,
                text,
                FontId::proportional(map.len(annot.font_size)),
                annot.color,
            );
        }
        Shape::Step { center, number } => {
            let c = map.to_screen(*center);
            let r = map.len(step_radius(annot.width));
            painter.circle_filled(c, r, annot.color);
            painter.text(
                c,
                Align2::CENTER_CENTER,
                number.to_string(),
                FontId::proportional(r * 1.15),
                contrast_text(annot.color),
            );
        }
        Shape::Pixelate(r) => {
            let screen = map.rect(*r);
            match pixelated {
                Some(tex) => {
                    let uv = Rect::from_min_max(
                        pos2(r.min.x / img_size.x, r.min.y / img_size.y),
                        pos2(r.max.x / img_size.x, r.max.y / img_size.y),
                    );
                    painter.image(tex, screen, uv, Color32::WHITE);
                }
                None => {
                    painter.rect_filled(screen, 0.0, Color32::from_gray(128));
                }
            }
        }
    }
}
