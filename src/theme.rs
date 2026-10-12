use egui::{Color32, CornerRadius, Stroke, Visuals};

pub const ACCENT: Color32 = Color32::from_rgb(59, 130, 246);

pub fn apply(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    let mut visuals = Visuals::dark();
    visuals.selection.bg_fill = ACCENT;
    visuals.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    visuals.window_corner_radius = CornerRadius::same(10);
    visuals.menu_corner_radius = CornerRadius::same(8);
    visuals.panel_fill = Color32::from_rgb(24, 24, 27);
    visuals.window_fill = Color32::from_rgb(30, 30, 34);
    visuals.extreme_bg_color = Color32::from_rgb(18, 18, 20);
    for widget in [
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.corner_radius = CornerRadius::same(6);
    }
    ctx.set_visuals(visuals);
    ctx.global_style_mut(|style| {
        style.spacing.item_spacing = egui::vec2(6.0, 6.0);
        style.spacing.button_padding = egui::vec2(8.0, 4.0);
    });
}
