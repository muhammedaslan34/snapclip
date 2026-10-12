//! Background thread that listens for clipboard changes and reports new content.
//!
//! On Windows this uses `AddClipboardFormatListener` (event driven, no polling).

use std::path::PathBuf;
use std::time::Duration;

use clipboard_master::{CallbackResult, ClipboardHandler, Master};
use winit::event_loop::EventLoopProxy;

use crate::app::AppEvent;
use crate::history::{ClipContent, ensure_dir, image_file_names, image_hash, text_hash};

/// Larger texts are ignored to keep the history small.
const MAX_TEXT_BYTES: usize = 2 * 1024 * 1024;
const THUMB_MAX: (u32, u32) = (320, 160);

#[derive(Debug)]
pub struct ClipEvent {
    pub hash: u64,
    pub content: ClipContent,
}

pub fn spawn(images_dir: PathBuf, proxy: EventLoopProxy<AppEvent>) {
    std::thread::Builder::new()
        .name("clipboard-watch".into())
        .spawn(move || {
            ensure_dir(&images_dir);
            let handler = Handler {
                images_dir,
                proxy,
                clipboard: None,
            };
            match Master::new(handler) {
                Ok(mut master) => {
                    if let Err(err) = master.run() {
                        log::error!("clipboard watcher stopped: {err}");
                    }
                }
                Err(err) => log::error!("cannot start clipboard watcher: {err}"),
            }
        })
        .expect("failed to spawn clipboard thread");
}

struct Handler {
    images_dir: PathBuf,
    proxy: EventLoopProxy<AppEvent>,
    clipboard: Option<arboard::Clipboard>,
}

impl Handler {
    fn read(&mut self) -> Option<ClipEvent> {
        if self.clipboard.is_none() {
            self.clipboard = arboard::Clipboard::new()
                .inspect_err(|err| log::warn!("clipboard unavailable: {err}"))
                .ok();
        }
        let clipboard = self.clipboard.as_mut()?;

        // Another app may still hold the clipboard open right after the change notification.
        for attempt in 0..5 {
            match clipboard.get_text() {
                Ok(text) if !text.trim().is_empty() && text.len() <= MAX_TEXT_BYTES => {
                    return Some(ClipEvent {
                        hash: text_hash(&text),
                        content: ClipContent::Text { text },
                    });
                }
                Ok(_) => return None,
                Err(arboard::Error::ContentNotAvailable) => break,
                Err(arboard::Error::ClipboardOccupied) if attempt < 4 => {
                    std::thread::sleep(Duration::from_millis(40));
                }
                Err(err) => {
                    log::debug!("get_text: {err}");
                    break;
                }
            }
        }

        let image = clipboard.get_image().ok()?;
        let (width, height) = (image.width as u32, image.height as u32);
        let mut rgba = image.bytes.into_owned();
        // Some apps put 32-bit bitmaps with an all-zero alpha channel on the clipboard.
        if rgba.chunks_exact(4).all(|px| px[3] == 0) {
            rgba.chunks_exact_mut(4).for_each(|px| px[3] = 255);
        }
        let hash = image_hash(width, height, &rgba);
        let (file, thumb) = image_file_names(hash);
        let file_path = self.images_dir.join(&file);
        let thumb_path = self.images_dir.join(&thumb);
        if !file_path.exists() {
            let img = image::RgbaImage::from_raw(width, height, rgba)?;
            if let Err(err) = img.save(&file_path) {
                log::error!("cannot save clipboard image: {err}");
                return None;
            }
            let (tw, th) = fit(width, height, THUMB_MAX.0, THUMB_MAX.1);
            let small = image::imageops::thumbnail(&img, tw, th);
            if let Err(err) = small.save(&thumb_path) {
                log::warn!("cannot save thumbnail: {err}");
            }
        }
        Some(ClipEvent {
            hash,
            content: ClipContent::Image {
                file,
                thumb,
                width,
                height,
            },
        })
    }
}

impl ClipboardHandler for Handler {
    fn on_clipboard_change(&mut self) -> CallbackResult {
        if let Some(event) = self.read() {
            if self.proxy.send_event(AppEvent::Clip(event)).is_err() {
                return CallbackResult::Stop;
            }
        }
        CallbackResult::Next
    }

    fn on_clipboard_error(&mut self, error: std::io::Error) -> CallbackResult {
        log::warn!("clipboard watcher error: {error}");
        CallbackResult::Next
    }
}

/// Scales `(w, h)` down to fit inside `(max_w, max_h)`, keeping the aspect ratio.
pub fn fit(w: u32, h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    let scale = (max_w as f32 / w as f32).min(max_h as f32 / h as f32).min(1.0);
    (
        ((w as f32 * scale).round() as u32).max(1),
        ((h as f32 * scale).round() as u32).max(1),
    )
}
