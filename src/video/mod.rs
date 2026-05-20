pub mod decoder;

use crate::config::VideoConfig;
use crossbeam_channel::{bounded, Receiver, Sender};
use slint::SharedPixelBuffer;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

/// A decoded RGBA video frame, safe to send across threads.
pub struct VideoFrame {
    pub pixels: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub pts: f64,
}

pub enum VideoCommand {
    Pause,
    Resume,
    Seek(f64),
    SetVolume(f64),
    Stop,
}

pub struct SharedVideoState {
    pub position: AtomicU64,
    pub duration: AtomicU64,
    pub paused: AtomicBool,
    pub ended: AtomicBool,
}

impl SharedVideoState {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            position: AtomicU64::new(0f64.to_bits()),
            duration: AtomicU64::new(0f64.to_bits()),
            paused: AtomicBool::new(false),
            ended: AtomicBool::new(false),
        })
    }
}

pub struct VideoController {
    frame_rx: Option<Receiver<VideoFrame>>,
    control_tx: Option<Sender<VideoCommand>>,
    state: Arc<SharedVideoState>,
    worker: Option<JoinHandle<()>>,
    config: VideoConfig,
}

impl VideoController {
    pub fn new(config: VideoConfig) -> Self {
        Self {
            frame_rx: None,
            control_tx: None,
            state: SharedVideoState::new(),
            worker: None,
            config,
        }
    }

    pub fn open(&mut self, path: &str) -> anyhow::Result<()> {
        self.stop();

        let state = SharedVideoState::new();
        self.state = state.clone();

        let (frame_tx, frame_rx) = bounded::<VideoFrame>(4);
        let (control_tx, control_rx) = bounded::<VideoCommand>(8);

        let path = path.to_owned();
        let cfg = self.config.clone();
        let handle = std::thread::Builder::new()
            .name("video-decoder".into())
            .spawn(move || {
                decoder::run_decoder(path, frame_tx, control_rx, state, cfg);
            })?;

        self.frame_rx = Some(frame_rx);
        self.control_tx = Some(control_tx);
        self.worker = Some(handle);
        Ok(())
    }

    pub fn stop(&mut self) {
        if let Some(tx) = self.control_tx.take() {
            tx.send(VideoCommand::Stop).ok();
        }
        self.frame_rx = None;
        if let Some(handle) = self.worker.take() {
            handle.join().ok();
        }
        self.state = SharedVideoState::new();
    }

    pub fn toggle_pause(&self) {
        if let Some(ref tx) = self.control_tx {
            let cmd = if self.state.paused.load(Ordering::Relaxed) {
                VideoCommand::Resume
            } else {
                VideoCommand::Pause
            };
            tx.send(cmd).ok();
        }
    }

    pub fn seek(&self, position: f64) {
        if let Some(ref tx) = self.control_tx {
            tx.send(VideoCommand::Seek(position)).ok();
        }
    }

    pub fn set_volume(&self, volume: f64) {
        if let Some(ref tx) = self.control_tx {
            tx.send(VideoCommand::SetVolume(volume.clamp(0.0, 1.0)))
                .ok();
        }
    }

    /// Drain the frame channel and return the latest frame as a `slint::Image`.
    /// Must be called from the main (Slint) thread.
    pub fn poll_frame(&mut self) -> Option<slint::Image> {
        let rx = self.frame_rx.as_ref()?;
        let mut latest: Option<VideoFrame> = None;
        while let Ok(frame) = rx.try_recv() {
            latest = Some(frame);
        }
        let frame = latest?;
        let buf = SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
            &frame.pixels,
            frame.width,
            frame.height,
        );
        Some(slint::Image::from_rgba8(buf))
    }

    /// Returns true if the decoder thread has finished (video ended or stopped).
    pub fn check_exited(&mut self) -> bool {
        if self.state.ended.load(Ordering::Relaxed) {
            // Also check worker actually finished
            if self
                .worker
                .as_ref()
                .map(|h| h.is_finished())
                .unwrap_or(true)
            {
                self.worker = None;
                self.control_tx = None;
                self.frame_rx = None;
                return true;
            }
        }
        false
    }

    pub fn is_running(&self) -> bool {
        self.worker.is_some()
    }

    pub fn get_position(&self) -> f64 {
        f64::from_bits(self.state.position.load(Ordering::Relaxed))
    }

    pub fn get_duration(&self) -> f64 {
        f64::from_bits(self.state.duration.load(Ordering::Relaxed))
    }

    pub fn is_paused(&self) -> bool {
        self.state.paused.load(Ordering::Relaxed)
    }

    pub fn is_playing(&self) -> bool {
        !self.is_paused() && self.is_running()
    }
}
