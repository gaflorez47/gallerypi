pub mod gesture;

use crate::db::queries::MediaItem;
use anyhow::Result;
use slint::SharedPixelBuffer;

pub struct ViewerController {
    /// Items in the current month for swipe navigation.
    pub month_items: Vec<MediaItem>,
    /// Index into month_items of the currently displayed item.
    pub current_index: usize,
}

impl ViewerController {
    pub fn new() -> Self {
        Self {
            month_items: Vec::new(),
            current_index: 0,
        }
    }

    pub fn open(&mut self, item_id: i64, month_items: Vec<MediaItem>) {
        self.current_index = month_items
            .iter()
            .position(|i| i.id == item_id)
            .unwrap_or(0);
        self.month_items = month_items;
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

/// Load a full-resolution image from disk as a Slint Image.
pub fn load_image(path: &str) -> Result<slint::Image> {
    let img = image::open(path)?.to_rgba8();
    let (w, h) = img.dimensions();
    let buffer =
        SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(img.as_raw(), w, h);
    Ok(slint::Image::from_rgba8(buffer))
}
