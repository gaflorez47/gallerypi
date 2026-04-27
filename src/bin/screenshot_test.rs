// Headless screenshot tests for GalleryPi using Slint's software renderer.
// Run:   cargo run --bin screenshot_test
// Output: screenshots/report.html  +  screenshots/*.png

slint::include_modules!();

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType, Rgb565Pixel};
use slint::platform::{Platform, WindowAdapter};
use slint::{ModelRc, SharedString, VecModel};
use std::path::Path;
use std::rc::Rc;

// ── Headless Platform ────────────────────────────────────────────────────────

struct TestPlatform {
    window: Rc<MinimalSoftwareWindow>,
}

impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }
}

// ── Mock Data Helpers ─────────────────────────────────────────────────────────

fn gradient_image(width: u32, height: u32, c1: [u8; 3], c2: [u8; 3]) -> slint::Image {
    let mut pixels: Vec<u8> = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        let t = y as f32 / height as f32;
        let r = (c1[0] as f32 * (1.0 - t) + c2[0] as f32 * t) as u8;
        let g = (c1[1] as f32 * (1.0 - t) + c2[1] as f32 * t) as u8;
        let b = (c1[2] as f32 * (1.0 - t) + c2[2] as f32 * t) as u8;
        for _ in 0..width {
            pixels.extend_from_slice(&[r, g, b, 255]);
        }
    }
    let buf =
        slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&pixels, width, height);
    slint::Image::from_rgba8(buf)
}

fn make_thumb(id: i32, media_type: &str, c1: [u8; 3], c2: [u8; 3]) -> ThumbnailData {
    ThumbnailData {
        item_id: id,
        thumb_image: gradient_image(200, 200, c1, c2),
        thumb_ready: true,
        media_type: media_type.into(),
    }
}

fn make_image_row(items: Vec<ThumbnailData>) -> GalleryRowData {
    GalleryRowData {
        is_header: false,
        header_label: SharedString::default(),
        item_count: items.len() as i32,
        items: ModelRc::new(VecModel::from(items)),
    }
}

fn make_header(label: &str) -> GalleryRowData {
    GalleryRowData {
        is_header: true,
        header_label: label.into(),
        item_count: 0,
        items: ModelRc::new(VecModel::from(vec![])),
    }
}

fn make_month(label: &str, year: i32, month: i32, row_index: i32) -> MonthEntry {
    MonthEntry { label: label.into(), year, month, row_index }
}

// ── Gallery scenario helpers ──────────────────────────────────────────────────

fn set_gallery(ui: &AppWindow, rows: Vec<GalleryRowData>, months: Vec<MonthEntry>) {
    ui.set_current_screen(Screen::Gallery);
    ui.set_gallery_rows(ModelRc::new(VecModel::from(rows)));
    ui.set_month_entries(ModelRc::new(VecModel::from(months)));
}

// ── Test Scenarios ────────────────────────────────────────────────────────────

fn scenario_empty(ui: &AppWindow) {
    set_gallery(ui, vec![], vec![]);
}

fn scenario_single_month_partial(ui: &AppWindow) {
    let rows = vec![
        make_header("April 2025"),
        make_image_row(vec![
            make_thumb(1, "image", [220, 80, 60], [60, 80, 220]),
            make_thumb(2, "image", [60, 200, 90], [200, 90, 60]),
            make_thumb(3, "image", [80, 60, 220], [220, 200, 60]),
        ]),
    ];
    let months = vec![make_month("Apr 2025", 2025, 4, 0)];
    set_gallery(ui, rows, months);
}

