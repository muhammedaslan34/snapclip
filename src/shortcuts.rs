//! User-configurable keyboard shortcuts used inside the app's windows
//! (the global shortcuts live in `hotkeys.rs`).

use std::collections::BTreeMap;

use egui::{Event, Key, KeyboardShortcut, Modifiers};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Act {
    ToolAdjust,
    ToolPen,
    ToolMarker,
    ToolLine,
    ToolArrow,
    ToolRect,
    ToolEllipse,
    ToolText,
    ToolStep,
    ToolPixelate,
    Copy,
    Save,
    SaveAs,
    Pin,
    ScrollCapture,
    Undo,
    Redo,
    SelectAll,
    ClipPaste,
    ClipDelete,
    ClipPin,
    ClipEdit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Tools,
    Editor,
    Clipboard,
}

impl Group {
    pub const ALL: [Group; 3] = [Group::Tools, Group::Editor, Group::Clipboard];

    pub fn title(self) -> &'static str {
        match self {
            Group::Tools => "Screenshot tools",
            Group::Editor => "Screenshot actions",
            Group::Clipboard => "Clipboard panel",
        }
    }
}

impl Act {
    pub const ALL: [Act; 22] = [
        Act::ToolAdjust,
        Act::ToolPen,
        Act::ToolMarker,
        Act::ToolLine,
        Act::ToolArrow,
        Act::ToolRect,
        Act::ToolEllipse,
        Act::ToolText,
        Act::ToolStep,
        Act::ToolPixelate,
        Act::Copy,
        Act::Save,
        Act::SaveAs,
        Act::Pin,
        Act::ScrollCapture,
        Act::Undo,
        Act::Redo,
        Act::SelectAll,
        Act::ClipPaste,
        Act::ClipDelete,
        Act::ClipPin,
        Act::ClipEdit,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Act::ToolAdjust => "Move / resize selection",
            Act::ToolPen => "Pen",
            Act::ToolMarker => "Highlighter",
            Act::ToolLine => "Line",
            Act::ToolArrow => "Arrow",
            Act::ToolRect => "Rectangle",
            Act::ToolEllipse => "Ellipse",
            Act::ToolText => "Text",
            Act::ToolStep => "Numbered step",
            Act::ToolPixelate => "Pixelate",
            Act::Copy => "Copy to clipboard",
            Act::Save => "Save to folder",
            Act::SaveAs => "Save as…",
            Act::Pin => "Pin to screen",
            Act::ScrollCapture => "Scrolling capture",
            Act::Undo => "Undo",
            Act::Redo => "Redo",
            Act::SelectAll => "Select full screen",
            Act::ClipPaste => "Paste selected item",
            Act::ClipDelete => "Delete selected item",
            Act::ClipPin => "Pin / unpin selected item",
            Act::ClipEdit => "Edit selected item",
        }
    }

    pub fn group(self) -> Group {
        match self {
            Act::ToolAdjust
            | Act::ToolPen
            | Act::ToolMarker
            | Act::ToolLine
            | Act::ToolArrow
            | Act::ToolRect
            | Act::ToolEllipse
            | Act::ToolText
            | Act::ToolStep
            | Act::ToolPixelate => Group::Tools,
            Act::ClipPaste | Act::ClipDelete | Act::ClipPin | Act::ClipEdit => Group::Clipboard,
            _ => Group::Editor,
        }
    }

    pub fn default_keys(self) -> &'static str {
        match self {
            Act::ToolAdjust => "V",
            Act::ToolPen => "P",
            Act::ToolMarker => "M",
            Act::ToolLine => "L",
            Act::ToolArrow => "A",
            Act::ToolRect => "R",
            Act::ToolEllipse => "E",
            Act::ToolText => "T",
            Act::ToolStep => "N",
            Act::ToolPixelate => "B",
            Act::Copy => "Enter, Ctrl+C",
            Act::Save => "Ctrl+S",
            Act::SaveAs => "Ctrl+Shift+S",
            Act::Pin => "Ctrl+P",
            Act::ScrollCapture => "Ctrl+L",
            Act::Undo => "Ctrl+Z",
            Act::Redo => "Ctrl+Y, Ctrl+Shift+Z",
            Act::SelectAll => "Ctrl+A",
            Act::ClipPaste => "Enter",
            Act::ClipDelete => "Shift+Delete",
            Act::ClipPin => "Ctrl+P",
            Act::ClipEdit => "Ctrl+E",
        }
    }
}

/// Only shortcuts that differ from the defaults are stored.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Keymap(BTreeMap<Act, String>);

impl Keymap {
    /// The shortcut text for an action, e.g. `"Enter, Ctrl+C"`. Empty means disabled.
    pub fn get(&self, act: Act) -> &str {
        self.0.get(&act).map_or(act.default_keys(), String::as_str)
    }

    pub fn set(&mut self, act: Act, keys: String) {
        if keys == act.default_keys() {
            self.0.remove(&act);
        } else {
            self.0.insert(act, keys);
        }
    }

