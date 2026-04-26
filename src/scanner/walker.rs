use std::path::{Path, PathBuf};
use walkdir::WalkDir;

const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp"];
const VIDEO_EXTENSIONS: &[&str] = &["mp4"];

pub struct MediaFile {
    pub path: PathBuf,
    pub media_type: &'static str,
}

/// Walk `dir` and yield all supported media files.
pub fn walk_media(dir: &Path) -> impl Iterator<Item = MediaFile> {
    WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
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
