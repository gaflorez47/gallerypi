# GalleryPi

Offline media gallery app for Raspberry Pi 4 (1GB RAM) and Linux desktop. Displays tens of thousands of images/videos organized by month, with thumbnail caching, image viewer, and video playback.

## Stack

- **UI**: Slint 1.9 with winit/Wayland backend + Skia renderer (NOT bare-metal DRM/KMS)
- **DB**: SQLite via rusqlite (bundled feature)
- **Thumbnails**: rayon + fast_image_resize v5 (SIMD/ARM NEON)
- **Video**: mpv subprocess with Unix IPC JSON socket (NOT libmpv2 — v5.x only has OpenGL render API, no SW buffer)
- **Config**: TOML via serde

## Building

```bash
# Dev build
cargo build

# Cross-compile for RPi (aarch64)
sudo apt install gcc-aarch64-linux-gnu
cargo build --target aarch64-unknown-linux-gnu --release

# Run (needs a display)
RUST_LOG=gallerypi=info ./target/debug/gallerypi
```

## Config

```bash
mkdir -p ~/.config/gallerypi
cp config.toml.example ~/.config/gallerypi/config.toml
# Edit media_dir to point to your pictures folder
```

## Project Structure

```
src/
  main.rs         # entry point: config → DB → app::run()
  app.rs          # Slint callback wiring, timers, screen routing
  config.rs       # TOML config, XDG paths
  db/             # schema, queries, MediaItem struct
  scanner/        # WalkDir + EXIF extraction + SQLite indexing
  thumbnail/
    generator.rs  # rayon parallel JPEG generation
    loader.rs     # channel-based LRU loader (Send-safe)
  gallery/        # GalleryController, VecModel, row building
  viewer/         # ViewerController, swipe navigation
  video/          # VideoController: mpv subprocess + Unix IPC
  util/           # hash, paths, time helpers
ui/
  app.slint       # AppWindow, Screen enum, all properties
  types.slint     # shared structs (avoids circular imports)
  gallery.slint   # GalleryScreen, ListView, month scroller
  viewer.slint    # ViewerScreen, touch gestures
  video_player.slint
  month_scroller.slint
  thumbnail_cell.slint
deploy/
  gallerypi-kiosk.service  # systemd unit using cage
  README.md
```

## Key Design Decisions

**Video**: Uses mpv as a subprocess launched with `--fullscreen --input-ipc-server=<socket>`. Controls (play/pause, seek, volume) go over JSON IPC. This avoids libmpv2's OpenGL-only render API which is incompatible with Slint's Skia renderer without complex FBO bridging.

**Thumbnail thread safety**: `slint::Image` is not `Send`. Worker threads load JPEG bytes into `Vec<u8>` and send over a crossbeam channel. The main thread converts to `slint::Image` in a 50ms Slint Timer (`poll_results()`).

**Gallery model updates**: `GalleryController` keeps a `HashMap<item_id, (row_idx, col_idx)>` for O(1) thumbnail cell lookup when updating the `VecModel`.

**Scroll-to-month**: Uses `changed scroll-to-row => { list-view.viewport-y = ...; }` in gallery.slint. Cannot use `states` because `viewport-y` has a two-way binding in Slint's `ListView`.

**Slint quirks**:
- `PointerEvent` has no `.position` field — use `self.mouse-x` / `self.mouse-y` inside `TouchArea`
- `e.delta-y` is `length` type — divide by `1px` to get a dimensionless float: `(e.delta-y / 1px) * 0.002`
- Length properties map to plain `f32` in Rust (no `LogicalLength` wrapper in v1.9)
- Must `use slint::Model` to call `row_count`/`row_data`/`set_row_data` on `Rc<VecModel<>>`

## RPi Deployment

```bash
# Copy binary
scp target/aarch64-unknown-linux-gnu/release/gallerypi pi@raspberrypi:~/gallerypi/

# Kiosk mode (no desktop needed)
cage -- ./gallerypi

# Or as a systemd service
sudo cp deploy/gallerypi-kiosk.service /etc/systemd/system/
sudo systemctl enable --now gallerypi-kiosk
```

`/boot/config.txt`: set `gpu_mem=128` for Skia allocations.

## System Dependencies

```bash
sudo apt install libmpv-dev  # required for build (mpv IPC uses system mpv binary at runtime)
sudo apt install cage         # for kiosk mode
sudo apt install libxkbcommon-x11-0
```

# WSL
`export DISPLAY=localhost:0`