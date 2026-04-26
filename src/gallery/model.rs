use crate::db::queries::MediaItem;
use crate::gallery::row_types::{GalleryRow, GalleryThumb};
use crate::ui::{GalleryRowData, ThumbnailData};
use crate::util::time::format_month_label;
use slint::{ModelRc, SharedString, VecModel};
use std::collections::HashMap;

/// Build the flat row sequence, month entry list, and item position index from all DB items.
/// Items must be pre-sorted: year DESC, month DESC, media_date ASC.
///
/// Returns: (rows, month_entries[(year, month, label, row_index)], item_positions[item_id -> (row_idx, col_idx)])
pub fn build_rows(
    items: &[MediaItem],
    n_cols: usize,
) -> (
    Vec<GalleryRow>,
    Vec<(i32, i32, String, usize)>,
    HashMap<i64, (usize, usize)>,
) {
    let mut rows: Vec<GalleryRow> = Vec::new();
    let mut month_entries: Vec<(i32, i32, String, usize)> = Vec::new();
    let mut positions: HashMap<i64, (usize, usize)> = HashMap::new();

    let mut current_key: Option<(i32, i32)> = None;
    let mut current_group: Vec<GalleryThumb> = Vec::new();

    let mut flush_group = |rows: &mut Vec<GalleryRow>,
                           positions: &mut HashMap<i64, (usize, usize)>,
                           group: &mut Vec<GalleryThumb>| {
        for chunk in group.chunks(n_cols) {
            let row_idx = rows.len();
            for (col_idx, thumb) in chunk.iter().enumerate() {
                positions.insert(thumb.item_id, (row_idx, col_idx));
            }
            rows.push(GalleryRow::ImageRow {
                items: chunk.to_vec(),
            });
        }
        group.clear();
    };

    for item in items {
        let key = (item.year, item.month);

        if current_key != Some(key) {
            flush_group(&mut rows, &mut positions, &mut current_group);
            let label = format_month_label(item.year, item.month);
            month_entries.push((item.year, item.month, label.clone(), rows.len()));
            rows.push(GalleryRow::MonthHeader {
                label,
                year: item.year,
                month: item.month,
            });
            current_key = Some(key);
        }

        current_group.push(GalleryThumb::from(item));
    }

    flush_group(&mut rows, &mut positions, &mut current_group);

    (rows, month_entries, positions)
}

/// Convert a GalleryRow to the Slint GalleryRowData struct.
pub fn row_to_slint(row: &GalleryRow) -> GalleryRowData {
    match row {
        GalleryRow::MonthHeader { label, .. } => GalleryRowData {
            is_header: true,
            header_label: label.as_str().into(),
            items: ModelRc::new(VecModel::from(vec![])),
            item_count: 0,
        },
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
            GalleryRowData {
                is_header: false,
                header_label: SharedString::default(),
                item_count: slint_items.len() as i32,
                items: ModelRc::new(VecModel::from(slint_items)),
            }
        }
    }
}
