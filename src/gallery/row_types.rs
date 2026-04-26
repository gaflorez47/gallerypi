use crate::db::queries::MediaItem;

/// Maximum columns supported by the Slint array type in GalleryRowData.
/// Must match the array size in app.slint ThumbnailData items.
pub const MAX_COLS: usize = 6;

#[derive(Debug, Clone)]
pub struct GalleryThumb {
    pub item_id: i64,
    pub path: String,
    pub thumb_path: Option<String>,
    pub thumb_ready: bool,
    pub media_type: String,
    pub mtime: i64,
}

impl From<&MediaItem> for GalleryThumb {
    fn from(item: &MediaItem) -> Self {
        Self {
            item_id: item.id,
            path: item.path.clone(),
            thumb_path: item.thumb_path.clone(),
            thumb_ready: item.thumb_ready,
            media_type: item.media_type.clone(),
            mtime: item.mtime,
        }
    }
}

#[derive(Debug, Clone)]
pub enum GalleryRow {
    MonthHeader {
        label: String,
        year: i32,
        month: i32,
    },
    ImageRow {
        items: Vec<GalleryThumb>,
    },
}

impl GalleryRow {
    pub fn is_header(&self) -> bool {
        matches!(self, GalleryRow::MonthHeader { .. })
    }
}
