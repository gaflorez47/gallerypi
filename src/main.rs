mod app;
mod config;
mod db;
mod gallery;
mod scanner;
mod thumbnail;
mod ui;
mod util;
mod video;
mod viewer;

use anyhow::Result;
use tracing_subscriber::EnvFilter;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    tracing::info!("GalleryPi starting");

    let config = config::Config::load()?;
    tracing::info!("Media dir: {:?}", config.gallery.media_dir);

    util::paths::ensure_thumb_dir()?;

    let db_path = config::db_path();
    app::run(config, db_path)
}
