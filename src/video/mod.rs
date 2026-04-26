/// Video playback via mpv subprocess with JSON IPC control.
///
/// mpv renders fullscreen (leveraging its built-in hardware decode and OSC).
/// We control play/pause/seek/volume via the mpv JSON IPC socket.
/// The Slint video screen shows a minimal overlay; detecting mpv exit returns to gallery.
pub mod mpv;

use crate::config::VideoConfig;
use anyhow::{anyhow, Result};
use crossbeam_channel::{bounded, Receiver};
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::time::Duration;

pub struct VideoController {
    child: Option<Child>,
    ipc_socket: PathBuf,
    pub exit_rx: Option<Receiver<()>>,
    config: VideoConfig,
    cached_position: f64,
    cached_duration: f64,
    cached_paused: bool,
}

impl VideoController {
    pub fn new(config: VideoConfig) -> Self {
        Self {
            child: None,
            ipc_socket: PathBuf::from("/tmp/gallerypi-mpv.sock"),
            exit_rx: None,
            config,
            cached_position: 0.0,
            cached_duration: 0.0,
            cached_paused: false,
        }
    }

    pub fn open(&mut self, path: &str) -> Result<()> {
        self.stop();

        let socket = PathBuf::from(format!(
            "/tmp/gallerypi-mpv-{}.sock",
            std::process::id()
        ));

        let hwdec = if self.config.hardware_decode {
            if cfg!(target_arch = "aarch64") { "v4l2m2m-copy" } else { "auto-safe" }
        } else {
            "no"
        };

        let loop_arg = if self.config.loop_videos { "yes" } else { "no" };
        let ipc_arg = format!("--input-ipc-server={}", socket.display());
        let volume_arg = format!("--volume={}", self.config.default_volume);

        let child = std::process::Command::new("mpv")
            .args([
                "--fullscreen",
                &ipc_arg,
                "--osc=yes",              // mpv's native on-screen controls
                "--osd-level=1",
                "--touch-devices=auto",   // touch support in mpv
                "--hwdec",
                hwdec,
                "--loop-file",
                loop_arg,
                &volume_arg,
                "--no-terminal",
                "--input-default-bindings=yes",
                path,
            ])
            .spawn()
            .map_err(|e| anyhow!("Failed to launch mpv: {}", e))?;

        self.ipc_socket = socket.clone();
        self.cached_position = 0.0;
        self.cached_duration = 0.0;
        self.cached_paused = false;

        // Watch for exit in a background thread
        let (exit_tx, exit_rx) = bounded(1);
        let mut child = child;
        std::thread::spawn(move || {
            child.wait().ok();
            let _ = std::fs::remove_file(&socket);
            exit_tx.send(()).ok();
        });
        self.exit_rx = Some(exit_rx);

        Ok(())
    }

    /// Returns true if mpv has exited since last check.
    pub fn check_exited(&mut self) -> bool {
        if let Some(ref rx) = self.exit_rx {
            if rx.try_recv().is_ok() {
                self.exit_rx = None;
                return true;
            }
        }
        false
    }

    pub fn is_running(&self) -> bool {
        self.exit_rx.is_some()
    }

    pub fn stop(&mut self) {
        self.ipc_send(r#"{"command": ["quit"]}"#).ok();
        // Brief wait for graceful exit
        std::thread::sleep(Duration::from_millis(150));
        let _ = std::fs::remove_file(&self.ipc_socket);
        self.exit_rx = None;
    }

    pub fn toggle_pause(&self) {
        self.ipc_send(r#"{"command": ["cycle", "pause"]}"#).ok();
    }

    pub fn seek(&self, position: f64) {
        self.ipc_send(&format!(
            r#"{{"command": ["seek", {:.3}, "absolute"]}}"#,
            position
        ))
        .ok();
    }

    pub fn set_volume(&self, volume: f64) {
        let vol = (volume * 100.0).clamp(0.0, 100.0);
        self.ipc_send(&format!(
            r#"{{"command": ["set_property", "volume", {:.1}]}}"#,
            vol
        ))
        .ok();
    }

    /// Poll position and duration from mpv IPC. Updates internal cache.
    /// Call from a timer on the main thread.
    pub fn poll_state(&mut self) {
        if let Ok(pos) = self.ipc_get_f64("time-pos") {
            self.cached_position = pos;
        }
        if self.cached_duration <= 0.0 {
            if let Ok(dur) = self.ipc_get_f64("duration") {
                self.cached_duration = dur;
            }
        }
        if let Ok(paused) = self.ipc_get_bool("pause") {
            self.cached_paused = paused;
        }
    }

    pub fn get_position(&self) -> f64 { self.cached_position }
    pub fn get_duration(&self) -> f64 { self.cached_duration }
    pub fn is_paused(&self) -> bool { self.cached_paused }
    pub fn is_playing(&self) -> bool { !self.cached_paused && self.is_running() }

    // --- IPC helpers ---

    fn ipc_send(&self, cmd: &str) -> Result<()> {
        let mut stream = UnixStream::connect(&self.ipc_socket)
            .map_err(|e| anyhow!("IPC connect: {}", e))?;
        stream
            .set_write_timeout(Some(Duration::from_millis(100)))
            .ok();
        writeln!(stream, "{}", cmd).map_err(|e| anyhow!("IPC write: {}", e))?;
        Ok(())
    }

    fn ipc_get_f64(&self, property: &str) -> Result<f64> {
        use std::io::{BufRead, BufReader};
        let stream = UnixStream::connect(&self.ipc_socket)
            .map_err(|e| anyhow!("IPC connect: {}", e))?;
        stream
            .set_read_timeout(Some(Duration::from_millis(80)))
            .ok();
        stream
            .set_write_timeout(Some(Duration::from_millis(80)))
            .ok();
        let mut writer = stream.try_clone()?;
        let cmd = format!(r#"{{"command": ["get_property", "{}"]}}"#, property);
        writeln!(writer, "{}", cmd)?;

        let reader = BufReader::new(&stream);
        for line in reader.lines().take(5) {
            let line = line?;
            // {"data": 12.345, "error": "success", "request_id": 0}
            if line.contains("\"data\"") && line.contains("\"error\": \"success\"") {
                if let Some(val) = parse_data_f64(&line) {
                    return Ok(val);
                }
            }
        }
        Err(anyhow!("No response for property {}", property))
    }

    fn ipc_get_bool(&self, property: &str) -> Result<bool> {
        use std::io::{BufRead, BufReader};
        let stream = UnixStream::connect(&self.ipc_socket)
            .map_err(|e| anyhow!("IPC connect: {}", e))?;
        stream
            .set_read_timeout(Some(Duration::from_millis(80)))
            .ok();
        stream
            .set_write_timeout(Some(Duration::from_millis(80)))
            .ok();
        let mut writer = stream.try_clone()?;
        let cmd = format!(r#"{{"command": ["get_property", "{}"]}}"#, property);
        writeln!(writer, "{}", cmd)?;

        let reader = BufReader::new(&stream);
        for line in reader.lines().take(5) {
            let line = line?;
            if line.contains("\"data\": true") {
                return Ok(true);
            }
            if line.contains("\"data\": false") {
                return Ok(false);
            }
        }
        Err(anyhow!("No response for property {}", property))
    }
}

fn parse_data_f64(json_line: &str) -> Option<f64> {
    // Simple extraction of "data": <number> without a full JSON parser
    let data_pos = json_line.find("\"data\":")?;
    let after = json_line[data_pos + 7..].trim();
    let end = after
        .find(|c: char| c == ',' || c == '}')
        .unwrap_or(after.len());
    after[..end].trim().parse().ok()
}
