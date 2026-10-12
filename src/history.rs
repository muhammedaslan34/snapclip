//! Persistent clipboard history: an index in `history.json` plus PNG files for images.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::settings::write_atomic;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ClipContent {
    Text {
        text: String,
    },
    Image {
        file: String,
        thumb: String,
        width: u32,
        height: u32,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClipItem {
    pub id: u64,
    pub hash: u64,
    pub pinned: bool,
    /// Unix time in milliseconds of the last copy.
    pub time: i64,
    pub content: ClipContent,
}

impl ClipItem {
    pub fn text(&self) -> Option<&str> {
        match &self.content {
            ClipContent::Text { text } => Some(text),
            ClipContent::Image { .. } => None,
        }
    }
}

pub struct History {
    /// Newest first.
    items: Vec<ClipItem>,
    dir: PathBuf,
    next_id: u64,
    pub limit: usize,
    /// Bumped on every change so views can invalidate caches.
    pub revision: u64,
}

impl History {
    pub fn load(dir: PathBuf, limit: usize) -> Self {
        let items: Vec<ClipItem> = std::fs::read(dir.join("history.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let next_id = items.iter().map(|i| i.id).max().unwrap_or(0) + 1;
        Self {
            items,
            dir,
            next_id,
            limit,
            revision: 0,
        }
    }

    pub fn items(&self) -> &[ClipItem] {
        &self.items
    }

    pub fn get(&self, id: u64) -> Option<&ClipItem> {
        self.items.iter().find(|i| i.id == id)
    }

    pub fn images_dir(&self) -> PathBuf {
        self.dir.join("images")
    }

    pub fn file_path(&self, name: &str) -> PathBuf {
        self.images_dir().join(name)
    }

    /// Adds a new entry, or moves an identical existing entry to the top.
    pub fn push(&mut self, hash: u64, content: ClipContent) {
        let now = chrono::Utc::now().timestamp_millis();
        if let Some(pos) = self.items.iter().position(|i| i.hash == hash) {
            let mut item = self.items.remove(pos);
            item.time = now;
            self.items.insert(0, item);
        } else {
            let id = self.next_id;
            self.next_id += 1;
            self.items.insert(
                0,
                ClipItem {
                    id,
                    hash,
                    pinned: false,
                    time: now,
                    content,
                },
            );
            self.enforce_limit();
        }
        self.changed();
    }

    pub fn remove(&mut self, id: u64) {
        if let Some(pos) = self.items.iter().position(|i| i.id == id) {
            let item = self.items.remove(pos);
            self.delete_files(&item);
            self.changed();
        }
    }

    pub fn toggle_pin(&mut self, id: u64) {
        if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
            item.pinned = !item.pinned;
            self.changed();
        }
    }

    pub fn set_text(&mut self, id: u64, new_text: String) {
        if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
            item.hash = text_hash(&new_text);
            item.content = ClipContent::Text { text: new_text };
            self.changed();
        }
    }

    pub fn clear_unpinned(&mut self) {
        let (keep, drop): (Vec<_>, Vec<_>) = self.items.drain(..).partition(|i| i.pinned);
        self.items = keep;
        for item in &drop {
            self.delete_files(item);
        }
        self.changed();
    }

    pub fn set_limit(&mut self, limit: usize) {
        self.limit = limit;
        if self.enforce_limit() {
            self.changed();
        }
    }

    fn enforce_limit(&mut self) -> bool {
        let mut removed = false;
        while self.items.iter().filter(|i| !i.pinned).count() > self.limit {
            let Some(pos) = self.items.iter().rposition(|i| !i.pinned) else {
                break;
            };
            let item = self.items.remove(pos);
            self.delete_files(&item);
            removed = true;
        }
        removed
    }

    fn delete_files(&self, item: &ClipItem) {
        if let ClipContent::Image { file, thumb, .. } = &item.content {
            let _ = std::fs::remove_file(self.file_path(file));
            let _ = std::fs::remove_file(self.file_path(thumb));
        }
    }

    fn changed(&mut self) {
        self.revision += 1;
        self.save();
    }

    fn save(&self) {
        let result = serde_json::to_vec(&self.items)
            .map_err(anyhow::Error::from)
            .and_then(|bytes| write_atomic(&self.dir.join("history.json"), &bytes));
        if let Err(err) = result {
            log::error!("failed to save clipboard history: {err}");
        }
    }
}

/// FNV-1a: stable across Rust versions, so hashes stored on disk stay valid.
pub fn fnv1a(parts: &[&[u8]]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for &byte in *part {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    }
    hash
}

pub fn text_hash(text: &str) -> u64 {
    fnv1a(&[b"text:", text.as_bytes()])
}

pub fn image_hash(width: u32, height: u32, rgba: &[u8]) -> u64 {
    fnv1a(&[
        b"image:",
        &width.to_le_bytes(),
        &height.to_le_bytes(),
        rgba,
    ])
}

pub fn image_file_names(hash: u64) -> (String, String) {
    (format!("img_{hash:016x}.png"), format!("thumb_{hash:016x}.png"))
}

pub fn ensure_dir(path: &Path) {
    if let Err(err) = std::fs::create_dir_all(path) {
        log::error!("cannot create {}: {err}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("snapclip-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn text(t: &str) -> (u64, ClipContent) {
        (text_hash(t), ClipContent::Text { text: t.into() })
    }

    #[test]
    fn dedupes_limits_pins_and_persists() {
        let dir = temp_dir("history");
        let mut h = History::load(dir.clone(), 2);
        for t in ["a", "b", "a"] {
            let (hash, c) = text(t);
            h.push(hash, c);
        }
        let texts: Vec<_> = h.items().iter().filter_map(ClipItem::text).collect();
        assert_eq!(texts, ["a", "b"], "re-copying moves the item to the top");

        let b = h.items()[1].id;
        h.toggle_pin(b);
        for t in ["c", "d", "e"] {
            let (hash, c) = text(t);
            h.push(hash, c);
        }
        let texts: Vec<_> = h.items().iter().filter_map(ClipItem::text).collect();
        assert_eq!(texts, ["e", "d", "b"], "pinned items survive the limit");

        h.set_text(b, "edited".into());
        let reloaded = History::load(dir.clone(), 2);
        assert_eq!(reloaded.get(b).and_then(ClipItem::text), Some("edited"));

        h.clear_unpinned();
        assert_eq!(h.items().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
