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
    // Only pre-load gallery from DB when not scanning; if scanning we wait for Complete.
    if !config.performance.scan_on_startup {
        gallery_ctrl.reload(&db.borrow(), thumb_size)?;
    }

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

    if config.performance.scan_on_startup {
        window.set_current_screen(Screen::Scanning);
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
                        ScanEvent::Progress { scanned, .. } => {
                            tracing::debug!("Scan: {} files", scanned);
                            if let Some(w) = window_weak.upgrade() {
                                w.set_scan_file_count(scanned as i32);
                            }
                        }
                        ScanEvent::Complete { total } => {
                            tracing::info!("Scan complete ({} files), loading gallery", total);
                            if let Some(w) = window_weak.upgrade() {
                                let mut gallery = gallery_clone.borrow_mut();
                                if let Err(e) = gallery.reload(&db_clone.borrow(), thumb_size) {
                                    tracing::error!("Gallery reload: {}", e);
                                    return;
                                }
                                w.set_gallery_rows(gallery.row_model_rc());
                                w.set_month_entries(gallery.build_month_model());
                                w.set_current_screen(Screen::Gallery);
                            }
                        }
                        ScanEvent::Error(e) => tracing::error!("Scan error: {}", e),
                    }
                }
            },
        );
    }

    // --- Gallery callbacks ---
    let window_weak = window.as_weak();

    window.on_touch_tapped_at({
        let gallery_clone = gallery_ctrl.clone();
        let viewer_clone = viewer_ctrl.clone();
        let video_clone = video_ctrl.clone();
        let window_weak = window_weak.clone();
        move |x_px, abs_y_px| {
            tracing::info!("on_touch_tapped_at: x={} abs_y={}", x_px, abs_y_px);
            let Some(window) = window_weak.upgrade() else { return };
            let gallery = gallery_clone.borrow();
            let Some((item_id, media_type)) = gallery.item_at_position(x_px, abs_y_px) else {
                tracing::debug!("on_touch_tapped_at: no item at ({}, {})", x_px, abs_y_px);
                return;
            };
            let Some(item) = gallery.item_by_id(item_id) else {
                tracing::warn!("on_touch_tapped_at: item_id={} not found", item_id);
                return;
            };
            tracing::info!("on_touch_tapped_at: item_id={} media_type={} path={}", item_id, media_type, item.path);
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
        let mut vid_last_pos: f32 = -1.0;
        let mut vid_last_dur: f32 = -1.0;
        let mut vid_last_playing: bool = false;
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
                        let pos = vc.get_position() as f32;
                        let dur = vc.get_duration() as f32;
                        let playing = vc.is_playing();
                        if (pos - vid_last_pos).abs() > 0.001 {
                            tracing::trace!("[video_timer] pos {:.2} -> {:.2}", vid_last_pos, pos);
                            vid_last_pos = pos;
                        }
                        if (dur - vid_last_dur).abs() > 0.001 {
                            tracing::trace!("[video_timer] dur {:.2} -> {:.2}", vid_last_dur, dur);
                            vid_last_dur = dur;
                        }
                        if playing != vid_last_playing {
                            tracing::trace!("[video_timer] playing {} -> {}", vid_last_playing, playing);
                            vid_last_playing = playing;
                        }
                        w.set_video_position(pos);
                        w.set_video_duration(dur);
                        w.set_video_playing(playing);
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
        let mut stats_tick: u32 = 0;
        let mut stats_thumb_updates: u32 = 0;
        let mut stats_row_clears: u32 = 0;
        thumb_timer.start(
            TimerMode::Repeated,
            std::time::Duration::from_millis(1000),
            move || {
                // Deliver newly generated thumbnails → enqueue disk load.
                while let Ok((id, path)) = gen_rx.try_recv() {
                    let img = thumb_loader.borrow_mut().request(id, &path);
                    if let Some(img) = img {
                        gallery_clone.borrow().update_thumbnail(id, img);
                        loaded_items.insert(id);
                        stats_thumb_updates += 1;
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
                        stats_thumb_updates += 1;
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
                        stats_row_clears += 1;
                    }
                }

                for thumb in visible {
                    if loaded_items.contains(&thumb.item_id) {
                        continue; // already in VecModel, nothing to do
                    }
                    if thumb.thumb_ready {
                        if let Some(path) = &thumb.thumb_path {
                            if let Some(img) = thumb_loader.borrow_mut().request(thumb.item_id, path) {
                                gallery.update_thumbnail(thumb.item_id, img);
                                loaded_items.insert(thumb.item_id);
                                stats_thumb_updates += 1;
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

                // Log per-second summary every 20 ticks (~1s at 50ms).
                stats_tick += 1;
                if stats_tick >= 20 {
                    tracing::debug!(
                        "[thumb_timer/s] thumb_updates={} row_clears={} loaded={}",
                        stats_thumb_updates, stats_row_clears, loaded_items.len()
                    );
                    stats_tick = 0;
                    stats_thumb_updates = 0;
                    stats_row_clears = 0;
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
