//! Screenshot overlay: select a region, annotate it, then copy / save / pin.

mod annot;
mod overlay;
mod render;
mod text;

pub use overlay::{Action, CaptureView};
pub use text::fallback_font_data;
