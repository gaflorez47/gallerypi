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
use fast_image_resize::{images::Image as FirImage, PixelType, Resizer};
use slint::{ComponentHandle, SharedPixelBuffer, Timer, TimerMode};
use std::cell::RefCell;
use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;

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
    let preload_count = config.performance.viewer_preload_count;
    let viewer_max_w = config.performance.viewer_max_width;
    let viewer_max_h = config.performance.viewer_max_height;

    // Row indices reported visible by GalleryRowDelegate.init; drained by thumb_timer.
    let pending_rows: Rc<RefCell<HashSet<usize>>> = Rc::new(RefCell::new(HashSet::new()));

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
                        ScanEvent::Complete { total, removed } => {
                            tracing::info!("Scan complete ({} files, {} removed), loading gallery", total, removed);
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

    window.on_cell_tapped({
        let gallery_clone = gallery_ctrl.clone();
        let viewer_clone = viewer_ctrl.clone();
        let video_clone = video_ctrl.clone();
        let window_weak = window_weak.clone();
        move |item_id: i32| {
            tracing::debug!("on_cell_tapped: item_id={}", item_id);
            let Some(window) = window_weak.upgrade() else { return };
            let gallery = gallery_clone.borrow();
            let Some(item) = gallery.item_by_id(item_id as i64) else {
                tracing::debug!("on_cell_tapped: item_id={} not found", item_id);
                return;
            };
            tracing::debug!("on_cell_tapped: media_type={} path={}", item.media_type, item.path);
            let media_type = item.media_type.clone();
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
                viewer_clone.borrow_mut().open(item_id as i64, month_items);
                window.set_viewer_loading(true);
                window.set_current_screen(Screen::Viewer);
                load_image_async(&path, &window_weak, viewer_max_w, viewer_max_h);
                preload_adjacent(&viewer_clone, preload_count, viewer_max_w, viewer_max_h);
            }
        }
    });

    window.on_jump_to_month({
        let gallery_clone = gallery_ctrl.clone();
        let window_weak = window_weak.clone();
        move |year, month| {
            let Some(w) = window_weak.upgrade() else { return };
            let mut gallery = gallery_clone.borrow_mut();
            let lv_width = w.get_gallery_list_view_width();
            tracing::info!("[jump_to_month] year={} month={} lv_width={:.1}", year, month, lv_width);
            if lv_width > 0.0 {
                gallery.ensure_row_tops(lv_width, thumb_size);
            }
            if let Some(row_idx) = gallery.row_index_for_month(year, month) {
                let scroll_y = gallery.pixel_offset_for_row(row_idx);
                tracing::info!("[jump_to_month] row_idx={} scroll_y={:.1} -> set_gallery_scroll_to_y", row_idx, scroll_y);
                w.set_gallery_scroll_to_y(scroll_y);
            } else {
                tracing::warn!("[jump_to_month] no row found for year={} month={}", year, month);
            }
        }
    });

    window.on_reminisce_tapped({
        let gallery_clone = gallery_ctrl.clone();
        let window_weak = window_weak.clone();
        move || {
            let Some(w) = window_weak.upgrade() else { return };
            let mut gallery = gallery_clone.borrow_mut();
            let lv_width = w.get_gallery_list_view_width();
            if lv_width > 0.0 {
                gallery.ensure_row_tops(lv_width, thumb_size);
            }
            if let Some(entry) = gallery.random_month() {
                let scroll_y = gallery.pixel_offset_for_row(entry.row_index);
                tracing::info!("[reminisce] year={} month={} row_idx={} lv_width={:.1} scroll_y={:.1}",
                    entry.year, entry.month, entry.row_index, lv_width, scroll_y);
                w.set_gallery_scroll_to_y(scroll_y);
            }
        }
    });

    window.on_request_row_thumbnails({
        let pending_rows = pending_rows.clone();
        move |row_idx: i32| {
            pending_rows.borrow_mut().insert(row_idx as usize);
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
                let cached = viewer_clone.borrow_mut().take_from_cache(&path);
                if let Some((pixels, w, h)) = cached {
                    if let Some(win) = window_weak.upgrade() {
                        let buf = SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&pixels, w, h);
                        win.set_viewer_image(slint::Image::from_rgba8(buf));
                        win.set_viewer_loading(false);
                    }
                } else {
                    if let Some(w) = window_weak.upgrade() {
                        w.set_viewer_loading(true);
                    }
                    load_image_async(&path, &window_weak, viewer_max_w, viewer_max_h);
                }
                preload_adjacent(&viewer_clone, preload_count, viewer_max_w, viewer_max_h);
            }
        }
    });

    window.on_viewer_swipe_right({
        let viewer_clone = viewer_ctrl.clone();
        let window_weak = window_weak.clone();
        move || {
            let path = viewer_clone.borrow_mut().go_prev().map(|i| i.path.clone());
            if let Some(path) = path {
                let cached = viewer_clone.borrow_mut().take_from_cache(&path);
                if let Some((pixels, w, h)) = cached {
                    if let Some(win) = window_weak.upgrade() {
                        let buf = SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&pixels, w, h);
                        win.set_viewer_image(slint::Image::from_rgba8(buf));
                        win.set_viewer_loading(false);
                    }
                } else {
                    if let Some(w) = window_weak.upgrade() {
                        w.set_viewer_loading(true);
                    }
                    load_image_async(&path, &window_weak, viewer_max_w, viewer_max_h);
                }
                preload_adjacent(&viewer_clone, preload_count, viewer_max_w, viewer_max_h);
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
            std::time::Duration::from_millis(33),
            move || {
                if let Some(w) = window_weak2.upgrade() {
                    if w.get_current_screen() == Screen::Video {
                        let mut vc = video_clone.borrow_mut();
                        // Detect decoder exit → return to gallery
                        if vc.check_exited() {
                            w.set_current_screen(Screen::Gallery);
                            return;
                        }
                        if let Some(img) = vc.poll_frame() {
                            w.set_video_frame(img);
                        }
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

    // Thumbnail polling (50ms): deliver completed loads/generations and process newly visible rows.
    // pending_rows is filled by on_request_row_thumbnails (fired from GalleryRowDelegate.init).
    // gen_queued/load_requested deduplicate in-flight work; loaded_items tracks VecModel state.
    let thumb_timer = Timer::default();
    {
        let thumb_loader = thumb_loader.clone();
        let gallery_clone = gallery_ctrl.clone();
        let mut gen_queued: HashSet<i64> = HashSet::new();
        let mut load_requested: HashSet<i64> = HashSet::new();
        let mut loaded_items: HashSet<i64> = HashSet::new();
        let mut visible_item_ids: HashSet<i64> = HashSet::new();
        thumb_timer.start(
            TimerMode::Repeated,
            std::time::Duration::from_millis(50),
            move || {
                // Deliver newly generated thumbnails → enqueue disk load.
                while let Ok((id, path)) = gen_rx.try_recv() {
                    tracing::debug!("[thumb_timer] gen_rx: id={} path={}", id, path);
                    gen_queued.remove(&id);
                    load_requested.remove(&id);
                    let img = thumb_loader.borrow_mut().request(id, &path);
                    if let Some(img) = img {
                        gallery_clone.borrow().update_thumbnail(id, img);
                        loaded_items.insert(id);
                    } else {
                        load_requested.insert(id);
                    }
                }

                // Deliver completed disk loads to the gallery model.
                let results = thumb_loader.borrow_mut().poll_results();
                if !results.is_empty() {
                    let gallery = gallery_clone.borrow();
                    for (item_id, img) in results {
                        gallery.update_thumbnail(item_id, img);
                        load_requested.remove(&item_id);
                        loaded_items.insert(item_id);
                    }
                }

                // Process rows that became visible since last tick.
                let rows_to_load: Vec<usize> = pending_rows.borrow_mut().drain().collect();
                if !rows_to_load.is_empty() {
                    let gallery = gallery_clone.borrow();
                    visible_item_ids.clear();
                    for &row_idx in &rows_to_load {
                        for thumb in gallery.items_in_row(row_idx) {
                            visible_item_ids.insert(thumb.item_id);
                            if loaded_items.contains(&thumb.item_id) {
                                continue;
                            }
                            if thumb.thumb_ready {
                                if let Some(path) = &thumb.thumb_path {
                                    if !load_requested.contains(&thumb.item_id) {
                                        if let Some(img) = thumb_loader.borrow_mut().request(thumb.item_id, path) {
                                            gallery.update_thumbnail(thumb.item_id, img);
                                            loaded_items.insert(thumb.item_id);
                                        } else {
                                            load_requested.insert(thumb.item_id);
                                        }
                                    }
                                }
                            } else if !gen_queued.contains(&thumb.item_id) {
                                gen_tx.push(GenJob {
                                    item_id: thumb.item_id,
                                    path: thumb.path.clone(),
                                    mtime: thumb.mtime,
                                    media_type: thumb.media_type.clone(),
                                });
                                gen_queued.insert(thumb.item_id);
                            }
                        }
                    }

                    // Evict thumbnails from VecModel when too many are loaded.
                    if loaded_items.len() > MAX_LOADED_ITEMS {
                        let to_evict: Vec<i64> = loaded_items
                            .iter()
                            .filter(|&&id| !visible_item_ids.contains(&id))
                            .copied()
                            .collect();
                        for id in to_evict {
                            loaded_items.remove(&id);
                            gen_queued.remove(&id);
                            load_requested.remove(&id);
                            gallery.clear_thumbnail(id);
                        }
                    }
                }
            },
        );
    }

    window.run()?;
    Ok(())
}

/// Spawn background threads to preload adjacent images into the viewer cache.
fn preload_adjacent(viewer_ctrl: &Rc<RefCell<ViewerController>>, count: usize, max_w: u32, max_h: u32) {
    if count == 0 {
        return;
    }
    let viewer = viewer_ctrl.borrow();
    let paths = viewer.preload_paths(count);
    let cache = viewer.preload_cache.clone();
    drop(viewer);
    for path in paths {
        let cache = cache.clone();
        let path_clone = path.clone();
        std::thread::spawn(move || match load_image_raw(&path_clone, max_w, max_h) {
            Ok((pixels, w, h)) => {
                cache.lock().unwrap().insert(path_clone, (pixels, w, h));
            }
            Err(e) => tracing::warn!("Preload failed {}: {}", path_clone, e),
        });
    }
}

/// Load an image from disk in a background thread and deliver it to the viewer via the event loop.
/// Raw pixels are sent (Vec<u8> is Send); the slint::Image is created on the main thread.
fn load_image_async(path: &str, window_weak: &slint::Weak<AppWindow>, max_w: u32, max_h: u32) {
    let path = path.to_owned();
    let window_weak = window_weak.clone();
    std::thread::spawn(move || {
        match load_image_raw(&path, max_w, max_h) {
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

/// Decode an image and downscale it to fit within max_w × max_h if it exceeds those bounds.
/// Images already within the bounds are returned as-is (no quality loss).
fn load_image_raw(path: &str, max_w: u32, max_h: u32) -> anyhow::Result<(Vec<u8>, u32, u32)> {
    let img = image::open(path)?.to_rgba8();
    let (orig_w, orig_h) = img.dimensions();

    let scale = (max_w as f32 / orig_w as f32)
        .min(max_h as f32 / orig_h as f32)
        .min(1.0);

    if scale < 1.0 {
        let new_w = ((orig_w as f32 * scale).round() as u32).max(1);
        let new_h = ((orig_h as f32 * scale).round() as u32).max(1);
        tracing::debug!(
            "Scaling viewer image {}×{} → {}×{} ({:.1} MB → {:.1} MB): {}",
            orig_w, orig_h, new_w, new_h,
            (orig_w * orig_h * 4) as f32 / 1_048_576.0,
            (new_w * new_h * 4) as f32 / 1_048_576.0,
            path
        );
        let src = FirImage::from_vec_u8(orig_w, orig_h, img.into_raw(), PixelType::U8x4)?;
        let mut dst = FirImage::new(new_w, new_h, PixelType::U8x4);
        Resizer::new().resize(&src, &mut dst, None)?;
        return Ok((dst.into_vec(), new_w, new_h));
    }

    tracing::debug!(
        "Loading viewer image {}×{} ({:.1} MB): {}",
        orig_w, orig_h,
        (orig_w * orig_h * 4) as f32 / 1_048_576.0,
        path
    );
    Ok((img.into_raw(), orig_w, orig_h))
}
