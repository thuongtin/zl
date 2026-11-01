//! Audio I/O for VoIP calls.
//!
//! Phase 1 (this file): device enumeration + mic->speaker pass-through on a
//! dedicated thread (Send-safe handle).
//! Phase 2 (TODO): Opus encode/decode + RTP packetization over the call's UDP socket.
//! Phase 3 (TODO): jitter buffer, echo cancellation, mixing, device selection.

use anyhow::Result;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread::{self, JoinHandle};

/// Send-safe handle to a running audio session. Dropping it stops the thread.
pub struct AudioHandle {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl AudioHandle {
    /// Start mic->speaker pass-through on a dedicated thread.
    pub fn start() -> Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();
        let thread = thread::spawn(move || {
            if let Err(e) = run_pass_through(stop_thread.clone()) {
                eprintln!("audio thread error: {e}");
            }
        });
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for AudioHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn run_pass_through(stop: Arc<AtomicBool>) -> Result<()> {
    let host = cpal::default_host();
    let input_dev = host
        .default_input_device()
        .ok_or_else(|| anyhow::anyhow!("no default input device"))?;
    let output_dev = host
        .default_output_device()
        .ok_or_else(|| anyhow::anyhow!("no default output device"))?;

    let in_cfg = input_dev.default_input_config()?;
    let out_cfg = output_dev.default_output_config()?;

    let channels = in_cfg.channels() as usize;

    let ring: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::with_capacity(48000)));
    let ring_in = ring.clone();
    let ring_out = ring.clone();

    let input_stream = input_dev.build_input_stream(
        &in_cfg.into(),
        move |data: &[f32], _| {
            if let Ok(mut buf) = ring_in.lock() {
                buf.extend_from_slice(data);
                if buf.len() > 48000 * channels {
                    let drop = buf.len() - 48000 * channels;
                    buf.drain(0..drop);
                }
            }
        },
        |e| eprintln!("input stream error: {e}"),
        None,
    )?;
    input_stream.play()?;

    let output_stream = output_dev.build_output_stream(
        &out_cfg.into(),
        move |data: &mut [f32], _| {
            if let Ok(mut buf) = ring_out.lock() {
                let n = data.len().min(buf.len());
                for (i, slot) in data.iter_mut().enumerate() {
                    *slot = if i < n { buf[i] } else { 0.0 };
                }
                if n > 0 {
                    buf.drain(0..n);
                }
            } else {
                for s in data.iter_mut() {
                    *s = 0.0;
                }
            }
        },
        |e| eprintln!("output stream error: {e}"),
        None,
    )?;
    output_stream.play()?;

    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    drop(input_stream);
    drop(output_stream);
    Ok(())
}

/// List available input (microphone) device names.
pub fn list_input_devices() -> Result<Vec<String>> {
    let host = cpal::default_host();
    let mut out = Vec::new();
    for d in host.input_devices()? {
        if let Ok(name) = d.name() {
            out.push(name);
        }
    }
    Ok(out)
}

/// List available output (speaker) device names.
pub fn list_output_devices() -> Result<Vec<String>> {
    let host = cpal::default_host();
    let mut out = Vec::new();
    for d in host.output_devices()? {
        if let Ok(name) = d.name() {
            out.push(name);
        }
    }
    Ok(out)
}

/// Open the default mic and speaker and run pass-through for `seconds` (smoke test).
pub fn run_loopback(seconds: u64) -> Result<()> {
    let session = AudioHandle::start()?;
    std::thread::sleep(std::time::Duration::from_secs(seconds));
    drop(session);
    Ok(())
}
