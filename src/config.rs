use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

fn default_media_dir() -> PathBuf {
    dirs::picture_dir().unwrap_or_else(|| PathBuf::from("~/Pictures"))
}
fn default_grid_columns() -> u8 { 4 }
fn default_thumbnail_size() -> u32 { 256 }

fn default_thumb_gen_threads() -> usize {
    let cpus = num_cpus::get();
    if cpus <= 4 { 2 } else { 4 }
}
fn default_thumb_cache_entries() -> usize { 150 }
fn default_scan_on_startup() -> bool { true }
fn default_viewer_preload_count() -> usize { 1 }
fn default_viewer_max_width() -> u32 { 1920 }
fn default_viewer_max_height() -> u32 { 1080 }

fn default_fullscreen() -> bool { true }

fn default_volume() -> u8 { 80 }
fn default_loop_videos() -> bool { true }
fn default_hw_accel() -> String { "auto".to_string() }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GalleryConfig {
    #[serde(default = "default_media_dir")]
    pub media_dir: PathBuf,
    #[serde(default = "default_grid_columns")]
    pub grid_columns: u8,
    #[serde(default = "default_thumbnail_size")]
    pub thumbnail_size: u32,
}

impl Default for GalleryConfig {
    fn default() -> Self {
        Self {
            media_dir: default_media_dir(),
            grid_columns: default_grid_columns(),
            thumbnail_size: default_thumbnail_size(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceConfig {
    #[serde(default = "default_thumb_gen_threads")]
    pub thumb_gen_threads: usize,
    #[serde(default = "default_thumb_cache_entries")]
    pub thumb_cache_entries: usize,
    #[serde(default = "default_scan_on_startup")]
    pub scan_on_startup: bool,
    /// Number of images to preload ahead and behind in the viewer (0 = disabled).
    #[serde(default = "default_viewer_preload_count")]
    pub viewer_preload_count: usize,
    /// Maximum width/height to load viewer images at. Images larger than this are
    /// downscaled to fit, saving significant memory on high-res cameras.
    #[serde(default = "default_viewer_max_width")]
    pub viewer_max_width: u32,
    #[serde(default = "default_viewer_max_height")]
    pub viewer_max_height: u32,
}

impl Default for PerformanceConfig {
    fn default() -> Self {
        Self {
            thumb_gen_threads: default_thumb_gen_threads(),
            thumb_cache_entries: default_thumb_cache_entries(),
            scan_on_startup: default_scan_on_startup(),
            viewer_preload_count: default_viewer_preload_count(),
            viewer_max_width: default_viewer_max_width(),
            viewer_max_height: default_viewer_max_height(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiConfig {
    #[serde(default = "default_fullscreen")]
    pub fullscreen: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self { fullscreen: default_fullscreen() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoConfig {
    #[serde(default = "default_volume")]
    pub default_volume: u8,
    #[serde(default = "default_loop_videos")]
    pub loop_videos: bool,
    /// Hardware acceleration mode: "auto" (try h264_v4l2m2m, fall back to SW),
    /// "v4l2m2m" (force, fail if unavailable), or "none" (always SW decode).
    #[serde(default = "default_hw_accel")]
    pub hw_accel: String,
}

impl Default for VideoConfig {
    fn default() -> Self {
        Self {
            default_volume: default_volume(),
            loop_videos: default_loop_videos(),
            hw_accel: default_hw_accel(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub gallery: GalleryConfig,
    #[serde(default)]
    pub performance: PerformanceConfig,
    #[serde(default)]
    pub ui: UiConfig,
    #[serde(default)]
    pub video: VideoConfig,
}

impl Config {
    pub fn load() -> Result<Self> {
        let config_path = config_path();
        if !config_path.exists() {
            tracing::info!("No config file found at {:?}, using defaults", config_path);
            return Ok(Self::default());
        }
        let contents = std::fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read config at {:?}", config_path))?;
        let config: Self = toml::from_str(&contents)
            .with_context(|| format!("Failed to parse config at {:?}", config_path))?;
        tracing::info!("Loaded config from {:?}", config_path);
        Ok(config)
    }

    pub fn save(&self) -> Result<()> {
        let config_path = config_path();
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let contents = toml::to_string_pretty(self)?;
        std::fs::write(&config_path, contents)?;
        Ok(())
    }
}

pub fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("gallerypi")
        .join("config.toml")
}

pub fn cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("~/.cache"))
        .join("gallerypi")
}

pub fn thumb_dir() -> PathBuf {
    cache_dir().join("thumbs")
}

pub fn db_path() -> PathBuf {
    cache_dir().join("metadata.db")
}
