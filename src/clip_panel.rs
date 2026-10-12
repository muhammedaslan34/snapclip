//! The clipboard history popup.

use std::collections::HashMap;

use egui::{
    Align, Color32, Frame, Key, Layout, Margin, Modifiers, RichText, ScrollArea, Sense, Stroke,
    TextEdit, TextureHandle, TextureOptions, ViewportCommand,
};

use crate::app::Command;
use crate::gl_window::{Cx, View, ViewKind};
use crate::history::{ClipContent, ClipItem, History};
use crate::shortcuts::Act;
use crate::theme::ACCENT;

const PREVIEW_CHARS: usize = 220;
const PREVIEW_LINES: usize = 3;

#[derive(Default)]
pub struct ClipboardView {
    search: String,
    selected: usize,
    editing: Option<(u64, String)>,
    thumbs: HashMap<u64, Option<TextureHandle>>,
    focus_search: bool,
    scroll_to_selected: bool,
    confirm_clear: bool,
}

impl ClipboardView {
    pub fn new() -> Self {
        Self {
            focus_search: true,
            ..Default::default()
        }
    }

    /// Pinned items first, then most recent; filtered by the search box.
    fn visible(&self, history: &History) -> Vec<u64> {
        let needle = self.search.to_lowercase();
        let matches = |item: &ClipItem| match &item.content {
            ClipContent::Text { text } => needle.is_empty() || text.to_lowercase().contains(&needle),
            ClipContent::Image { .. } => needle.is_empty() || "image".contains(&needle),
        };
        let items = history.items();
        items
            .iter()
            .filter(|i| i.pinned && matches(i))
            .chain(items.iter().filter(|i| !i.pinned && matches(i)))
            .map(|i| i.id)
            .collect()
    }

    fn thumb(&mut self, ctx: &egui::Context, history: &History, item: &ClipItem) -> Option<TextureHandle> {
        let ClipContent::Image { thumb, .. } = &item.content else {
            return None;
        };
        self.thumbs
            .entry(item.id)
            .or_insert_with(|| {
                let img = image::open(history.file_path(thumb)).ok()?.to_rgba8();
                let size = [img.width() as usize, img.height() as usize];
                let color = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
                Some(ctx.load_texture(format!("thumb-{}", item.id), color, TextureOptions::LINEAR))
            })
            .clone()
    }

    fn paste(&mut self, id: u64, cx: &mut Cx<'_>) {
        cx.commands.push(Command::PasteItem(id));
        cx.close = true;
    }

    fn start_edit(&mut self, id: u64, history: &History) {
        if let Some(text) = history.get(id).and_then(ClipItem::text) {
            self.editing = Some((id, text.to_owned()));
        }
    }

    fn handle_keys(&mut self, ctx: &egui::Context, visible: &[u64], cx: &mut Cx<'_>) {
        let key = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_key(m, k));

        if self.editing.is_some() {
            if key(Modifiers::NONE, Key::Escape) {
                self.editing = None;
            } else if key(Modifiers::COMMAND, Key::Enter) || key(Modifiers::COMMAND, Key::S) {
                if let Some((id, text)) = self.editing.take() {
                    cx.shared.history.set_text(id, text);
                }
            }
            return;
        }

        if key(Modifiers::NONE, Key::Escape) {
            cx.close = true;
            return;
        }
        if visible.is_empty() {
            return;
        }
        let last = visible.len() - 1;
        if key(Modifiers::NONE, Key::ArrowDown) {
            self.selected = (self.selected + 1).min(last);
            self.scroll_to_selected = true;
        }
        if key(Modifiers::NONE, Key::ArrowUp) {
            self.selected = self.selected.saturating_sub(1);
            self.scroll_to_selected = true;
        }
        if key(Modifiers::NONE, Key::PageDown) {
            self.selected = (self.selected + 8).min(last);
            self.scroll_to_selected = true;
        }
        if key(Modifiers::NONE, Key::PageUp) {
            self.selected = self.selected.saturating_sub(8);
            self.scroll_to_selected = true;
        }
        self.selected = self.selected.min(last);
        let id = visible[self.selected];

        let keys = cx.shared.settings.keymap.clone();
        if keys.pressed(ctx, Act::ClipPaste) {
            self.paste(id, cx);
        } else if keys.pressed(ctx, Act::ClipDelete) {
            cx.shared.history.remove(id);
            self.thumbs.remove(&id);
        } else if keys.pressed(ctx, Act::ClipPin) {
            cx.shared.history.toggle_pin(id);
        } else if keys.pressed(ctx, Act::ClipEdit) {
            self.start_edit(id, &cx.shared.history);
        }

