use crate::config::Config;
use crate::db::{queries, Database};
use crate::util::hash::thumb_cache_key;
use anyhow::Result;
use fast_image_resize::images::Image as FirImage;
use fast_image_resize::{PixelType, Resizer};
use std::path::{Path, PathBuf};

pub fn generate_thumbnail(
    source_path: &Path,
    source_mtime: i64,
    thumb_dir: &Path,
    thumb_size: u32,
) -> Result<PathBuf> {
    let key = thumb_cache_key(source_path.to_str().unwrap_or(""), source_mtime);
    let thumb_path = thumb_dir.join(format!("{}.jpg", key));

    if thumb_path.exists() {
        return Ok(thumb_path);
    }

    // Load and decode image
    let img = image::open(source_path)?.to_rgba8();
    let (orig_w, orig_h) = img.dimensions();

    // Center-crop to square
    let min_dim = orig_w.min(orig_h);
    let x_off = (orig_w - min_dim) / 2;
    let y_off = (orig_h - min_dim) / 2;
    let cropped = image::imageops::crop_imm(&img, x_off, y_off, min_dim, min_dim).to_image();

    // Resize using fast_image_resize (v5 API uses plain u32, not NonZeroU32)
    let src_image = FirImage::from_vec_u8(min_dim, min_dim, cropped.into_raw(), PixelType::U8x4)?;
    let mut dst_image = FirImage::new(thumb_size, thumb_size, PixelType::U8x4);

    let mut resizer = Resizer::new();
    resizer.resize(&src_image, &mut dst_image, None)?;

    // Convert RGBA to RGB and save as JPEG
    let rgb_data: Vec<u8> = dst_image
        .buffer()
        .chunks(4)
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect();

    let mut out = std::fs::File::create(&thumb_path)?;
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85);
    encoder.encode(&rgb_data, thumb_size, thumb_size, image::ColorType::Rgb8.into())?;

    Ok(thumb_path)
}

/// Spawn background thumbnail generation for all images missing thumbnails.
pub fn run_generation(config: &Config, db_path: &Path) {
    let thumb_dir = crate::config::thumb_dir();
    let thumb_size = config.gallery.thumbnail_size;
    let n_threads = config.performance.thumb_gen_threads;
    let db_path = db_path.to_path_buf();

    std::thread::spawn(move || {
        let pool = match rayon::ThreadPoolBuilder::new()
            .num_threads(n_threads)
            .build()
        {
            Ok(p) => p,
            Err(e) => {
                tracing::error!("Failed to build rayon pool: {}", e);
                return;
            }
        };

        let conn = match rusqlite::Connection::open(&db_path) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("Thumb generator failed to open DB: {}", e);
                return;
            }
        };

        let items = match queries::get_items_needing_thumbnails(&conn) {
            Ok(items) => items,
            Err(e) => {
                tracing::error!("Failed to query items needing thumbnails: {}", e);
                return;
            }
        };

        if items.is_empty() {
            tracing::info!("All thumbnails up to date");
            return;
        }

        tracing::info!(
            "Generating {} thumbnails with {} threads",
            items.len(),
            n_threads
        );

        pool.install(|| {
            use rayon::prelude::*;
            items.par_iter().for_each(|(id, path)| {
                // Small sleep on RPi to prevent thermal throttle
                #[cfg(target_arch = "aarch64")]
                std::thread::sleep(std::time::Duration::from_millis(5));

                let source_path = Path::new(path);
                let mtime = std::fs::metadata(source_path)
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| {
                        t.duration_since(std::time::SystemTime::UNIX_EPOCH)
                            .ok()
                            .map(|d| d.as_secs() as i64)
                    })
                    .unwrap_or(0);

                match generate_thumbnail(source_path, mtime, &thumb_dir, thumb_size) {
                    Ok(thumb_path) => {
                        if let Ok(conn) = rusqlite::Connection::open(&db_path) {
                            let thumb_str = thumb_path.to_string_lossy();
                            if let Err(e) = queries::mark_thumb_ready(&conn, *id, &thumb_str) {
                                tracing::warn!("Failed to mark thumb ready for id {}: {}", id, e);
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Failed to generate thumb for {}: {}", path, e);
                    }
                }
            });
        });

        tracing::info!("Thumbnail generation complete");
    });
}