fn scenario_single_month_full(ui: &AppWindow) {
    // 12 items → 3 full rows of 4
    let palette: [[u8; 3]; 12] = [
        [230, 80, 80],  [80, 230, 80],  [80, 80, 230],  [230, 230, 80],
        [230, 80, 230], [80, 230, 230], [180, 120, 60],  [60, 180, 120],
        [120, 60, 180], [180, 180, 60], [60, 180, 180],  [180, 60, 180],
    ];
    let mut rows = vec![make_header("March 2025")];
    for (chunk_idx, chunk) in palette.chunks(4).enumerate() {
        let items = chunk
            .iter()
            .enumerate()
            .map(|(i, &c)| make_thumb((chunk_idx * 4 + i + 1) as i32, "image", c, [c[2], c[0], c[1]]))
            .collect();
        rows.push(make_image_row(items));
    }
    let months = vec![make_month("Mar 2025", 2025, 3, 0)];
    set_gallery(ui, rows, months);
}

fn scenario_multi_month(ui: &AppWindow) {
    let specs: &[(&str, &str, i32, i32, usize, [u8; 3])] = &[
        ("April 2025", "Apr 2025", 2025, 4, 6,  [200, 120, 50]),
        ("March 2025", "Mar 2025", 2025, 3, 4,  [50,  150, 200]),
        ("February 2025", "Feb 2025", 2025, 2, 8, [150, 50, 200]),
    ];
    let mut rows: Vec<GalleryRowData> = vec![];
    let mut months: Vec<MonthEntry> = vec![];
    let mut id = 1i32;

    for (label, short, year, month, count, base_color) in specs {
        let row_idx = rows.len() as i32;
        rows.push(make_header(label));
        let items: Vec<ThumbnailData> = (0..*count)
            .map(|i| {
                let t = i as f32 / *count as f32;
                let c1 = [
                    (base_color[0] as f32 * (1.0 - t) + 60.0 * t) as u8,
                    (base_color[1] as f32 * (1.0 - t) + 60.0 * t) as u8,
                    (base_color[2] as f32 * (1.0 - t) + 60.0 * t) as u8,
                ];
                let thumb = make_thumb(id, "image", c1, [255 - c1[0], 255 - c1[1], 255 - c1[2]]);
                id += 1;
                thumb
            })
            .collect();
        for chunk in items.chunks(4) {
            rows.push(make_image_row(chunk.to_vec()));
        }
        months.push(make_month(short, *year, *month, row_idx));
    }
    set_gallery(ui, rows, months);
}

fn scenario_mixed_media(ui: &AppWindow) {
    let rows = vec![
        make_header("May 2025"),
        make_image_row(vec![
            make_thumb(1, "image", [200, 150, 50],  [50, 150, 200]),
            make_thumb(2, "video", [100, 200, 100], [200, 100, 100]),
            make_thumb(3, "image", [50, 100, 200],  [200, 200, 50]),
            make_thumb(4, "video", [200, 50, 150],  [50, 200, 150]),
        ]),
        make_image_row(vec![
            make_thumb(5, "video", [150, 200, 50],  [50, 100, 200]),
            make_thumb(6, "image", [200, 100, 200], [100, 200, 100]),
            make_thumb(7, "image", [50, 200, 100],  [200, 50, 100]),
        ]),
        make_header("April 2025"),
        make_image_row(vec![
            make_thumb(8,  "video", [80, 180, 220],  [220, 80, 100]),
            make_thumb(9,  "image", [220, 80, 180],  [80, 220, 100]),
            make_thumb(10, "video", [180, 220, 80],  [100, 80, 220]),
            make_thumb(11, "image", [120, 80, 200],  [200, 200, 80]),
        ]),
    ];
    let months = vec![
        make_month("May 2025", 2025, 5, 0),
        make_month("Apr 2025", 2025, 4, 3),
    ];
    set_gallery(ui, rows, months);
}

