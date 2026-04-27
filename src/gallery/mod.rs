pub mod model;
pub mod month_model;
pub mod row_types;

use crate::db::queries::{self, MediaItem};
use crate::db::Database;
use crate::ui::{GalleryRowData, MonthEntry as SlintMonthEntry, ThumbnailData};
use anyhow::Result;
use month_model::MonthEntry;
use row_types::GalleryRow;
use slint::{Model, ModelRc, VecModel};
use std::collections::HashMap;
use std::rc::Rc;

pub struct GalleryController {
    rows: Vec<GalleryRow>,
    month_entries: Vec<MonthEntry>,
    n_cols: usize,
    all_items: Vec<MediaItem>,
    /// item_id -> (row_idx, col_idx) for thumbnail updates
    pub item_positions: HashMap<i64, (usize, usize)>,
    /// The live Slint row model — kept here so we can update individual rows
    pub row_model: Rc<VecModel<GalleryRowData>>,
}

impl GalleryController {
    pub fn new(n_cols: usize) -> Self {
        Self {
            rows: Vec::new(),
            month_entries: Vec::new(),
            n_cols,
            all_items: Vec::new(),
            item_positions: HashMap::new(),
            row_model: Rc::new(VecModel::from(vec![])),
        }
    }

    /// Load all items from DB and rebuild the model.
    pub fn reload(&mut self, db: &Database) -> Result<()> {
        self.all_items = queries::get_all_items_ordered(&db.conn)?;
        let (rows, month_data, positions) =
            model::build_rows(&self.all_items, self.n_cols);
        self.rows = rows;
        self.month_entries = month_data
            .into_iter()
            .map(|(y, m, _, row_idx)| MonthEntry::new(y, m, row_idx))
            .collect();

        tracing::info!("Gallery reload month entries {}", self.month_entries.len());

        self.item_positions = positions;

        // Rebuild Slint model — set_vec replaces all rows at once
        let slint_rows: Vec<GalleryRowData> =
            self.rows.iter().map(model::row_to_slint).collect();
        self.row_model.set_vec(slint_rows);
        Ok(())
    }

    /// Returns a ModelRc wrapping the shared VecModel.
    pub fn row_model_rc(&self) -> ModelRc<GalleryRowData> {
        ModelRc::new(self.row_model.clone())
    }

    /// Build Slint VecModel for month entries.
    pub fn build_month_model(&self) -> ModelRc<SlintMonthEntry> {
        let slint_months: Vec<SlintMonthEntry> =
            self.month_entries.iter().map(month_model::to_slint).collect();
        ModelRc::new(VecModel::from(slint_months))
    }

    /// Update a single thumbnail cell in the live model.
    pub fn update_thumbnail(&self, item_id: i64, image: slint::Image) {
        let Some(&(row_idx, col_idx)) = self.item_positions.get(&item_id) else {
            return;
        };
        let Some(mut row) = self.row_model.row_data(row_idx) else {
            return;
        };
        // Rebuild items for this row with the new image at col_idx
        let items_count = row.item_count as usize;
        let old_items = row.items.clone();
        let mut new_items: Vec<ThumbnailData> = (0..items_count)
            .map(|i| old_items.row_data(i).unwrap_or_default())
            .collect();
        if col_idx < new_items.len() {
            new_items[col_idx].thumb_image = image;
            new_items[col_idx].thumb_ready = true;
        }
        row.items = ModelRc::new(VecModel::from(new_items));
        self.row_model.set_row_data(row_idx, row);
    }

    /// Find the row index for a given (year, month).
    pub fn row_index_for_month(&self, year: i32, month: i32) -> Option<usize> {
        self.month_entries
            .iter()
            .find(|e| e.year == year && e.month == month)
            .map(|e| e.row_index)
    }

    /// Get a random month entry (for Reminisce).
    pub fn random_month(&self) -> Option<&MonthEntry> {
        if self.month_entries.is_empty() {
            return None;
        }
        use std::time::{SystemTime, UNIX_EPOCH};
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as usize)
            .unwrap_or(0);
        let idx = seed % self.month_entries.len();
        Some(&self.month_entries[idx])
    }

    /// Get all items in a given month (for viewer navigation).
    pub fn items_in_month(&self, year: i32, month: i32) -> Vec<MediaItem> {
        self.all_items
            .iter()
            .filter(|i| i.year == year && i.month == month)
            .cloned()
            .collect()
    }

    pub fn item_by_id(&self, id: i64) -> Option<&MediaItem> {
        self.all_items.iter().find(|i| i.id == id)
    }

    /// Enqueue thumbnail load requests for items that already have disk thumbnails.
    /// Prioritizes the most recent items (top of gallery). Call after reload.
    pub fn request_ready_thumbnails(
        &self,
        loader: &mut crate::thumbnail::ThumbnailLoader,
        limit: usize,
    ) {
        let mut count = 0;
        for item in &self.all_items {
            if item.thumb_ready {
                if let Some(ref path) = item.thumb_path {
                    // Cache hit: image is returned immediately — apply it now.
                    // Cache miss: load job is enqueued; result arrives via poll_results.
                    if let Some(img) = loader.request(item.id, path) {
                        self.update_thumbnail(item.id, img);
                    }
                    count += 1;
                    if count >= limit {
                        break;
                    }
                }
            }
        }
        tracing::debug!("Enqueued {} thumbnail requests", count);
    }
}
