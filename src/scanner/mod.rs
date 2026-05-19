pub mod exif;
pub mod walker;

use crate::db::{queries, Database};
use anyhow::Result;
use chrono::{Datelike, TimeZone, Utc};
use crossbeam_channel::Sender;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Debug)]
pub enum ScanEvent {
    Progress { scanned: usize, total_estimate: usize },
    Complete { total: usize, removed: usize },
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

        // Step 1: Load cached directory mtimes from DB.
        let dir_mtime_cache = queries::get_all_dir_mtimes(&db.conn)?;

        // Step 2: Walk directory tree (metadata only) and identify changed directories.
        let mut unchanged_dirs: HashSet<PathBuf> = HashSet::new();
        let mut changed_dirs: Vec<(String, i64)> = Vec::new();

        for entry in WalkDir::new(&self.media_dir)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_dir())
        {
            let path = entry.path();
            let path_str = match path.to_str() {
                Some(s) => s,
                None => continue,
            };
            let current_mtime = match exif::file_mtime(path) {
                Ok(m) => m,
                Err(_) => continue,
            };
            if dir_mtime_cache.get(path_str) == Some(&current_mtime) {
                unchanged_dirs.insert(path.to_path_buf());
            } else {
                changed_dirs.push((path_str.to_owned(), current_mtime));
            }
        }

        // Detect directories that were in the cache but no longer exist on disk.
        let seen_dirs: HashSet<&str> = changed_dirs
            .iter()
            .map(|(p, _)| p.as_str())
            .chain(unchanged_dirs.iter().filter_map(|p| p.to_str()))
            .collect();
        let removed_dirs: Vec<String> = dir_mtime_cache
            .keys()
            .filter(|k| !seen_dirs.contains(k.as_str()))
            .cloned()
            .collect();

        tracing::info!(
            "Dir check: {} unchanged, {} changed/new, {} removed",
            unchanged_dirs.len(),
            changed_dirs.len(),
            removed_dirs.len(),
        );

        // Step 3: Walk media files only in changed directories.
        let mut count = 0usize;
        let mut new_items = 0usize;

        for media_file in walker::walk_media_filtered(&self.media_dir, unchanged_dirs) {
            count += 1;

            let path_str = match media_file.path.to_str() {
                Some(s) => s.to_owned(),
                None => continue,
            };

            let mtime = match exif::file_mtime(&media_file.path) {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!("Failed to get mtime for {:?}: {}", media_file.path, e);
                    continue;
                }
            };

            // Skip if already indexed with same mtime.
            match queries::get_existing_mtime(&db.conn, &path_str) {
                Ok(Some(existing_mtime)) if existing_mtime == mtime => continue,
                Err(e) => {
                    tracing::warn!("DB check failed for {:?}: {}", media_file.path, e);
                    continue;
                }
                _ => {}
            }

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

        // Step 3.5: Reconcile changed directories — delete DB records for files no longer on disk.
        let mut orphaned_thumbs: Vec<String> = Vec::new();
        let mut total_removed = 0usize;

        for (dir_str, _) in &changed_dirs {
            let disk_files = walker::collect_media_in_dir(Path::new(dir_str));
            match queries::get_direct_children_in_dir(&db.conn, dir_str) {
                Ok(db_records) => {
                    let to_delete: Vec<i64> = db_records
                        .iter()
                        .filter(|(_, path, _)| !disk_files.contains(path))
                        .map(|(id, _, _)| *id)
                        .collect();
                    let thumb_paths: Vec<String> = db_records
                        .iter()
                        .filter(|(_, path, _)| !disk_files.contains(path))
                        .filter_map(|(_, _, tp)| tp.clone())
                        .collect();
                    if !to_delete.is_empty() {
                        match queries::delete_items_by_ids(&db.conn, &to_delete) {
                            Ok(n) => {
                                tracing::debug!("Removed {} orphaned records from {}", n, dir_str);
                                total_removed += n;
                                orphaned_thumbs.extend(thumb_paths);
                            }
                            Err(e) => tracing::warn!("Failed to delete orphans in {}: {}", dir_str, e),
                        }
                    }
                }
                Err(e) => tracing::warn!("Reconcile query failed for {}: {}", dir_str, e),
            }
        }

        // Step 3.6: Clean up entirely removed directories.
        for dir_str in &removed_dirs {
            match queries::get_all_descendants_in_dir(&db.conn, dir_str) {
                Ok(records) => {
                    let ids: Vec<i64> = records.iter().map(|(id, _, _)| *id).collect();
                    orphaned_thumbs.extend(records.iter().filter_map(|(_, _, tp)| tp.clone()));
                    if !ids.is_empty() {
                        match queries::delete_items_by_ids(&db.conn, &ids) {
                            Ok(n) => {
                                tracing::debug!("Removed {} records from deleted dir {}", n, dir_str);
                                total_removed += n;
                            }
                            Err(e) => tracing::warn!("Failed to delete records for removed dir {}: {}", dir_str, e),
                        }
                    }
                }
                Err(e) => tracing::warn!("Descendants query failed for {}: {}", dir_str, e),
            }
            if let Err(e) = queries::delete_scanned_dirs_with_prefix(&db.conn, dir_str) {
                tracing::warn!("Failed to clean scanned_dirs for {}: {}", dir_str, e);
            }
        }

        // Step 3.7: Delete orphaned thumbnail files from disk.
        for tp in &orphaned_thumbs {
            if let Err(e) = std::fs::remove_file(tp) {
                tracing::warn!("Failed to delete orphaned thumbnail {}: {}", tp, e);
            }
        }

        // Step 4: Persist updated directory mtimes so next launch can skip them.
        if !changed_dirs.is_empty() {
            if let Err(e) = queries::update_dir_mtimes(&db.conn, &changed_dirs) {
                tracing::warn!("Failed to update dir mtime cache: {}", e);
            }
        }

        tracing::info!(
            "Scan complete: {} files checked, {} new/updated, {} removed",
            count, new_items, total_removed
        );
        let _ = self.progress_tx.send(ScanEvent::Complete { total: count, removed: total_removed });
        Ok(count)
    }
}
