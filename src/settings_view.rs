use std::collections::HashMap;

use egui::{CollapsingHeader, Event, Grid, RichText, TextEdit};

use crate::app::Command;
use crate::host::{Cx, View, ViewKind};
use crate::settings::{AfterCapture, Settings};
use crate::shortcuts::{self, Act, Group, Keymap};

#[derive(Clone, Copy, PartialEq)]
enum Field {
    Capture,
    Clipboard,
    App(Act),
}

pub struct SettingsView {
    capture: String,
    clipboard: String,
    /// Editable text for each in-app shortcut.
    keys: HashMap<Act, String>,
    recording: Option<Field>,
}

impl SettingsView {
    pub fn new(settings: &Settings) -> Self {
        let mut view = Self {
            capture: String::new(),
            clipboard: String::new(),
            keys: HashMap::new(),
            recording: None,
        };
        view.load(settings);
        view
    }

    fn load(&mut self, settings: &Settings) {
        self.capture = settings.hotkey_capture.clone();
        self.clipboard = settings.hotkey_clipboard.clone();
        self.keys = Act::ALL
            .iter()
            .map(|&act| (act, settings.keymap.get(act).to_owned()))
            .collect();
    }

    fn value_mut(&mut self, field: Field) -> &mut String {
        match field {
            Field::Capture => &mut self.capture,
            Field::Clipboard => &mut self.clipboard,
            Field::App(act) => self.keys.entry(act).or_default(),
        }
    }

    fn draft_keymap(&self) -> Keymap {
        let mut keymap = Keymap::default();
        for (&act, keys) in &self.keys {
            keymap.set(act, keys.trim().to_owned());
        }
        keymap
    }

    fn record(&mut self, ctx: &egui::Context) {
        let Some(field) = self.recording else { return };
        let pressed = ctx.input_mut(|i| {
            let found = i.events.iter().find_map(|e| match e {
                Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => Some((*key, *modifiers)),
                _ => None,
            });
            if found.is_some() {
                i.events.retain(|e| !matches!(e, Event::Key { .. } | Event::Text(_)));
            }
            found
        });
        let Some((key, mods)) = pressed else { return };
        self.recording = None;
        if key == egui::Key::Escape {
            return;
        }
        *self.value_mut(field) = shortcuts::format_parts(mods, key);
    }

    fn shortcut_row(&mut self, ui: &mut egui::Ui, label: &str, field: Field, default: &str) {
        ui.label(label);
        let recording = self.recording == Some(field);
        let value = self.value_mut(field);
        let valid = match field {
            Field::App(_) => shortcuts::parse_list(value).is_ok(),
            _ => value.trim().is_empty() || crate::hotkeys::parse(value).is_ok(),
        };
        let mut edit = TextEdit::singleline(value).hint_text("disabled");
        if !valid {
            edit = edit.text_color(ui.visuals().error_fg_color);
        }
        ui.add_enabled_ui(!recording, |ui| ui.add_sized([170.0, 24.0], edit));
        let is_default = value.trim() == default;
        let button = if recording { "Press keys…" } else { "⏺ Record" };
        if ui
            .button(button)
            .on_hover_text("Click, then press the key combination (Esc cancels)")
            .clicked()
        {
            self.recording = if recording { None } else { Some(field) };
        }
        if ui
            .add_enabled(!is_default, egui::Button::new("↺").small())
            .on_hover_text(format!("Reset to default: {default}"))
            .clicked()
        {
            *self.value_mut(field) = default.to_owned();
        }
        ui.end_row();
    }

