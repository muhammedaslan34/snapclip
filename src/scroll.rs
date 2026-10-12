//! Scrolling ("long") screenshots: capture a region repeatedly while the page
//! scrolls, and stitch the frames together by matching overlapping rows.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::time::Duration;

use image::RgbaImage;
use winit::event_loop::EventLoopProxy;

use crate::app::AppEvent;
use crate::history::fnv1a;

pub const RUNNING: u8 = 0;
pub const FINISH: u8 = 1;
pub const CANCEL: u8 = 2;

/// Stop once the stitched image is this tall.
const MAX_HEIGHT: usize = 30_000;
/// Stop auto-scrolling after this many frames without movement (end of page).
const STILL_FRAMES_TO_STOP: u32 = 3;

/// A region in monitor-relative physical pixels.
#[derive(Clone, Copy, Debug)]
pub struct Region {
    pub monitor_pos: (i32, i32),
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Shared between the capture thread and the control window.
#[derive(Clone)]
pub struct ScrollStatus {
    pub state: Arc<AtomicU8>,
    pub auto_scroll: Arc<AtomicBool>,
    pub frames: u32,
    pub height: u32,
}

#[derive(Debug)]
pub enum ScrollMsg {
    Progress { frames: u32, height: u32 },
    Done(Option<RgbaImage>),
}

pub fn spawn(region: Region, status: &ScrollStatus, proxy: EventLoopProxy<AppEvent>) {
    let state = status.state.clone();
    let auto = status.auto_scroll.clone();
    std::thread::Builder::new()
        .name("scroll-capture".into())
        .spawn(move || {
            let result = run(region, &state, &auto, &proxy);
            if let Err(err) = &result {
                log::error!("scrolling capture failed: {err}");
            }
            let image = result.ok().flatten();
            let _ = proxy.send_event(AppEvent::Scroll(ScrollMsg::Done(image)));
        })
        .expect("failed to spawn scroll thread");
}

fn run(
    region: Region,
    state: &AtomicU8,
    auto: &AtomicBool,
    proxy: &EventLoopProxy<AppEvent>,
) -> anyhow::Result<Option<RgbaImage>> {
    // Give the overlay time to disappear.
    std::thread::sleep(Duration::from_millis(300));
    let monitor = xcap::Monitor::from_point(region.monitor_pos.0, region.monitor_pos.1)?;
    let capture = || monitor.capture_region(region.x, region.y, region.width, region.height);

    let center = (
        region.monitor_pos.0 + (region.x + region.width / 2) as i32,
        region.monitor_pos.1 + (region.y + region.height / 2) as i32,
    );
    // About a third of the region per step keeps a large overlap for matching.
    let notches = ((region.height / 300).max(1)).min(5) as i32;

    let mut stitcher = Stitcher::new(&capture()?);
    let mut still = 0;
    let mut frames = 1;
    loop {
        match state.load(Ordering::Relaxed) {
            CANCEL => return Ok(None),
            FINISH => break,
            _ => {}
        }
        let auto_now = auto.load(Ordering::Relaxed);
        if auto_now {
            crate::platform::scroll_at(center, notches);
            std::thread::sleep(Duration::from_millis(380));
        } else {
            std::thread::sleep(Duration::from_millis(150));
        }

        match stitcher.push(&capture()?) {
            Push::Added => {
                still = 0;
                frames += 1;
            }
            Push::Same => still += 1,
            Push::NoMatch => log::warn!("scrolled too far between frames; skipping one"),
        }
        let _ = proxy.send_event(AppEvent::Scroll(ScrollMsg::Progress {
            frames,
            height: stitcher.height() as u32,
        }));
        if (auto_now && still >= STILL_FRAMES_TO_STOP) || stitcher.height() >= MAX_HEIGHT {
            break;
        }
    }
    Ok(Some(stitcher.finish()))
}

enum Push {
    Added,
    Same,
    NoMatch,
}

struct Stitcher {
    width: usize,
    /// Stitched RGBA rows; always ends with the current sticky footer.
    out: Vec<u8>,
    prev_hashes: Vec<u64>,
}

impl Stitcher {
    fn new(first: &RgbaImage) -> Self {
        Self {
            width: first.width() as usize,
            out: first.as_raw().clone(),
            prev_hashes: row_hashes(first),
        }
    }

    fn height(&self) -> usize {
        self.out.len() / (self.width * 4)
    }

