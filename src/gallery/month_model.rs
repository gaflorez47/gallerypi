use crate::util::time::format_month_label;

#[derive(Debug, Clone)]
pub struct MonthEntry {
    pub year: i32,
    pub month: i32,
    pub label: String,
    pub row_index: usize,
}

impl MonthEntry {
    pub fn new(year: i32, month: i32, row_index: usize) -> Self {
        Self {
            year,
            month,
            label: format_month_label(year, month),
            row_index,
        }
    }
}

/// Convert to the Slint MonthEntry struct.
pub fn to_slint(entry: &MonthEntry) -> crate::ui::MonthEntry {
    crate::ui::MonthEntry {
        label: entry.label.as_str().into(),
        year: entry.year,
        month: entry.month,
        row_index: entry.row_index as i32,
    }
}
