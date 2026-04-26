use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GalleryConfig {
    pub media_dir: PathBuf,
    pub grid_columns: u8,
    pub thumbnail_size: u32,
}

impl Default for GalleryConfig {
    fn default() -> Self {
        Self {
            media_dir: dirs::picture_dir().unwrap_or_else(|| PathBuf::from("~/Pictures")),
            grid_columns: 4,
            thumbnail_size: 256,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceConfig {
    pub thumb_gen_threads: usize,
    pub thumb_cache_entries: usize,
    pub scan_on_startup: bool,
}

impl Default for PerformanceConfig {
    fn default() -> Self {
        let cpus = num_cpus::get();
        Self {
            thumb_gen_threads: if cpus <= 4 { 2 } else { 4 },
            thumb_cache_entries: 150,
            scan_on_startup: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiConfig {
    pub fullscreen: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self { fullscreen: true }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoConfig {
    pub hardware_decode: bool,
    pub default_volume: u8,
    pub loop_videos: bool,
}

impl Default for VideoConfig {
    fn default() -> Self {
        Self {
            hardware_decode: true,
            default_volume: 80,
            loop_videos: true,
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