    fn push(&mut self, frame: &RgbaImage) -> Push {
        let hashes = row_hashes(frame);
        if hashes == self.prev_hashes {
            return Push::Same;
        }
        let h = hashes.len();
        let row_bytes = self.width * 4;

        // Rows that stay put at the bottom (sticky footer / status bar).
        let footer = hashes
            .iter()
            .rev()
            .zip(self.prev_hashes.iter().rev())
            .take_while(|(a, b)| a == b)
            .count()
            .min(h / 3);

        let Some(dy) = best_offset(&self.prev_hashes, &hashes, footer) else {
            self.prev_hashes = hashes;
            return Push::NoMatch;
        };

        // Drop the old footer, append the newly revealed rows, then the footer again.
        self.out.truncate(self.out.len() - footer * row_bytes);
        let raw = frame.as_raw();
        self.out
            .extend_from_slice(&raw[(h - footer - dy) * row_bytes..(h - footer) * row_bytes]);
        self.out.extend_from_slice(&raw[(h - footer) * row_bytes..]);
        self.prev_hashes = hashes;
        Push::Added
    }

    fn finish(self) -> RgbaImage {
        let height = self.height() as u32;
        RgbaImage::from_raw(self.width as u32, height, self.out)
            .expect("stitched buffer matches its dimensions")
    }
}

/// Finds how far the content moved up between two frames, comparing row hashes.
/// Uniform rows (blank background) are ignored because they match anything.
fn best_offset(prev: &[u64], next: &[u64], footer: usize) -> Option<usize> {
    let h = prev.len().min(next.len());
    let content = h - footer;
    let distinct = |rows: &[u64], i: usize| i == 0 || rows[i] != rows[i - 1];
    let mut best: Option<(usize, usize)> = None;
    for dy in 1..content.saturating_sub(8) {
        let overlap = content - dy;
        let mut score = 0;
        let mut candidates = 0;
        for i in 0..overlap {
            if !distinct(next, i) {
                continue;
            }
            candidates += 1;
            if next[i] == prev[i + dy] {
                score += 1;
            }
        }
        if candidates >= 6 && score * 2 >= candidates && best.is_none_or(|(s, _)| score > s) {
            best = Some((score, dy));
        }
    }
    best.map(|(_, dy)| dy)
}

/// One hash per row, ignoring the right edge where a scrollbar thumb moves.
fn row_hashes(img: &RgbaImage) -> Vec<u64> {
    let w = img.width() as usize;
    let skip = (w / 12).min(28);
    let used = (w - skip) * 4;
    img.as_raw()
        .chunks_exact(w * 4)
        .map(|row| fnv1a(&[&row[..used]]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tall "page" whose rows are all distinct.
    fn page(width: u32, height: u32) -> RgbaImage {
        RgbaImage::from_fn(width, height, |x, y| {
            let v = (y.wrapping_mul(2_654_435_761) ^ x.wrapping_mul(40_503)) as u8;
            image::Rgba([v, (y % 251) as u8, (y / 251) as u8, 255])
        })
    }

    fn window(page: &RgbaImage, top: u32, height: u32) -> RgbaImage {
        image::imageops::crop_imm(page, 0, top, page.width(), height).to_image()
    }

    #[test]
    fn stitches_scrolled_frames_back_into_the_page() {
        let page = page(200, 1200);
        let mut stitcher = Stitcher::new(&window(&page, 0, 300));
        for top in [110, 230, 380, 500, 640, 800, 900] {
            assert!(matches!(stitcher.push(&window(&page, top, 300)), Push::Added));
        }
        // No movement at the end of the page.
        assert!(matches!(stitcher.push(&window(&page, 900, 300)), Push::Same));
        let out = stitcher.finish();
        assert_eq!(out.dimensions(), (200, 1200));
        assert!(out == page);
    }

    #[test]
    fn sticky_footer_appears_once_at_the_bottom() {
        let page = page(160, 900);
        let footer = RgbaImage::from_pixel(160, 40, image::Rgba([10, 200, 30, 255]));
        let frame = |top| {
            let mut f = window(&page, top, 260);
            image::imageops::replace(&mut f, &footer, 0, 220);
            f
        };
        let mut stitcher = Stitcher::new(&frame(0));
        for top in [100, 200, 300] {
            assert!(matches!(stitcher.push(&frame(top)), Push::Added));
        }
        let out = stitcher.finish();
        // Content rows 0..520 followed by a single footer.
        assert_eq!(out.height(), 520 + 40);
        assert!(window(&out, 0, 520) == window(&page, 0, 520));
        assert!(window(&out, 520, 40) == footer);
    }
}
