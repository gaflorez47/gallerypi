pub mod model;
pub mod month_model;
pub mod row_types;

use crate::db::queries::{self, MediaItem};
use crate::db::Database;
use crate::ui::{GalleryRowData, MonthEntry as SlintMonthEntry, ThumbnailData};
use anyhow::Result;
use month_model::MonthEntry;
use row_types::{GalleryRow, GalleryThumb};
use slint::{Model, ModelRc, VecModel};
use std::collections::HashMap;
use std::rc::Rc;

pub struct GalleryController {
    rows: Vec<GalleryRow>,
    /// Cumulative Y offset (px) of each row; mixed heights (header=48, image=cell_size+4).
    row_tops: Vec<f32>,
    /// Last list-view width used for row_tops computation; -1 forces first recompute.
    last_lv_width: f32,
    /// Cell size capped at thumb_size (mirrors Slint's GalleryRowDelegate formula).
    last_cell_size: f32,
    month_entries: Vec<MonthEntry>,
    n_cols: usize,
    all_items: Vec<MediaItem>,
    /// item_id -> (row_idx, col_idx) for thumbnail updates
    pub item_positions: HashMap<i64, (usize, usize)>,
    /// The live Slint row model — kept here so we can update individual rows
    pub row_model: Rc<VecModel<GalleryRowData>>,
    /// Direct references to each image row's inner items model (None for header rows).
    /// Updating these in-place avoids replacing the ModelRc on the outer row, which
    /// means Slint's `for item in items:` binding observes changes directly.
    item_models: Vec<Option<Rc<VecModel<ThumbnailData>>>>,
}

impl GalleryController {
    pub fn new(n_cols: usize) -> Self {
        Self {
            rows: Vec::new(),
            row_tops: Vec::new(),
            last_lv_width: -1.0,
            last_cell_size: 0.0,
            month_entries: Vec::new(),
            n_cols,
            all_items: Vec::new(),
            item_positions: HashMap::new(),
            row_model: Rc::new(VecModel::from(vec![])),
            item_models: Vec::new(),
        }
    }

