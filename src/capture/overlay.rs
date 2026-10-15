use egui::{
    Align2, Area, Color32, Context, CursorIcon, FontId, Frame, Id, Key, KeyboardShortcut,
    Margin, Mesh, Modifiers, Order, PointerButton, Pos2, Rect, Response, RichText, Sense, Stroke,
    StrokeKind, TextEdit, TextureOptions, Vec2, pos2, vec2,
};
use image::RgbaImage;

use super::annot::{self, Annot, Effects, Mapper, SelShape, Selection, Shape, Tool};
use super::{render, text};
use crate::app::Command;
use crate::host::{Cx, View, ViewKind};
use crate::shortcuts::Act;
use crate::theme::ACCENT;
use crate::tiles::TiledTexture;

const PALETTE: [Color32; 10] = [
    Color32::from_rgb(239, 68, 68),
    Color32::from_rgb(249, 115, 22),
    Color32::from_rgb(250, 204, 21),
    Color32::from_rgb(34, 197, 94),
    Color32::from_rgb(6, 182, 212),
    Color32::from_rgb(59, 130, 246),
    Color32::from_rgb(168, 85, 247),
    Color32::from_rgb(236, 72, 153),
    Color32::WHITE,
    Color32::BLACK,
];
const DIM: Color32 = Color32::from_black_alpha(140);
/// Half the size of a resize handle, in points.
const HANDLE: f32 = 4.5;
const MAGNIFIER_ZOOM: f32 = 8.0;
const MAGNIFIER_SIZE: f32 = 120.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Copy,
    Save,
    SaveAs,
    Pin,
    /// Start a scrolling (long) capture of the selected area.
    Scroll,
}

/// Which edges of the selection a resize handle moves.
#[derive(Clone, Copy, PartialEq)]
struct Handle {
    left: bool,
    right: bool,
    top: bool,
    bottom: bool,
}

impl Handle {
    const ALL: [Handle; 8] = [
        Handle::new(true, false, true, false),
        Handle::new(false, false, true, false),
        Handle::new(false, true, true, false),
        Handle::new(false, true, false, false),
        Handle::new(false, true, false, true),
        Handle::new(false, false, false, true),
        Handle::new(true, false, false, true),
        Handle::new(true, false, false, false),
    ];

    const fn new(left: bool, right: bool, top: bool, bottom: bool) -> Self {
        Self {
            left,
            right,
            top,
            bottom,
        }
    }

    fn pos(self, r: Rect) -> Pos2 {
        let x = if self.left {
            r.left()
        } else if self.right {
            r.right()
        } else {
            r.center().x
        };
        let y = if self.top {
            r.top()
        } else if self.bottom {
            r.bottom()
        } else {
            r.center().y
        };
        pos2(x, y)
    }

    fn cursor(self) -> CursorIcon {
        match (self.left || self.right, self.top || self.bottom) {
            (true, false) => CursorIcon::ResizeHorizontal,
            (false, true) => CursorIcon::ResizeVertical,
            _ if (self.left && self.top) || (self.right && self.bottom) => CursorIcon::ResizeNwSe,
            _ => CursorIcon::ResizeNeSw,
        }
    }
}

enum Drag {
    None,
    NewSel(Pos2),
    Free(Vec<Pos2>),
    Move { grab: Pos2, orig: Rect },
    Resize { handle: Handle, grab: Pos2, orig: Rect },
    Draw(Pos2),
    /// Moving an annotation, or resizing a text box, with the move tool.
    EditAnnot { index: usize, grip: Grip, grab: Pos2, orig: Annot, moved: bool },
    /// Dragging out the width of a new text box.
    TextBox { start: Pos2, end: Pos2 },
    /// Erasing everything the pointer passes over; `before` is kept for undo.
    Erase { before: Vec<Annot>, last: Pos2 },
}

/// Which part of an annotation the move tool grabbed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Grip {
    Body,
    /// The left or right edge of a text box: changes its width.
    Left,
    Right,
}

const ERASER_COLOR: Color32 = Color32::from_rgb(239, 68, 68);

struct TextDraft {
    pos: Pos2,
    text: String,
    /// Wrap width in image pixels; `None` grows with the text.
    box_width: Option<f32>,
    focused: bool,
}

pub struct CaptureView {
    shot: RgbaImage,
    /// Physical desktop position of the captured monitor, used to place pinned images.
    monitor_pos: (i32, i32),
    shot_tex: Option<TiledTexture>,
    pixelated: Option<RgbaImage>,
    pix_tex: Option<TiledTexture>,
    blurred: Option<RgbaImage>,
    blur_tex: Option<TiledTexture>,
    sel_shape: SelShape,
    sel: Option<Selection>,
    drag: Drag,
    tool: Tool,
    color: Color32,
    width: f32,
    font_size: f32,
    filled: bool,
    annots: Vec<Annot>,
    /// Earlier versions of `annots`, so moves and "clear all" can be undone too.
    undo_stack: Vec<Vec<Annot>>,
    redo_stack: Vec<Vec<Annot>>,
    current: Option<Annot>,
    draft: Option<TextDraft>,
    toolbar_size: Vec2,
    /// Standalone editor window instead of the full-screen capture overlay.
    editor: bool,
    /// Editor zoom relative to "fit to width".
    zoom: f32,
    /// Short confirmation shown in the editor, e.g. "Copied".
    status: Option<(String, std::time::Instant)>,
}

impl CaptureView {
    pub fn new(shot: RgbaImage, monitor_pos: (i32, i32), settings: &crate::settings::Settings) -> Self {
        let [r, g, b, a] = settings.pen_color;
        Self {
            shot,
            monitor_pos,
            shot_tex: None,
            pixelated: None,
            pix_tex: None,
            blurred: None,
            blur_tex: None,
            sel_shape: SelShape::Rect,
            sel: None,
            drag: Drag::None,
            tool: Tool::Adjust,
            color: Color32::from_rgba_unmultiplied(r, g, b, a),
            width: settings.pen_width.clamp(1.0, 24.0),
            font_size: settings.font_size.clamp(10.0, 120.0),
            filled: false,
            annots: Vec::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            current: None,
            draft: None,
            toolbar_size: vec2(640.0, 84.0),
            editor: false,
            zoom: 1.0,
            status: None,
        }
    }

    /// Opens an existing image for annotation in a normal window.
    pub fn editor(image: RgbaImage, settings: &crate::settings::Settings) -> Self {
        let mut view = Self::new(image, (0, 0), settings);
        view.editor = true;
        view.tool = Tool::Pen;
        view.select_all();
        view
    }

    fn img_size(&self) -> Vec2 {
        vec2(self.shot.width() as f32, self.shot.height() as f32)
    }

    fn img_rect(&self) -> Rect {
        Rect::from_min_size(Pos2::ZERO, self.img_size())
    }

    fn clamp_img(&self, p: Pos2) -> Pos2 {
        p.clamp(Pos2::ZERO, self.img_size().to_pos2())
    }

    fn creating_selection(&self) -> bool {
        matches!(self.drag, Drag::NewSel(_) | Drag::Free(_))
    }

    fn editing(&self) -> bool {
        self.sel.is_some() && !self.creating_selection()
    }

