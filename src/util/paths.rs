use std::path::PathBuf;

pub use crate::config::{cache_dir, config_path, db_path, thumb_dir};

/// Ensure the thumb cache directory exists.
pub fn ensure_thumb_dir() -> anyhow::Result<PathBuf> {
    let dir = thumb_dir();
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}
