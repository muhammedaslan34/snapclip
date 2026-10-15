//! Images larger than the GPU's texture size limit (often 8192 px), such as long
//! scrolling captures, uploaded as several tiles that each fit.

use egui::{Color32, Context, Painter, Rect, TextureHandle, TextureId, TextureOptions, pos2, vec2};
use image::RgbaImage;

pub struct TiledTexture {
    /// Each texture with the part of the image it covers, in image pixels.
    tiles: Vec<(TextureHandle, Rect)>,
}

impl TiledTexture {
    pub fn new(ctx: &Context, name: &str, img: &RgbaImage, options: TextureOptions) -> Self {
        let side = ctx.input(|i| i.max_texture_side).max(256) as u32;
        let mut tiles = Vec::new();
        for y in (0..img.height()).step_by(side as usize) {
            for x in (0..img.width()).step_by(side as usize) {
                let (w, h) = (side.min(img.width() - x), side.min(img.height() - y));
                let part = image::imageops::crop_imm(img, x, y, w, h).to_image();
                let color = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], part.as_raw());
                let texture = ctx.load_texture(format!("{name}-{x}-{y}"), color, options);
                let covers = Rect::from_min_size(pos2(x as f32, y as f32), vec2(w as f32, h as f32));
                tiles.push((texture, covers));
            }
        }
        Self { tiles }
    }

    /// The texture when the whole image fits in one, as a screen capture always does.
    pub fn single(&self) -> Option<TextureId> {
        match self.tiles.as_slice() {
            [(texture, _)] => Some(texture.id()),
            _ => None,
        }
    }

    /// Draws the `src` part of the image (image pixels) stretched over `dest` (screen points).
    pub fn paint(&self, painter: &Painter, src: Rect, dest: Rect, tint: Color32) {
        if src.width() <= 0.0 || src.height() <= 0.0 {
            return;
        }
        let scale = dest.size() / src.size();
        let to_dest = |p: egui::Pos2| dest.min + (p - src.min) * scale;
        for (texture, covers) in &self.tiles {
            let part = covers.intersect(src);
            if part.width() <= 0.0 || part.height() <= 0.0 {
                continue;
            }
            let uv = Rect::from_min_max(
                ((part.min - covers.min) / covers.size()).to_pos2(),
                ((part.max - covers.min) / covers.size()).to_pos2(),
            );
            painter.image(texture.id(), Rect::from_min_max(to_dest(part.min), to_dest(part.max)), uv, tint);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_images_taller_than_the_texture_limit() {
        let ctx = Context::default();
        let side = ctx.input(|i| i.max_texture_side) as u32;
        let img = RgbaImage::new(300, side * 2 + 10);
        let tiled = TiledTexture::new(&ctx, "test", &img, TextureOptions::LINEAR);
        assert_eq!(tiled.tiles.len(), 3);
        assert!(tiled.single().is_none());
        let covered: f32 = tiled.tiles.iter().map(|(_, r)| r.height()).sum();
        assert_eq!(covered, img.height() as f32);
        assert!(TiledTexture::new(&ctx, "small", &RgbaImage::new(10, 10), TextureOptions::LINEAR).single().is_some());
    }
}