    fn ensure_textures(&mut self, ctx: &Context) {
        if self.shot_tex.is_none() {
            // NEAREST keeps the 1:1 overlay crisp and the magnifier pixel-exact;
            // the editor scales the image, where LINEAR looks better.
            let options = if self.editor {
                TextureOptions::LINEAR
            } else {
                TextureOptions::NEAREST
            };
            self.shot_tex = Some(TiledTexture::new(ctx, "screenshot", &self.shot, options));
        }
    }

    fn ensure_pixelated(&mut self, ctx: Option<&Context>) {
        if self.pixelated.is_none() {
            let block = (self.shot.width() / 160).max(8);
            self.pixelated = Some(render::pixelate(&self.shot, block));
        }
        if let (Some(ctx), None, Some(pix)) = (ctx, &self.pix_tex, &self.pixelated) {
            self.pix_tex = Some(TiledTexture::new(ctx, "pixelated", pix, TextureOptions::NEAREST));
        }
    }

    fn ensure_blurred(&mut self, ctx: Option<&Context>) {
        if self.blurred.is_none() {
            self.blurred = Some(render::blur(&self.shot));
        }
        if let (Some(ctx), None, Some(blurred)) = (ctx, &self.blur_tex, &self.blurred) {
            self.blur_tex = Some(TiledTexture::new(ctx, "blurred", blurred, TextureOptions::LINEAR));
        }
    }

    fn effect_textures(&self) -> Effects<&TiledTexture> {
        Effects {
            pixelated: self.pix_tex.as_ref(),
            blurred: self.blur_tex.as_ref(),
        }
    }

    fn new_annot(&self, shape: Shape) -> Annot {
        Annot {
            shape,
            color: self.color,
            width: self.width,
            filled: self.filled,
            font_size: self.font_size,
        }
    }

    /// Remembers the current annotations for undo. Call before changing them.
    fn checkpoint(&mut self) {
        self.undo_stack.push(self.annots.clone());
        self.redo_stack.clear();
    }

    fn push_annot(&mut self, annot: Annot) {
        if !annot.is_degenerate() {
            self.checkpoint();
            self.annots.push(annot);
        }
    }

    fn clear_all(&mut self) {
        self.commit_draft();
        if !self.annots.is_empty() {
            self.checkpoint();
            self.annots.clear();
        }
    }

    fn set_tool(&mut self, tool: Tool, ctx: &Context) {
        self.commit_draft();
        match tool {
            Tool::Pixelate => self.ensure_pixelated(Some(ctx)),
            Tool::Blur => self.ensure_blurred(Some(ctx)),
            _ => {}
        }
        self.tool = tool;
    }

    fn commit_draft(&mut self) {
        if let Some(draft) = self.draft.take() {
            let text = draft.text.trim_end().to_owned();
            self.push_annot(self.new_annot(Shape::Text {
                pos: draft.pos,
                text,
                box_width: draft.box_width,
            }));
        }
    }

    fn undo(&mut self) {
        self.commit_draft();
        if let Some(previous) = self.undo_stack.pop() {
            self.redo_stack.push(std::mem::replace(&mut self.annots, previous));
        }
    }

    fn redo(&mut self) {
        if let Some(next) = self.redo_stack.pop() {
            self.undo_stack.push(std::mem::replace(&mut self.annots, next));
        }
    }

    /// Index of the topmost annotation under `p` (image pixels).
    fn annot_at(&self, p: Pos2, map: &Mapper) -> Option<usize> {
        let tolerance = 5.0 * map.ppp;
        self.annots.iter().rposition(|a| a.hit(p, tolerance))
    }

    /// The annotation under `p` and the part of it the move tool would grab.
    fn grip_at(&self, p: Pos2, map: &Mapper) -> Option<(usize, Grip)> {
        let index = self.annot_at(p, map)?;
        let annot = &self.annots[index];
        let mut grip = Grip::Body;
        if matches!(annot.shape, Shape::Text { .. }) {
            let b = annot.bounds();
            let reach = 6.0 * map.ppp;
            if (b.min.y - reach..=b.max.y + reach).contains(&p.y) {
                if (p.x - b.max.x).abs() <= reach {
                    grip = Grip::Right;
                } else if (p.x - b.min.x).abs() <= reach {
                    grip = Grip::Left;
                }
            }
        }
        Some((index, grip))
    }

    /// Deletes the annotations touched along the segment `from`→`to`. Returns whether any were.
    fn erase_along(&mut self, from: Pos2, to: Pos2, map: &Mapper) -> bool {
        let step = 4.0 * map.ppp;
        let n = (from.distance(to) / step).ceil().max(1.0) as usize;
        let mut erased = false;
        for i in 0..=n {
            let p = from.lerp(to, i as f32 / n as f32);
            while let Some(index) = self.annot_at(p, map) {
                self.annots.remove(index);
                erased = true;
            }
        }
        erased
    }

    /// The editor has no selection to adjust, so it leaves that tool out.
    fn tools(&self) -> impl Iterator<Item = Tool> + use<> {
        let editor = self.editor;
        Tool::ALL.into_iter().filter(move |t| !(editor && *t == Tool::Adjust))
    }

    fn select_all(&mut self) {
        self.sel = Some(Selection::rect(self.img_rect(), SelShape::Rect));
        self.drag = Drag::None;
    }

    fn save_style(&self, cx: &mut Cx<'_>) {
        let settings = &mut cx.shared.settings;
        let color = self.color.to_srgba_unmultiplied();
        if settings.pen_color != color
            || settings.pen_width != self.width
            || settings.font_size != self.font_size
        {
            settings.pen_color = color;
            settings.pen_width = self.width;
            settings.font_size = self.font_size;
            settings.save();
        }
    }

    fn cancel(&mut self, cx: &mut Cx<'_>) {
        self.save_style(cx);
        cx.close = true;
    }

    fn finish(&mut self, action: Action, cx: &mut Cx<'_>) {
        self.commit_draft();
        if self.annots.iter().any(|a| matches!(a.shape, Shape::Pixelate(_))) {
            self.ensure_pixelated(None);
        }
        if self.annots.iter().any(|a| matches!(a.shape, Shape::Blur(_))) {
            self.ensure_blurred(None);
        }
        let Some(sel) = &self.sel else { return };
        let effects = Effects {
            pixelated: self.pixelated.as_ref(),
            blurred: self.blurred.as_ref(),
        };
        let Some(image) = render::export(&self.shot, effects, sel, &self.annots) else {
            return;
        };
        let origin = (
            self.monitor_pos.0 + sel.rect.min.x as i32,
            self.monitor_pos.1 + sel.rect.min.y as i32,
        );
        self.save_style(cx);

        if self.editor {
            // The editor stays open so the user can copy and save the same edit.
            let pin_pos = crate::platform::cursor_pos().unwrap_or((120, 120));
            let (command, status) = match action {
                Action::Copy => (Command::CopyImage(image), "✔ Copied to clipboard"),
                Action::Save => (Command::SaveImage(image), "✔ Saved to the screenshots folder"),
                Action::SaveAs => (Command::SaveImageAs(image), "Choose where to save…"),
                Action::Pin => (Command::Pin(image, pin_pos), "✔ Pinned"),
                Action::Scroll => return,
            };
            cx.commands.push(command);
            self.status = Some((status.to_owned(), std::time::Instant::now()));
            return;
        }

        cx.commands.push(match action {
            Action::Pin => Command::Pin(image, origin),
            Action::Copy | Action::Save | Action::SaveAs => Command::Captured {
                image,
                action,
                origin,
            },
            Action::Scroll => return,
        });
        cx.close = true;
    }

