//! Global keyboard shortcuts.

use std::str::FromStr;

use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager};
use winit::event_loop::EventLoopProxy;

use crate::app::AppEvent;
use crate::settings::Settings;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HotkeyAction {
    Capture,
    Clipboard,
}

pub struct Hotkeys {
    manager: GlobalHotKeyManager,
    registered: Vec<(HotKey, HotkeyAction)>,
}

impl Hotkeys {
    pub fn new(proxy: &EventLoopProxy<AppEvent>) -> anyhow::Result<Self> {
        let proxy = proxy.clone();
        GlobalHotKeyEvent::set_event_handler(Some(move |event| {
            let _ = proxy.send_event(AppEvent::HotKey(event));
        }));
        Ok(Self {
            manager: GlobalHotKeyManager::new()?,
            registered: Vec::new(),
        })
    }

    /// Re-registers all shortcuts from the settings. Returns human-readable errors.
    pub fn apply(&mut self, settings: &Settings) -> Vec<String> {
        for (hotkey, _) in self.registered.drain(..) {
            let _ = self.manager.unregister(hotkey);
        }
        let mut errors = Vec::new();
        for (text, action, label) in [
            (&settings.hotkey_capture, HotkeyAction::Capture, "Screenshot"),
            (&settings.hotkey_clipboard, HotkeyAction::Clipboard, "Clipboard"),
        ] {
            if text.trim().is_empty() {
                continue;
            }
            match parse(text) {
                Ok(hotkey) => match self.manager.register(hotkey) {
                    Ok(()) => self.registered.push((hotkey, action)),
                    Err(err) => errors.push(format!(
                        "{label} shortcut \"{text}\" could not be registered (already used by another app?): {err}"
                    )),
                },
                Err(err) => errors.push(format!("{label} shortcut: {err}")),
            }
        }
        errors
    }

    pub fn action_for(&self, id: u32) -> Option<HotkeyAction> {
        self.registered
            .iter()
            .find(|(hotkey, _)| hotkey.id() == id)
            .map(|(_, action)| *action)
    }
}

pub fn parse(text: &str) -> Result<HotKey, String> {
    HotKey::from_str(text.trim()).map_err(|err| format!("\"{text}\" is not a valid shortcut ({err})"))
}
