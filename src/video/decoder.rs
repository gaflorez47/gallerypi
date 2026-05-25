use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use ffmpeg_next::{
    self as ffmpeg,
    codec::{context::Context as CodecContext, Id as CodecId},
    format::Pixel,
    frame,
    software::scaling::{context::Context as SwsContext, flag::Flags as SwsFlags},
};
use rodio::{buffer::SamplesBuffer, OutputStream, Sink};
use tracing::{error, warn};

use super::{SharedVideoState, VideoCommand, VideoFrame};
use crate::config::VideoConfig;

pub fn run_decoder(
    path: String,
    frame_tx: Sender<VideoFrame>,
    control_rx: Receiver<VideoCommand>,
    state: Arc<SharedVideoState>,
    config: VideoConfig,
) {
    if let Err(e) = ffmpeg::init() {
        error!("ffmpeg init failed: {e}");
        state.ended.store(true, Ordering::Relaxed);
        return;
    }

    let mut ictx = match ffmpeg::format::input(&path) {
        Ok(c) => c,
        Err(e) => {
            error!("Cannot open {path}: {e}");
            state.ended.store(true, Ordering::Relaxed);
            return;
        }
    };

    // Duration
    let duration_secs = ictx.duration() as f64 / ffmpeg::ffi::AV_TIME_BASE as f64;
    state
        .duration
        .store(duration_secs.to_bits(), Ordering::Relaxed);

    // Find streams
    let video_stream_idx = ictx
        .streams()
        .best(ffmpeg::media::Type::Video)
        .map(|s| s.index());
    let audio_stream_idx = ictx
        .streams()
        .best(ffmpeg::media::Type::Audio)
        .map(|s| s.index());

    let Some(vsidx) = video_stream_idx else {
        error!("No video stream in {path}");
        state.ended.store(true, Ordering::Relaxed);
        return;
    };

    // Video decoder + scaler
    let (mut video_dec, video_time_base, video_width, video_height) = {
        let stream = ictx.stream(vsidx).unwrap();
        let tb = stream.time_base();
        let dec = match try_hw_decoder(&stream, &config).or_else(|| {
            let ctx = CodecContext::from_parameters(stream.parameters()).ok()?;
            ctx.decoder().video().ok()
        }) {
            Some(d) => d,
            None => {
                error!("No video decoder available for {path}");
                state.ended.store(true, Ordering::Relaxed);
                return;
            }
        };
        let w = dec.width();
        let h = dec.height();
        (dec, tb, w, h)
    };

    let mut sws = match SwsContext::get(
        video_dec.format(),
        video_width,
        video_height,
        Pixel::RGBA,
        video_width,
        video_height,
        SwsFlags::BILINEAR,
    ) {
        Ok(s) => s,
        Err(e) => {
            error!("SwsContext: {e}");
            state.ended.store(true, Ordering::Relaxed);
            return;
        }
    };

    // Optional audio decoder + resampler
    let mut audio_dec_and_resampler: Option<(
        ffmpeg::codec::decoder::Audio,
        ffmpeg::software::resampling::context::Context,
        u32, // sample_rate
    )> = audio_stream_idx.and_then(|asidx| {
        let stream = ictx.stream(asidx)?;
        let ctx = CodecContext::from_parameters(stream.parameters()).ok()?;
        let dec = ctx.decoder().audio().ok()?;
        let rate = dec.rate();
        let resampler = ffmpeg::software::resampling::context::Context::get(
            dec.format(),
            dec.channel_layout(),
            rate,
            ffmpeg::util::format::sample::Sample::F32(
                ffmpeg::util::format::sample::Type::Packed,
            ),
            ffmpeg::util::channel_layout::ChannelLayout::STEREO,
            rate,
        )
        .ok()?;
        Some((dec, resampler, rate))
    });

    // Audio output (must stay alive for the duration of playback)
    let (_audio_stream, audio_handle) = match OutputStream::try_default() {
        Ok(p) => p,
        Err(e) => {
            warn!("Audio output unavailable: {e}");
            // Continue without audio
            return run_no_audio(
                ictx,
                vsidx,
                video_dec,
                sws,
                video_time_base,
                video_width,
                video_height,
                frame_tx,
                control_rx,
                state,
                config,
            );
        }
    };
    let sink = match Sink::try_new(&audio_handle) {
        Ok(s) => s,
        Err(e) => {
            warn!("Audio sink: {e}");
            return run_no_audio(
                ictx,
                vsidx,
                video_dec,
                sws,
                video_time_base,
                video_width,
                video_height,
                frame_tx,
                control_rx,
                state,
                config,
            );
        }
    };
    let volume = config.default_volume as f32 / 100.0;
    sink.set_volume(volume);

    // Playback loop
    let mut is_paused = false;
    let mut pause_start = Instant::now();
    let mut start_instant = Instant::now();

    'outer: loop {
        let mut seek_to: Option<f64> = None;

        'packets: for (stream, packet) in ictx.packets() {
            // Process pending commands
            loop {
                match control_rx.try_recv() {
                    Ok(VideoCommand::Stop) => break 'outer,
                    Ok(VideoCommand::Pause) if !is_paused => {
                        is_paused = true;
                        pause_start = Instant::now();
                        sink.pause();
                        state.paused.store(true, Ordering::Relaxed);
                    }
                    Ok(VideoCommand::Resume) if is_paused => {
                        is_paused = false;
                        start_instant += pause_start.elapsed();
                        sink.play();
                        state.paused.store(false, Ordering::Relaxed);
                    }
                    Ok(VideoCommand::Seek(t)) => {
                        seek_to = Some(t);
                        break 'packets;
                    }
                    Ok(VideoCommand::SetVolume(v)) => sink.set_volume(v as f32),
                    Ok(_) | Err(crossbeam_channel::TryRecvError::Empty) => break,
                    Err(crossbeam_channel::TryRecvError::Disconnected) => break 'outer,
                }
            }

            // While paused, wait for commands (don't consume video packets)
            while is_paused {
                match control_rx.recv_timeout(Duration::from_millis(20)) {
                    Ok(VideoCommand::Stop) => break 'outer,
                    Ok(VideoCommand::Resume) => {
                        is_paused = false;
                        start_instant += pause_start.elapsed();
                        sink.play();
                        state.paused.store(false, Ordering::Relaxed);
                    }
                    Ok(VideoCommand::Seek(t)) => {
                        seek_to = Some(t);
                        break 'packets;
                    }
                    Ok(VideoCommand::SetVolume(v)) => sink.set_volume(v as f32),
                    Ok(_) | Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break 'outer,
                }
            }
            if seek_to.is_some() {
                break 'packets;
            }

            let sidx = stream.index();

            // Video packet
            if sidx == vsidx {
                if video_dec.send_packet(&packet).is_err() {
                    continue;
                }
                let mut decoded = frame::Video::empty();
                while video_dec.receive_frame(&mut decoded).is_ok() {
                    let mut rgba = frame::Video::empty();
                    if sws.run(&decoded, &mut rgba).is_err() {
                        continue;
                    }
                    let pts_secs = decoded
                        .pts()
                        .map(|pts| {
                            pts as f64 * video_time_base.numerator() as f64
                                / video_time_base.denominator() as f64
                        })
                        .unwrap_or(0.0);

                    // Frame timing: sleep until this frame's display time
                    let display_at =
                        start_instant + Duration::from_secs_f64(pts_secs.max(0.0));
                    let now = Instant::now();
                    if display_at > now {
                        std::thread::sleep(display_at - now);
                    }

                    // Build pixel buffer (handle stride)
                    let stride = rgba.stride(0);
                    let row_bytes = video_width as usize * 4;
                    let mut pixels = Vec::with_capacity(row_bytes * video_height as usize);
                    let data = rgba.data(0);
                    for row in 0..video_height as usize {
                        let start = row * stride;
                        pixels.extend_from_slice(&data[start..start + row_bytes]);
                    }

                    let vf = VideoFrame {
                        pixels,
                        width: video_width,
                        height: video_height,
                        pts: pts_secs,
                    };
                    if frame_tx.send(vf).is_err() {
                        break 'outer;
                    }
                    state
                        .position
                        .store(pts_secs.to_bits(), Ordering::Relaxed);
                }
            }

            // Audio packet
            if let (Some(asidx), Some((ref mut adec, ref mut resampler, rate))) =
                (audio_stream_idx, audio_dec_and_resampler.as_mut())
            {
                if sidx == asidx {
                    if adec.send_packet(&packet).is_err() {
                        continue;
                    }
                    let mut audio_frame = frame::Audio::empty();
                    while adec.receive_frame(&mut audio_frame).is_ok() {
                        let mut resampled = frame::Audio::empty();
                        if resampler.run(&audio_frame, &mut resampled).is_err() {
                            continue;
                        }
                        let byte_slice = resampled.data(0);
                        let f32_samples: Vec<f32> = byte_slice
                            .chunks_exact(4)
                            .map(|b| f32::from_ne_bytes(b.try_into().unwrap()))
                            .collect();
                        if !f32_samples.is_empty() {
                            sink.append(SamplesBuffer::new(2, *rate, f32_samples));
                        }
                    }
                }
            }
        }

        // Handle seek or loop
        if let Some(t) = seek_to {
            let ts = (t * ffmpeg::ffi::AV_TIME_BASE as f64) as i64;
            if let Err(e) = ictx.seek(ts, ts..) {
                warn!("seek failed: {e}");
            }
            flush_video_decoder(&mut video_dec);
            if let Some((ref mut adec, _, _)) = audio_dec_and_resampler {
                flush_audio_decoder(adec);
            }
            sink.clear();
            if !is_paused {
                sink.play();
            }
            start_instant = Instant::now() - Duration::from_secs_f64(t.max(0.0));
            state.position.store(t.to_bits(), Ordering::Relaxed);
        } else if config.loop_videos {
            if let Err(e) = ictx.seek(0, 0..) {
                warn!("loop seek failed: {e}");
            }
            flush_video_decoder(&mut video_dec);
            if let Some((ref mut adec, _, _)) = audio_dec_and_resampler {
                flush_audio_decoder(adec);
            }
            sink.clear();
            if !is_paused {
                sink.play();
            }
            start_instant = Instant::now();
            state.position.store(0f64.to_bits(), Ordering::Relaxed);
        } else {
            break 'outer;
        }
    }

    state.ended.store(true, Ordering::Relaxed);
}