fn scenario_large_gallery(ui: &AppWindow) {
    let month_specs: &[(&str, &str, i32, i32)] = &[
        ("October 2024",  "Oct 2024", 2024, 10),
        ("September 2024","Sep 2024", 2024,  9),
        ("August 2024",   "Aug 2024", 2024,  8),
        ("July 2024",     "Jul 2024", 2024,  7),
        ("June 2024",     "Jun 2024", 2024,  6),
        ("May 2024",      "May 2024", 2024,  5),
        ("April 2024",    "Apr 2024", 2024,  4),
        ("March 2024",    "Mar 2024", 2024,  3),
        ("February 2024", "Feb 2024", 2024,  2),
        ("January 2024",  "Jan 2024", 2024,  1),
    ];
    let mut rows: Vec<GalleryRowData> = vec![];
    let mut months: Vec<MonthEntry> = vec![];
    let mut id = 1i32;

    for (month_idx, (label, short, year, month)) in month_specs.iter().enumerate() {
        let row_idx = rows.len() as i32;
        rows.push(make_header(label));
        let base = (month_idx as u8).wrapping_mul(25);
        let items: Vec<ThumbnailData> = (0..8)
            .map(|i| {
                let hue = base.wrapping_add((i as u8) * 30);
                let thumb = make_thumb(
                    id,
                    "image",
                    [hue, 255 - hue, base.wrapping_add(128)],
                    [255 - hue, hue, base],
                );
                id += 1;
                thumb
            })
            .collect();
        for chunk in items.chunks(4) {
            rows.push(make_image_row(chunk.to_vec()));
        }
        months.push(make_month(short, *year, *month, row_idx));
    }
    set_gallery(ui, rows, months);
}

// ── Viewer Scenarios ──────────────────────────────────────────────────────────

fn scenario_viewer_image(ui: &AppWindow) {
    ui.set_current_screen(Screen::Viewer);
    ui.set_viewer_image(gradient_image(WIDTH, HEIGHT, [30, 80, 180], [180, 60, 30]));
    ui.set_viewer_loading(false);
}

fn scenario_viewer_loading(ui: &AppWindow) {
    ui.set_current_screen(Screen::Viewer);
    ui.set_viewer_image(gradient_image(WIDTH, HEIGHT, [20, 60, 140], [140, 40, 20]));
    ui.set_viewer_loading(true);
}

// ── Rendering ─────────────────────────────────────────────────────────────────

const WIDTH: u32 = 1280;
const HEIGHT: u32 = 800;

fn render_screen(
    ui: &AppWindow,
    window: &Rc<MinimalSoftwareWindow>,
    setup: fn(&AppWindow),
) -> Vec<u8> {
    setup(ui);
    ui.window().request_redraw();

    let mut pixel_buf = vec![Rgb565Pixel(0); (WIDTH * HEIGHT) as usize];
    // Multiple draw passes so any pending property reactions settle
    for _ in 0..3 {
        window.draw_if_needed(|renderer| {
            renderer.render(&mut pixel_buf, WIDTH as usize);
        });
    }
    // Convert RGB565 → RGB8
    pixel_buf
        .iter()
        .flat_map(|p| {
            let v = p.0;
            let r = ((v >> 11) & 0x1F) as u8;
            let g = ((v >> 5) & 0x3F) as u8;
            let b = (v & 0x1F) as u8;
            // Scale to 8-bit
            [
                (r << 3) | (r >> 2),
                (g << 2) | (g >> 4),
                (b << 3) | (b >> 2),
            ]
        })
        .collect()
}

fn pixels_to_png_bytes(pixels: &[u8]) -> Vec<u8> {
    use std::io::Cursor;
    let img = image::RgbImage::from_raw(WIDTH, HEIGHT, pixels.to_vec())
        .expect("pixel buffer size mismatch");
    let mut buf = Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png).expect("PNG encode failed");
    buf.into_inner()
}

// ── Base64 encoder (no external dep) ─────────────────────────────────────────

fn to_base64(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as usize;
        let b1 = if chunk.len() > 1 { chunk[1] as usize } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as usize } else { 0 };
        out.push(CHARS[b0 >> 2] as char);
        out.push(CHARS[((b0 & 3) << 4) | (b1 >> 4)] as char);
        out.push(if chunk.len() > 1 { CHARS[((b1 & 15) << 2) | (b2 >> 6)] as char } else { '=' });
        out.push(if chunk.len() > 2 { CHARS[b2 & 63] as char } else { '=' });
    }
    out
}

