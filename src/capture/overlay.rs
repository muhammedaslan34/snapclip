use egui::{
    Align2, Area, Color32, Context, CursorIcon, FontId, Frame, Id, Key, KeyboardShortcut,
    Margin, Mesh, Modifiers, Order, PointerButton, Pos2, Rect, Response, RichText, Sense, Stroke,
    StrokeKind, TextEdit, TextureHandle, TextureOptions, Vec2, pos2, vec2,
};
use image::RgbaImage;

use super::annot::{self, Annot, Mapper, SelShape, Selection, Shape, Tool};
use super::render;
use crate::app::Command;
use crate::gl_window::{Cx, View, ViewKind};
use crate::shortcuts::Act;
use crate::theme::ACCENT;

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
}

struct TextDraft {
    pos: Pos2,
    text: String,
    focused: bool,
}

pub struct CaptureView {
    shot: RgbaImage,
    /// Physical desktop position of the captured monitor, used to place pinned images.
    monitor_pos: (i32, i32),
    shot_tex: Option<TextureHandle>,
    pixelated: Option<RgbaImage>,
    pix_tex: Option<TextureHandle>,
    sel_shape: SelShape,
    sel: Option<Selection>,
    drag: Drag,
    tool: Tool,
    color: Color32,
    width: f32,
    font_size: f32,
    filled: bool,
    annots: Vec<Annot>,
    redo: Vec<Annot>,
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
            sel_shape: SelShape::Rect,
            sel: None,
            drag: Drag::None,
            tool: Tool::Adjust,
            color: Color32::from_rgba_unmultiplied(r, g, b, a),
            width: settings.pen_width.clamp(1.0, 24.0),
            font_size: settings.font_size.clamp(10.0, 120.0),
            filled: false,
            annots: Vec::new(),
            redo: Vec::new(),
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
            let size = [self.shot.width() as usize, self.shot.height() as usize];
            let image = egui::ColorImage::from_rgba_unmultiplied(size, self.shot.as_raw());
            // NEAREST keeps the 1:1 overlay crisp and the magnifier pixel-exact;
            // the editor scales the image, where LINEAR looks better.
            let options = if self.editor {
                TextureOptions::LINEAR
            } else {
                TextureOptions::NEAREST
            };
            self.shot_tex = Some(ctx.load_texture("screenshot", image, options));
        }
    }

    fn ensure_pixelated(&mut self, ctx: Option<&Context>) {
        if self.pixelated.is_none() {
            let block = (self.shot.width() / 160).max(8);
            self.pixelated = Some(render::pixelate(&self.shot, block));
        }
        if let (Some(ctx), None, Some(pix)) = (ctx, &self.pix_tex, &self.pixelated) {
            let size = [pix.width() as usize, pix.height() as usize];
            let image = egui::ColorImage::from_rgba_unmultiplied(size, pix.as_raw());
            self.pix_tex = Some(ctx.load_texture("pixelated", image, TextureOptions::NEAREST));
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

    fn push_annot(&mut self, annot: Annot) {
        if !annot.is_degenerate() {
            self.annots.push(annot);
            self.redo.clear();
        }
    }

    fn set_tool(&mut self, tool: Tool, ctx: &Context) {
        self.commit_draft();
        if tool == Tool::Pixelate {
            self.ensure_pixelated(Some(ctx));
        }
        self.tool = tool;
    }

    fn commit_draft(&mut self) {
        if let Some(draft) = self.draft.take() {
            let text = draft.text.trim_end().to_owned();
            self.push_annot(self.new_annot(Shape::Text {
                pos: draft.pos,
                text,
            }));
        }
    }

    fn undo(&mut self) {
        self.commit_draft();
        if let Some(a) = self.annots.pop() {
            self.redo.push(a);
        }
    }

    fn redo(&mut self) {
        if let Some(a) = self.redo.pop() {
            self.annots.push(a);
        }
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
        if self
            .annots
            .iter()
            .any(|a| matches!(a.shape, Shape::Pixelate(_)))
        {
            self.ensure_pixelated(None);
        }
        let Some(sel) = &self.sel else { return };
        let Some(image) = render::export(&self.shot, self.pixelated.as_ref(), sel, &self.annots)
        else {
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
            self.update_drag(p, raw_img, shift);
        }
        if resp.drag_stopped_by(PointerButton::Primary) {
            self.end_drag();
        }
        if resp.clicked_by(PointerButton::Primary) {
            self.click(p);
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
            Drag::Move { .. } => return CursorIcon::Grabbing,
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
            (Some(_), Tool::Text | Tool::Step) => Drag::None,
            (Some(_), tool) => {
                let shape = match tool {
                    Tool::Pen => Shape::Pen(vec![origin]),
                    Tool::Marker => Shape::Marker(vec![origin]),
                    Tool::Line => Shape::Line(origin, origin),
                    Tool::Arrow => Shape::Arrow(origin, origin),
                    Tool::Rect => Shape::Rect(Rect::from_two_pos(origin, origin)),
                    Tool::Ellipse => Shape::Ellipse(Rect::from_two_pos(origin, origin)),
                    _ => Shape::Pixelate(Rect::from_two_pos(origin, origin)),
                };
                self.current = Some(self.new_annot(shape));
                Drag::Draw(origin)
            }
        };
    }

    fn update_drag(&mut self, p: Pos2, raw: Pos2, shift: bool) {
        let bounds = self.img_rect();
        match &mut self.drag {
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
                    Shape::Rect(r) | Shape::Ellipse(r) | Shape::Pixelate(r) => {
                        let end = if shift { annot::square(start, p) } else { p };
                        *r = Rect::from_two_pos(start, end);
                    }
                    Shape::Text { .. } | Shape::Step { .. } => {}
                }
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
            Drag::None | Drag::Move { .. } | Drag::Resize { .. } => {}
        }
    }

    fn click(&mut self, p: Pos2) {
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
                    focused: false,
                });
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
                self.drag = Drag::None;
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
        let full_uv = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
        painter.image(tex.id(), map.rect(self.img_rect()), full_uv, Color32::WHITE);
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
            SelShape::Rect => {
                let uv = Rect::from_min_max(
                    (sel.rect.min.to_vec2() / img_size).to_pos2(),
                    (sel.rect.max.to_vec2() / img_size).to_pos2(),
                );
                painter.image(tex.id(), sel_screen, uv, Color32::WHITE);
            }
            _ => {
                if let Some(mesh) = textured_polygon(&polygon, tex.id(), img_size, map) {
                    painter.add(mesh);
                }
            }
        }

        let clipped = painter.with_clip_rect(sel_screen.intersect(screen));
        let pix = self.pix_tex.as_ref().map(|t| t.id());
        for a in self.annots.iter().chain(self.current.as_ref()) {
            annot::paint(&clipped, a, map, pix, img_size);
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

    fn paint_magnifier(&self, painter: &egui::Painter, pointer: Pos2, map: &Mapper, screen: Rect) {
        let Some(tex) = &self.shot_tex else { return };
        let img_size = self.img_size();
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
        let uv = Rect::from_min_max(
            ((center - Vec2::splat(half)).to_vec2() / img_size).to_pos2(),
            ((center + Vec2::splat(half)).to_vec2() / img_size).to_pos2(),
        );
        painter.rect_filled(zoom_rect, 0.0, Color32::BLACK);
        painter.image(tex.id(), zoom_rect, uv, Color32::WHITE);
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
            for tool in self.tools() {
                if ui
                    .selectable_label(self.tool == tool, big(tool.icon()))
                    .on_hover_text(format!("{}{}", tool.label(), cx.shared.settings.keymap.hint(tool.act())))
                    .clicked()
                {
                    self.set_tool(tool, ctx);
                }
            }
            ui.separator();
            if ui
                .add_enabled(!self.annots.is_empty() || self.draft.is_some(), egui::Button::new(big("⟲")))
                .on_hover_text(format!("Undo{}", keys.hint(Act::Undo)))
                .clicked()
            {
                self.undo();
            }
            if ui
                .add_enabled(!self.redo.is_empty(), egui::Button::new(big("⟳")))
                .on_hover_text(format!("Redo{}", keys.hint(Act::Redo)))
                .clicked()
            {
                self.redo();
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
                        let full_uv = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
                        if let Some(tex) = &self.shot_tex {
                            painter.image(tex.id(), image_rect, full_uv, Color32::WHITE);
                        }
                        let clipped = painter.with_clip_rect(image_rect.intersect(painter.clip_rect()));
                        let pix = self.pix_tex.as_ref().map(|t| t.id());
                        for a in self.annots.iter().chain(self.current.as_ref()) {
                            annot::paint(&clipped, a, &map, pix, img);
                        }
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
        let width = ctx
            .fonts_mut(|f| f.layout_no_wrap(format!("{}W", draft.text), font.clone(), color))
            .size()
            .x;
        let inner = Area::new(Id::new("text_draft"))
            .order(Order::Foreground)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                ui.add(
                    TextEdit::multiline(&mut draft.text)
                        .font(font)
                        .text_color(color)
                        .frame(Frame::NONE)
                        .margin(Margin::ZERO)
                        .desired_rows(1)
                        .desired_width(width.max(20.0)),
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
