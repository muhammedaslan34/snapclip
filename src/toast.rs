//! Small popups that don't steal focus: the post-capture notification and the
//! scrolling-capture controls.

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use egui::{
    Align, Color32, Frame, Layout, Margin, RichText, Sense, Stroke, TextureHandle, TextureOptions,
};
use image::RgbaImage;

use crate::app::Command;
use crate::host::{Cx, View, ViewKind};
use crate::scroll::{CANCEL, FINISH};
use crate::theme::ACCENT;

const TOAST_LIFETIME: Duration = Duration::from_secs(7);

fn popup_frame(ui: &egui::Ui) -> Frame {
    Frame::new()
        .fill(ui.visuals().window_fill)
        .stroke(Stroke::new(1.0, Color32::from_gray(70)))
        .inner_margin(Margin::same(10))
}

/// "Screenshot copied" notification with a preview. Click it to open the editor.
pub struct ToastView {
    image: RgbaImage,
    tex: Option<TextureHandle>,
    title: String,
    origin: (i32, i32),
    last_activity: Instant,
}

impl ToastView {
    pub fn new(image: RgbaImage, title: String, origin: (i32, i32)) -> Self {
        Self {
            image,
            tex: None,
            title,
            origin,
            last_activity: Instant::now(),
        }
    }

    fn edit(&self, cx: &mut Cx<'_>) {
        cx.commands.push(Command::OpenEditor(self.image.clone()));
        cx.close = true;
    }
}

impl View for ToastView {
    fn kind(&self) -> ViewKind {
        ViewKind::Toast
    }

    fn takes_focus(&self) -> bool {
        false
    }

    fn ui(&mut self, ui: &mut egui::Ui, cx: &mut Cx<'_>) {
        let ctx = ui.ctx().clone();
        let tex = self
            .tex
            .get_or_insert_with(|| {
                let (w, h) = crate::clip_watch::fit(self.image.width(), self.image.height(), 320, 200);
                let small = image::imageops::thumbnail(&self.image, w, h);
                let size = [small.width() as usize, small.height() as usize];
                let img = egui::ColorImage::from_rgba_unmultiplied(size, small.as_raw());
                ctx.load_texture("toast", img, TextureOptions::LINEAR)
            })
            .clone();

        // Stay open while hovered; otherwise fade out after a few seconds.
        if ctx.input(|i| i.pointer.hover_pos().is_some()) {
            self.last_activity = Instant::now();
        } else if self.last_activity.elapsed() > TOAST_LIFETIME {
            cx.close = true;
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(500));

        egui::CentralPanel::default().frame(popup_frame(ui)).show(ui, |ui| {
            ui.horizontal(|ui| {
                let size = tex.size_vec2();
                let scale = (150.0 / size.x).min(86.0 / size.y);
                let thumb = ui
                    .add(egui::Image::new(&tex).fit_to_exact_size(size * scale).sense(Sense::click()))
                    .on_hover_text("Click to edit");
                if thumb.clicked() {
                    self.edit(cx);
                }
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&self.title).strong());
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.small_button("✖").clicked() {
                                cx.close = true;
                            }
                        });
                    });
                    ui.label(
                        RichText::new(format!("{} × {} · click the preview to edit", self.image.width(), self.image.height()))
                            .small()
                            .weak(),
                    );
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if ui.button(RichText::new("✏ Edit").color(ACCENT)).clicked() {
                            self.edit(cx);
                        }
                        if ui.button("📋").on_hover_text("Copy again").clicked() {
                            cx.commands.push(Command::CopyImage(self.image.clone()));
                        }
                        if ui.button("💾").on_hover_text("Save as…").clicked() {
                            cx.commands.push(Command::SaveImageAs(self.image.clone()));
                            cx.close = true;
                        }
                        if ui.button("📌").on_hover_text("Pin to screen").clicked() {
                            cx.commands.push(Command::Pin(self.image.clone(), self.origin));
                            cx.close = true;
                        }
                    });
                });
            });
        });
    }
}

/// Controls shown while a scrolling capture runs.
pub struct ScrollControlView;

impl View for ScrollControlView {
    fn kind(&self) -> ViewKind {
        ViewKind::ScrollControl
    }

    fn takes_focus(&self) -> bool {
        false
    }

    fn ui(&mut self, ui: &mut egui::Ui, cx: &mut Cx<'_>) {
        let Some(status) = &cx.shared.scroll else {
            cx.close = true;
            return;
        };
        let status = status.clone();
        egui::CentralPanel::default().frame(popup_frame(ui)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("📜 Scrolling capture").strong());
                ui.label(
                    RichText::new(format!("{} frames · {} px", status.frames, status.height))
                        .small()
                        .weak(),
                );
            });
            let mut auto = status.auto_scroll.load(Ordering::Relaxed);
            let hint = if cfg!(windows) {
                "Scrolls the page under the selection for you. Turn off to scroll by hand."
            } else {
                "Scroll the page by hand; frames are stitched as you go."
            };
            if ui
                .add_enabled(cfg!(windows), egui::Checkbox::new(&mut auto, "Auto-scroll"))
                .on_hover_text(hint)
                .changed()
            {
                status.auto_scroll.store(auto, Ordering::Relaxed);
            }
            ui.horizontal(|ui| {
                if ui.button(RichText::new("✔ Done").color(ACCENT)).clicked() {
                    status.state.store(FINISH, Ordering::Relaxed);
                }
                if ui.button("Cancel").clicked() {
                    status.state.store(CANCEL, Ordering::Relaxed);
                }
            });
        });
    }
}
