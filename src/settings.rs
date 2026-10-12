use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const APP_NAME: &str = "SnapClip";

/// What happens after a screenshot is copied or saved from the capture overlay.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AfterCapture {
    /// Show a small notification with a preview; click it to open the editor.
    #[default]
    Notify,
    /// Open the editor window right away.
    Editor,
    /// Do nothing else.
    Nothing,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Global shortcut that opens the screenshot overlay, e.g. `Alt+Shift+S`.
    pub hotkey_capture: String,
    /// Global shortcut that opens the clipboard history panel.
    pub hotkey_clipboard: String,
    /// Start the app when the user logs in.
    pub autostart: bool,
    /// After picking an item in the clipboard panel, paste it into the previous window.
    pub auto_paste: bool,
    /// Maximum number of unpinned clipboard items to keep.
    pub history_limit: usize,
    pub after_capture: AfterCapture,
    /// Folder used by "Save" in the screenshot editor.
    pub save_dir: PathBuf,
    /// Last-used annotation style, restored the next time the editor opens.
    pub pen_color: [u8; 4],
    pub pen_width: f32,
    pub font_size: f32,
    /// Shortcuts inside the editor and clipboard panel (only changed ones are stored).
    pub keymap: crate::shortcuts::Keymap,
}

impl Default for Settings {
    fn default() -> Self {
        let pictures = dirs::picture_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        Self {
            hotkey_capture: "Alt+Shift+S".into(),
            hotkey_clipboard: "Alt+Shift+V".into(),
            autostart: true,
            auto_paste: true,
            history_limit: 300,
            after_capture: AfterCapture::Notify,
            save_dir: pictures.join(APP_NAME),
            pen_color: [239, 68, 68, 255],
            pen_width: 4.0,
            font_size: 28.0,
            keymap: Default::default(),
        }
    }
}

impl Settings {
    fn path() -> PathBuf {
        config_dir().join("settings.json")
    }

    pub fn load() -> Self {
        match std::fs::read(Self::path()) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|err| {
                log::warn!("settings.json is invalid, using defaults: {err}");
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) {
        let result = serde_json::to_vec_pretty(self)
            .map_err(anyhow::Error::from)
            .and_then(|bytes| write_atomic(&Self::path(), &bytes));
        if let Err(err) = result {
            log::error!("failed to save settings: {err}");
        }
    }
}

pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(APP_NAME)
}

pub fn data_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(APP_NAME)
}

/// Writes to a temporary file first so a crash never leaves a half-written file behind.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}