    fn start_scroll_capture(&mut self, cx: &mut Cx<'_>) {
        let Some(sel) = &self.sel else { return };
        let r = sel.rect;
        if r.width() < 20.0 || r.height() < 40.0 {
            return;
        }
        cx.commands.push(Command::ScrollCapture(crate::scroll::Region {
            monitor_pos: self.monitor_pos,
            x: r.min.x as u32,
            y: r.min.y as u32,
            width: r.width() as u32,
            height: r.height() as u32,
        }));
        self.save_style(cx);
        cx.close = true;
    }

    // ------------------------------------------------------------------ input

    fn hit_handle(&self, screen_pos: Pos2, map: &Mapper) -> Option<Handle> {
        let sel = self.sel.as_ref()?;
        let r = map.rect(sel.rect);
        Handle::ALL.into_iter().find(|h| {
            let d = h.pos(r) - screen_pos;
            d.x.abs() <= HANDLE + 3.0 && d.y.abs() <= HANDLE + 3.0
        })
    }

    fn handle_pointer(&mut self, ctx: &Context, resp: &Response, map: &Mapper, cx: &mut Cx<'_>) {
        let Some(pointer) = ctx.input(|i| i.pointer.interact_pos().or(i.pointer.hover_pos())) else {
            return;
        };
        let raw_img = map.to_img(pointer);
        let p = self.clamp_img(raw_img);
        let shift = ctx.input(|i| i.modifiers.shift);

        if resp.hovered() || resp.dragged() {
            ctx.set_cursor_icon(self.cursor_for(pointer, raw_img, map));
        }

        if resp.drag_started_by(PointerButton::Primary) {
            let origin = ctx.input(|i| i.pointer.press_origin()).unwrap_or(pointer);
            self.begin_drag(origin, self.clamp_img(map.to_img(origin)), map);
        }
        if resp.dragged_by(PointerButton::Primary) {
            self.update_drag(p, raw_img, shift, map);
        }
        if resp.drag_stopped_by(PointerButton::Primary) {
            self.end_drag();
        }
        if resp.clicked_by(PointerButton::Primary) {
            self.click(p, map);
        }
        if resp.secondary_clicked() {
            if self.draft.is_some() || self.editor {
                self.commit_draft();
            } else if self.sel.is_none() {
                self.cancel(cx);
            } else if self.annots.is_empty() {
                // Right click with nothing drawn yet: start the selection over.
                self.sel = None;
                self.tool = Tool::Adjust;
            }
        }
    }

    fn cursor_for(&self, screen_pos: Pos2, img_pos: Pos2, map: &Mapper) -> CursorIcon {
        match &self.drag {
            Drag::Move { .. } | Drag::EditAnnot { grip: Grip::Body, .. } => return CursorIcon::Grabbing,
            Drag::EditAnnot { .. } => return CursorIcon::ResizeHorizontal,
            Drag::TextBox { .. } => return CursorIcon::Text,
            Drag::Erase { .. } => return CursorIcon::Crosshair,
            Drag::Resize { handle, .. } => return handle.cursor(),
            Drag::NewSel(_) | Drag::Free(_) | Drag::Draw(_) => return CursorIcon::Crosshair,
            Drag::None => {}
        }
        let Some(sel) = &self.sel else {
            return CursorIcon::Crosshair;
        };
        match self.tool {
            Tool::Adjust => {
                if let Some(h) = self.hit_handle(screen_pos, map) {
                    h.cursor()
                } else if sel.rect.contains(img_pos) {
                    CursorIcon::Move
                } else {
                    CursorIcon::Default
                }
            }
            Tool::Move => match self.grip_at(img_pos, map) {
                Some((_, Grip::Body)) => CursorIcon::Grab,
                Some(_) => CursorIcon::ResizeHorizontal,
                None => CursorIcon::Default,
            },
            Tool::Eraser => {
                if self.annot_at(img_pos, map).is_some() {
                    CursorIcon::PointingHand
                } else {
                    CursorIcon::Crosshair
                }
            }
            Tool::Text => CursorIcon::Text,
            _ => CursorIcon::Crosshair,
        }
    }

    fn begin_drag(&mut self, origin_screen: Pos2, origin: Pos2, map: &Mapper) {
        self.drag = match (&self.sel, self.tool) {
            (None, _) => match self.sel_shape {
                SelShape::Free => Drag::Free(vec![origin]),
                shape => {
                    self.sel = Some(Selection::rect(Rect::from_two_pos(origin, origin), shape));
                    Drag::NewSel(origin)
                }
            },
            (Some(sel), Tool::Adjust) => {
                if let Some(handle) = self.hit_handle(origin_screen, map) {
                    Drag::Resize {
                        handle,
                        grab: origin,
                        orig: sel.rect,
                    }
                } else if sel.rect.contains(origin) {
                    Drag::Move {
                        grab: origin,
                        orig: sel.rect,
                    }
                } else {
                    Drag::None
                }
            }
            (Some(_), Tool::Move) => match self.grip_at(origin, map) {
                Some((index, grip)) => Drag::EditAnnot {
                    index,
                    grip,
                    grab: origin,
                    orig: self.annots[index].clone(),
                    moved: false,
                },
                None => Drag::None,
            },
            (Some(_), Tool::Eraser) => {
                let before = self.annots.clone();
                self.erase_along(origin, origin, map);
                Drag::Erase { before, last: origin }
            }
            (Some(_), Tool::Text) => Drag::TextBox {
                start: origin,
                end: origin,
            },
            (Some(_), Tool::Step) => Drag::None,
            (Some(_), tool) => {
                let shape = match tool {
                    Tool::Pen => Shape::Pen(vec![origin]),
                    Tool::Marker => Shape::Marker(vec![origin]),
                    Tool::Line => Shape::Line(origin, origin),
                    Tool::Arrow => Shape::Arrow(origin, origin),
                    Tool::Rect => Shape::Rect(Rect::from_two_pos(origin, origin)),
                    Tool::Ellipse => Shape::Ellipse(Rect::from_two_pos(origin, origin)),
                    Tool::Blur => Shape::Blur(Rect::from_two_pos(origin, origin)),
                    _ => Shape::Pixelate(Rect::from_two_pos(origin, origin)),
                };
                self.current = Some(self.new_annot(shape));
                Drag::Draw(origin)
            }
        };
    }

