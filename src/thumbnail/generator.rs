use crate::config::Config;
use crate::db::queries;
use crate::util::hash::thumb_cache_key;
use anyhow::Result;
use crossbeam_channel::{bounded, Receiver};
use fast_image_resize::images::Image as FirImage;
use fast_image_resize::{PixelType, Resizer};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub struct GenJob {
    pub item_id: i64,
    pub path: String,
    pub mtime: i64,
    pub media_type: String,
}

/// LIFO job queue: push appends to the back; the generator pops from the back.
/// Newest submissions (currently-visible items) are processed before older ones,
/// so a scroll jump doesn't stall behind a large backlog of off-screen work.
#[derive(Clone)]
pub struct GenQueue {
    stack: Arc<Mutex<Vec<GenJob>>>,
    /// One-shot wake-up channel: the worker blocks here when the stack is empty.
    notify_tx: crossbeam_channel::Sender<()>,
}

impl GenQueue {
    pub fn push(&self, job: GenJob) {
        self.stack.lock().unwrap().push(job);
        let _ = self.notify_tx.try_send(());
    }
}

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

pub fn generate_video_thumbnail(
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

    let filter = format!(
        "scale={s}:{s}:force_original_aspect_ratio=increase,crop={s}:{s}",
        s = thumb_size
    );
    let status = std::process::Command::new("ffmpeg")
        .args([
            "-ss", "1",
            "-i", source_path.to_str().unwrap_or(""),
            "-frames:v", "1",
            "-vf", &filter,
            "-q:v", "2",
            "-y",
            thumb_path.to_str().unwrap_or(""),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;

    if !status.success() {
        anyhow::bail!("ffmpeg exited with status {}", status);
    }
    Ok(thumb_path)
}

/// Start a persistent on-demand thumbnail generator.
/// Returns (GenQueue, result_rx).  Push GenJob items via queue.push(); completed
/// (item_id, thumb_path) arrive on result_rx.  The worker runs for the app lifetime.
/// The queue is LIFO: most recently pushed jobs (currently visible items) run first,
/// so scroll jumps don't stall behind a backlog of off-screen work.
pub fn start_on_demand_generator(
    config: &Config,
    db_path: &Path,
) -> (GenQueue, Receiver<(i64, String)>) {
    let thumb_dir = crate::config::thumb_dir();
    let thumb_size = config.gallery.thumbnail_size;
    let db_path = db_path.to_path_buf();

    let stack: Arc<Mutex<Vec<GenJob>>> = Arc::new(Mutex::new(Vec::new()));
    let (notify_tx, notify_rx) = bounded::<()>(1);
    let (result_tx, result_rx) = bounded::<(i64, String)>(256);

    let queue = GenQueue { stack: stack.clone(), notify_tx };

    std::thread::Builder::new()
        .name("thumb-gen".into())
        .spawn(move || {
            let conn = match rusqlite::Connection::open(&db_path) {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!("Thumb generator failed to open DB: {}", e);
                    return;
                }
            };

            loop {
                // Pop from the back (LIFO): newest job = most recently visible items.
                let job = stack.lock().unwrap().pop();
                let job = match job {
                    Some(j) => j,
                    None => {
                        // Stack empty — wait for a push notification, then retry.
                        let _ = notify_rx.recv();
                        continue;
                    }
                };

                // Small sleep on RPi to prevent thermal throttle.
                #[cfg(target_arch = "aarch64")]
                std::thread::sleep(std::time::Duration::from_millis(5));

                tracing::info!("Generating thumbnail: id={} path={}", job.item_id, job.path);
                let source_path = Path::new(&job.path);
                let result = if job.media_type == "video" {
                    generate_video_thumbnail(source_path, job.mtime, &thumb_dir, thumb_size)
                } else {
                    generate_thumbnail(source_path, job.mtime, &thumb_dir, thumb_size)
                };
                match result {
                    Ok(thumb_path) => {
                        let thumb_str = thumb_path.to_string_lossy().into_owned();
                        if let Err(e) = queries::mark_thumb_ready(&conn, job.item_id, &thumb_str) {
                            tracing::warn!("Failed to mark thumb ready for id {}: {}", job.item_id, e);
                        } else {
                            tracing::info!("Thumbnail ready: id={} thumb={}", job.item_id, thumb_str);
                            let _ = result_tx.send((job.item_id, thumb_str));
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Failed to generate thumb for {}: {}", job.path, e);
                    }
                }
            }
        })
        .expect("Failed to spawn thumb-gen thread");

    (queue, result_rx)
}