/// Attempt to open a hardware-accelerated video decoder based on `config.hw_accel`.
/// Returns `None` to signal the caller should fall back to software decode.
fn try_hw_decoder(
    stream: &ffmpeg::format::stream::Stream,
    config: &VideoConfig,
) -> Option<ffmpeg::codec::decoder::Video> {
    if config.hw_accel == "none" {
        return None;
    }

    // Only H.264 has a reliable V4L2M2M decoder on RPi4.
    if stream.parameters().id() != CodecId::H264 {
        return None;
    }

    let hw_codec = ffmpeg::decoder::find_by_name("h264_v4l2m2m")?;

    let mut ctx = CodecContext::new_with_codec(hw_codec);
    ctx.set_parameters(stream.parameters()).ok()?;

    match ctx.decoder().video() {
        Ok(dec) => {
            tracing::info!("Hardware decoder h264_v4l2m2m opened");
            Some(dec)
        }
        Err(e) => {
            warn!("h264_v4l2m2m open failed ({e}), falling back to SW decode");
            None
        }
    }
}

fn flush_video_decoder(dec: &mut ffmpeg::codec::decoder::Video) {
    unsafe {
        ffmpeg::ffi::avcodec_flush_buffers(dec.as_mut_ptr());
    }
}

fn flush_audio_decoder(dec: &mut ffmpeg::codec::decoder::Audio) {
    unsafe {
        ffmpeg::ffi::avcodec_flush_buffers(dec.as_mut_ptr());
    }
}

