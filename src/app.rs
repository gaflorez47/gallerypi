use crate::config::Config;
use crate::db::Database;
use crate::gallery::GalleryController;
use crate::scanner::{ScanEvent, Scanner};
use crate::thumbnail::generator;
use crate::thumbnail::ThumbnailLoader;
use crate::ui::{AppWindow, Screen};
use crate::video::VideoController;
use crate::viewer::ViewerController;
use anyhow::Result;
use crossbeam_channel::{bounded, Receiver};
use slint::{ComponentHandle, SharedPixelBuffer, Timer, TimerMode};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

pub fn run(config: Config, db_path: PathBuf) -> Result<()> {
    let db = Rc::new(RefCell::new(Database::open(&db_path)?));

    let n_cols = config.gallery.grid_columns as usize;
    let mut gallery_ctrl = GalleryController::new(n_cols);
    gallery_ctrl.reload(&db.borrow())?;

    // Create thumb_loader early so we can pre-request thumbnails
    let mut initial_loader = ThumbnailLoader::new(config.performance.thumb_cache_entries);
    gallery_ctrl.request_ready_thumbnails(&mut initial_loader, 200);

    let window = AppWindow::new()?;
    window.set_grid_columns(n_cols as i32);
    window.set_thumb_size(config.gallery.thumbnail_size as f32);
    window.set_gallery_rows(gallery_ctrl.row_model_rc());
    window.set_month_entries(gallery_ctrl.build_month_model());

    if config.ui.fullscreen {
        window.window().set_fullscreen(true);
    }

    let gallery_ctrl = Rc::new(RefCell::new(gallery_ctrl));
    let viewer_ctrl = Rc::new(RefCell::new(ViewerController::new()));
    let video_ctrl = Rc::new(RefCell::new(VideoController::new(config.video.clone())));
    let thumb_loader = Rc::new(RefCell::new(initial_loader));

    // --- Scanner ---
    let scan_rx: Option<Receiver<ScanEvent>> = if config.performance.scan_on_startup {
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

        // Start thumbnail generation after scan completes (with delay)
        let config_clone = config.clone();
        let db_path_gen = db_path.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(5));
            crate::util::paths::ensure_thumb_dir().ok();
            generator::run_generation(&config_clone, &db_path_gen);
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
        let thumb_loader_clone = thumb_loader.clone();
        let window_weak = window.as_weak();
        scan_timer.start(
            TimerMode::Repeated,
            std::time::Duration::from_millis(200),
            move || {
                while let Ok(event) = rx.try_recv() {
                    match event {
                        ScanEvent::Complete { total } => {
                            tracing::info!("Scan complete ({} files), reloading", total);
                            if let Some(w) = window_weak.upgrade() {
                                let mut gallery = gallery_clone.borrow_mut();
                                if let Err(e) = gallery.reload(&db_clone.borrow()) {
                                    tracing::error!("Gallery reload: {}", e);
                                    return;
                                }
                                gallery.request_ready_thumbnails(
                                    &mut thumb_loader_clone.borrow_mut(),
                                    200,
                                );
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
            let Some(window) = window_weak.upgrade() else { return };
            let gallery = gallery_clone.borrow();
            let item_id = item_id as i64;

            if let Some(item) = gallery.item_by_id(item_id) {
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

    // Thumbnail result polling (50ms) — all on main thread, no Send needed
    let thumb_timer = Timer::default();
    {
        let thumb_loader = thumb_loader.clone();
        let gallery_clone = gallery_ctrl.clone();
        thumb_timer.start(
            TimerMode::Repeated,
            std::time::Duration::from_millis(50),
            move || {
                let results = thumb_loader.borrow_mut().poll_results();
                if !results.is_empty() {
                    let gallery = gallery_clone.borrow();
                    for (item_id, img) in results {
                        gallery.update_thumbnail(item_id, img);
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