    fn update_drag(&mut self, p: Pos2, raw: Pos2, shift: bool, map: &Mapper) {
        if let Drag::Erase { last, .. } = &mut self.drag {
            let from = std::mem::replace(last, p);
            self.erase_along(from, p, map);
            return;
        }
        let bounds = self.img_rect();
        match &mut self.drag {
            Drag::Erase { .. } => {}
            Drag::TextBox { end, .. } => *end = p,
            Drag::None => {}
            Drag::NewSel(start) => {
                let end = if shift { annot::square(*start, p) } else { p };
                if let Some(sel) = &mut self.sel {
                    sel.rect = round_rect(Rect::from_two_pos(*start, end)).intersect(bounds);
                }
            }
            Drag::Free(points) => {
                if points.last().is_none_or(|last| last.distance(p) >= 2.0) {
                    points.push(p);
                }
            }
            Drag::Move { grab, orig } => {
                let mut r = orig.translate(raw - *grab);
                r = r.translate(vec2(
                    (bounds.min.x - r.min.x).max(0.0) + (bounds.max.x - r.max.x).min(0.0),
                    (bounds.min.y - r.min.y).max(0.0) + (bounds.max.y - r.max.y).min(0.0),
                ));
                if let Some(sel) = &mut self.sel {
                    sel.rect = round_rect(r);
                }
            }
            Drag::Resize { handle, grab, orig } => {
                let d = p - *grab;
                let mut r = *orig;
                if handle.left {
                    r.min.x += d.x;
                }
                if handle.right {
                    r.max.x += d.x;
                }
                if handle.top {
                    r.min.y += d.y;
                }
                if handle.bottom {
                    r.max.y += d.y;
                }
                if let Some(sel) = &mut self.sel {
                    let r = round_rect(Rect::from_two_pos(r.min, r.max)).intersect(bounds);
                    if r.width() >= 1.0 && r.height() >= 1.0 {
                        sel.rect = r;
                    }
                }
            }
            Drag::Draw(start) => {
                let start = *start;
                let Some(current) = &mut self.current else {
                    return;
                };
                match &mut current.shape {
                    Shape::Pen(pts) | Shape::Marker(pts) => {
                        if pts.last().is_none_or(|last| last.distance(p) >= 1.0) {
                            pts.push(p);
                        }
                    }
                    Shape::Line(_, b) | Shape::Arrow(_, b) => {
                        *b = if shift { annot::snap_angle(start, p) } else { p };
                    }
                    Shape::Rect(r) | Shape::Ellipse(r) | Shape::Pixelate(r) | Shape::Blur(r) => {
                        let end = if shift { annot::square(start, p) } else { p };
                        *r = Rect::from_two_pos(start, end);
                    }
                    Shape::Text { .. } | Shape::Step { .. } => {}
                }
            }
            Drag::EditAnnot {
                index,
                grip,
                grab,
                orig,
                moved,
            } => {
                let delta = raw - *grab;
                if delta != Vec2::ZERO {
                    *moved = true;
                }
                let mut annot = orig.clone();
                match grip {
                    Grip::Body => annot.translate(delta),
                    Grip::Left | Grip::Right => {
                        let b = orig.bounds();
                        let min_width = orig.font_size;
                        let new_width = match grip {
                            Grip::Right => b.width() + delta.x,
                            _ => b.width() - delta.x,
                        }
                        .max(min_width);
                        if let Shape::Text { pos, box_width, .. } = &mut annot.shape {
                            if *grip == Grip::Left {
                                pos.x = b.max.x - new_width;
                            }
                            *box_width = Some(new_width);
                        }
                    }
                }
                self.annots[*index] = annot;
            }
        }
    }

    fn end_drag(&mut self) {
        match std::mem::replace(&mut self.drag, Drag::None) {
            Drag::NewSel(_) => {
                if self
                    .sel
                    .as_ref()
                    .is_some_and(|s| s.rect.width() < 3.0 || s.rect.height() < 3.0)
                {
                    self.sel = None;
                }
            }
            Drag::Free(points) => self.sel = Selection::free(&points),
            Drag::Draw(_) => {
                if let Some(annot) = self.current.take() {
                    self.push_annot(annot);
                }
            }
            Drag::EditAnnot {
                index,
                orig,
                moved: true,
                ..
            } => {
                let mut before = self.annots.clone();
                before[index] = orig;
                self.undo_stack.push(before);
                self.redo_stack.clear();
            }
            Drag::Erase { before, .. } => {
                if before.len() != self.annots.len() {
                    self.undo_stack.push(before);
                    self.redo_stack.clear();
                }
            }
            Drag::TextBox { start, end } => {
                self.commit_draft();
                // A short drag is treated like a click: the text grows as you type.
                let width = (end.x - start.x).abs();
                let box_width = (width >= 12.0).then_some(width);
                self.draft = Some(TextDraft {
                    pos: pos2(start.x.min(end.x), start.y.min(end.y)),
                    text: String::new(),
                    box_width,
                    focused: false,
                });
            }
            Drag::None | Drag::Move { .. } | Drag::Resize { .. } | Drag::EditAnnot { .. } => {}
        }
    }

    fn click(&mut self, p: Pos2, map: &Mapper) {
        if self.sel.is_none() {
            self.select_all();
            return;
        }
        match self.tool {
            Tool::Text => {
                self.commit_draft();
                self.draft = Some(TextDraft {
                    pos: p,
                    text: String::new(),
                    box_width: None,
                    focused: false,
                });
            }
            Tool::Eraser => {
                if let Some(index) = self.annot_at(p, map) {
                    self.checkpoint();
                    self.annots.remove(index);
                }
            }
            Tool::Step => {
                let number = self
                    .annots
                    .iter()
                    .filter(|a| matches!(a.shape, Shape::Step { .. }))
                    .count() as u32
                    + 1;
                self.push_annot(self.new_annot(Shape::Step { center: p, number }));
            }
            _ => {}
        }
    }

    fn handle_keys(&mut self, ctx: &Context, cx: &mut Cx<'_>) {
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            if self.draft.is_some() {
                self.commit_draft();
            } else if !matches!(self.drag, Drag::None) {
                // Esc undoes the move, resize or erase in progress.
                match std::mem::replace(&mut self.drag, Drag::None) {
                    Drag::EditAnnot { index, orig, .. } => self.annots[index] = orig,
                    Drag::Erase { before, .. } => self.annots = before,
                    _ => {}
                }
                self.current = None;
                if self.sel.as_ref().is_some_and(|s| s.rect.area() < 9.0) {
                    self.sel = None;
                }
            } else if !self.editor {
                self.cancel(cx);
            }
            return;
        }