// ── HTML Report ───────────────────────────────────────────────────────────────

fn write_report(out_dir: &Path, entries: &[(&str, Vec<u8>)]) {
    let mut cards = String::new();
    for (name, png_bytes) in entries {
        let b64 = to_base64(png_bytes);
        cards.push_str(&format!(
            "    <div class=\"card\">\n      <h3>{name}</h3>\n      <img src=\"data:image/png;base64,{b64}\" alt=\"{name}\">\n    </div>\n"
        ));
    }

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_default();

    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <title>GalleryPi Screenshot Tests</title>
  <style>
    * {{ box-sizing: border-box; margin: 0; padding: 0; }}
    body {{ background: #0d0d0d; color: #eee; font-family: system-ui, sans-serif; padding: 24px; }}
    header {{ margin-bottom: 28px; }}
    header h1 {{ font-size: 20px; font-weight: 600; color: #fff; }}
    header p {{ font-size: 12px; color: #555; margin-top: 4px; }}
    .grid {{ display: flex; flex-direction: column; gap: 28px; }}
    .card {{ background: #181818; border-radius: 10px; padding: 16px; border: 1px solid #2a2a2a; }}
    .card h3 {{ font-size: 11px; color: #666; margin-bottom: 10px; font-weight: 500;
                letter-spacing: 0.08em; text-transform: uppercase; }}
    .card img {{ display: block; width: 100%; max-width: 1280px; border-radius: 6px;
                 border: 1px solid #2a2a2a; }}
  </style>
</head>
<body>
  <header>
    <h1>GalleryPi Screenshot Tests</h1>
    <p>Generated at unix:{timestamp} &nbsp;|&nbsp; {count} scenarios &nbsp;|&nbsp; {width}x{height}px</p>
  </header>
  <div class="grid">
{cards}  </div>
</body>
</html>
"#,
        count = entries.len(),
        width = WIDTH,
        height = HEIGHT,
    );

    let report_path = out_dir.join("report.html");
    std::fs::write(&report_path, html).expect("failed to write report.html");
    println!("  => {}", report_path.display());
}

// ── Main ──────────────────────────────────────────────────────────────────────

fn main() {
    let out_dir = Path::new("screenshots");
    std::fs::create_dir_all(out_dir).expect("failed to create screenshots/");

    // Set up headless platform — must happen before any AppWindow::new()
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    window.set_size(slint::PhysicalSize::new(WIDTH, HEIGHT));
    slint::platform::set_platform(Box::new(TestPlatform { window: window.clone() }))
        .expect("failed to set platform");

    let ui = AppWindow::new().expect("failed to create AppWindow");
    ui.set_grid_columns(4);
    ui.set_thumb_size(200.0);
    ui.show().expect("failed to show window");

    let scenarios: &[(&str, fn(&AppWindow))] = &[
        ("01_empty_gallery",        scenario_empty),
        ("02_single_month_partial", scenario_single_month_partial),
        ("03_single_month_full",    scenario_single_month_full),
        ("04_multi_month",          scenario_multi_month),
        ("05_mixed_media",          scenario_mixed_media),
        ("06_large_gallery",        scenario_large_gallery),
        ("07_viewer_image",         scenario_viewer_image),
        ("08_viewer_loading",       scenario_viewer_loading),
    ];

    let mut report_entries: Vec<(&str, Vec<u8>)> = vec![];

    for (name, setup) in scenarios {
        print!("[{name}] rendering ... ");
        let pixels = render_screen(&ui, &window, *setup);
        let png_bytes = pixels_to_png_bytes(&pixels);

        // Also save individual PNG
        let png_path = out_dir.join(format!("{name}.png"));
        std::fs::write(&png_path, &png_bytes).expect("failed to write PNG");
        println!("ok ({} KB)", png_bytes.len() / 1024);

        report_entries.push((name, png_bytes));
    }

    println!("\nWriting report ...");
    write_report(out_dir, &report_entries);
    println!("Done. Open screenshots/report.html in a browser.");
}
