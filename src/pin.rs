//! A screenshot pinned on top of all windows.

use egui::{Color32, Key, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, ViewportCommand, vec2};
use image::RgbaImage;

use crate::app::Command;
use crate::host::{Cx, View, ViewKind};
use crate::theme::ACCENT;

pub struct PinView {
    image: RgbaImage,
    tex: Option<TextureHandle>,
    zoom: f32,
}

impl PinView {
    pub fn new(image: RgbaImage) -> Self {
        Self {
            image,
            tex: None,
            zoom: 1.0,
        }
    }
}

impl View for PinView {
    fn kind(&self) -> ViewKind {
        ViewKind::Pin
    }

    fn ui(&mut self, ui: &mut egui::Ui, cx: &mut Cx<'_>) {
        let ctx = ui.ctx().clone();
        let tex = self
            .tex
            .get_or_insert_with(|| {
                let size = [self.image.width() as usize, self.image.height() as usize];
                let img = egui::ColorImage::from_rgba_unmultiplied(size, self.image.as_raw());
                ctx.load_texture("pin", img, TextureOptions::LINEAR)
            })
            .clone();

        let rect = ctx.content_rect();
        let response = ui.allocate_rect(rect, Sense::click_and_drag());
        ui.painter().rect_filled(rect, 0.0, Color32::from_gray(40));
        ui.painter().image(
            tex.id(),
            rect,
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        let border = if response.hovered() { ACCENT } else { Color32::from_gray(90) };
        ui.painter()
            .rect_stroke(rect, 0.0, Stroke::new(1.0, border), StrokeKind::Inside);

        if response.drag_started() {
            ctx.send_viewport_cmd(ViewportCommand::StartDrag);
        }
        if response.double_clicked() || ctx.input(|i| i.key_pressed(Key::Escape)) {
            cx.close = true;
        }

        // Scroll to zoom the pinned image.
        let scroll = ctx.input(|i| i.smooth_scroll_delta.y);
        if response.hovered() && scroll != 0.0 {
            self.zoom = (self.zoom * (1.0 + scroll * 0.002)).clamp(0.1, 4.0);
            let ppp = ctx.pixels_per_point();
            let size = vec2(self.image.width() as f32, self.image.height() as f32) / ppp * self.zoom;
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(size));
        }

        response.context_menu(|ui| {
            if ui.button("📋 Copy").clicked() {
                cx.commands.push(Command::CopyImage(self.image.clone()));
                ui.close();
            }
            if ui.button("🗁 Save as…").clicked() {
                cx.commands.push(Command::SaveImageAs(self.image.clone()));
                ui.close();
            }
            if ui.button("1:1 Original size").clicked() {
                self.zoom = 1.0;
                let size = vec2(self.image.width() as f32, self.image.height() as f32)
                    / ctx.pixels_per_point();
                ctx.send_viewport_cmd(ViewportCommand::InnerSize(size));
                ui.close();
            }
            ui.separator();
            if ui.button("✖ Close").clicked() {
                cx.close = true;
                ui.close();
            }
        });
        response.on_hover_text("Drag to move · scroll to zoom · double-click or Esc to close · right-click for more");
    }
}