        let shortcut =
            |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(m, k)));

        if self.draft.is_some() && ctx.egui_wants_keyboard_input() {
            if shortcut(Modifiers::COMMAND, Key::Enter) {
                self.commit_draft();
            }
            return;
        }

        let keys = cx.shared.settings.keymap.clone();
        if !self.editor && keys.pressed(ctx, Act::SelectAll) {
            self.select_all();
        }
        if self.sel.is_none() {
            return;
        }

        if keys.pressed(ctx, Act::Redo) {
            self.redo();
        }
        if keys.pressed(ctx, Act::Undo) {
            self.undo();
        }
        if keys.pressed(ctx, Act::ClearAll) {
            self.clear_all();
        }
        for (act, action) in [
            (Act::SaveAs, Action::SaveAs),
            (Act::Save, Action::Save),
            (Act::Copy, Action::Copy),
            (Act::Pin, Action::Pin),
            (Act::ScrollCapture, Action::Scroll),
        ] {
            if keys.pressed(ctx, act) && !(self.editor && action == Action::Scroll) {
                self.run_action(Some(action), cx);
                return;
            }
        }

        for tool in self.tools() {
            if keys.pressed(ctx, tool.act()) {
                self.set_tool(tool, ctx);
            }
        }

        if self.tool == Tool::Adjust {
            let mut nudge = Vec2::ZERO;
            for (key, dir) in [
                (Key::ArrowLeft, vec2(-1.0, 0.0)),
                (Key::ArrowRight, vec2(1.0, 0.0)),
                (Key::ArrowUp, vec2(0.0, -1.0)),
                (Key::ArrowDown, vec2(0.0, 1.0)),
            ] {
                if shortcut(Modifiers::SHIFT, key) {
                    nudge += dir * 10.0;
                } else if shortcut(Modifiers::NONE, key) {
                    nudge += dir;
                }
            }
            if nudge != Vec2::ZERO {
                let bounds = self.img_rect();
                if let Some(sel) = &mut self.sel {
                    let moved = sel.rect.translate(nudge);
                    if bounds.contains_rect(moved) {
                        sel.rect = moved;
                    }
                }
            }
        }
    }

    // ---------------------------------------------------------------- drawing

    fn paint_scene(&self, painter: &egui::Painter, map: &Mapper, screen: Rect) {
        let Some(tex) = &self.shot_tex else { return };
        let img_size = self.img_size();
        tex.paint(painter, self.img_rect(), map.rect(self.img_rect()), Color32::WHITE);
        painter.rect_filled(screen, 0.0, DIM);

        if let Drag::Free(points) = &self.drag {
            let pts = points.iter().map(|p| map.to_screen(*p)).collect();
            painter.add(egui::Shape::line(pts, Stroke::new(2.0, ACCENT)));
        }

        let Some(sel) = &self.sel else { return };
        let sel_screen = map.rect(sel.rect);
        let polygon = sel.polygon();

        // Un-dim the selected area.
        match sel.shape {
            SelShape::Rect => tex.paint(painter, sel.rect, sel_screen, Color32::WHITE),
            _ => {
                if let Some(mesh) = tex.single().and_then(|id| textured_polygon(&polygon, id, img_size, map)) {
                    painter.add(mesh);
                }
            }
        }

        let clipped = painter.with_clip_rect(sel_screen.intersect(screen));
        for a in self.annots.iter().chain(self.current.as_ref()) {
            annot::paint(&clipped, a, map, self.effect_textures());
        }

        // Outline.
        let outline = Stroke::new(1.5, ACCENT);
        match sel.shape {
            SelShape::Rect => {
                painter.rect_stroke(sel_screen, 0.0, outline, StrokeKind::Outside);
            }
            _ => {
                let pts = polygon.iter().map(|p| map.to_screen(*p)).collect();
                painter.add(egui::Shape::closed_line(pts, outline));
                painter.rect_stroke(
                    sel_screen,
                    0.0,
                    Stroke::new(1.0, ACCENT.gamma_multiply(0.4)),
                    StrokeKind::Outside,
                );
            }
        }

        if self.tool == Tool::Adjust && !self.creating_selection() {
            for h in Handle::ALL {
                let r = Rect::from_center_size(h.pos(sel_screen), Vec2::splat(HANDLE * 2.0));
                painter.rect_filled(r, 1.0, Color32::WHITE);
                painter.rect_stroke(r, 1.0, Stroke::new(1.0, ACCENT), StrokeKind::Inside);
            }
        }

        // Size label above the selection.
        let label = format!("{} × {}", sel.rect.width() as u32, sel.rect.height() as u32);
        let galley = painter.layout_no_wrap(label, FontId::proportional(12.0), Color32::WHITE);
        let mut pos = sel_screen.left_top() - vec2(0.0, galley.size().y + 8.0);
        if pos.y < screen.top() {
            pos.y = sel_screen.top() + 4.0;
            pos.x += 4.0;
        }
        let bg = Rect::from_min_size(pos, galley.size() + vec2(10.0, 4.0));
        painter.rect_filled(bg, 4.0, Color32::from_black_alpha(190));
        painter.galley(pos + vec2(5.0, 2.0), galley, Color32::WHITE);
    }

    /// Feedback for the move, eraser and text tools: what they would act on.
    fn paint_tool_hints(&self, painter: &egui::Painter, map: &Mapper, pointer: Option<Pos2>) {
        let hovered = || pointer.and_then(|p| self.annot_at(map.to_img(p), map));
        match self.tool {
            Tool::Move => {
                let index = match &self.drag {
                    Drag::EditAnnot { index, .. } => Some(*index),
                    _ => hovered(),
                };
                let Some(a) = index.and_then(|i| self.annots.get(i)) else {
                    return;
                };
                let r = map.rect(a.bounds()).expand(4.0);
                painter.rect_stroke(r, 2.0, Stroke::new(1.5, ACCENT), StrokeKind::Outside);
                if matches!(a.shape, Shape::Text { .. }) {
                    // Handles on both sides: drag them to change the box width.
                    for x in [r.left(), r.right()] {
                        let h = Rect::from_center_size(pos2(x, r.center().y), vec2(HANDLE * 1.6, HANDLE * 3.6));
                        painter.rect_filled(h, 2.0, Color32::WHITE);
                        painter.rect_stroke(h, 2.0, Stroke::new(1.0, ACCENT), StrokeKind::Inside);
                    }
                }
            }
            Tool::Eraser if !matches!(self.drag, Drag::Erase { .. }) => {
                if let Some(a) = hovered().and_then(|i| self.annots.get(i)) {
                    let r = map.rect(a.bounds()).expand(4.0);
                    painter.rect_stroke(r, 2.0, Stroke::new(1.5, ERASER_COLOR), StrokeKind::Outside);
                }
            }
            Tool::Text => {
                if let Drag::TextBox { start, end } = &self.drag {
                    let height = text::glyph_height(self.font_size).max((end.y - start.y).abs());
                    let top_left = pos2(start.x.min(end.x), start.y.min(end.y));
                    let r = Rect::from_min_size(top_left, vec2((end.x - start.x).abs(), height));
                    painter.rect_stroke(map.rect(r), 2.0, Stroke::new(1.0, Color32::WHITE), StrokeKind::Outside);
                }
            }
            _ => {}
        }
    }

    fn paint_magnifier(&self, painter: &egui::Painter, pointer: Pos2, map: &Mapper, screen: Rect) {
        let Some(tex) = &self.shot_tex else { return };
        let p = map.to_img(pointer).floor();
        if !self.img_rect().contains(p) {
            return;
        }

        // Crosshair guides across the screen.
        let guide = Stroke::new(1.0, ACCENT.gamma_multiply(0.6));
        painter.line_segment([pos2(screen.left(), pointer.y), pos2(screen.right(), pointer.y)], guide);
        painter.line_segment([pos2(pointer.x, screen.top()), pos2(pointer.x, screen.bottom())], guide);

        let label_h = 34.0;
        let size = vec2(MAGNIFIER_SIZE, MAGNIFIER_SIZE + label_h);
        let mut pos = pointer + vec2(24.0, 24.0);
        if pos.x + size.x > screen.right() {
            pos.x = pointer.x - 24.0 - size.x;
        }
        if pos.y + size.y > screen.bottom() {
            pos.y = pointer.y - 24.0 - size.y;
        }
        let zoom_rect = Rect::from_min_size(pos, Vec2::splat(MAGNIFIER_SIZE));
        let half = MAGNIFIER_SIZE / MAGNIFIER_ZOOM / 2.0;
        let center = p + vec2(0.5, 0.5);
        painter.rect_filled(zoom_rect, 0.0, Color32::BLACK);
        let src = Rect::from_center_size(center, Vec2::splat(half * 2.0));
        let visible = src.intersect(self.img_rect());
        let to_zoom = |p: Pos2| zoom_rect.min + (p - src.min) * MAGNIFIER_ZOOM;
        tex.paint(painter, visible, Rect::from_min_max(to_zoom(visible.min), to_zoom(visible.max)), Color32::WHITE);
        let cell = MAGNIFIER_ZOOM;
        let c = zoom_rect.center();
        painter.rect_stroke(
            Rect::from_center_size(c, Vec2::splat(cell)),
            0.0,
            Stroke::new(1.0, Color32::WHITE),
            StrokeKind::Middle,
        );
        painter.rect_stroke(zoom_rect, 0.0, Stroke::new(1.0, ACCENT), StrokeKind::Outside);

        let px = self.shot.get_pixel(p.x as u32, p.y as u32).0;
        let info = Rect::from_min_size(zoom_rect.left_bottom(), vec2(MAGNIFIER_SIZE, label_h));
        painter.rect_filled(info, 0.0, Color32::from_black_alpha(220));
        painter.text(
            info.left_top() + vec2(6.0, 3.0),
            Align2::LEFT_TOP,
            format!("{}, {}\n#{:02X}{:02X}{:02X}", p.x, p.y, px[0], px[1], px[2]),
            FontId::monospace(11.0),
            Color32::WHITE,
        );
    }

    // -------------------------------------------------------------- toolbars

    fn mode_bar(&mut self, ctx: &Context, screen: Rect, cx: &mut Cx<'_>) {
        Area::new(Id::new("mode_bar"))
            .order(Order::Foreground)
            .anchor(Align2::CENTER_TOP, vec2(0.0, 16.0))
            .constrain_to(screen)
            .show(ctx, |ui| {
                Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for (shape, icon, label) in [
                            (SelShape::Rect, "⬜", "Rectangle"),
                            (SelShape::Ellipse, "⭕", "Ellipse"),
                            (SelShape::Free, "〰", "Freehand"),
                        ] {
                            let text = RichText::new(format!("{icon} {label}"));
                            if ui.selectable_label(self.sel_shape == shape, text).clicked() {
                                self.sel_shape = shape;
                            }
                        }
                        ui.separator();
                        if ui.button("🖵 Full screen").clicked() {
                            self.select_all();
                        }
                        if ui.button("✖").on_hover_text("Cancel (Esc)").clicked() {
                            self.cancel(cx);
                        }
                    });
                    ui.label(
                        RichText::new(
                            "Drag to select · click for full screen · Shift = square · Esc to cancel",
                        )
                        .small()
                        .weak(),
                    );
                });
            });
    }

    fn toolbar(&mut self, ctx: &Context, map: &Mapper, screen: Rect, cx: &mut Cx<'_>) {
        let Some(sel) = &self.sel else { return };
        let sel_screen = map.rect(sel.rect);
        let size = self.toolbar_size;
        let gap = 8.0;
        let mut pos = pos2(sel_screen.right() - size.x, sel_screen.bottom() + gap);
        if pos.y + size.y > screen.bottom() - 4.0 {
            pos.y = sel_screen.top() - size.y - gap;
            if pos.y < screen.top() + 4.0 {
                pos.y = sel_screen.bottom() - size.y - gap;
            }
        }
        pos.x = pos.x.clamp(screen.left() + 4.0, (screen.right() - size.x - 4.0).max(4.0));
        pos.y = pos.y.clamp(screen.top() + 4.0, (screen.bottom() - size.y - 4.0).max(4.0));

        let inner = Area::new(Id::new("toolbar"))
            .order(Order::Foreground)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                Frame::popup(ui.style())
                    .show(ui, |ui| self.toolbar_contents(ui, ctx, cx))
                    .inner
            });
        self.toolbar_size = inner.response.rect.size();
        self.run_action(inner.inner, cx);
    }

    fn run_action(&mut self, action: Option<Action>, cx: &mut Cx<'_>) {
        match action {
            Some(Action::Scroll) => self.start_scroll_capture(cx),
            Some(action) => self.finish(action, cx),
            None => {}
        }
    }

    /// Tool buttons, actions and style controls. Returns the action the user picked.
    fn toolbar_contents(&mut self, ui: &mut egui::Ui, ctx: &Context, cx: &mut Cx<'_>) -> Option<Action> {
        let mut action = None;
        let keys = cx.shared.settings.keymap.clone();
        let big = |s: &str| RichText::new(s).size(17.0);
        ui.horizontal(|ui| {
            let mut button_size = Vec2::ZERO;
            for tool in self.tools() {
                let selected = self.tool == tool;
                let button = egui::Button::selectable(selected, big(tool.icon())).min_size(button_size);
                let resp = ui.add(button);
                if tool == Tool::Eraser {
                    let color = if selected {
                        ui.visuals().selection.stroke.color
                    } else {
                        ui.visuals().widgets.style(&resp).fg_stroke.color
                    };
                    paint_eraser_icon(ui.painter(), resp.rect, color);
                }
                button_size = resp.rect.size();
                if resp
                    .on_hover_text(format!("{}{}", tool.label(), cx.shared.settings.keymap.hint(tool.act())))
                    .clicked()
                {
                    self.set_tool(tool, ctx);
                }
            }
            ui.separator();
            if ui
                .add_enabled(!self.undo_stack.is_empty() || self.draft.is_some(), egui::Button::new(big("⟲")))
                .on_hover_text(format!("Undo{}", keys.hint(Act::Undo)))
                .clicked()
            {
                self.undo();
            }
            if ui
                .add_enabled(!self.redo_stack.is_empty(), egui::Button::new(big("⟳")))
                .on_hover_text(format!("Redo{}", keys.hint(Act::Redo)))
                .clicked()
            {
                self.redo();
            }
            if ui
                .add_enabled(!self.annots.is_empty() || self.draft.is_some(), egui::Button::new(big("🗑")))
                .on_hover_text(format!("Clear all annotations{} (can be undone)", keys.hint(Act::ClearAll)))
                .clicked()
            {
                self.clear_all();
            }
            ui.separator();
            if !self.editor
                && ui
                    .button(big("📜"))
                    .on_hover_text(format!(
                        "Scrolling capture{}: capture this area while the page scrolls (long screenshot)",
                        keys.hint(Act::ScrollCapture)
                    ))
                    .clicked()
            {
                action = Some(Action::Scroll);
            }
            for (icon, key, act) in [
                ("📌", Act::Pin, Action::Pin),
                ("🗁", Act::SaveAs, Action::SaveAs),
                ("💾", Act::Save, Action::Save),
                ("📋", Act::Copy, Action::Copy),
            ] {
                let tip = format!("{}{}", key.label(), keys.hint(key));
                if ui.button(big(icon)).on_hover_text(tip).clicked() {
                    action = Some(act);
                }
            }
            if !self.editor && ui.button(big("✖")).on_hover_text("Cancel (Esc)").clicked() {
                self.cancel(cx);
            }
        });
        ui.horizontal(|ui| {
            for color in PALETTE {
                let (rect, resp) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::click());
                let ring = if self.color == color {
                    Stroke::new(2.0, Color32::WHITE)
                } else if resp.hovered() {
                    Stroke::new(1.0, Color32::LIGHT_GRAY)
                } else {
                    Stroke::new(1.0, Color32::from_gray(70))
                };
                ui.painter().rect_filled(rect, 4.0, color);
                ui.painter().rect_stroke(rect, 4.0, ring, StrokeKind::Outside);
                if resp.clicked() {
                    self.color = color;
                }
            }
            ui.color_edit_button_srgba(&mut self.color)
                .on_hover_text("Custom color");
            ui.separator();
            if self.tool == Tool::Text || self.draft.is_some() {
                ui.label("Font");
                ui.add(egui::Slider::new(&mut self.font_size, 10.0..=120.0).integer());
            } else {
                ui.label("Size");
                ui.add(egui::Slider::new(&mut self.width, 1.0..=24.0).integer());
            }
            if self.tool.supports_fill() {
                ui.toggle_value(&mut self.filled, "⬛ Fill");
            }
        });
        action
    }

    /// The standalone editor: toolbar on top, the image (fit to width, scrollable) below.
    fn editor_ui(&mut self, ui: &mut egui::Ui, cx: &mut Cx<'_>) {
        let ctx = ui.ctx().clone();
        self.ensure_textures(&ctx);
        self.handle_keys(&ctx, cx);

        let zoom_delta = ctx.input(|i| i.zoom_delta());
        if zoom_delta != 1.0 {
            self.zoom = (self.zoom * zoom_delta).clamp(0.1, 8.0);
        }

        let mut action = None;
        egui::Panel::top("editor_toolbar")
            .frame(Frame::new().fill(ui.visuals().window_fill).inner_margin(Margin::same(8)))
            .show(ui, |ui| {
                action = self.toolbar_contents(ui, &ctx, cx);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Zoom").small().weak());
                    if ui.small_button("➖").clicked() {
                        self.zoom = (self.zoom / 1.25).max(0.1);
                    }
                    ui.label(RichText::new(format!("{:.0}%", self.zoom * 100.0)).small());
                    if ui.small_button("➕").clicked() {
                        self.zoom = (self.zoom * 1.25).min(8.0);
                    }
                    if ui.small_button("Fit").clicked() {
                        self.zoom = 1.0;
                    }
                    ui.label(RichText::new("Ctrl + mouse wheel to zoom").small().weak());
                    if let Some((text, at)) = &self.status {
                        if at.elapsed().as_secs() < 3 {
                            ui.label(RichText::new(text).small().color(ACCENT));
                            ctx.request_repaint_after(std::time::Duration::from_millis(500));
                        }
                    }
                });
            });
        self.run_action(action, cx);

        egui::CentralPanel::default()
            .frame(Frame::new().fill(Color32::from_rgb(16, 16, 18)))
            .show(ui, |ui| {
                let img = self.img_size();
                let ppp = ctx.pixels_per_point();
                let margin = 16.0;
                // Points per image pixel: fit the width, never larger than 1:1 on screen.
                let fit = ((ui.available_width() - 2.0 * margin) / img.x).min(1.0 / ppp).max(0.01);
                let scale = fit * self.zoom;
                let shown = img * scale;
                egui::ScrollArea::both()
                    .auto_shrink([false, false])
                    .scroll_source(egui::scroll_area::ScrollSource::SCROLL_BAR | egui::scroll_area::ScrollSource::MOUSE_WHEEL)
                    .show(ui, |ui| {
                        let total = vec2(
                            (shown.x + 2.0 * margin).max(ui.available_width()),
                            shown.y + 2.0 * margin,
                        );
                        let (area, response) = ui.allocate_exact_size(total, Sense::click_and_drag());
                        let image_rect = Rect::from_min_size(
                            pos2(area.center().x - shown.x / 2.0, area.min.y + margin),
                            shown,
                        );
                        let map = Mapper {
                            origin: image_rect.min,
                            ppp: 1.0 / scale,
                        };
                        self.handle_pointer(&ctx, &response, &map, cx);

                        let painter = ui.painter();
                        if let Some(tex) = &self.shot_tex {
                            tex.paint(painter, self.img_rect(), image_rect, Color32::WHITE);
                        }
                        let clipped = painter.with_clip_rect(image_rect.intersect(painter.clip_rect()));
                        for a in self.annots.iter().chain(self.current.as_ref()) {
                            annot::paint(&clipped, a, &map, self.effect_textures());
                        }
                        self.paint_tool_hints(painter, &map, response.hover_pos());
                        painter.rect_stroke(
                            image_rect,
                            0.0,
                            Stroke::new(1.0, Color32::from_gray(60)),
                            StrokeKind::Outside,
                        );
                        self.text_draft_ui(&ctx, &map);
                    });
            });
    }

    fn text_draft_ui(&mut self, ctx: &Context, map: &Mapper) {
        let font = FontId::proportional(map.len(self.font_size));
        let color = self.color;
        let Some(draft) = &mut self.draft else { return };
        let pos = map.to_screen(draft.pos);
        let wrap = draft.box_width.map(|w| map.len(w));
        let width = wrap.unwrap_or_else(|| {
            let shown: Vec<String> = draft.text.split('\n').map(text::visual_same_length).collect();
            ctx.fonts_mut(|f| f.layout_no_wrap(format!("{}W", shown.join("\n")), font.clone(), color))
                .size()
                .x
                .max(20.0)
        });
        // Wrap only at the box width (or never): the area's own width lags a frame
        // behind the text and would otherwise break lines after every few letters.
        // Arabic is shown joined and right-to-left while typing, as it will be drawn.
        let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, _: f32| {
            let shown: Vec<String> = buf.as_str().split('\n').map(text::visual_same_length).collect();
            let job = egui::text::LayoutJob::simple(
                shown.join("\n"),
                font.clone(),
                color,
                wrap.unwrap_or(f32::INFINITY),
            );
            ui.ctx().fonts_mut(|f| f.layout_job(job))
        };
        let inner = Area::new(Id::new("text_draft"))
            .order(Order::Foreground)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                ui.set_min_width(width);
                ui.add(
                    TextEdit::multiline(&mut draft.text)
                        .font(font.clone())
                        .text_color(color)
                        .frame(Frame::NONE)
                        .margin(Margin::ZERO)
                        .desired_rows(1)
                        .desired_width(width)
                        .layouter(&mut layouter),
                )
            })
            .inner;
        if !draft.focused {
            inner.request_focus();
            draft.focused = true;
        }
        ctx.layer_painter(egui::LayerId::new(Order::Foreground, Id::new("text_draft_box")))
            .rect_stroke(
                inner.rect.expand(3.0),
                2.0,
                Stroke::new(1.0, Color32::from_white_alpha(180)),
                StrokeKind::Outside,
            );
    }
}