    fn shortcuts_ui(&mut self, ui: &mut egui::Ui, cx: &mut Cx<'_>) {
        let defaults = Settings::default();
        ui.label(RichText::new("Keyboard shortcuts").strong());
        ui.label(RichText::new("Global — work from any app").small().weak());
        Grid::new("global_shortcuts").num_columns(4).spacing([10.0, 6.0]).show(ui, |ui| {
            self.shortcut_row(ui, "Take screenshot", Field::Capture, &defaults.hotkey_capture);
            self.shortcut_row(ui, "Clipboard history", Field::Clipboard, &defaults.hotkey_clipboard);
        });

        for group in Group::ALL {
            CollapsingHeader::new(group.title()).show(ui, |ui| {
                Grid::new(group.title()).num_columns(4).spacing([10.0, 6.0]).show(ui, |ui| {
                    for act in Act::ALL.into_iter().filter(|a| a.group() == group) {
                        self.shortcut_row(ui, act.label(), Field::App(act), act.default_keys());
                    }
                });
            });
        }
        ui.label(
            RichText::new(
                "Type a combination like Ctrl+Shift+S, F9 or a single letter. \
                 Separate several shortcuts with a comma. Leave empty to disable.",
            )
            .small()
            .weak(),
        );

        let keymap = self.draft_keymap();
        let mut problems = keymap.problems();
        for (text, label) in [(&self.capture, "Take screenshot"), (&self.clipboard, "Clipboard history")] {
            if !text.trim().is_empty() && crate::hotkeys::parse(text).is_err() {
                problems.push(format!("{label}: \"{text}\" is not a valid shortcut"));
            }
        }
        if !self.capture.trim().is_empty() && self.capture.trim() == self.clipboard.trim() {
            problems.push("Both global shortcuts are the same".into());
        }
        let error = ui.visuals().error_fg_color;
        for problem in &problems {
            ui.colored_label(error, format!("⚠ {problem}"));
        }

        let settings = &cx.shared.settings;
        let changed = self.capture.trim() != settings.hotkey_capture
            || self.clipboard.trim() != settings.hotkey_clipboard
            || keymap != settings.keymap;
        ui.horizontal(|ui| {
            if ui
                .add_enabled(changed && problems.is_empty(), egui::Button::new("✔ Apply shortcuts"))
                .clicked()
            {
                let settings = &mut cx.shared.settings;
                settings.hotkey_capture = self.capture.trim().to_owned();
                settings.hotkey_clipboard = self.clipboard.trim().to_owned();
                settings.keymap = keymap.clone();
                cx.commands.push(Command::ApplySettings);
            }
            if changed && ui.button("Discard changes").clicked() {
                self.load(&cx.shared.settings);
            }
            if ui.button("Reset all to defaults").clicked() {
                let mut reset = cx.shared.settings.clone();
                reset.hotkey_capture = defaults.hotkey_capture.clone();
                reset.hotkey_clipboard = defaults.hotkey_clipboard.clone();
                reset.keymap = Keymap::default();
                self.load(&reset);
            }
        });
        for err in &cx.shared.errors {
            ui.colored_label(error, err);
        }
    }
}

impl View for SettingsView {
    fn kind(&self) -> ViewKind {
        ViewKind::Settings
    }

    fn ui(&mut self, ui: &mut egui::Ui, cx: &mut Cx<'_>) {
        let ctx = ui.ctx().clone();
        self.record(&ctx);

        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.heading("SnapClip settings");
                ui.add_space(8.0);

                ui.group(|ui| {
                    ui.set_width(ui.available_width());
                    self.shortcuts_ui(ui, cx);
                });

                ui.add_space(6.0);
                ui.group(|ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new("General").strong());
                    let s = &mut cx.shared.settings;
                    let mut changed = false;
                    changed |= ui
                        .checkbox(&mut s.autostart, "Start SnapClip when I sign in")
                        .changed();
                    changed |= ui
                        .checkbox(
                            &mut s.auto_paste,
                            "Paste automatically after choosing a clipboard item",
                        )
                        .changed();
                    ui.horizontal(|ui| {
                        ui.label("Keep up to");
                        changed |= ui
                            .add(egui::DragValue::new(&mut s.history_limit).range(10..=5000))
                            .changed();
                        ui.label("clipboard items (pinned items are always kept)");
                    });
                    if changed {
                        cx.commands.push(Command::ApplySettings);
                    }
                });

                ui.add_space(6.0);
                ui.group(|ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new("Screenshots").strong());
                    ui.label("After copying or saving a screenshot:");
                    let mut changed = false;
                    for (value, label) in [
                        (AfterCapture::Notify, "Show a notification with a preview (click it to edit)"),
                        (AfterCapture::Editor, "Open the editor right away"),
                        (AfterCapture::Nothing, "Do nothing"),
                    ] {
                        changed |= ui
                            .radio_value(&mut cx.shared.settings.after_capture, value, label)
                            .changed();
                    }
                    if changed {
                        cx.commands.push(Command::ApplySettings);
                    }
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label("Save folder:");
                        ui.label(RichText::new(cx.shared.settings.save_dir.display().to_string()).monospace());
                    });
                    ui.horizontal(|ui| {
                        if ui.button("Change…").clicked() {
                            cx.commands.push(Command::PickSaveFolder);
                        }
                        if ui.button("Open folder").clicked() {
                            cx.commands.push(Command::OpenPath(cx.shared.settings.save_dir.clone()));
                        }
                    });
                });

                ui.add_space(6.0);
                ui.group(|ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new("Data").strong());
                    ui.horizontal(|ui| {
                        if ui.button("Open data folder").clicked() {
                            cx.commands.push(Command::OpenPath(crate::settings::data_dir()));
                        }
                        if ui.button("Clear clipboard history").on_hover_text("Pinned items are kept").clicked() {
                            cx.shared.history.clear_unpinned();
                        }
                    });
                });

                ui.add_space(10.0);
                ui.label(
                    RichText::new(format!("SnapClip {}", env!("CARGO_PKG_VERSION")))
                        .small()
                        .weak(),
                );
            });
        });
    }
}