        const DIGITS: [Key; 9] = [
            Key::Num1,
            Key::Num2,
            Key::Num3,
            Key::Num4,
            Key::Num5,
            Key::Num6,
            Key::Num7,
            Key::Num8,
            Key::Num9,
        ];
        for (n, digit) in DIGITS.into_iter().enumerate() {
            if key(Modifiers::COMMAND, digit) {
                if let Some(&id) = visible.get(n) {
                    self.paste(id, cx);
                }
            }
        }
    }

    fn header(&mut self, ui: &mut egui::Ui, cx: &mut Cx<'_>) {
        ui.horizontal(|ui| {
            let title = ui.add(
                egui::Label::new(RichText::new("📋 Clipboard").strong().size(15.0))
                    .sense(Sense::drag()),
            );
            if title.drag_started() {
                ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button("✖").on_hover_text("Close (Esc)").clicked() {
                    cx.close = true;
                }
                if ui.small_button("⚙").on_hover_text("Settings").clicked() {
                    cx.commands.push(Command::OpenSettings);
                    cx.close = true;
                }
                // Remaining header space also drags the window.
                let rest = ui.allocate_response(ui.available_size(), Sense::drag());
                if rest.drag_started() {
                    ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
                }
            });
        });
    }

    fn row(&mut self, ui: &mut egui::Ui, id: u64, index: usize, cx: &mut Cx<'_>) {
        let Some(item) = cx.shared.history.get(id).cloned() else {
            return;
        };
        let selected = index == self.selected;
        let keys = cx.shared.settings.keymap.clone();

        if let Some((edit_id, text)) = &mut self.editing {
            if *edit_id == id {
                let mut save = false;
                let mut cancel = false;
                Frame::new()
                    .fill(ACCENT.gamma_multiply(0.15))
                    .stroke(Stroke::new(1.0, ACCENT))
                    .corner_radius(8)
                    .inner_margin(Margin::same(8))
                    .show(ui, |ui| {
                        let edit = ui.add(
                            TextEdit::multiline(text)
                                .desired_width(f32::INFINITY)
                                .desired_rows(4),
                        );
                        if !edit.has_focus() && !edit.lost_focus() {
                            edit.request_focus();
                        }
                        ui.horizontal(|ui| {
                            save = ui.button("💾 Save").on_hover_text("Ctrl+Enter").clicked();
                            cancel = ui.button("Cancel").on_hover_text("Esc").clicked();
                        });
                    });
                if save {
                    if let Some((id, text)) = self.editing.take() {
                        cx.shared.history.set_text(id, text);
                    }
                } else if cancel {
                    self.editing = None;
                }
                return;
            }
        }

        let fill = if selected {
            ACCENT.gamma_multiply(0.22)
        } else {
            Color32::from_white_alpha(6)
        };
        let frame = Frame::new()
            .fill(fill)
            .stroke(if selected {
                Stroke::new(1.0, ACCENT)
            } else {
                Stroke::NONE
            })
            .corner_radius(8)
            .inner_margin(Margin::symmetric(10, 8));

        let mut action: Option<RowAction> = None;
        // The row senses clicks itself; its buttons are added later so they sit on top.
        let builder = egui::UiBuilder::new()
            .id_salt(("row", id))
            .sense(Sense::click());
        let click = ui
            .scope_builder(builder, |ui| frame.show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let number = if index < 9 {
                        (index + 1).to_string()
                    } else {
                        " ".into()
                    };
                    ui.label(RichText::new(number).monospace().weak());

                    ui.vertical(|ui| {
                        ui.set_width(ui.available_width() - 84.0);
                        match &item.content {
                            ClipContent::Text { text } => {
                                ui.add(egui::Label::new(preview(text)).selectable(false));
                            }
                            ClipContent::Image { width, height, .. } => {
                                if let Some(tex) = self.thumb(ui.ctx(), &cx.shared.history, &item) {
                                    let size = tex.size_vec2();
                                    let scale = (ui.available_width() / size.x).min(90.0 / size.y).min(1.0);
                                    ui.add(egui::Image::new(&tex).fit_to_exact_size(size * scale));
                                }
                                ui.label(RichText::new(format!("🖼 {width} × {height}")).small().weak());
                            }
                        }
                        let mut meta = time_ago(item.time);
                        if item.pinned {
                            meta = format!("📌 pinned · {meta}");
                        }
                        ui.label(RichText::new(meta).small().weak());
                    });

                    ui.with_layout(Layout::right_to_left(Align::TOP), |ui| {
                        if ui.small_button("🗑").on_hover_text(format!("Delete{}", keys.hint(Act::ClipDelete))).clicked() {
                            action = Some(RowAction::Delete);
                        }
                        if item.text().is_some()
                            && ui.small_button("✏").on_hover_text(format!("Edit{}", keys.hint(Act::ClipEdit))).clicked()
                        {
                            action = Some(RowAction::Edit);
                        }
                        let pin = RichText::new("📌").color(if item.pinned {
                            ACCENT
                        } else {
                            ui.visuals().weak_text_color()
                        });
                        if ui.small_button(pin).on_hover_text(format!("Pin / unpin{}", keys.hint(Act::ClipPin))).clicked() {
                            action = Some(RowAction::Pin);
                        }
                    });
                });
            }))
            .response;

        if click.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if selected && self.scroll_to_selected {
            click.scroll_to_me(None);
            self.scroll_to_selected = false;
        }

        match action {
            Some(RowAction::Delete) => {
                cx.shared.history.remove(id);
                self.thumbs.remove(&id);
            }
            Some(RowAction::Edit) => self.start_edit(id, &cx.shared.history),
            Some(RowAction::Pin) => cx.shared.history.toggle_pin(id),
            None => {
                if click.clicked() {
                    self.paste(id, cx);
                } else if click.contains_pointer() && ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO) {
                    self.selected = index;
                }
            }
        }
    }
}