    /// Short text for tooltips, e.g. `"Enter"`.
    pub fn hint(&self, act: Act) -> String {
        match self.get(act).trim() {
            "" => String::new(),
            keys => format!(" ({keys})"),
        }
    }

    /// Consumes a key press matching `act`. Call once per action per frame.
    pub fn pressed(&self, ctx: &egui::Context, act: Act) -> bool {
        parse_list(self.get(act))
            .unwrap_or_default()
            .iter()
            .any(|shortcut| consume(ctx, shortcut))
    }

    /// Human-readable problems: invalid text or two actions sharing a shortcut.
    pub fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        let mut seen: Vec<(Group, KeyboardShortcut, Act)> = Vec::new();
        for act in Act::ALL {
            match parse_list(self.get(act)) {
                Err(err) => problems.push(format!("{}: {err}", act.label())),
                Ok(shortcuts) => {
                    for sc in shortcuts {
                        // Tools and actions share a window, so they share a namespace.
                        let ns = if act.group() == Group::Clipboard {
                            Group::Clipboard
                        } else {
                            Group::Editor
                        };
                        if let Some((_, _, other)) = seen.iter().find(|(g, s, _)| *g == ns && *s == sc) {
                            problems.push(format!(
                                "\"{}\" is used by both \"{}\" and \"{}\"",
                                format(sc),
                                other.label(),
                                act.label()
                            ));
                        }
                        seen.push((ns, sc, act));
                    }
                }
            }
        }
        problems
    }
}

/// Parses `"Ctrl+Shift+S"`. Returns an error for unknown keys.
pub fn parse(text: &str) -> Result<KeyboardShortcut, String> {
    let mut modifiers = Modifiers::NONE;
    let mut key = None;
    for token in text.split('+').map(str::trim) {
        if key.is_some() {
            return Err(format!("\"{text}\": the key must come last"));
        }
        match token.to_ascii_lowercase().as_str() {
            "ctrl" | "control" | "cmd" | "command" => modifiers = modifiers | Modifiers::COMMAND,
            "alt" | "option" => modifiers.alt = true,
            "shift" => modifiers.shift = true,
            _ => {
                let parsed = Key::from_name(token).or_else(|| Key::from_name(&capitalize(token)));
                key = Some(parsed.ok_or_else(|| format!("unknown key \"{token}\""))?);
            }
        }
    }
    let key = key.ok_or_else(|| format!("\"{text}\" has no key"))?;
    Ok(KeyboardShortcut::new(modifiers, key))
}

/// Parses a comma-separated list. An empty string disables the action.
pub fn parse_list(text: &str) -> Result<Vec<KeyboardShortcut>, String> {
    text.split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(parse)
        .collect()
}

pub fn format(shortcut: KeyboardShortcut) -> String {
    format_parts(shortcut.modifiers, shortcut.logical_key)
}

pub fn format_parts(modifiers: Modifiers, key: Key) -> String {
    let mut parts = Vec::new();
    if modifiers.ctrl || modifiers.command {
        parts.push("Ctrl");
    }
    if modifiers.alt {
        parts.push("Alt");
    }
    if modifiers.shift {
        parts.push("Shift");
    }
    parts.push(key.name());
    parts.join("+")
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars.flat_map(char::to_lowercase)).collect())
        .unwrap_or_default()
}

/// Removes the first key press that matches exactly (extra modifiers don't match).
fn consume(ctx: &egui::Context, shortcut: &KeyboardShortcut) -> bool {
    // egui-winit reports Ctrl+C as a Copy event instead of a key press.
    let is_ctrl_c = shortcut.logical_key == Key::C && shortcut.modifiers.matches_exact(Modifiers::COMMAND);
    ctx.input_mut(|i| {
        let found = i.events.iter().position(|e| match e {
            Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => *key == shortcut.logical_key && modifiers.matches_exact(shortcut.modifiers),
            Event::Copy => is_ctrl_c,
            _ => false,
        });
        if let Some(index) = found {
            i.events.remove(index);
        }
        found.is_some()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_formats() {
        let sc = parse("ctrl+shift+s").unwrap();
        assert_eq!(format(sc), "Ctrl+Shift+S");
        assert_eq!(parse_list("Enter, Ctrl+C").unwrap().len(), 2);
        assert!(parse_list("").unwrap().is_empty());
        assert!(parse("Ctrl+Nope").is_err());
        assert_eq!(format(parse("F9").unwrap()), "F9");
    }

    #[test]
    fn defaults_are_valid_and_conflict_free() {
        assert!(Keymap::default().problems().is_empty());
    }

    #[test]
    fn detects_conflicts_and_stores_only_overrides() {
        let mut km = Keymap::default();
        km.set(Act::ToolPen, "A".into());
        assert_eq!(km.problems().len(), 1);
        km.set(Act::ToolPen, "P".into());
        assert_eq!(km, Keymap::default());
    }
}
