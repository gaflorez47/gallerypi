use crossbeam_channel::{bounded, Receiver, Sender};
use lru::LruCache;
use slint::SharedPixelBuffer;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::thread;

/// Work item sent to the background loader thread.
struct LoadJob {
    item_id: i64,
    thumb_path: PathBuf,
}

/// Raw pixel result — all fields are Send.
pub struct LoadResult {
    pub item_id: i64,
    pub pixels: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

pub struct ThumbnailLoader {
    cache: LruCache<i64, slint::Image>,
    work_tx: Sender<LoadJob>,
    result_rx: Receiver<LoadResult>,
}

impl ThumbnailLoader {
    pub fn new(cache_capacity: usize) -> Self {
        let (work_tx, work_rx) = bounded::<LoadJob>(64);
        let (result_tx, result_rx) = bounded::<LoadResult>(64);
        start_worker_thread(work_rx, result_tx);
        Self {
            cache: LruCache::new(NonZeroUsize::new(cache_capacity).unwrap()),
            work_tx,
            result_rx,
        }
    }

    /// Check the LRU cache. On miss, enqueue a load job.
    /// Returns the cached image if available, otherwise `None`.
    pub fn request(&mut self, item_id: i64, thumb_path: &str) -> Option<slint::Image> {
        if let Some(img) = self.cache.get(&item_id) {
            return Some(img.clone());
        }
        let path = PathBuf::from(thumb_path);
        if path.exists() {
            let _ = self.work_tx.try_send(LoadJob { item_id, thumb_path: path });
        }
        None
    }

    /// Poll for completed loads (call from main thread / Slint timer).
    /// Returns all results since last poll, already inserted into the cache.
    pub fn poll_results(&mut self) -> Vec<(i64, slint::Image)> {
        let mut out = Vec::new();
        while let Ok(result) = self.result_rx.try_recv() {
            let buffer = SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                &result.pixels,
                result.width,
                result.height,
            );
            let img = slint::Image::from_rgba8(buffer);
            self.cache.put(result.item_id, img.clone());
            out.push((result.item_id, img));
        }
        out
    }
}

fn start_worker_thread(work_rx: Receiver<LoadJob>, result_tx: Sender<LoadResult>) {
    thread::Builder::new()
        .name("thumb-loader".into())
        .spawn(move || {
            for job in work_rx {
                match load_thumb_raw(&job.thumb_path) {
                    Ok((pixels, width, height)) => {
                        let _ = result_tx.send(LoadResult {
                            item_id: job.item_id,
                            pixels,
                            width,
                            height,
                        });
                    }
                    Err(e) => {
                        tracing::warn!("Failed to load {:?}: {}", job.thumb_path, e);
                    }
                }
            }
        })
        .expect("Failed to spawn thumb-loader thread");
}

fn load_thumb_raw(path: &std::path::Path) -> anyhow::Result<(Vec<u8>, u32, u32)> {
    let img = image::open(path)?.to_rgba8();
    let (w, h) = img.dimensions();
    Ok((img.into_raw(), w, h))
}
