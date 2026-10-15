//! Selection and annotation model, plus on-screen drawing with egui.
//!
//! Everything is stored in screenshot pixel coordinates so the same data can be
//! drawn on screen (via [`Mapper`]) and rendered into the final image.

use egui::{Align2, Color32, FontId, Pos2, Rect, Shape as EShape, Stroke, StrokeKind, Vec2, pos2, vec2};

use super::text;
use crate::tiles::TiledTexture;

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
    Move,
    Pen,
    Marker,
    Line,
    Arrow,
    Rect,
    Ellipse,
    Text,
    Step,
    Pixelate,
    Blur,
    Eraser,
}

impl Tool {
    pub const ALL: [Tool; 13] = [
        Tool::Adjust,
        Tool::Move,
        Tool::Pen,
        Tool::Marker,
        Tool::Line,
        Tool::Arrow,
        Tool::Rect,
        Tool::Ellipse,
        Tool::Text,
        Tool::Step,
        Tool::Pixelate,
        Tool::Blur,
        Tool::Eraser,
    ];

    pub fn icon(self) -> &'static str {
        match self {
            Tool::Adjust => "⛶",
            Tool::Move => "✋",
            Tool::Pen => "✏",
            Tool::Marker => "🖊",
            Tool::Line => "📏",
            Tool::Arrow => "⬈",
            Tool::Rect => "⬜",
            Tool::Ellipse => "⭕",
            Tool::Text => "🔤",
            Tool::Step => "🔢",
            Tool::Pixelate => "🔲",
            Tool::Blur => "💧",
            // No eraser glyph in egui's fonts: the toolbar draws this one itself.
            Tool::Eraser => "",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Tool::Adjust => "Move / resize selection",
            Tool::Move => "Move annotations (drag anything you added)",
            Tool::Pen => "Pen",
            Tool::Marker => "Highlighter",
            Tool::Line => "Line",
            Tool::Arrow => "Arrow",
            Tool::Rect => "Rectangle",
            Tool::Ellipse => "Ellipse",
            Tool::Text => "Text",
            Tool::Step => "Numbered step",
            Tool::Pixelate => "Pixelate / hide",
            Tool::Blur => "Blur",
            Tool::Eraser => "Eraser (click or drag over annotations to delete them)",
        }
    }

    pub fn act(self) -> crate::shortcuts::Act {
        use crate::shortcuts::Act;
        match self {
            Tool::Adjust => Act::ToolAdjust,
            Tool::Move => Act::ToolMove,
            Tool::Pen => Act::ToolPen,
            Tool::Marker => Act::ToolMarker,
            Tool::Line => Act::ToolLine,
            Tool::Arrow => Act::ToolArrow,
            Tool::Rect => Act::ToolRect,
            Tool::Ellipse => Act::ToolEllipse,
            Tool::Text => Act::ToolText,
            Tool::Step => Act::ToolStep,
            Tool::Pixelate => Act::ToolPixelate,
            Tool::Blur => Act::ToolBlur,
            Tool::Eraser => Act::ToolEraser,
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
    /// Top-left corner. With `box_width` the text wraps inside a box that wide.
    Text { pos: Pos2, text: String, box_width: Option<f32> },
    Step { center: Pos2, number: u32 },
    Pixelate(Rect),
    Blur(Rect),
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
            Shape::Rect(r) | Shape::Ellipse(r) | Shape::Pixelate(r) | Shape::Blur(r) => {
                r.width() < 2.0 || r.height() < 2.0
            }
            Shape::Text { text, .. } => text.trim().is_empty(),
            Shape::Step { .. } => false,
        }
    }

    pub fn translate(&mut self, d: Vec2) {
        match &mut self.shape {
            Shape::Pen(pts) | Shape::Marker(pts) => pts.iter_mut().for_each(|p| *p += d),
            Shape::Line(a, b) | Shape::Arrow(a, b) => {
                *a += d;
                *b += d;
            }
            Shape::Rect(r) | Shape::Ellipse(r) | Shape::Pixelate(r) | Shape::Blur(r) => {
                *r = r.translate(d);
            }
            Shape::Text { pos, .. } => *pos += d,
            Shape::Step { center, .. } => *center += d,
        }
    }

    /// Bounding box in image pixels.
    pub fn bounds(&self) -> Rect {
        let pad = match &self.shape {
            Shape::Marker(_) => marker_width(self.width) / 2.0,
            _ => self.width / 2.0,
        };
        match &self.shape {
            Shape::Pen(pts) | Shape::Marker(pts) => Rect::from_points(pts).expand(pad),
            Shape::Line(a, b) | Shape::Arrow(a, b) => Rect::from_two_pos(*a, *b).expand(pad),
            Shape::Rect(r) | Shape::Ellipse(r) => r.expand(if self.filled { 0.0 } else { pad }),
            Shape::Pixelate(r) | Shape::Blur(r) => *r,
            Shape::Text { pos, text, box_width } => {
                Rect::from_min_size(*pos, text::block_size(text, self.font_size, *box_width))
            }
            Shape::Step { center, .. } => {
                Rect::from_center_size(*center, Vec2::splat(step_radius(self.width) * 2.0))
            }
        }
    }

    /// Whether `p` (image pixels) touches the annotation, with `tol` pixels of slack.
    pub fn hit(&self, p: Pos2, tol: f32) -> bool {
        let near_line = |pts: &[Pos2], half_width: f32| {
            let reach = half_width + tol;
            match pts {
                [only] => only.distance(p) <= reach,
                _ => pts.windows(2).any(|w| segment_distance(p, w[0], w[1]) <= reach),
            }
        };
        match &self.shape {
            Shape::Pen(pts) => near_line(pts, self.width / 2.0),
            Shape::Marker(pts) => near_line(pts, marker_width(self.width) / 2.0),
            Shape::Line(a, b) | Shape::Arrow(a, b) => near_line(&[*a, *b], self.width / 2.0),
            Shape::Rect(r) if !self.filled => {
                let reach = self.width / 2.0 + tol;
                r.expand(reach).contains(p) && !r.shrink(reach).contains(p)
            }
            Shape::Ellipse(r) if !self.filled => {
                let radii = r.size() / 2.0;
                if radii.min_elem() < 1.0 {
                    return r.expand(tol).contains(p);
                }
                let v = (p - r.center()) / radii;
                (v.length() - 1.0).abs() * radii.min_elem() <= self.width / 2.0 + tol
            }
            Shape::Step { center, .. } => center.distance(p) <= step_radius(self.width) + tol,
            _ => self.bounds().expand(tol).contains(p),
        }
    }
}