enum RowAction {
    Delete,
    Edit,
    Pin,
}

impl View for ClipboardView {
    fn kind(&self) -> ViewKind {
        ViewKind::Clipboard
    }

    fn close_on_focus_lost(&self) -> bool {
        true
    }

    fn ui(&mut self, ui: &mut egui::Ui, cx: &mut Cx<'_>) {
        let ctx = ui.ctx().clone();
        let visible = self.visible(&cx.shared.history);
        self.handle_keys(&ctx, &visible, cx);
        if cx.close {
            return;
        }
        let visible = self.visible(&cx.shared.history);

        let frame = Frame::new()
            .fill(ui.visuals().panel_fill)
            .stroke(Stroke::new(1.0, Color32::from_gray(60)))
            .inner_margin(Margin::same(10));
        egui::CentralPanel::default().frame(frame).show(ui, |ui| {
            self.header(ui, cx);

            let search = ui.add(
                TextEdit::singleline(&mut self.search)
                    .hint_text("🔍 Search…")
                    .desired_width(f32::INFINITY),
            );
            if self.focus_search {
                search.request_focus();
                self.focus_search = false;
            }
            if search.changed() {
                self.selected = 0;
            }
            ui.add_space(4.0);

            let footer_h = 30.0;
            ScrollArea::vertical()
                .auto_shrink([false, false])
                .max_height(ui.available_height() - footer_h)
                .show(ui, |ui| {
                    if visible.is_empty() {
                        ui.add_space(40.0);
                        ui.vertical_centered(|ui| {
                            ui.label(RichText::new(if self.search.is_empty() {
                                "Nothing copied yet.\nCopy some text or an image and it will show up here."
                            } else {
                                "No matches."
                            }).weak());
                        });
                    }
                    for (index, &id) in visible.iter().enumerate() {
                        self.row(ui, id, index, cx);
                    }
                });

            ui.separator();
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!(
                        "{} items · {} paste · Ctrl+1…9",
                        cx.shared.history.items().len(),
                        cx.shared.settings.keymap.get(Act::ClipPaste)
                    ))
                        .small()
                        .weak(),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if self.confirm_clear {
                        if ui.small_button("Yes, clear").clicked() {
                            cx.shared.history.clear_unpinned();
                            self.thumbs.clear();
                            self.confirm_clear = false;
                        }
                        if ui.small_button("Keep").clicked() {
                            self.confirm_clear = false;
                        }
                    } else if ui
                        .small_button("Clear")
                        .on_hover_text("Delete all items except pinned ones")
                        .clicked()
                    {
                        self.confirm_clear = true;
                    }
                });
            });
        });
    }
}

fn preview(text: &str) -> String {
    let mut out: String = text
        .trim()
        .lines()
        .take(PREVIEW_LINES)
        .collect::<Vec<_>>()
        .join("\n");
    if out.chars().count() > PREVIEW_CHARS {
        out = out.chars().take(PREVIEW_CHARS).collect::<String>() + "…";
    } else if text.trim().lines().count() > PREVIEW_LINES {
        out.push('…');
    }
    out
}

fn time_ago(time_ms: i64) -> String {
    let secs = (chrono::Utc::now().timestamp_millis() - time_ms).max(0) / 1000;
    match secs {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", secs / 60),
        3600..86400 => format!("{} h ago", secs / 3600),
        _ => format!("{} d ago", secs / 86400),
    }
}