/// Fallback playback path when audio output is unavailable.
fn run_no_audio(
    mut ictx: ffmpeg::format::context::Input,
    vsidx: usize,
    mut video_dec: ffmpeg::codec::decoder::Video,
    mut sws: SwsContext,
    video_time_base: ffmpeg::Rational,
    video_width: u32,
    video_height: u32,
    frame_tx: Sender<VideoFrame>,
    control_rx: Receiver<VideoCommand>,
    state: Arc<SharedVideoState>,
    config: VideoConfig,
) {
    let mut is_paused = false;
    let mut pause_start = Instant::now();
    let mut start_instant = Instant::now();

    'outer: loop {
        let mut seek_to: Option<f64> = None;

        'packets: for (stream, packet) in ictx.packets() {
            loop {
                match control_rx.try_recv() {
                    Ok(VideoCommand::Stop) => break 'outer,
                    Ok(VideoCommand::Pause) if !is_paused => {
                        is_paused = true;
                        pause_start = Instant::now();
                        state.paused.store(true, Ordering::Relaxed);
                    }
                    Ok(VideoCommand::Resume) if is_paused => {
                        is_paused = false;
                        start_instant += pause_start.elapsed();
                        state.paused.store(false, Ordering::Relaxed);
                    }
                    Ok(VideoCommand::Seek(t)) => {
                        seek_to = Some(t);
                        break 'packets;
                    }
                    Ok(_) | Err(crossbeam_channel::TryRecvError::Empty) => break,
                    Err(crossbeam_channel::TryRecvError::Disconnected) => break 'outer,
                }
            }
            while is_paused {
                match control_rx.recv_timeout(Duration::from_millis(20)) {
                    Ok(VideoCommand::Stop) => break 'outer,
                    Ok(VideoCommand::Resume) => {
                        is_paused = false;
                        start_instant += pause_start.elapsed();
                        state.paused.store(false, Ordering::Relaxed);
                    }
                    Ok(VideoCommand::Seek(t)) => {
                        seek_to = Some(t);
                        break 'packets;
                    }
                    Ok(_) | Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break 'outer,
                }
            }
            if seek_to.is_some() {
                break 'packets;
            }

            if stream.index() != vsidx {
                continue;
            }
            if video_dec.send_packet(&packet).is_err() {
                continue;
            }
            let mut decoded = frame::Video::empty();
            while video_dec.receive_frame(&mut decoded).is_ok() {
                let mut rgba = frame::Video::empty();
                if sws.run(&decoded, &mut rgba).is_err() {
                    continue;
                }
                let pts_secs = decoded
                    .pts()
                    .map(|pts| {
                        pts as f64 * video_time_base.numerator() as f64
                            / video_time_base.denominator() as f64
                    })
                    .unwrap_or(0.0);

                let display_at = start_instant + Duration::from_secs_f64(pts_secs.max(0.0));
                let now = Instant::now();
                if display_at > now {
                    std::thread::sleep(display_at - now);
                }

                let stride = rgba.stride(0);
                let row_bytes = video_width as usize * 4;
                let mut pixels = Vec::with_capacity(row_bytes * video_height as usize);
                let data = rgba.data(0);
                for row in 0..video_height as usize {
                    let start = row * stride;
                    pixels.extend_from_slice(&data[start..start + row_bytes]);
                }

                let vf = VideoFrame { pixels, width: video_width, height: video_height, pts: pts_secs };
                if frame_tx.send(vf).is_err() {
                    break 'outer;
                }
                state.position.store(pts_secs.to_bits(), Ordering::Relaxed);
            }
        }

        if let Some(t) = seek_to {
            let ts = (t * ffmpeg::ffi::AV_TIME_BASE as f64) as i64;
            ictx.seek(ts, ts..).ok();
            flush_video_decoder(&mut video_dec);
            start_instant = Instant::now() - Duration::from_secs_f64(t.max(0.0));
            state.position.store(t.to_bits(), Ordering::Relaxed);
        } else if config.loop_videos {
            ictx.seek(0, 0..).ok();
            flush_video_decoder(&mut video_dec);
            start_instant = Instant::now();
            state.position.store(0f64.to_bits(), Ordering::Relaxed);
        } else {
            break 'outer;
        }
    }

    state.ended.store(true, Ordering::Relaxed);
}
