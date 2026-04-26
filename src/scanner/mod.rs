pub mod exif;
pub mod walker;

use crate::db::{queries, Database};
use anyhow::Result;
use chrono::{Datelike, TimeZone, Utc};
use crossbeam_channel::Sender;
use std::path::PathBuf;

#[derive(Debug)]
pub enum ScanEvent {
    Progress { scanned: usize, total_estimate: usize },
    Complete { total: usize },
    Error(String),
}

pub struct Scanner {
    media_dir: PathBuf,
    progress_tx: Sender<ScanEvent>,
}

impl Scanner {
    pub fn new(media_dir: PathBuf, progress_tx: Sender<ScanEvent>) -> Self {
        Self { media_dir, progress_tx }
    }

    pub fn run(&self, db: &mut Database) -> Result<usize> {
        tracing::info!("Starting scan of {:?}", self.media_dir);
        let mut count = 0usize;
        let mut new_items = 0usize;

        for media_file in walker::walk_media(&self.media_dir) {
            count += 1;

            let path_str = match media_file.path.to_str() {
                Some(s) => s.to_owned(),
                None => continue,
            };

            // Get mtime
            let mtime = match exif::file_mtime(&media_file.path) {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!("Failed to get mtime for {:?}: {}", media_file.path, e);
                    continue;
                }
            };

            // Skip if already indexed with same mtime
            match queries::get_existing_mtime(&db.conn, &path_str) {
                Ok(Some(existing_mtime)) if existing_mtime == mtime => continue,
                Err(e) => {
                    tracing::warn!("DB check failed for {:?}: {}", media_file.path, e);
                    continue;
                }
                _ => {}
            }

            // Extract date
            let media_date = exif::extract_date(&media_file.path, mtime);
            let dt = Utc.timestamp_opt(media_date, 0).single().unwrap_or_default();

            let item = queries::MediaItem {
                id: 0,
                path: path_str,
                mtime,
                media_date,
                year: dt.year(),
                month: dt.month() as i32,
                media_type: media_file.media_type.to_owned(),
                width: None,
                height: None,
                thumb_path: None,
                thumb_ready: false,
            };

            if let Err(e) = queries::upsert_item(&db.conn, &item) {
                tracing::warn!("Failed to insert item: {}", e);
                continue;
            }

            new_items += 1;

            if count % 100 == 0 {
                let _ = self.progress_tx.try_send(ScanEvent::Progress {
                    scanned: count,
                    total_estimate: count + 100,
                });
            }
        }

        tracing::info!("Scan complete: {} files found, {} new/updated", count, new_items);
        let _ = self.progress_tx.send(ScanEvent::Complete { total: count });
        Ok(count)
    }
}
