use std::collections::HashSet;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp"];
const VIDEO_EXTENSIONS: &[&str] = &["mp4"];

pub struct MediaFile {
    pub path: PathBuf,
    pub media_type: &'static str,
}

/// Collect media files that are direct children of `dir` (non-recursive).
pub fn collect_media_in_dir(dir: &Path) -> HashSet<String> {
    let mut set = HashSet::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return set };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.to_lowercase());
        let ext = ext.as_deref().unwrap_or("");
        if IMAGE_EXTENSIONS.contains(&ext) || VIDEO_EXTENSIONS.contains(&ext) {
            if let Some(s) = path.to_str() {
                set.insert(s.to_owned());
            }
        }
    }
    set
}

/// Walk `dir` and yield all supported media files.
pub fn walk_media(dir: &Path) -> impl Iterator<Item = MediaFile> {
    walk_media_filtered(dir, HashSet::new())
}

/// Walk `dir`, skipping any directory (and its subtree) whose path is in `skip_dirs`.
pub fn walk_media_filtered(
    dir: &Path,
    skip_dirs: HashSet<PathBuf>,
) -> impl Iterator<Item = MediaFile> {
    WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(move |e| {
            if e.file_type().is_dir() {
                !skip_dirs.contains(e.path())
            } else {
                true
            }
        })
        .filter_map(|entry| entry.ok())
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| {
            let path = e.into_path();
            let ext = path
                .extension()
                .and_then(|s| s.to_str())
                .map(|s| s.to_lowercase());
            let ext = ext.as_deref().unwrap_or("");

            if IMAGE_EXTENSIONS.contains(&ext) {
                Some(MediaFile {
                    path,
                    media_type: "image",
                })
            } else if VIDEO_EXTENSIONS.contains(&ext) {
                Some(MediaFile {
                    path,
                    media_type: "video",
                })
            } else {
                None
            }
        })
}