impl View for CaptureView {
    fn kind(&self) -> ViewKind {
        if self.editor {
            ViewKind::Editor
        } else {
            ViewKind::Capture
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, cx: &mut Cx<'_>) {
        if self.editor {
            self.editor_ui(ui, cx);
            return;
        }
        let ctx = ui.ctx().clone();
        let screen = ctx.content_rect();
        let map = Mapper {
            origin: screen.min,
            ppp: ctx.pixels_per_point(),
        };
        self.ensure_textures(&ctx);

        let response = ui.allocate_rect(screen, Sense::click_and_drag());
        self.handle_pointer(&ctx, &response, &map, cx);
        self.handle_keys(&ctx, cx);
        if cx.close {
            return;
        }

        let painter = ui.painter().clone();
        self.paint_scene(&painter, &map, screen);
        self.paint_tool_hints(&painter, &map, response.hover_pos());

        if self.editing() {
            self.text_draft_ui(&ctx, &map);
            self.toolbar(&ctx, &map, screen, cx);
        } else if self.sel.is_none() && matches!(self.drag, Drag::None) {
            self.mode_bar(&ctx, screen, cx);
        }

        let show_magnifier = matches!(
            self.drag,
            Drag::NewSel(_) | Drag::Free(_) | Drag::Resize { .. }
        ) || (self.sel.is_none() && response.hovered());
        if show_magnifier {
            if let Some(pointer) = ctx.input(|i| i.pointer.hover_pos()) {
                let top = ctx.layer_painter(egui::LayerId::new(Order::Tooltip, Id::new("magnifier")));
                self.paint_magnifier(&top, pointer, &map, screen);
            }
        }
    }
}

/// A tilted eraser, since egui's fonts have no eraser symbol.
fn paint_eraser_icon(painter: &egui::Painter, rect: Rect, color: Color32) {
    let c = rect.center();
    let along = Vec2::angled(-0.75);
    let across = along.rot90();
    let corner = |a: f32, b: f32| c + along * a + across * b;
    let tip = vec![corner(-8.0, -4.5), corner(-1.5, -4.5), corner(-1.5, 4.5), corner(-8.0, 4.5)];
    let body = vec![corner(-8.0, -4.5), corner(8.0, -4.5), corner(8.0, 4.5), corner(-8.0, 4.5)];
    painter.add(egui::Shape::convex_polygon(tip, color, Stroke::NONE));
    painter.add(egui::Shape::closed_line(body, Stroke::new(1.5, color)));
}

fn round_rect(r: Rect) -> Rect {
    Rect::from_min_max(r.min.round(), r.max.round())
}

/// A mesh that shows the screenshot inside `polygon` (image coordinates).
fn textured_polygon(
    polygon: &[Pos2],
    tex: egui::TextureId,
    img_size: Vec2,
    map: &Mapper,
) -> Option<Mesh> {
    if polygon.len() < 3 {
        return None;
    }
    let mut indices: Vec<u32> = Vec::new();
    earcut::Earcut::new().earcut(
        polygon.iter().map(|p| [p.x as f64, p.y as f64]),
        &[] as &[u32],
        &mut indices,
    );
    let mut mesh = Mesh::with_texture(tex);
    for p in polygon {
        mesh.vertices.push(egui::epaint::Vertex {
            pos: map.to_screen(*p),
            uv: (p.to_vec2() / img_size).to_pos2(),
            color: Color32::WHITE,
        });
    }
    mesh.indices = indices;
    Some(mesh)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAP: Mapper = Mapper { origin: Pos2::ZERO, ppp: 1.0 };

    fn view() -> CaptureView {
        let shot = RgbaImage::new(200, 100);
        CaptureView::editor(shot, &crate::settings::Settings::default())
    }

    fn line(view: &CaptureView, x: f32) -> Annot {
        view.new_annot(Shape::Line(pos2(x, 10.0), pos2(x, 90.0)))
    }

    #[test]
    fn clear_all_and_move_can_be_undone() {
        let mut v = view();
        v.push_annot(line(&v, 10.0));
        v.push_annot(line(&v, 50.0));

        v.clear_all();
        assert!(v.annots.is_empty());
        v.undo();
        assert_eq!(v.annots.len(), 2);
        v.redo();
        assert!(v.annots.is_empty());
        v.undo();

        // Drag the second line 30 px to the right.
        v.drag = Drag::EditAnnot {
            index: 1,
            grip: Grip::Body,
            grab: pos2(50.0, 50.0),
            orig: v.annots[1].clone(),
            moved: false,
        };
        v.update_drag(pos2(80.0, 50.0), pos2(80.0, 50.0), false, &MAP);
        v.end_drag();
        assert!(matches!(v.annots[1].shape, Shape::Line(a, _) if a.x == 80.0));
        v.undo();
        assert!(matches!(v.annots[1].shape, Shape::Line(a, _) if a.x == 50.0));
    }

    #[test]
    fn eraser_drag_deletes_what_it_crosses_in_one_undo_step() {
        let mut v = view();
        for x in [20.0, 60.0, 150.0] {
            v.push_annot(line(&v, x));
        }
        v.tool = Tool::Eraser;
        v.begin_drag(Pos2::ZERO, pos2(10.0, 50.0), &MAP);
        v.update_drag(pos2(100.0, 50.0), pos2(100.0, 50.0), false, &MAP);
        v.end_drag();
        assert_eq!(v.annots.len(), 1, "the lines at x=20 and x=60 are erased");
        v.undo();
        assert_eq!(v.annots.len(), 3);
    }

    #[test]
    fn dragging_a_text_edge_sets_its_box_width() {
        let mut v = view();
        v.push_annot(v.new_annot(Shape::Text { pos: pos2(10.0, 10.0), text: "hello world".into(), box_width: None }));
        let b = v.annots[0].bounds();
        v.tool = Tool::Move;
        let edge = pos2(b.max.x, b.center().y);
        assert!(matches!(v.grip_at(edge, &MAP), Some((0, Grip::Right))));
        v.begin_drag(Pos2::ZERO, edge, &MAP);
        let target = pos2(b.max.x - 30.0, edge.y);
        v.update_drag(target, target, false, &MAP);
        v.end_drag();
        let Shape::Text { pos, box_width, .. } = &v.annots[0].shape else { unreachable!() };
        assert_eq!(*pos, pos2(10.0, 10.0));
        assert_eq!(*box_width, Some(b.width() - 30.0));
    }

    #[test]
    fn a_click_without_movement_adds_no_undo_step() {
        let mut v = view();
        v.push_annot(line(&v, 10.0));
        v.drag = Drag::EditAnnot {
            index: 0,
            grip: Grip::Body,
            grab: pos2(10.0, 50.0),
            orig: v.annots[0].clone(),
            moved: false,
        };
        v.end_drag();
        assert_eq!(v.undo_stack.len(), 1);
    }
}
