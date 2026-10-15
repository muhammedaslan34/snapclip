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
    /// The page moved too far between two frames; the user should scroll back a little.
    pub lost: bool,
}

#[derive(Debug)]
pub enum ScrollMsg {
    Progress { frames: u32, height: u32, lost: bool },
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
    let monitor = xcap::Monitor::from_point(region.monitor_pos.0, region.monitor_pos.1)?;
    let capture = || monitor.capture_region(region.x, region.y, region.width, region.height);
    // Wait for the overlay to disappear (Windows fades closing windows out): until
    // two captures in a row agree.
    std::thread::sleep(Duration::from_millis(150));
    let mut first = capture()?;
    for _ in 0..15 {
        std::thread::sleep(Duration::from_millis(60));
        let next = capture()?;
        if next.as_raw() == first.as_raw() {
            break;
        }
        first = next;
    }

    let center = (
        region.monitor_pos.0 + (region.x + region.width / 2) as i32,
        region.monitor_pos.1 + (region.y + region.height / 2) as i32,
    );
    // About a third of the region per step keeps a large overlap for matching.
    let notches = ((region.height / 300).max(1)).min(5) as i32;

    let mut stitcher = Stitcher::new(&first);
    let mut still = 0;
    let mut frames = 1;
    let mut lost = false;
    let mut misses = 0;
    // True while the mouse is where auto-scroll put it.
    let mut placed = false;
    loop {
        match state.load(Ordering::Relaxed) {
            CANCEL => return Ok(None),
            FINISH => break,
            _ => {}
        }
        let mut auto_now = auto.load(Ordering::Relaxed);
        if auto_now && placed && crate::platform::cursor_pos().is_some_and(|p| p != center) {
            // The user moved the mouse: give it back and let them scroll by hand.
            auto.store(false, Ordering::Relaxed);
            auto_now = false;
        }
        placed = auto_now;
        if auto_now {
            crate::platform::scroll_at(center, notches);
            std::thread::sleep(Duration::from_millis(380));
        } else {
            // Short, so a quick turn of the mouse wheel doesn't outrun the capture.
            std::thread::sleep(Duration::from_millis(60));
        }

        let was_lost = lost;
        let frame = capture()?;
        match stitcher.push(&frame) {
            Push::Added => {
                still = 0;
                frames += 1;
                lost = false;
                misses = 0;
            }
            Push::Same => {
                still += 1;
                lost = false;
                misses = 0;
            }
            Push::NoMatch if frames == 1 && misses >= 2 => {
                // Nothing stitched yet, so the first frame was probably not the page
                // (something on top of it): start over from this one.
                log::warn!("first frame never matched ({}); starting over", stitcher.miss);
                stitcher = Stitcher::new(&frame);
                lost = false;
                misses = 0;
            }
            Push::NoMatch => {
                lost = true;
                misses += 1;
                if !was_lost {
                    log::warn!("scrolled too far between frames ({}); waiting for an overlapping frame", stitcher.miss);
                }
            }
        }
        if lost && auto_now {
            // Auto-scroll outran the capture: step back so the next frame overlaps.
            crate::platform::scroll_at(center, -notches);
        }
        let _ = proxy.send_event(AppEvent::Scroll(ScrollMsg::Progress {
            frames,
            height: stitcher.height() as u32,
            lost,
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
    /// The last frame stitched in; new frames are matched against it.
    prev: RgbaImage,
    /// Why the last frame didn't match, for the log.
    miss: String,
}

/// Each row is compared in this many vertical strips, so parts of the page that
/// move differently (a sticky side menu) or not at all can't spoil the match.
const STRIPS: usize = 16;

impl Stitcher {
    fn new(first: &RgbaImage) -> Self {
        Self {
            width: first.width() as usize,
            out: first.as_raw().clone(),
            prev: first.clone(),
            miss: String::new(),
        }
    }

    fn height(&self) -> usize {
        self.out.len() / (self.width * 4)
    }

    fn push(&mut self, frame: &RgbaImage) -> Push {
        if frame.dimensions() != self.prev.dimensions() {
            return Push::NoMatch;
        }
        if frame.as_raw() == self.prev.as_raw() {
            return Push::Same;
        }
        let h = frame.height() as usize;
        let row_bytes = self.width * 4;

        // Whole rows that stay put: a sticky header at the top, a footer at the bottom.
        let (prev_rows, next_rows) = (row_hashes(&self.prev), row_hashes(frame));
        let header = prev_rows
            .iter()
            .zip(&next_rows)
            .take_while(|(a, b)| a == b)
            .count()
            .min(h / 3);
        let footer = prev_rows
            .iter()
            .rev()
            .zip(next_rows.iter().rev())
            .take_while(|(a, b)| a == b)
            .count()
            .min(h / 3);

        // Leave out columns that never changed (a fixed sidebar, the browser's toolbar).
        let strips = strips(&moving_columns(&self.prev, frame));
        let prev_cells = cell_hashes(&self.prev, &strips);
        let cells = cell_hashes(frame, &strips);

        let dy = match best_offset(&prev_cells, &cells, header, footer) {
            Offset::Moved(dy) => dy,
            // Something small changed in place (a caret, a hover effect).
            Offset::InPlace => {
                self.prev = frame.clone();
                return Push::Same;
            }
            // Keep comparing against the last good frame: once the user scrolls back
            // into range the capture continues, with no gap in the page.
            Offset::None(why) => {
                self.miss = format!("{why}, header {header}, footer {footer}, {} strips", strips.len());
                return Push::NoMatch;
            }
        };
        // Drop the old footer, append the newly revealed rows, then the footer again.
        self.out.truncate(self.out.len() - footer * row_bytes);
        let raw = frame.as_raw();
        self.out
            .extend_from_slice(&raw[(h - footer - dy) * row_bytes..(h - footer) * row_bytes]);
        self.out.extend_from_slice(&raw[(h - footer) * row_bytes..]);
        self.prev = frame.clone();
        Push::Added
    }

    fn finish(self) -> RgbaImage {
        let height = self.height() as u32;
        RgbaImage::from_raw(self.width as u32, height, self.out)
            .expect("stitched buffer matches its dimensions")
    }
}

enum Offset {
    /// The content moved up by this many rows.
    Moved(usize),
    InPlace,
    None(String),
}

/// Finds how far the content moved up between two frames by matching the cells
/// (strip × row) that changed, below the `header` and above the `footer`. Each
/// strip votes on its own, so the offset most of the page agrees on wins.
fn best_offset(prev: &[Vec<u64>], next: &[Vec<u64>], header: usize, footer: usize) -> Offset {
    let h = next.first().map_or(0, Vec::len);
    let content = h - footer;
    // Cells with something in them (not the same as the row above, which is what
    // blank background looks like), split into those that changed and the rest.
    let mut changed = Vec::new();
    let mut filled = 0usize;
    for (strip, (rows, old)) in next.iter().zip(prev).enumerate() {
        for i in header.max(1)..content {
            if rows[i] == rows[i - 1] {
                continue;
            }
            filled += 1;
            if rows[i] != old[i] {
                changed.push((strip, i));
            }
        }
    }
    let mut best: Option<(usize, usize, usize)> = None; // (score, candidates, dy)
    for dy in 1..content.saturating_sub(header + 8) {
        let limit = content - dy;
        let mut score = 0;
        let mut candidates = 0;
        for &(strip, i) in &changed {
            if i < limit {
                candidates += 1;
                if next[strip][i] == prev[strip][i + dy] {
                    score += 1;
                }
            }
        }
        if best.is_none_or(|(s, _, _)| score > s) {
            best = Some((score, candidates, dy));
        }
    }
    match best {
        Some((score, candidates, dy)) if score >= 8 && score * 4 >= candidates => Offset::Moved(dy),
        // Most of the page didn't change: not a scroll.
        _ if changed.len() * 7 < filled => Offset::InPlace,
        Some((score, candidates, dy)) => Offset::None(format!(
            "best offset {dy} matched {score} of {candidates} cells ({} of {filled} changed)",
            changed.len()
        )),
        None => Offset::None("frame too small".into()),
    }
}

/// Which columns differ between two frames, leaving out the right edge where a
/// scrollbar thumb moves. Falls back to all columns if nothing else changed.
fn moving_columns(prev: &RgbaImage, next: &RgbaImage) -> Vec<bool> {
    let w = prev.width() as usize;
    let skip = (w / 12).min(28);
    let mut moving = vec![false; w];
    for (a, b) in prev.as_raw().chunks_exact(w * 4).zip(next.as_raw().chunks_exact(w * 4)) {
        for (x, flag) in moving.iter_mut().enumerate().take(w - skip) {
            if !*flag && a[x * 4..x * 4 + 4] != b[x * 4..x * 4 + 4] {
                *flag = true;
            }
        }
    }
    if !moving.contains(&true) {
        moving[..w - skip].fill(true);
    }
    moving
}

/// Splits the used columns into up to [`STRIPS`] groups of about equal width, each
/// as byte ranges within a row.
fn strips(columns: &[bool]) -> Vec<Vec<(usize, usize)>> {
    let used: Vec<usize> = (0..columns.len()).filter(|&x| columns[x]).collect();
    let per = used.len().div_ceil(STRIPS).max(8);
    used.chunks(per)
        .map(|xs| {
            let mut runs: Vec<(usize, usize)> = Vec::new();
            for &x in xs {
                match runs.last_mut() {
                    Some((_, end)) if *end == x * 4 => *end += 4,
                    _ => runs.push((x * 4, x * 4 + 4)),
                }
            }
            runs
        })
        .collect()
}

/// One hash per row for each strip: `[strip][row]`.
fn cell_hashes(img: &RgbaImage, strips: &[Vec<(usize, usize)>]) -> Vec<Vec<u64>> {
    let w = img.width() as usize;
    strips
        .iter()
        .map(|runs| {
            img.as_raw()
                .chunks_exact(w * 4)
                .map(|row| {
                    let parts: Vec<&[u8]> = runs.iter().map(|&(a, b)| &row[a..b]).collect();
                    fnv1a(&parts)
                })
                .collect()
        })
        .collect()
}

/// One hash per whole row.
fn row_hashes(img: &RgbaImage) -> Vec<u64> {
    let w = img.width() as usize;
    img.as_raw().chunks_exact(w * 4).map(|row| fnv1a(&[row])).collect()
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
    fn scrolling_too_far_waits_for_an_overlapping_frame() {
        let page = page(200, 1500);
        let mut stitcher = Stitcher::new(&window(&page, 0, 300));
        assert!(matches!(stitcher.push(&window(&page, 200, 300)), Push::Added));
        // Jumped 500 px: nothing overlaps, so nothing is added...
        assert!(matches!(stitcher.push(&window(&page, 700, 300)), Push::NoMatch));
        // ...until the user scrolls back to a frame that overlaps the last good one.
        assert!(matches!(stitcher.push(&window(&page, 400, 300)), Push::Added));
        assert!(matches!(stitcher.push(&window(&page, 600, 300)), Push::Added));
        let out = stitcher.finish();
        assert!(out == window(&page, 0, 900), "no gap in the stitched page");
    }

    /// A frame of a web app: fixed sidebar on the left, fixed header on top,
    /// and the page scrolled to `top` in the rest.
    fn app_frame(page: &RgbaImage, top: u32) -> RgbaImage {
        let mut frame = RgbaImage::from_pixel(260, 300, image::Rgba([240, 240, 240, 255]));
        image::imageops::replace(&mut frame, &window(page, top, 300), 60, 0);
        for y in 0..300 {
            for x in 0..60 {
                // Sidebar menu: text-like stripes that don't scroll.
                let v = if (y / 6) % 3 == 0 && x > 8 { 40 } else { 230 };
                frame.put_pixel(x, y, image::Rgba([v, v, v, 255]));
            }
        }
        let header = RgbaImage::from_pixel(260, 30, image::Rgba([20, 60, 160, 255]));
        image::imageops::replace(&mut frame, &header, 0, 0);
        frame
    }

    #[test]
    fn ignores_a_fixed_sidebar_and_header() {
        let page = page(200, 1000);
        let mut stitcher = Stitcher::new(&app_frame(&page, 0));
        for top in [90, 200, 330, 450] {
            assert!(matches!(stitcher.push(&app_frame(&page, top)), Push::Added), "scrolled to {top}");
        }
        let out = stitcher.finish();
        assert_eq!(out.height(), 300 + 450);
        // The page column of the result is the page itself, below the header.
        let column = image::imageops::crop_imm(&out, 60, 30, 200, 300 + 450 - 30).to_image();
        assert!(column == window(&page, 30, 300 + 450 - 30));
    }

    #[test]
    fn a_side_column_moving_at_its_own_pace_is_ignored() {
        // A sticky menu (like Wikipedia's contents) scrolls more slowly than the page.
        let menu = page(60, 1200);
        let page = page(200, 1200);
        let frame = |top: u32| {
            let mut f = RgbaImage::new(260, 300);
            image::imageops::replace(&mut f, &window(&page, top, 300), 60, 0);
            let menu_view = image::imageops::crop_imm(&menu, 0, top * 2 / 3, 60, 300).to_image();
            image::imageops::replace(&mut f, &menu_view, 0, 0);
            f
        };
        let mut stitcher = Stitcher::new(&frame(0));
        for top in [100, 300, 400, 600] {
            assert!(matches!(stitcher.push(&frame(top)), Push::Added), "scrolled to {top}");
        }
        let out = stitcher.finish();
        assert_eq!(out.height(), 900);
        let column = image::imageops::crop_imm(&out, 60, 0, 200, 900).to_image();
        assert!(column == window(&page, 0, 900));
    }

    #[test]
    fn a_small_change_in_place_is_not_a_scroll() {
        let page = page(200, 600);
        let mut stitcher = Stitcher::new(&window(&page, 0, 300));
        let mut blink = window(&page, 0, 300);
        for x in 50..52 {
            for y in 100..116 {
                blink.put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
            }
        }
        assert!(matches!(stitcher.push(&blink), Push::Same));
        assert_eq!(stitcher.finish().height(), 300);
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