    /// Load all items from DB and rebuild the model.
    pub fn reload(&mut self, db: &Database, thumb_size: f32) -> Result<()> {
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

        // Precompute cumulative row Y offsets for visibility queries.
        let image_row_h = thumb_size + 4.0;
        let mut y = 0.0f32;
        self.row_tops = self.rows.iter().map(|r| {
            let top = y;
            y += match r {
                GalleryRow::MonthHeader { .. } => 48.0,
                GalleryRow::ImageRow { .. } => image_row_h,
            };
            top
        }).collect();

        // Rebuild Slint model and item_models together so every image row shares
        // the same Rc<VecModel<ThumbnailData>> between item_models and the Slint row.
        // This lets update_thumbnail mutate the inner model directly, which Slint's
        // `for item in items:` binding observes without any outer set_row_data call.
        let mut slint_rows: Vec<GalleryRowData> = Vec::with_capacity(self.rows.len());
        let mut item_models: Vec<Option<Rc<VecModel<ThumbnailData>>>> =
            Vec::with_capacity(self.rows.len());
        for row in &self.rows {
            match row {
                GalleryRow::MonthHeader { label, .. } => {
                    slint_rows.push(GalleryRowData {
                        is_header: true,
                        header_label: label.as_str().into(),
                        items: ModelRc::new(VecModel::from(vec![])),
                        item_count: 0,
                    });
                    item_models.push(None);
                }
                GalleryRow::ImageRow { items } => {
                    let slint_items: Vec<ThumbnailData> = items
                        .iter()
                        .map(|t| ThumbnailData {
                            item_id: t.item_id as i32,
                            thumb_image: Default::default(),
                            thumb_ready: false,
                            media_type: t.media_type.as_str().into(),
                        })
                        .collect();
                    let inner = Rc::new(VecModel::from(slint_items));
                    slint_rows.push(GalleryRowData {
                        is_header: false,
                        header_label: Default::default(),
                        item_count: items.len() as i32,
                        items: ModelRc::new(inner.clone()),
                    });
                    item_models.push(Some(inner));
                }
            }
        }
        self.item_models = item_models;
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

    /// Recompute row_tops using the actual ListView width.
    /// Only recomputes when lv_width changes by more than 1px.
    pub fn ensure_row_tops(&mut self, lv_width: f32, thumb_size: f32) {
        if (lv_width - self.last_lv_width).abs() < 1.0 {
            return;
        }
        self.last_lv_width = lv_width;
        let cell_size = ((lv_width - 4.0 - (self.n_cols.saturating_sub(1)) as f32 * 2.0)
            / self.n_cols.max(1) as f32)
            .min(thumb_size);
        self.last_cell_size = cell_size;
        let image_row_h = cell_size + 4.0;
        tracing::info!(
            "[ensure_row_tops] lv_width={} n_cols={} thumb_size={} cell_size={:.2} image_row_h={:.2} total_rows={}",
            lv_width, self.n_cols, thumb_size, cell_size, image_row_h, self.rows.len()
        );
        let mut y = 0.0f32;
        self.row_tops = self.rows.iter().map(|r| {
            let top = y;
            y += match r {
                GalleryRow::MonthHeader { .. } => 48.0,
                GalleryRow::ImageRow { .. } => image_row_h,
            };
            top
        }).collect();

        // Log total content height and last few row_tops for sanity check.
        if let Some(&last_top) = self.row_tops.last() {
            tracing::info!(
                "[ensure_row_tops] recomputed: {} rows, last row_tops={:.1} (total_h≈{:.1})",
                self.row_tops.len(), last_top, last_top + image_row_h
            );
        }
    }

    /// Clear a single thumbnail cell in the live model (eviction).
    pub fn clear_thumbnail(&self, item_id: i64) {
        let Some(&(row_idx, col_idx)) = self.item_positions.get(&item_id) else {
            return;
        };
        let Some(Some(inner)) = self.item_models.get(row_idx) else {
            return;
        };
        tracing::trace!("clear_thumbnail: item_id={} row={} col={}", item_id, row_idx, col_idx);
        if let Some(mut cell) = inner.row_data(col_idx) {
            if cell.thumb_ready {
                cell.thumb_image = Default::default();
                cell.thumb_ready = false;
                inner.set_row_data(col_idx, cell);
            }
        }
    }

    /// Update a single thumbnail cell in the live model.
    pub fn update_thumbnail(&self, item_id: i64, image: slint::Image) {
        let Some(&(row_idx, col_idx)) = self.item_positions.get(&item_id) else {
            tracing::warn!("update_thumbnail: item_id={} not in item_positions", item_id);
            return;
        };
        let Some(Some(inner)) = self.item_models.get(row_idx) else {
            tracing::warn!("update_thumbnail: item_id={} row={} has no inner model", item_id, row_idx);
            return;
        };
        tracing::trace!("update_thumbnail: item_id={} row={} col={}", item_id, row_idx, col_idx);
        if let Some(mut cell) = inner.row_data(col_idx) {
            cell.thumb_image = image;
            cell.thumb_ready = true;
            inner.set_row_data(col_idx, cell);
        } else {
            tracing::warn!("update_thumbnail: item_id={} row={} col={} out of bounds (inner len={})",
                item_id, row_idx, col_idx, inner.row_count());
        }
    }

    /// Returns the pixel Y offset of a row (top of that row), from row_tops.
    pub fn pixel_offset_for_row(&self, row_idx: usize) -> f32 {
        let offset = self.row_tops.get(row_idx).copied().unwrap_or(0.0);
        tracing::info!(
            "[pixel_offset_for_row] row_idx={} offset={:.1} last_lv_width={:.1} last_cell_size={:.2} total_row_tops={}",
            row_idx, offset, self.last_lv_width, self.last_cell_size, self.row_tops.len()
        );
        offset
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

    /// Returns the thumbnails in a single row by index (empty slice for header rows).
    pub fn items_in_row(&self, row_idx: usize) -> &[GalleryThumb] {
        match self.rows.get(row_idx) {
            Some(GalleryRow::ImageRow { items }) => items,
            _ => &[],
        }
    }

}