fn segment_distance(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let len_sq = ab.length_sq();
    if len_sq == 0.0 {
        return p.distance(a);
    }
    let t = ((p - a).dot(ab) / len_sq).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// Pre-rendered copies of the screenshot that pixelate and blur annotations reveal.
#[derive(Clone, Copy)]
pub struct Effects<T> {
    pub pixelated: Option<T>,
    pub blurred: Option<T>,
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
    effects: Effects<&TiledTexture>,
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
        Shape::Text { pos, text, box_width } => {
            // Lines are placed one by one so they break where the export breaks them.
            let font = FontId::proportional(map.len(annot.font_size));
            let line_height = text::line_height(annot.font_size);
            for (i, line) in text::lines(text, annot.font_size, *box_width).iter().enumerate() {
                let top = *pos + vec2(0.0, i as f32 * line_height);
                let shown = text::visual(line);
                painter.text(map.to_screen(top), Align2::LEFT_TOP, shown, font.clone(), annot.color);
            }
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
        Shape::Pixelate(r) | Shape::Blur(r) => {
            let screen = map.rect(*r);
            let source = match annot.shape {
                Shape::Blur(_) => effects.blurred,
                _ => effects.pixelated,
            };
            match source {
                Some(tex) => tex.paint(painter, *r, screen, Color32::WHITE),
                None => {
                    painter.rect_filled(screen, 0.0, Color32::from_gray(128));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn annot(shape: Shape, filled: bool) -> Annot {
        Annot {
            shape,
            color: Color32::RED,
            width: 4.0,
            filled,
            font_size: 20.0,
        }
    }

    #[test]
    fn hits_strokes_and_outlines_but_not_empty_insides() {
        let line = annot(Shape::Line(pos2(0.0, 0.0), pos2(100.0, 0.0)), false);
        assert!(line.hit(pos2(50.0, 4.0), 3.0));
        assert!(!line.hit(pos2(50.0, 20.0), 3.0));

        let r = Rect::from_min_max(pos2(10.0, 10.0), pos2(110.0, 110.0));
        let outline = annot(Shape::Rect(r), false);
        assert!(outline.hit(pos2(10.0, 60.0), 3.0));
        assert!(!outline.hit(pos2(60.0, 60.0), 3.0), "empty middle lets clicks through");
        assert!(annot(Shape::Rect(r), true).hit(pos2(60.0, 60.0), 3.0));

        let ring = annot(Shape::Ellipse(r), false);
        assert!(ring.hit(pos2(110.0, 60.0), 3.0));
        assert!(!ring.hit(pos2(60.0, 60.0), 3.0));

        let boxed = Shape::Text { pos: pos2(0.0, 0.0), text: "Hi".into(), box_width: Some(200.0) };
        let text = annot(boxed, false);
        assert!(text.hit(pos2(150.0, 10.0), 0.0), "the whole box is grabbable");
        assert!(!text.hit(pos2(250.0, 10.0), 0.0));
    }

    #[test]
    fn translate_moves_every_point() {
        let mut pen = annot(Shape::Pen(vec![pos2(1.0, 1.0), pos2(5.0, 5.0)]), false);
        pen.translate(Vec2::new(10.0, -1.0));
        let Shape::Pen(pts) = &pen.shape else { unreachable!() };
        assert_eq!(pts, &vec![pos2(11.0, 0.0), pos2(15.0, 4.0)]);

        let mut blur = annot(Shape::Blur(Rect::from_min_max(pos2(0.0, 0.0), pos2(4.0, 4.0))), false);
        blur.translate(Vec2::splat(2.0));
        assert!(matches!(blur.shape, Shape::Blur(r) if r.min == pos2(2.0, 2.0)));
    }
}
