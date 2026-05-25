pub mod gesture;

use crate::db::queries::MediaItem;
use anyhow::Result;
use slint::SharedPixelBuffer;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Thread-safe preload cache: path → (raw RGBA8 pixels, width, height).
pub type PreloadCache = Arc<Mutex<HashMap<String, (Vec<u8>, u32, u32)>>>;

pub struct ViewerController {
    /// Items in the current month for swipe navigation.
    pub month_items: Vec<MediaItem>,
    /// Index into month_items of the currently displayed item.
    pub current_index: usize,
    /// Preloaded raw pixels. Arc<Mutex<...>> so background threads can insert directly.
    pub preload_cache: PreloadCache,
}

impl ViewerController {
    pub fn new() -> Self {
        Self {
            month_items: Vec::new(),
            current_index: 0,
            preload_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn open(&mut self, item_id: i64, month_items: Vec<MediaItem>) {
        self.preload_cache.lock().unwrap().clear();
        self.current_index = month_items
            .iter()
            .position(|i| i.id == item_id)
            .unwrap_or(0);
        self.month_items = month_items;
    }

    /// Returns paths of adjacent images that should be preloaded but are not yet cached.
    pub fn preload_paths(&self, count: usize) -> Vec<String> {
        let len = self.month_items.len();
        let ci = self.current_index;
        let start = ci.saturating_sub(count);
        let end = (ci + count + 1).min(len);
        let cache = self.preload_cache.lock().unwrap();
        (start..end)
            .filter(|&i| i != ci)
            .filter_map(|i| {
                let path = &self.month_items[i].path;
                if cache.contains_key(path) { None } else { Some(path.clone()) }
            })
            .collect()
    }

    /// Remove and return cached pixels for the given path (frees memory after display).
    pub fn take_from_cache(&self, path: &str) -> Option<(Vec<u8>, u32, u32)> {
        self.preload_cache.lock().unwrap().remove(path)
    }

    pub fn current_item(&self) -> Option<&MediaItem> {
        self.month_items.get(self.current_index)
    }

    pub fn go_next(&mut self) -> Option<&MediaItem> {
        if self.current_index + 1 < self.month_items.len() {
            self.current_index += 1;
            self.month_items.get(self.current_index)
        } else {
            None
        }
    }

    pub fn go_prev(&mut self) -> Option<&MediaItem> {
        if self.current_index > 0 {
            self.current_index -= 1;
            self.month_items.get(self.current_index)
        } else {
            None
        }
    }
}

/// Load an image from disk, downscaling to fit within max_w × max_h if necessary.
pub fn load_image(path: &str, max_w: u32, max_h: u32) -> Result<slint::Image> {
    use fast_image_resize::{images::Image as FirImage, PixelType, Resizer};
    let img = image::open(path)?.to_rgba8();
    let (orig_w, orig_h) = img.dimensions();
    let scale = (max_w as f32 / orig_w as f32)
        .min(max_h as f32 / orig_h as f32)
        .min(1.0);
    let (pixels, w, h) = if scale < 1.0 {
        let new_w = ((orig_w as f32 * scale).round() as u32).max(1);
        let new_h = ((orig_h as f32 * scale).round() as u32).max(1);
        let src = FirImage::from_vec_u8(orig_w, orig_h, img.into_raw(), PixelType::U8x4)?;
        let mut dst = FirImage::new(new_w, new_h, PixelType::U8x4);
        Resizer::new().resize(&src, &mut dst, None)?;
        (dst.into_vec(), new_w, new_h)
    } else {
        (img.into_raw(), orig_w, orig_h)
    };
    let buffer = SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&pixels, w, h);
    Ok(slint::Image::from_rgba8(buffer))
}
