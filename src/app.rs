use crate::config::Config;
use crate::db::Database;
use crate::gallery::GalleryController;
use crate::scanner::{ScanEvent, Scanner};
use crate::thumbnail::generator::{self, GenJob};
use crate::thumbnail::ThumbnailLoader;
use crate::ui::{AppWindow, Screen};
use crate::video::VideoController;
use crate::viewer::ViewerController;
use anyhow::Result;
use crossbeam_channel::bounded;
use slint::{ComponentHandle, SharedPixelBuffer, Timer, TimerMode};
use std::cell::RefCell;
use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;

/// How many extra rows above and below the visible area to pre-load.
const SCROLL_BUFFER_ROWS: usize = 2;
/// Maximum number of thumbnails kept live in the VecModel before eviction.
const MAX_LOADED_ITEMS: usize = 300;

pub fn run(config: Config, db_path: PathBuf) -> Result<()> {
    let db = Rc::new(RefCell::new(Database::open(&db_path)?));
    let thumb_size = config.gallery.thumbnail_size as f32;

    let n_cols = config.gallery.grid_columns as usize;
    let mut gallery_ctrl = GalleryController::new(n_cols);
    gallery_ctrl.reload(&db.borrow(), thumb_size)?;

    let thumb_loader = Rc::new(RefCell::new(ThumbnailLoader::new(
        config.performance.thumb_cache_entries,
    )));

    // Start the persistent on-demand generator immediately.
    crate::util::paths::ensure_thumb_dir().ok();
    let (gen_tx, gen_rx) = generator::start_on_demand_generator(&config, &db_path);
    let gen_tx = Rc::new(gen_tx);

    let window = AppWindow::new()?;
    window.set_grid_columns(n_cols as i32);
    window.set_thumb_size(thumb_size);
    window.set_gallery_rows(gallery_ctrl.row_model_rc());
    window.set_month_entries(gallery_ctrl.build_month_model());

    if config.ui.fullscreen {
        window.window().set_fullscreen(true);
    }

    let gallery_ctrl = Rc::new(RefCell::new(gallery_ctrl));
    let viewer_ctrl = Rc::new(RefCell::new(ViewerController::new()));
    let video_ctrl = Rc::new(RefCell::new(VideoController::new(config.video.clone())));

    // --- Scanner ---
    let scan_rx: Option<crossbeam_channel::Receiver<ScanEvent>> =
        if config.performance.scan_on_startup {
            let (scan_tx, scan_rx) = bounded::<ScanEvent>(32);
            let media_dir = config.gallery.media_dir.clone();
            let db_path_scan = db_path.clone();

            std::thread::spawn(move || {
                match Database::open(&db_path_scan) {
                    Ok(mut scan_db) => {
                        let scanner = Scanner::new(media_dir, scan_tx);
                        if let Err(e) = scanner.run(&mut scan_db) {
                            tracing::error!("Scanner error: {}", e);
                        }
                    }
                    Err(e) => tracing::error!("Scanner DB open failed: {}", e),
                }
            });

            Some(scan_rx)
        } else {
            None
        };

    // Poll scan channel from main thread via Timer (avoids Rc<> crossing thread boundary)
    let scan_timer = Timer::default();
    if let Some(rx) = scan_rx {
        let gallery_clone = gallery_ctrl.clone();
        let db_clone = db.clone();
        let window_weak = window.as_weak();
        scan_timer.start(
            TimerMode::Repeated,
            std::time::Duration::from_millis(200),
            move || {
                while let Ok(event) = rx.try_recv() {
                    match event {
                        ScanEvent::BatchComplete { new_items } => {
                            tracing::debug!("Scan batch: {} new items so far, reloading gallery", new_items);
                            if let Some(w) = window_weak.upgrade() {
                                let mut gallery = gallery_clone.borrow_mut();
                                if let Err(e) = gallery.reload(&db_clone.borrow(), thumb_size) {
                                    tracing::error!("Gallery reload: {}", e);
                                    return;
                                }
                                w.set_gallery_rows(gallery.row_model_rc());
                                w.set_month_entries(gallery.build_month_model());
                            }
                        }
                        ScanEvent::Complete { total } => {
                            tracing::info!("Scan complete ({} files), reloading", total);
                            if let Some(w) = window_weak.upgrade() {
                                let mut gallery = gallery_clone.borrow_mut();
                                if let Err(e) = gallery.reload(&db_clone.borrow(), thumb_size) {
                                    tracing::error!("Gallery reload: {}", e);
                                    return;
                                }
                                w.set_gallery_rows(gallery.row_model_rc());
                                w.set_month_entries(gallery.build_month_model());
                            }
                        }
                        ScanEvent::Error(e) => tracing::error!("Scan error: {}", e),
                        ScanEvent::Progress { scanned, .. } => {
                            tracing::debug!("Scan: {} files", scanned);
                        }
                    }
                }
            },
        );
    }

    // --- Gallery callbacks ---
    let window_weak = window.as_weak();

    window.on_item_tapped({
        let gallery_clone = gallery_ctrl.clone();
        let viewer_clone = viewer_ctrl.clone();
        let video_clone = video_ctrl.clone();
        let window_weak = window_weak.clone();
        move |item_id, media_type| {
            tracing::info!("on_item_tapped: item_id={} media_type={}", item_id, media_type);
            let Some(window) = window_weak.upgrade() else {
                tracing::warn!("on_item_tapped: window_weak upgrade failed");
                return;
            };
            let gallery = gallery_clone.borrow();
            let item_id = item_id as i64;

            if let Some(item) = gallery.item_by_id(item_id) {
                tracing::info!("on_item_tapped: found item path={}", item.path);
                let year = item.year;
                let month = item.month;
                let path = item.path.clone();

                if media_type == "video" {
                    drop(gallery);
                    if let Err(e) = video_clone.borrow_mut().open(&path) {
                        tracing::error!("Failed to open video: {}", e);
                        return;
                    }
                    window.set_current_screen(Screen::Video);
                } else {
                    let month_items = gallery.items_in_month(year, month);
                    drop(gallery);
                    viewer_clone.borrow_mut().open(item_id, month_items);
                    window.set_viewer_loading(true);
                    window.set_current_screen(Screen::Viewer);
                    load_image_async(&path, &window_weak);
                }
            } else {
                tracing::warn!("on_item_tapped: item_id={} not found in gallery", item_id);
            }
        }
    });

    window.on_jump_to_month({
        let gallery_clone = gallery_ctrl.clone();
        let window_weak = window_weak.clone();
        move |year, month| {
            if let Some(row_idx) = gallery_clone.borrow().row_index_for_month(year, month) {
                if let Some(w) = window_weak.upgrade() {
                    w.set_gallery_scroll_to_row(row_idx as i32);
                }
            }
        }
    });

    window.on_reminisce_tapped({
        let gallery_clone = gallery_ctrl.clone();
        let window_weak = window_weak.clone();
        move || {
            let gallery = gallery_clone.borrow();
            if let Some(entry) = gallery.random_month() {
                tracing::info!("Reminisce: jumping to {} {}", entry.month, entry.year);
                let row_idx = entry.row_index as i32;
                drop(gallery);
                if let Some(w) = window_weak.upgrade() {
                    w.set_gallery_scroll_to_row(row_idx);
                }
            }
        }
    });

    // --- Viewer callbacks ---
    window.on_viewer_close({
        let window_weak = window_weak.clone();
        move || {
            if let Some(w) = window_weak.upgrade() {
                w.set_current_screen(Screen::Gallery);
            }
        }
    });

    window.on_viewer_swipe_left({
        let viewer_clone = viewer_ctrl.clone();
        let window_weak = window_weak.clone();
        move || {
            let path = viewer_clone.borrow_mut().go_next().map(|i| i.path.clone());
            if let Some(path) = path {
                if let Some(w) = window_weak.upgrade() {
                    w.set_viewer_loading(true);
                }
                load_image_async(&path, &window_weak);
            }
        }
    });

    window.on_viewer_swipe_right({
        let viewer_clone = viewer_ctrl.clone();
        let window_weak = window_weak.clone();
        move || {
            let path = viewer_clone.borrow_mut().go_prev().map(|i| i.path.clone());
            if let Some(path) = path {
                if let Some(w) = window_weak.upgrade() {
                    w.set_viewer_loading(true);
                }
                load_image_async(&path, &window_weak);
            }
        }
    });

    // --- Video callbacks ---
    window.on_video_close({
        let video_clone = video_ctrl.clone();
        let window_weak = window_weak.clone();
        move || {
            video_clone.borrow_mut().stop();
            if let Some(w) = window_weak.upgrade() {
                w.set_current_screen(Screen::Gallery);
            }
        }
    });

    window.on_video_play_pause({
        let video_clone = video_ctrl.clone();
        move || {
            video_clone.borrow().toggle_pause();
        }
    });

    window.on_video_seek({
        let video_clone = video_ctrl.clone();
        move |pos| {
            video_clone.borrow().seek(pos as f64);
        }
    });

    window.on_video_volume_changed({
        let video_clone = video_ctrl.clone();
        move |vol| {
            video_clone.borrow().set_volume(vol as f64);
        }
    });

    // Video polling: position, playing state, and mpv exit detection
    let video_timer = Timer::default();
    {
        let video_clone = video_ctrl.clone();
        let window_weak2 = window.as_weak();
        video_timer.start(
            TimerMode::Repeated,
            std::time::Duration::from_millis(250),
            move || {
                if let Some(w) = window_weak2.upgrade() {
                    if w.get_current_screen() == Screen::Video {
                        let mut vc = video_clone.borrow_mut();
                        // Detect mpv exit → return to gallery
                        if vc.check_exited() {
                            w.set_current_screen(Screen::Gallery);
                            return;
                        }
                        vc.poll_state();
                        w.set_video_position(vc.get_position() as f32);
                        w.set_video_duration(vc.get_duration() as f32);
                        w.set_video_playing(vc.is_playing());
                    }
                }
            },
        );
    }

    // Thumbnail polling (50ms): determine visible rows, request load/generation as needed.
    // gen_queued deduplicates generation requests; loaded_items tracks what's live in VecModel.
    let thumb_timer = Timer::default();
    {
        let thumb_loader = thumb_loader.clone();
        let gallery_clone = gallery_ctrl.clone();
        let window_weak3 = window.as_weak();
        let mut gen_queued: HashSet<i64> = HashSet::new();
        let mut loaded_items: HashSet<i64> = HashSet::new();
        thumb_timer.start(
            TimerMode::Repeated,
            std::time::Duration::from_millis(50),
            move || {
                // Deliver newly generated thumbnails → enqueue disk load.
                while let Ok((id, path)) = gen_rx.try_recv() {
                    let img = thumb_loader.borrow_mut().request(id, &path);
                    if let Some(img) = img {
                        gallery_clone.borrow().update_thumbnail(id, img);
                        loaded_items.insert(id);
                    }
                    // else: load job enqueued; arrives via poll_results below
                }

                // Deliver completed disk loads to the gallery model.
                let results = thumb_loader.borrow_mut().poll_results();
                if !results.is_empty() {
                    let gallery = gallery_clone.borrow();
                    for (item_id, img) in results {
                        gallery.update_thumbnail(item_id, img);
                        loaded_items.insert(item_id);
                    }
                }

                // Determine visible rows and request load/generation for each item.
                let Some(w) = window_weak3.upgrade() else { return };
                let scroll_y = w.get_gallery_scroll_offset();
                let viewport_h = w.get_gallery_viewport_height();
                if viewport_h <= 0.0 {
                    return;
                }

                // Update row_tops when list-view width changes (e.g. window resize).
                let lv_width = w.get_gallery_list_view_width();
                if lv_width > 0.0 {
                    gallery_clone.borrow_mut().ensure_row_tops(lv_width, thumb_size);
                }

                let gallery = gallery_clone.borrow();
                let visible = gallery.rows_in_view(scroll_y, viewport_h, SCROLL_BUFFER_ROWS);

                // Evict thumbnails from VecModel when too many are loaded.
                if loaded_items.len() > MAX_LOADED_ITEMS {
                    let visible_ids: HashSet<i64> = visible.iter().map(|t| t.item_id).collect();
                    let to_evict: Vec<i64> = loaded_items
                        .iter()
                        .filter(|&&id| !visible_ids.contains(&id))
                        .copied()
                        .collect();
                    for id in to_evict {
                        loaded_items.remove(&id);
                        gen_queued.remove(&id);
                        gallery.clear_thumbnail(id);
                    }
                }

                for thumb in visible {
                    if thumb.thumb_ready {
                        if let Some(path) = &thumb.thumb_path {
                            if let Some(img) = thumb_loader.borrow_mut().request(thumb.item_id, path) {
                                gallery.update_thumbnail(thumb.item_id, img);
                                loaded_items.insert(thumb.item_id);
                            }
                        }
                    } else if !gen_queued.contains(&thumb.item_id) {
                        let _ = gen_tx.try_send(GenJob {
                            item_id: thumb.item_id,
                            path: thumb.path.clone(),
                            mtime: thumb.mtime,
                        });
                        gen_queued.insert(thumb.item_id);
                    }
                }
            },
        );
    }

    window.run()?;
    Ok(())
}

/// Load an image from disk in a background thread and deliver it to the viewer via the event loop.
/// Raw pixels are sent (Vec<u8> is Send); the slint::Image is created on the main thread.
fn load_image_async(path: &str, window_weak: &slint::Weak<AppWindow>) {
    let path = path.to_owned();
    let window_weak = window_weak.clone();
    std::thread::spawn(move || {
        match load_image_raw(&path) {
            Ok((pixels, w, h)) => {
                slint::invoke_from_event_loop(move || {
                    let buffer =
                        SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&pixels, w, h);
                    let img = slint::Image::from_rgba8(buffer);
                    if let Some(win) = window_weak.upgrade() {
                        win.set_viewer_image(img);
                        win.set_viewer_loading(false);
                    }
                })
                .ok();
            }
            Err(e) => tracing::error!("Failed to load image {}: {}", path, e),
        }
    });
}

fn load_image_raw(path: &str) -> anyhow::Result<(Vec<u8>, u32, u32)> {
    let img = image::open(path)?.to_rgba8();
    let (w, h) = img.dimensions();
    Ok((img.into_raw(), w, h))
}
