//! Phase 2 VoIP media: Opus encode/decode + RTP over UDP.
//!
//! v1: captures mic, encodes Opus, sends RTP to `rtp_addr`; receives RTP,
//! decodes Opus, plays to speaker. Simple blocking loop with 20 ms frames.
//! No jitter buffer / resampling / echo cancellation yet.

use anyhow::Result;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use opus::{Channels as OpusChannels, Decoder, Encoder};
use std::collections::VecDeque;
use std::net::UdpSocket;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const SAMPLE_RATE: u32 = 16000;
const FRAME_SAMPLES: usize = 320; // 20 ms @ 16 kHz

static SELECTED_MIC_DEVICE: Mutex<String> = Mutex::new(String::new());
static SELECTED_CAMERA_DEVICE: Mutex<String> = Mutex::new(String::new());
static CACHED_DEVICES: Mutex<Option<(Vec<String>, Vec<String>)>> = Mutex::new(None);
static SHARED_CAM_H264: Mutex<(u64, Vec<u8>, Vec<u8>)> = Mutex::new((0, Vec::new(), Vec::new()));
pub fn set_selected_mic_device(name: &str) {
    if let Ok(mut g) = SELECTED_MIC_DEVICE.lock() {
        *g = name.trim().to_string();
    }
}

pub fn set_selected_camera_device(name: &str) {
    if let Ok(mut g) = SELECTED_CAMERA_DEVICE.lock() {
        *g = name.trim().to_string();
    }
}

pub fn selected_mic_device() -> String {
    SELECTED_MIC_DEVICE
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default()
}

pub fn selected_camera_device() -> String {
    SELECTED_CAMERA_DEVICE
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default()
}

/// Check whether `ffmpeg` is installed and reachable in `PATH` (cached after first probe).
pub fn is_ffmpeg_available() -> bool {
    static FFMPEG_OK: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| {
        let mut cmd = std::process::Command::new("ffmpeg");
        cmd.arg("-version")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000);
        }
        cmd.status().map(|s| s.success()).unwrap_or(false)
    });
    *FFMPEG_OK
}
/// Return cached `(mic_devices, camera_devices)` or enumerate once (with `refresh=true` to rescan).
pub fn list_media_devices(refresh: bool) -> (Vec<String>, Vec<String>) {
    if !refresh {
        if let Ok(g) = CACHED_DEVICES.lock() {
            if let Some(ref cached) = *g {
                return cached.clone();
            }
        }
    }
    let mut mics = Vec::new();
    let host = cpal::default_host();
    if let Ok(devs) = host.input_devices() {
        for d in devs {
            if let Ok(name) = d.name() {
                let trimmed = name.trim().to_string();
                if !trimmed.is_empty() && !mics.contains(&trimmed) {
                    mics.push(trimmed);
                }
            }
        }
    }
    let cams = enumerate_ffmpeg_cameras();
    let pair = (mics, cams);
    if let Ok(mut g) = CACHED_DEVICES.lock() {
        *g = Some(pair.clone());
    }
    pair
}

fn enumerate_ffmpeg_cameras() -> Vec<String> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        let mut cmd = std::process::Command::new("ffmpeg");
        cmd.args([
            "-hide_banner",
            "-list_devices",
            "true",
            "-f",
            "dshow",
            "-i",
            "dummy",
        ])
        .creation_flags(0x08000000);
        if let Ok(out) = cmd.output() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let mut cams = Vec::new();
            for line in stderr.lines() {
                if line.contains("Alternative name") {
                    continue;
                }
                if line.contains("(video)") && line.contains('"') {
                    let parts: Vec<&str> = line.split('"').collect();
                    if parts.len() >= 2 {
                        let name = parts[1].trim().to_string();
                        if !name.is_empty() && !cams.contains(&name) {
                            cams.push(name);
                        }
                    }
                }
            }
            if cams.is_empty() {
                for line in stderr.lines() {
                    if line.contains("Alternative name") {
                        continue;
                    }
                    if line.contains("(none)") && line.contains('"') {
                        let parts: Vec<&str> = line.split('"').collect();
                        if parts.len() >= 2 {
                            let name = parts[1].trim().to_string();
                            if !name.is_empty() && !cams.contains(&name) {
                                cams.push(name);
                            }
                        }
                    }
                }
            }
            return cams;
        }
    }
    #[cfg(target_os = "macos")]
    {
        let mut cmd = std::process::Command::new("ffmpeg");
        cmd.args([
            "-hide_banner",
            "-f",
            "avfoundation",
            "-list_devices",
            "true",
            "-i",
            "",
        ]);
        if let Ok(out) = cmd.output() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let mut cams = Vec::new();
            let mut in_video_section = false;
            for line in stderr.lines() {
                if line.contains("AVFoundation video devices:") {
                    in_video_section = true;
                    continue;
                }
                if line.contains("AVFoundation audio devices:") {
                    break;
                }
                if in_video_section {
                    if let Some((_, right)) = line.rsplit_once(']') {
                        let name = right.trim().to_string();
                        if !name.is_empty() && !cams.contains(&name) {
                            cams.push(name);
                        }
                    }
                }
            }
            if !cams.is_empty() {
                return cams;
            }
        }
        return vec!["0".to_string()];
    }
    #[cfg(target_os = "linux")]
    {
        let mut cams = Vec::new();
        for idx in 0..8 {
            let dev = format!("/dev/video{idx}");
            if std::path::Path::new(&dev).exists() {
                cams.push(dev);
            }
        }
        return cams;
    }
    #[allow(unreachable_code)]
    Vec::new()
}

pub struct MediaHandle {
    stop: Arc<AtomicBool>,
    mic_muted: Arc<AtomicBool>,
    cam_enabled: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}
impl MediaHandle {
    pub fn start(rtp_addr: String, local_port: u16) -> Result<Self> {
        Self::start_with_sockets(
            rtp_addr,
            String::new(),
            0,
            0,
            true,
            0,
            None,
            None,
            local_port,
            false,
            None,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn start_with_sockets(
        rtp_addr: String,
        rtcp_addr: String,
        from_uid_u32: u32,
        partner_uid_u32: u32,
        is_caller: bool,
        relay_token: u32,
        udp_rtp_sock: Option<Arc<tokio::net::UdpSocket>>,
        udp_rtcp_sock: Option<Arc<tokio::net::UdpSocket>>,
        local_port: u16,
        is_video: bool,
        gui_tx: Option<std::sync::mpsc::Sender<crate::gui::GuiEvent>>,
        egui_ctx: Option<eframe::egui::Context>,
    ) -> Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let mic_muted = Arc::new(AtomicBool::new(false));
        let cam_enabled = Arc::new(AtomicBool::new(is_video));
        let mut threads = Vec::new();
        let slot_idx = if udp_rtp_sock.is_some() {
            local_port
        } else {
            0
        };
        let fallback_sock = if udp_rtp_sock.is_none() {
            let s = UdpSocket::bind(format!("0.0.0.0:{local_port}"))?;
            let _ = s.set_read_timeout(Some(Duration::from_millis(10)));
            Some(Arc::new(s))
        } else {
            None
        };

        let stop_t = stop.clone();
        let mute_t = mic_muted.clone();
        let cam_t = cam_enabled.clone();
        let t = thread::spawn(move || {
            run_media_loop(
                stop_t,
                mute_t,
                cam_t,
                udp_rtp_sock,
                udp_rtcp_sock,
                fallback_sock,
                rtp_addr,
                rtcp_addr,
                from_uid_u32,
                partner_uid_u32,
                is_caller,
                relay_token,
                slot_idx,
                is_video,
                gui_tx,
                egui_ctx,
            );
        });
        threads.push(t);

        Ok(Self {
            stop,
            mic_muted,
            cam_enabled,
            threads,
        })
    }

    pub fn set_mic_muted(&self, muted: bool) {
        self.mic_muted.store(muted, Ordering::Relaxed);
    }

    pub fn set_cam_enabled(&self, enabled: bool) {
        self.cam_enabled.store(enabled, Ordering::Relaxed);
    }
}

impl Drop for MediaHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

/// Unwrap a Zalo UDP relay packet (`0x00893524..0x008936D1` in `ZaloCall.exe`) into its inner RTP slice.
pub fn unwrap_zalo_relay_rtp(pkt: &[u8]) -> Option<&[u8]> {
    if pkt.len() < 12 {
        return None;
    }
    let inner = match pkt[0] {
        // 1-byte relay media header (`0x04` = incoming audio RTP, `0x07` = P2P RTP, `0x0E` = incoming video RTP)
        0x04 | 0x07 | 0x0E => pkt.get(1..)?,
        // 5-byte relay media header (`0x03` / `0x05` / `0x0D` / `0x0F` + `token: u32 LE`)
        0x03 | 0x05 | 0x0D | 0x0F => pkt.get(5..)?,
        // 9-byte `0x7F` relay media header (`0x7F, 0x00, role, call_id: u32 LE, cmd: u16 LE`)
        0x7F if pkt.len() > 9 => {
            let cmd = u16::from_le_bytes([pkt[7], pkt[8]]);
            if (6..=10).contains(&cmd) {
                pkt.get(9..)?
            } else {
                return None;
            }
        }
        // 25-byte `0x02` relay media header (`cmd == 0x0032` at `0x008937C8`)
        0x02 if pkt.len() > 25 && u16::from_le_bytes([pkt[18], pkt[19]]) == 0x0032 => {
            pkt.get(25..)?
        }
        // Standard raw RTP v2 (`V=2` in top 2 bits)
        b if (b & 0xC0) == 0x80 => pkt,
        _ => return None,
    };
    if inner.len() >= 12 && (inner[0] & 0xC0) == 0x80 {
        Some(inner)
    } else {
        None
    }
}

/// Inbox for datagrams received on the RTCP socket. The ZRTP keepalive task in `call.rs` is the
/// permanent reader of that socket; it forwards everything that is not a ZRTP init reply here so
/// `run_media_loop` never competes with it for datagrams. Single active call assumed.
static RTCP_INBOX: Mutex<VecDeque<Vec<u8>>> = Mutex::new(VecDeque::new());
const RTCP_INBOX_MAX: usize = 512;

/// Queue a raw (still relay-framed) datagram read from the RTCP socket. Bounded; oldest is dropped.
pub fn push_rtcp_inbox(pkt: &[u8]) {
    if let Ok(mut q) = RTCP_INBOX.lock() {
        if q.len() >= RTCP_INBOX_MAX {
            q.pop_front();
        }
        q.push_back(pkt.to_vec());
    }
}

fn drain_rtcp_inbox() -> Vec<Vec<u8>> {
    RTCP_INBOX
        .lock()
        .map(|mut q| q.drain(..).collect())
        .unwrap_or_default()
}

fn clear_rtcp_inbox() {
    if let Ok(mut q) = RTCP_INBOX.lock() {
        q.clear();
    }
}

/// Cross-leg audio conference bridge for multi-member group calls (up to 4 slots).
/// Decoded PCM from slot `s` is pushed into every other slot's queue and mixed into
/// each slot's outbound Opus stream so all group call participants hear one another.
static PEER_AUDIO_BRIDGE: [Mutex<VecDeque<f32>>; 4] = [
    Mutex::new(VecDeque::new()),
    Mutex::new(VecDeque::new()),
    Mutex::new(VecDeque::new()),
    Mutex::new(VecDeque::new()),
];
const PEER_BRIDGE_MAX_SAMPLES: usize = FRAME_SAMPLES * 8;

fn push_peer_bridge_audio(from_slot: u16, pcm: &[f32]) {
    let idx = from_slot as usize;
    if idx >= PEER_AUDIO_BRIDGE.len() {
        return;
    }
    for (other, q_mtx) in PEER_AUDIO_BRIDGE.iter().enumerate() {
        if other == idx {
            continue;
        }
        if let Ok(mut q) = q_mtx.lock() {
            let q_len = q.len();
            if q_len + pcm.len() > PEER_BRIDGE_MAX_SAMPLES {
                let drop_n = ((q_len + pcm.len()) - PEER_BRIDGE_MAX_SAMPLES).min(q_len);
                q.drain(..drop_n);
            }
            q.extend(pcm.iter().copied());
        }
    }
}

fn mix_peer_bridge_audio(for_slot: u16, frame: &mut [f32]) {
    let idx = for_slot as usize;
    if idx >= PEER_AUDIO_BRIDGE.len() {
        return;
    }
    if let Ok(mut q) = PEER_AUDIO_BRIDGE[idx].lock() {
        if q.len() >= frame.len() {
            for s in frame.iter_mut() {
                if let Some(p) = q.pop_front() {
                    *s = (*s + p).clamp(-1.0, 1.0);
                }
            }
        }
    }
}

/// True if a raw relay-framed datagram is RTCP (PT 200..=207) containing a PLI or FIR.
#[cfg(test)]
fn wire_requests_keyframe(wire: &[u8]) -> bool {
    match unwrap_zalo_relay_rtp(wire) {
        Some(inner) => {
            matches!(inner.get(1), Some(b) if (200..=207).contains(b))
                && rtcp_requests_keyframe(inner)
        }
        None => false,
    }
}

const VIDEO_PT: u8 = 97;

/// Zalo video wire format, observed in two live calls (`voip.log` 2026-10-30, peer -> us), NOT RFC 6184:
/// * Whole access unit as Annex-B (`00 00 00 01 <NAL>...`) in one RTP packet, marker = 1.
/// * Larger access units: `0x1C, flags, <chunk of the Annex-B AU>` with flags `0x80` = first, `0x40` = last,
///   `0x00` = middle (low 5 bits 0); marker only on the last fragment. The AU is split **evenly** into
///   `ceil(len / ~1200)` parts (captured: 2x1083, 3x1062 in one call, 853/853/851 in another; the largest
///   unfragmented payload was 1212 bytes).
/// * Payload type: `PT 98` was seen only on the fragments of the first keyframe (SPS/PPS/SEI/IDR); fragmented
///   P frames used `PT 97`. So "PT 98 = keyframe" is an inference from two samples; receive ignores the PT.
const VIDEO_PT_FRAG: u8 = 98;
const ZALO_FRAG_INDICATOR: u8 = 0x1C;
/// Max RTP payload per packet (2-byte fragment header included). Peer packets reached 1212; 1200 is the
/// threshold that reproduces its split sizes. Whether the relay/peer accepts it from us is unverified.
const RTP_MAX_PAYLOAD: usize = 1200;
const ZALO_FRAG_BUF_MAX: usize = 1 << 20;
const VIDEO_IDR_INTERVAL_FRAMES: u32 = 15;

// Valid libx264-encoded H.264 Constrained Baseline (320x240, 15 fps) SPS/PPS/IDR/P-slice NAL units
const H264_SPS: &[u8] = &[
    0x67, 0x42, 0xC0, 0x1E, 0xDA, 0x05, 0x07, 0xEC, 0x04, 0x40, 0x00, 0x00, 0x03, 0x00, 0x40, 0x00,
    0x00, 0x07, 0x83, 0xC5, 0x8B, 0xA8,
];
const H264_PPS: &[u8] = &[0x68, 0xCE, 0x0F, 0x2C, 0x80];
const H264_IDR: &[u8] = &[
    0x65, 0x88, 0x84, 0x5C, 0x46, 0x28, 0x00, 0x08, 0x6C, 0xC7, 0x00, 0x02, 0x51, 0xC0, 0x19, 0x39,
    0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39,
    0x39, 0x3A, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE,
    0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA,
    0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB,
    0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE,
    0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA,
    0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB,
    0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE,
    0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA,
    0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB,
    0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE,
    0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA,
    0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB,
    0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE,
    0xBA, 0xEB, 0xAE, 0xBC,
];
const H264_IDR_ALT: &[u8] = &[
    0x65, 0x88, 0x82, 0x1F, 0x11, 0x8A, 0x00, 0x02, 0x22, 0xF1, 0xC0, 0x00, 0xB4, 0x70, 0x01, 0x93,
    0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93,
    0x93, 0x93, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA,
    0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB,
    0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE,
    0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA,
    0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB,
    0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE,
    0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA,
    0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB,
    0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE,
    0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA,
    0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB,
    0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE,
    0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA,
    0xEB, 0xAE, 0xBA, 0xEB, 0xC0,
];
#[cfg(test)]
const H264_PSLICE: &[u8] = &[0x41, 0x9A, 0x21, 0x70, 0x09, 0x6C];
const H264_PSLICES: [&[u8]; 14] = [
    &[0x41, 0x9A, 0x21, 0x70, 0x09, 0x6C],
    &[0x41, 0x9A, 0x41, 0x70, 0x09, 0x6C],
    &[0x41, 0x9A, 0x61, 0x70, 0x09, 0x6C],
    &[0x41, 0x9A, 0x81, 0x70, 0x09, 0x6C],
    &[0x41, 0x9A, 0xA1, 0x70, 0x09, 0x6C],
    &[0x41, 0x9A, 0xC1, 0x70, 0x09, 0x6C],
    &[0x41, 0x9A, 0xE1, 0x70, 0x09, 0x6C],
    &[0x41, 0x9B, 0x01, 0x70, 0x09, 0x6C],
    &[0x41, 0x9B, 0x21, 0x70, 0x09, 0x6C],
    &[0x41, 0x9B, 0x41, 0x70, 0x09, 0x6C],
    &[0x41, 0x9B, 0x61, 0x70, 0x09, 0x6C],
    &[0x41, 0x9B, 0x81, 0x70, 0x09, 0x6C],
    &[0x41, 0x9B, 0xA1, 0x70, 0x09, 0x6C],
    &[0x41, 0x9B, 0xC1, 0x70, 0x09, 0x6C],
];

/// Build a 12-byte-header RTP v2 packet.
fn build_rtp_packet(pt: u8, marker: bool, seq: u16, ts: u32, ssrc: u32, payload: &[u8]) -> Vec<u8> {
    let mut p = Vec::with_capacity(12 + payload.len());
    p.push(0x80);
    p.push(if marker { 0x80 | pt } else { pt });
    p.extend_from_slice(&seq.to_be_bytes());
    p.extend_from_slice(&ts.to_be_bytes());
    p.extend_from_slice(&ssrc.to_be_bytes());
    p.extend_from_slice(payload);
    p
}

/// Concatenate NAL units (no start code) into one Annex-B access unit.
fn annexb_au(nals: &[&[u8]]) -> Vec<u8> {
    let mut v = Vec::new();
    for n in nals {
        v.extend_from_slice(&[0, 0, 0, 1]);
        v.extend_from_slice(n);
    }
    v
}

fn is_annexb(p: &[u8]) -> bool {
    p.starts_with(&[0, 0, 0, 1]) || p.starts_with(&[0, 0, 1])
}

fn is_zalo_frag(p: &[u8]) -> bool {
    p.len() >= 2 && (p[0] & 0x1F) == 28 && (p[1] & 0x1F) == 0
}

/// True if the Annex-B access unit contains a NAL unit whose type is in `types`.
fn au_has_nal_type(au: &[u8], types: &[u8]) -> bool {
    let mut i = 0usize;
    while i + 3 < au.len() {
        if au[i] == 0 && au[i + 1] == 0 && au[i + 2] == 1 {
            if types.contains(&(au[i + 3] & 0x1F)) {
                return true;
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    false
}

/// True if the Annex-B access unit contains an IDR slice (NAL type 5).
fn au_has_idr(au: &[u8]) -> bool {
    au_has_nal_type(au, &[5])
}

/// True if the access unit can start decoding: it carries an IDR slice (5) or an SPS (7).
fn au_is_sync_point(au: &[u8]) -> bool {
    au_has_nal_type(au, &[5, 7])
}

/// Split one Annex-B access unit into Zalo video payloads: `(pt, payload)` per RTP packet.
/// Fits in one packet -> `(pt, au)`. Otherwise it is split evenly into `ceil(len / (max_payload - 2))` fragments;
/// keyframes (`au_has_idr`) always use `PT 98` (`0x62`) and delta frames use `PT 97` (`0x61`), matching
/// `RTPSender::SendOutgoingData` (`0x727948`) and `RTPReceiverVideo::ParseRtpPacket` (`0x7185c0`) in `libzrtc.so`.
fn packetize_zalo_au(au: &[u8], max_payload: usize) -> Vec<(u8, Vec<u8>)> {
    if au.is_empty() {
        return Vec::new();
    }
    let pt = if au_has_idr(au) {
        VIDEO_PT_FRAG
    } else {
        VIDEO_PT
    };
    if au.len() <= max_payload || max_payload < 3 {
        return vec![(pt, au.to_vec())];
    }
    let parts = au.len().div_ceil(max_payload - 2);
    let chunk = au.len().div_ceil(parts);
    let n = au.len().div_ceil(chunk); // actual number of fragments produced by `chunks(chunk)`
    au.chunks(chunk)
        .enumerate()
        .map(|(i, c)| {
            let mut flags = 0u8;
            if i == 0 {
                flags |= 0x80;
            }
            if i + 1 == n {
                flags |= 0x40;
            }
            let mut v = Vec::with_capacity(2 + c.len());
            v.push(ZALO_FRAG_INDICATOR);
            v.push(flags);
            v.extend_from_slice(c);
            (pt, v)
        })
        .collect()
}

/// Feed one video RTP payload to the decoder channel. Dispatch is by content (unambiguous: RFC 6184
/// never starts with byte `0x00` and never uses FU type 0): Annex-B access unit, Zalo fragment, else RFC 6184.
fn depacketize_h264(
    payload: &[u8],
    frag_buf: &mut Vec<u8>,
    h264_tx: &std::sync::mpsc::SyncSender<Vec<u8>>,
) {
    if payload.is_empty() {
        return;
    }
    if is_annexb(payload) {
        let _ = h264_tx.try_send(payload.to_vec());
        return;
    }
    if is_zalo_frag(payload) {
        let flags = payload[1];
        let body = &payload[2..];
        if flags & 0x80 != 0 {
            frag_buf.clear();
            frag_buf.extend_from_slice(body);
        } else if !frag_buf.is_empty() {
            frag_buf.extend_from_slice(body);
        } else {
            return; // first fragment lost: drop the rest of this access unit
        }
        if frag_buf.len() > ZALO_FRAG_BUF_MAX {
            frag_buf.clear();
            return;
        }
        if flags & 0x40 != 0 {
            let done = std::mem::take(frag_buf);
            if !done.is_empty() {
                let _ = h264_tx.try_send(done);
            }
        }
        return;
    }
    depacketize_rfc6184_h264(payload, frag_buf, h264_tx);
}

/// RFC 6184 packetization of one NAL unit (header byte included, no start code).
/// No longer used on the wire (the peer sends/expects the Zalo format above); kept to test the RFC 6184 receive path.
/// Returns one single-NAL payload when it fits in `max_payload`, otherwise FU-A fragments.
#[cfg(test)]
fn packetize_h264_nal(nal: &[u8], max_payload: usize) -> Vec<Vec<u8>> {
    if nal.is_empty() {
        return Vec::new();
    }
    if nal.len() <= max_payload || max_payload < 3 {
        return vec![nal.to_vec()];
    }
    let indicator = (nal[0] & 0xE0) | 28;
    let nal_type = nal[0] & 0x1F;
    let body = &nal[1..];
    let chunk = max_payload - 2;
    let n = body.len().div_ceil(chunk);
    body.chunks(chunk)
        .enumerate()
        .map(|(i, c)| {
            let mut fu_hdr = nal_type;
            if i == 0 {
                fu_hdr |= 0x80;
            }
            if i + 1 == n {
                fu_hdr |= 0x40;
            }
            let mut v = Vec::with_capacity(2 + c.len());
            v.push(indicator);
            v.push(fu_hdr);
            v.extend_from_slice(c);
            v
        })
        .collect()
}

/// Middle 32 bits of the NTP timestamp of the first Sender Report in a (compound) RTCP packet (the `LSR`
/// value a receiver echoes back in its report block), or `None` if there is no SR.
fn sr_middle_ntp(rtcp: &[u8]) -> Option<u32> {
    let mut i = 0usize;
    while i + 4 <= rtcp.len() {
        if (rtcp[i] & 0xC0) != 0x80 {
            return None;
        }
        let words = u16::from_be_bytes([rtcp[i + 2], rtcp[i + 3]]) as usize;
        if rtcp[i + 1] == 200 {
            let b = rtcp.get(i + 10..i + 14)?;
            return Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
        }
        i += (words + 1) * 4;
    }
    None
}

/// Reception statistics of one incoming RTP stream (RFC 3550 A.1, jitter not computed).
#[derive(Default)]
struct RxStats {
    started: bool,
    base_seq: u16,
    max_seq: u16,
    cycles: u32,
    received: u32,
    expected_prior: u32,
    received_prior: u32,
}

impl RxStats {
    /// Count one received packet; returns true if it skipped ahead (at least one packet missing before it).
    fn update(&mut self, seq: u16) -> bool {
        let mut gap = false;
        if !self.started {
            self.started = true;
            self.base_seq = seq;
            self.max_seq = seq;
        } else {
            let d = seq.wrapping_sub(self.max_seq);
            if d != 0 && d < 0x8000 {
                gap = d > 1;
                if seq < self.max_seq {
                    self.cycles = self.cycles.wrapping_add(1 << 16);
                }
                self.max_seq = seq;
            }
        }
        self.received = self.received.wrapping_add(1);
        gap
    }

    /// 24-byte RTCP report block about `ssrc` (None until the first packet arrived). Advances the interval.
    fn block(&mut self, ssrc: u32, lsr: u32, dlsr: u32) -> Option<[u8; 24]> {
        if !self.started {
            return None;
        }
        let ext_max = self.cycles | self.max_seq as u32;
        let expected = ext_max.wrapping_sub(self.base_seq as u32).wrapping_add(1);
        let lost_total = expected.saturating_sub(self.received).min(0x7F_FFFF);
        let exp_int = expected.wrapping_sub(self.expected_prior);
        let rcv_int = self.received.wrapping_sub(self.received_prior);
        self.expected_prior = expected;
        self.received_prior = self.received;
        let lost_int = exp_int.saturating_sub(rcv_int);
        let fraction = if exp_int == 0 {
            0
        } else {
            ((lost_int as u64 * 256) / exp_int as u64).min(255) as u8
        };
        let mut b = [0u8; 24];
        b[0..4].copy_from_slice(&ssrc.to_be_bytes());
        b[4] = fraction;
        b[5..8].copy_from_slice(&lost_total.to_be_bytes()[1..]);
        b[8..12].copy_from_slice(&ext_max.to_be_bytes());
        // b[12..16] interarrival jitter: not computed (0)
        b[16..20].copy_from_slice(&lsr.to_be_bytes());
        b[20..24].copy_from_slice(&dlsr.to_be_bytes());
        Some(b)
    }
}

/// NTP timestamp (seconds since 1900, 2^-32 s fraction) for "now".
fn ntp_now() -> (u32, u32) {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = d.as_secs().wrapping_add(2_208_988_800) as u32;
    let frac = ((d.subsec_nanos() as u64) << 32) / 1_000_000_000;
    (secs, frac as u32)
}

/// RTCP Sender Report (PT 200) with an optional report block. Layout as captured from the peer
/// (`len=52` = 28 + one 24-byte block).
fn build_sender_report(
    ssrc: u32,
    ntp: (u32, u32),
    rtp_ts: u32,
    packets: u32,
    octets: u32,
    block: Option<[u8; 24]>,
) -> Vec<u8> {
    let rc = block.is_some() as u8;
    let words = 6 + 6 * rc as u16; // length field = total 32-bit words - 1
    let mut p = Vec::with_capacity(28 + 24);
    p.push(0x80 | rc);
    p.push(200);
    p.extend_from_slice(&words.to_be_bytes());
    p.extend_from_slice(&ssrc.to_be_bytes());
    p.extend_from_slice(&ntp.0.to_be_bytes());
    p.extend_from_slice(&ntp.1.to_be_bytes());
    p.extend_from_slice(&rtp_ts.to_be_bytes());
    p.extend_from_slice(&packets.to_be_bytes());
    p.extend_from_slice(&octets.to_be_bytes());
    if let Some(b) = block {
        p.extend_from_slice(&b);
    }
    p
}

/// Empty SDES chunk (12 bytes, as sent by the peer): one source, no items.
fn build_sdes_empty(ssrc: u32) -> Vec<u8> {
    let mut p = vec![0x81, 202, 0, 2];
    p.extend_from_slice(&ssrc.to_be_bytes());
    p.extend_from_slice(&[0, 0, 0, 0]);
    p
}

/// RTCP Picture Loss Indication (PSFB, fmt 1, 12 bytes): ask `media_ssrc` for a keyframe.
fn build_pli(sender_ssrc: u32, media_ssrc: u32) -> Vec<u8> {
    let mut p = vec![0x81, 206, 0, 2];
    p.extend_from_slice(&sender_ssrc.to_be_bytes());
    p.extend_from_slice(&media_ssrc.to_be_bytes());
    p
}
/// RTCP REMB (Payload-Specific Feedback fmt=15, PT=206, 24 bytes): advertise receiver max bitrate (`bitrate_bps`)
/// for `media_ssrc` (`RTCPSender::BuildREMB @ 0x706fb4` in `libzrtc.so`).
fn build_remb(sender_ssrc: u32, media_ssrc: u32, bitrate_bps: u32) -> Vec<u8> {
    let mut exp: u8 = 0;
    let mut mantissa: u32 = bitrate_bps;
    while mantissa > 0x3FFFF && exp < 63 {
        mantissa >>= 1;
        exp += 1;
    }
    let mut p = Vec::with_capacity(24);
    p.extend_from_slice(&[0x8F, 206, 0, 5]);
    p.extend_from_slice(&sender_ssrc.to_be_bytes());
    p.extend_from_slice(&0u32.to_be_bytes());
    p.extend_from_slice(b"REMB");
    p.push(1); // num_ssrc = 1
    p.push((exp << 2) | ((mantissa >> 16) as u8 & 0x03));
    p.extend_from_slice(&(mantissa as u16).to_be_bytes());
    p.extend_from_slice(&media_ssrc.to_be_bytes());
    p
}

/// Build `ZRTPPacket::initZRTPPacketIncallNegotiateInfo` (`0x4ad5a0` / `0x48c9e0` in `libzrtc.so`):
/// `cmd = 0x0020`, `sub_cmd = 0x14`, payload = `[seq: u32 BE, count=2: u8, H264DecCapacityInfo(720p,30fps), NegoNetworkInfo(1)]`.
fn build_zrtp_incall_negotiate_packet(call_id: u32, relay_token: u32, nego_seq: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(11);
    body.extend_from_slice(&nego_seq.to_be_bytes());
    body.push(2);
    body.extend_from_slice(&[0x0B, 0x02, 0xD0, 0x1E]);
    body.extend_from_slice(&[0x29, 0x01]);

    let mut pkt = Vec::with_capacity(25 + body.len());
    pkt.push(0x01); // magic
    pkt.push(0x01); // version
    pkt.extend_from_slice(&nego_seq.to_le_bytes()); // seq
    pkt.extend_from_slice(&0u32.to_le_bytes()); // sub_seq
    pkt.extend_from_slice(&call_id.to_le_bytes()); // [0x10] callId
    pkt.extend_from_slice(&relay_token.to_le_bytes()); // [0x14] token
    pkt.extend_from_slice(&0x0020u16.to_le_bytes()); // [0x18] cmd = 0x0020
    pkt.push(0x14); // [0x1A] sub_cmd = 0x14 (IncallNegotiateInfo)
    pkt.extend_from_slice(&(body.len() as u32).to_le_bytes());
    pkt.extend_from_slice(&body);
    pkt
}

/// Spawn a background webcam capture thread via `ffmpeg -f dshow` (on Windows) that encodes live 320x240 15fps
/// Constrained Baseline H.264 access units for RTP transmission AND decodes a local PIP preview for the call window.
fn spawn_camera_capture_thread(
    stop: Arc<AtomicBool>,
    cam_enabled: Arc<AtomicBool>,
    want_keyframe: Arc<AtomicBool>,
    cam_au_tx: std::sync::mpsc::SyncSender<Vec<u8>>,
    gui_tx: Option<std::sync::mpsc::Sender<crate::gui::GuiEvent>>,
    egui_ctx: Option<eframe::egui::Context>,
) {
    thread::spawn(move || {
        use std::io::Read;
        use std::process::{Command, Stdio};

        let mut current_dev = String::new();
        let mut child_opt: Option<std::process::Child> = None;
        let mut failed_devs: Vec<String> = Vec::new();
        let mut read_buf = [0u8; 8192];
        let mut stream_acc: Vec<u8> = Vec::with_capacity(65536);
        let mut warned_missing = false;

        // Optional local preview decoder (`ffmpeg` H.264 -> 240x180 RGBA)
        let (prev_tx, prev_rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(15);
        if let (Some(gtx), Some(ctx)) = (gui_tx.clone(), egui_ctx.clone()) {
            let stop_p = stop.clone();
            thread::spawn(move || {
                run_local_preview_decoder(stop_p, prev_rx, gtx, ctx);
            });
        }

        while !stop.load(Ordering::Relaxed) {
            if !cam_enabled.load(Ordering::Relaxed) {
                if let Some(mut c) = child_opt.take() {
                    let _ = c.kill();
                }
                thread::sleep(Duration::from_millis(80));
                continue;
            }

            if !is_ffmpeg_available() {
                if !warned_missing {
                    warned_missing = true;
                    crate::voip_log::line(
                        "VCAM",
                        "ffmpeg not found in PATH; camera capture unavailable",
                    );
                    if let Some(ref gtx) = gui_tx {
                        let _ = gtx.send(crate::gui::GuiEvent::StatusUpdate(
                            "⚠️ Không tìm thấy ffmpeg trong PATH để bật Camera (cài bằng: winget install Gyan.FFmpeg)".into(),
                        ));
                    }
                    if let Some(ref ctx) = egui_ctx {
                        ctx.request_repaint();
                    }
                }
                thread::sleep(Duration::from_millis(500));
                continue;
            }

            let desired = {
                let sel = selected_camera_device();
                if !sel.is_empty() && !failed_devs.contains(&sel) {
                    sel
                } else {
                    let (_, cams) = list_media_devices(false);
                    cams.into_iter()
                        .find(|c| !failed_devs.contains(c))
                        .unwrap_or_default()
                }
            };
            if desired.is_empty() {
                if !warned_missing {
                    warned_missing = true;
                    if let Some(ref gtx) = gui_tx {
                        let _ = gtx.send(crate::gui::GuiEvent::StatusUpdate(
                            "⚠️ Không tìm thấy thiết bị Camera khả dụng trên máy".into(),
                        ));
                    }
                    if let Some(ref ctx) = egui_ctx {
                        ctx.request_repaint();
                    }
                }
                thread::sleep(Duration::from_millis(250));
                continue;
            }

            if child_opt.is_none() || current_dev != desired {
                if let Some(mut c) = child_opt.take() {
                    let _ = c.kill();
                }
                current_dev = desired.clone();
                stream_acc.clear();

                let mut cmd = Command::new("ffmpeg");
                cmd.args(["-hide_banner", "-loglevel", "error"]);
                #[cfg(target_os = "windows")]
                cmd.args([
                    "-f",
                    "dshow",
                    "-rtbufsize",
                    "32M",
                    "-i",
                    &format!("video={current_dev}"),
                ]);
                #[cfg(target_os = "macos")]
                cmd.args([
                    "-f",
                    "avfoundation",
                    "-framerate",
                    "30",
                    "-i",
                    &format!("{current_dev}:none"),
                ]);
                #[cfg(target_os = "linux")]
                cmd.args(["-f", "v4l2", "-i", &current_dev]);
                cmd.args([
                    "-vf",
                    "fps=15,scale=320:240:force_original_aspect_ratio=decrease,pad=320:240:(ow-iw)/2:(oh-ih)/2,format=yuv420p",
                    "-c:v",
                    "libx264",
                    "-preset",
                    "ultrafast",
                    "-tune",
                    "zerolatency",
                    "-profile:v",
                    "baseline",
                    "-level",
                    "3.0",
                    "-b:v",
                    "260k",
                    "-maxrate",
                    "320k",
                    "-bufsize",
                    "640k",
                    "-x264-params",
                    "keyint=15:min-keyint=1:bframes=0:ref=1:cabac=0:sliced-threads=0:scenecut=0:aud=1:repeat-headers=1",
                    "-f",
                    "h264",
                    "pipe:1",
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null());
                #[cfg(target_os = "windows")]
                {
                    use std::os::windows::process::CommandExt;
                    cmd.creation_flags(0x08000000);
                }

                match cmd.spawn() {
                    Ok(c) => {
                        crate::voip_log::line(
                            "VCAM",
                            format!("started webcam capture: {current_dev}"),
                        );
                        let _ = want_keyframe.swap(false, Ordering::Relaxed);
                        child_opt = Some(c);
                    }
                    Err(e) => {
                        crate::voip_log::line(
                            "VCAM",
                            format!("webcam spawn failed ({current_dev}): {e}"),
                        );
                        thread::sleep(Duration::from_millis(500));
                        continue;
                    }
                }
            }

            let Some(child) = &mut child_opt else {
                continue;
            };
            let Some(stdout) = &mut child.stdout else {
                child_opt = None;
                continue;
            };
            match stdout.read(&mut read_buf) {
                Ok(0) | Err(_) => {
                    let _ = child.kill();
                    child_opt = None;
                    if stream_acc.is_empty()
                        && !current_dev.is_empty()
                        && !failed_devs.contains(&current_dev)
                    {
                        crate::voip_log::line(
                            "VCAM",
                            format!(
                                "webcam device produced 0 bytes, marking failed: {current_dev}"
                            ),
                        );
                        failed_devs.push(current_dev.clone());
                    }
                    thread::sleep(Duration::from_millis(250));
                }
                Ok(n) => {
                    stream_acc.extend_from_slice(&read_buf[..n]);
                    while let Some(pos) = find_aud_delimiter(&stream_acc, 6) {
                        let chunk = stream_acc[..pos].to_vec();
                        stream_acc.drain(..pos);
                        let clean = strip_leading_aud(&chunk);
                        if !clean.is_empty() {
                            if let Ok(mut sh) = SHARED_CAM_H264.lock() {
                                sh.0 = sh.0.wrapping_add(1);
                                if au_has_idr(&clean) {
                                    sh.2 = clean.clone();
                                }
                                sh.1 = clean.clone();
                            }
                            let _ = prev_tx.try_send(clean.clone());
                            let _ = cam_au_tx.try_send(clean);
                        }
                    }
                    if stream_acc.len() > 262_144 {
                        stream_acc.clear();
                    }
                }
            }
        }
        if let Some(mut c) = child_opt.take() {
            let _ = c.kill();
        }
    });
}

fn find_aud_delimiter(buf: &[u8], start_from: usize) -> Option<usize> {
    if buf.len() < start_from + 6 {
        return None;
    }
    for i in start_from..=buf.len() - 6 {
        if buf[i] == 0
            && buf[i + 1] == 0
            && buf[i + 2] == 0
            && buf[i + 3] == 1
            && (buf[i + 4] & 0x1F) == 9
        {
            return Some(i);
        }
    }
    None
}

fn strip_leading_aud(au: &[u8]) -> Vec<u8> {
    if au.len() >= 6 && au[0..4] == [0, 0, 0, 1] && (au[4] & 0x1F) == 9 {
        au[6..].to_vec()
    } else {
        au.to_vec()
    }
}

fn run_local_preview_decoder(
    stop: Arc<AtomicBool>,
    rx: std::sync::mpsc::Receiver<Vec<u8>>,
    gui_tx: std::sync::mpsc::Sender<crate::gui::GuiEvent>,
    egui_ctx: eframe::egui::Context,
) {
    use std::io::{Read, Write};
    use std::process::{Command, Stdio};

    const PREV_W: usize = 240;
    const PREV_H: usize = 180;
    const PREV_BYTES: usize = PREV_W * PREV_H * 4;

    let mut cmd = Command::new("ffmpeg");
    cmd.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-flags",
        "low_delay",
        "-probesize",
        "32",
        "-analyzeduration",
        "0",
        "-f",
        "h264",
        "-i",
        "pipe:0",
        "-vf",
        "scale=240:180:force_original_aspect_ratio=decrease,pad=240:180:(ow-iw)/2:(oh-ih)/2",
        "-pix_fmt",
        "rgba",
        "-f",
        "rawvideo",
        "pipe:1",
    ])
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::null());

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }

    let Ok(mut child) = cmd.spawn() else {
        return;
    };
    let Some(mut stdin) = child.stdin.take() else {
        let _ = child.kill();
        return;
    };
    let Some(mut stdout) = child.stdout.take() else {
        let _ = child.kill();
        return;
    };
    let stop_w = stop.clone();
    thread::spawn(move || {
        while !stop_w.load(Ordering::Relaxed) {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(au) => {
                    if stdin.write_all(H264_AUD).is_err() || stdin.write_all(&au).is_err() {
                        break;
                    }
                    let _ = stdin.flush();
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    });

    let mut frame = vec![0u8; PREV_BYTES];
    while !stop.load(Ordering::Relaxed) {
        if stdout.read_exact(&mut frame).is_ok() {
            let (cw, ch, cropped) = trim_black_borders(&frame, PREV_W, PREV_H);
            let img = eframe::egui::ColorImage::from_rgba_unmultiplied([cw, ch], &cropped);
            let _ = gui_tx.send(crate::gui::GuiEvent::StickerImageReady {
                key: "call_local_video".to_string(),
                image: img,
            });
            egui_ctx.request_repaint();
        } else {
            break;
        }
    }
    let _ = child.kill();
}

/// Result counters of every UDP send. They used to be dropped silently (`let _ = try_send_to`) while the
/// RTP sequence number still advanced. The peer's RTCP reported ~78% loss of our audio and video, in
/// bursts that hit both streams at once; whether any of that is a failed local send is **unknown** until
/// the `TXST` log line of the next call shows these counters.
#[derive(Default)]
struct TxStats {
    ok: u32,
    /// tokio answered `WouldBlock` (not necessarily a full OS buffer): the datagram was NOT sent.
    would_block: u32,
    short: u32,
    err: u32,
    last_err: Option<String>,
}

/// Send one datagram and record the outcome.
fn send_dgram(
    sock: &tokio::net::UdpSocket,
    data: &[u8],
    target: std::net::SocketAddr,
    st: &mut TxStats,
) {
    match sock.try_send_to(data, target) {
        Ok(n) if n == data.len() => st.ok += 1,
        Ok(_) => st.short += 1,
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => st.would_block += 1,
        Err(e) => {
            st.err += 1;
            st.last_err = Some(e.to_string());
        }
    }
}

/// True if a (compound) RTCP packet contains a PLI (PSFB fmt 1) or FIR (PSFB fmt 4).
#[cfg(test)]
pub(crate) fn rtcp_requests_keyframe(rtcp: &[u8]) -> bool {
    let mut i = 0usize;
    while i + 4 <= rtcp.len() {
        if (rtcp[i] & 0xC0) != 0x80 {
            return false;
        }
        let fmt = rtcp[i] & 0x1F;
        let pt = rtcp[i + 1];
        let words = u16::from_be_bytes([rtcp[i + 2], rtcp[i + 3]]) as usize;
        if pt == 206 && (fmt == 1 || fmt == 4) {
            return true;
        }
        i += (words + 1) * 4;
    }
    false
}

/// Extract the Opus payload slice from a standard RFC 3550 RTP v2 packet (handling CSRC & header extensions).
fn extract_rtp_opus_payload(rtp: &[u8]) -> Option<&[u8]> {
    if rtp.len() < 12 || (rtp[0] & 0xC0) != 0x80 {
        return None;
    }
    let cc = (rtp[0] & 0x0F) as usize;
    let has_ext = (rtp[0] & 0x10) != 0;
    let has_pad = (rtp[0] & 0x20) != 0;
    let mut offset = 12 + cc * 4;
    if has_ext {
        if rtp.len() < offset + 4 {
            return None;
        }
        let ext_words = u16::from_be_bytes([rtp[offset + 2], rtp[offset + 3]]) as usize;
        offset += 4 + ext_words * 4;
    }
    let mut end = rtp.len();
    if has_pad && end > offset {
        let pad_len = rtp[end - 1] as usize;
        if pad_len > 0 && pad_len <= end - offset {
            end -= pad_len;
        }
    }
    if offset < end {
        Some(&rtp[offset..end])
    } else {
        None
    }
}

#[allow(clippy::too_many_arguments)]
fn run_media_loop(
    stop: Arc<AtomicBool>,
    mic_muted: Arc<AtomicBool>,
    cam_enabled: Arc<AtomicBool>,
    udp_rtp_sock: Option<Arc<tokio::net::UdpSocket>>,
    udp_rtcp_sock: Option<Arc<tokio::net::UdpSocket>>,
    fallback_sock: Option<Arc<UdpSocket>>,
    rtp_addr: String,
    rtcp_addr: String,
    from_uid_u32: u32,
    partner_uid_u32: u32,
    is_caller: bool,
    relay_token: u32,
    slot_idx: u16,
    is_video: bool,
    gui_tx: Option<std::sync::mpsc::Sender<crate::gui::GuiEvent>>,
    egui_ctx: Option<eframe::egui::Context>,
) {
    let (h264_tx, h264_rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(120);
    let (cam_au_tx, cam_au_rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(4);
    let video_synced = Arc::new(AtomicBool::new(false));
    let want_keyframe = Arc::new(AtomicBool::new(false));
    let mut decoder_active = false;
    let tex_key = if slot_idx == 0 {
        "call_remote_video".to_string()
    } else {
        format!("call_remote_video_{slot_idx}")
    };
    if let (Some(tx), Some(ctx)) = (gui_tx.clone(), egui_ctx.clone()) {
        let stop_v = stop.clone();
        let synced_v = video_synced.clone();
        decoder_active = true;
        thread::spawn(move || {
            run_h264_ffmpeg_decoder(stop_v, h264_rx, tx, ctx, synced_v, tex_key);
        });
    }
    if slot_idx == 0 {
        spawn_camera_capture_thread(
            stop.clone(),
            cam_enabled.clone(),
            want_keyframe.clone(),
            cam_au_tx,
            gui_tx,
            egui_ctx,
        );
    }
    let mut fu_a_buf: Vec<u8> = Vec::with_capacity(65536);
    let host = cpal::default_host();
    let preferred_mic = selected_mic_device();
    let input_dev = host
        .input_devices()
        .ok()
        .and_then(|mut ds| {
            if preferred_mic.is_empty() {
                None
            } else {
                ds.find(|d| d.name().map(|n| n.trim() == preferred_mic).unwrap_or(false))
            }
        })
        .or_else(|| host.default_input_device());
    let input_dev = match input_dev {
        Some(d) => d,
        None => {
            eprintln!("media: no input device");
            return;
        }
    };
    let output_dev = match host.default_output_device() {
        Some(d) => d,
        None => {
            eprintln!("media: no output device");
            return;
        }
    };

    let in_cfg = match input_dev.default_input_config() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("media: input config err {e}");
            return;
        }
    };
    let out_cfg = match output_dev.default_output_config() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("media: output config err {e}");
            return;
        }
    };
    let in_rate = in_cfg.sample_rate().0.max(8000);
    let in_channels = (in_cfg.channels() as usize).max(1);
    let out_rate = out_cfg.sample_rate().0.max(8000);
    let out_channels = (out_cfg.channels() as usize).max(1);

    // Shared 16kHz mono buffers
    let input_buf: Arc<Mutex<Vec<f32>>> =
        Arc::new(Mutex::new(Vec::with_capacity(FRAME_SAMPLES * 8)));
    let output_buf: Arc<Mutex<Vec<f32>>> =
        Arc::new(Mutex::new(Vec::with_capacity(FRAME_SAMPLES * 8)));
    let ib = input_buf.clone();

    // Input stream: downmix stereo -> mono and downsample `in_rate` -> 16000 Hz
    let input_stream = match input_dev.build_input_stream(
        &in_cfg.into(),
        move |data: &[f32], _| {
            if let Ok(mut b) = ib.lock() {
                let total_frames = data.len() / in_channels;
                let step = (in_rate as f32 / SAMPLE_RATE as f32).max(1.0);
                let out_frames = (total_frames as f32 / step) as usize;
                for i in 0..out_frames {
                    let src_idx = ((i as f32 * step) as usize).min(total_frames.saturating_sub(1));
                    let mut sum = 0.0_f32;
                    for ch in 0..in_channels {
                        sum += data[src_idx * in_channels + ch];
                    }
                    b.push(sum / in_channels as f32);
                }
                if b.len() > FRAME_SAMPLES * 25 {
                    let drop_n = b.len() - FRAME_SAMPLES * 25;
                    b.drain(0..drop_n);
                }
            }
        },
        |e| eprintln!("input stream err {e}"),
        None,
    ) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("media: build input err {e}");
            return;
        }
    };
    if let Err(e) = input_stream.play() {
        eprintln!("media: play input err {e}");
    }

    // Output stream: upsample 16000 Hz mono -> `out_rate` and duplicate across `out_channels`
    let ob2 = output_buf.clone();
    let output_stream = match output_dev.build_output_stream(
        &out_cfg.into(),
        move |data: &mut [f32], _| {
            if let Ok(mut b) = ob2.lock() {
                let out_frames = data.len() / out_channels;
                let step = SAMPLE_RATE as f32 / out_rate as f32;
                let mut consumed = 0usize;
                for i in 0..out_frames {
                    let src_idx = (i as f32 * step) as usize;
                    let sample = if src_idx < b.len() {
                        consumed = consumed.max(src_idx + 1);
                        b[src_idx]
                    } else {
                        0.0
                    };
                    for ch in 0..out_channels {
                        data[i * out_channels + ch] = sample;
                    }
                }
                if consumed > 0 {
                    let drain_n = consumed.min(b.len());
                    b.drain(0..drain_n);
                }
            } else {
                for s in data.iter_mut() {
                    *s = 0.0;
                }
            }
        },
        |e| eprintln!("output stream err {e}"),
        None,
    ) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("media: build output err {e}");
            return;
        }
    };
    if let Err(e) = output_stream.play() {
        eprintln!("media: play output err {e}");
    }

    // Opus codecs
    let mut encoder = match Encoder::new(SAMPLE_RATE, OpusChannels::Mono, opus::Application::Voip) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("media: encoder err {e}");
            return;
        }
    };
    let mut decoder = match Decoder::new(SAMPLE_RATE, OpusChannels::Mono) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("media: decoder err {e}");
            return;
        }
    };

    let mut seq: u16 = 0;
    let mut ts: u32 = 0;
    let ssrc = if from_uid_u32 != 0 {
        from_uid_u32
    } else {
        0x5A41_4C4F
    };
    let rtp_sock_addr = rtp_addr.parse::<std::net::SocketAddr>().ok();
    let rtcp_sock_addr = rtcp_addr.parse::<std::net::SocketAddr>().ok();
    let mut last_rtcp = std::time::Instant::now() - Duration::from_secs(2);
    let mut last_rtp_tx = std::time::Instant::now();
    let mut last_video_tx = std::time::Instant::now();
    let mut vid_seq: u16 = 0;
    let mut vid_ts: u32 = 0;
    let mut vid_frame_no: u32 = 0;
    let mut vid_gop_pos: usize = 0;
    let mut vid_idr_parity: u8 = 0;
    clear_rtcp_inbox();
    // Reception stats for the peer's audio/video streams (they share one SSRC but have separate seq spaces),
    // and the last Sender Report seen (for LSR/DLSR in our report blocks).
    let mut rx_audio = RxStats::default();
    let mut rx_video = RxStats::default();
    let mut peer_ssrc: u32 = 0;
    let mut last_peer_sr: Option<(u32, std::time::Instant)> = None;
    let mut vid_pkts_tx: u32 = 0;
    let mut vid_octets_tx: u32 = 0;
    let mut wl = crate::voip_log::WireLog::new();
    // Send outcome counters + longest main-loop stall, reported as `TXST` every 5 s.
    let mut txs = TxStats::default();
    let mut last_txst = std::time::Instant::now();
    let mut last_iter = std::time::Instant::now();
    let mut max_loop_gap = Duration::ZERO;
    // Peer video that cannot be decoded yet (no SPS/IDR seen) or has a hole -> we ask for a keyframe (PLI).
    // `video_synced` is set by the decoder feeder when the first SPS/IDR access unit arrives.
    let mut pli_gap = false;
    let mut pli_sent: u32 = 0;
    let mut last_pli = std::time::Instant::now() - Duration::from_secs(10);
    crate::voip_log::line(
        "CALL",
        format!(
            "media start rtp={rtp_addr} rtcp={rtcp_addr} video={is_video} caller={is_caller} relay_token={} tx_video_pt={VIDEO_PT} tx_frag_pt={VIDEO_PT_FRAG} video_format=annexb-au tx_audio_pt=112 max_payload={RTP_MAX_PAYLOAD}",
            relay_token != 0
        ),
    );
    let vid_ssrc = ssrc.wrapping_add(1);
    let mut nego_seq: u32 = 1;
    let mut last_cam_idr: Option<Vec<u8>> = None;

    while !stop.load(Ordering::Relaxed) {
        // --- Capture: take 320 samples (or emit comfort-noise every 20ms so peer never sees 0kbps) ---
        let mut frame: Vec<f32> = {
            let mut b = input_buf.lock().unwrap();
            if b.len() >= FRAME_SAMPLES {
                b.drain(0..FRAME_SAMPLES).collect()
            } else {
                Vec::new()
            }
        };
        if frame.is_empty() && last_rtp_tx.elapsed() >= Duration::from_millis(20) {
            frame = vec![0.0_f32; FRAME_SAMPLES];
        }
        if frame.len() == FRAME_SAMPLES {
            last_rtp_tx = std::time::Instant::now();
            if mic_muted.load(Ordering::Relaxed) {
                frame.fill(0.0);
            }
            mix_peer_bridge_audio(slot_idx, &mut frame);
            let mut out = vec![0u8; 4000];
            match encoder.encode_float(&frame, &mut out) {
                Ok(n) => {
                    // Build inner RTP header (12 bytes, PT=112 matching Zalo's opus/16000/1 codec)
                    let mut rtp_pkt = Vec::with_capacity(12 + n);
                    rtp_pkt.push(0x80); // V=2, P=0, X=0, CC=0
                    rtp_pkt.push(112); // payload type 112 (opus/16000/1)
                    rtp_pkt.extend_from_slice(&seq.to_be_bytes());
                    rtp_pkt.extend_from_slice(&ts.to_be_bytes());
                    rtp_pkt.extend_from_slice(&ssrc.to_be_bytes());
                    rtp_pkt.extend_from_slice(&out[..n]);

                    if let (Some(ref tsock), Some(target)) = (&udp_rtp_sock, rtp_sock_addr) {
                        if relay_token != 0 {
                            let type_byte: u8 = 0x03; // `ZRTPPacket::initZRTPPacketAudio` (`isRtcp=false`)
                            let mut wire = Vec::with_capacity(5 + rtp_pkt.len());
                            wire.push(type_byte);
                            wire.extend_from_slice(&relay_token.to_le_bytes());
                            wire.extend_from_slice(&rtp_pkt);
                            send_dgram(tsock, &wire, target, &mut txs);
                            wl.rtp(">", type_byte, wire.len(), &rtp_addr, &rtp_pkt);
                        } else {
                            send_dgram(tsock, &rtp_pkt, target, &mut txs);
                            wl.rtp(">", rtp_pkt[0], rtp_pkt.len(), &rtp_addr, &rtp_pkt);
                        }
                    } else if let Some(ref fsock) = fallback_sock {
                        let _ = fsock.send_to(&rtp_pkt, &rtp_addr);
                    }
                    seq = seq.wrapping_add(1);
                    ts = ts.wrapping_add(FRAME_SAMPLES as u32);
                }
                Err(e) => eprintln!("media: encode err {e}"),
            }
        }

        // --- Video: synthetic H.264 stream (90 kHz clock, 15 fps) in the Zalo wire format ---
        // One Annex-B access unit per frame: `PT 97` when it fits `RTP_MAX_PAYLOAD`, else `PT 98` fragments
        // (see `packetize_zalo_au`); marker bit only on the last packet of the frame.
        // A PLI/FIR from the peer (RTCP inbox, or RTP socket if the relay mixes them) forces an immediate SPS+PPS+IDR.
        if (is_video || cam_enabled.load(Ordering::Relaxed))
            && last_video_tx.elapsed() >= Duration::from_millis(66)
        {
            last_video_tx = std::time::Instant::now();
            let force_kf = vid_frame_no == 0
                || vid_gop_pos >= H264_PSLICES.len()
                || vid_frame_no % VIDEO_IDR_INTERVAL_FRAMES == 0
                || want_keyframe.swap(false, Ordering::Relaxed);

            // Drain latest live webcam access unit if available
            // Drain latest live webcam access unit if available (or read shared webcam stream for extra legs)
            let mut latest_cam_au: Option<Vec<u8>> = None;
            while let Ok(au) = cam_au_rx.try_recv() {
                if au_has_idr(&au) {
                    last_cam_idr = Some(au.clone());
                }
                latest_cam_au = Some(au);
            }
            if latest_cam_au.is_none() && slot_idx > 0 {
                if let Ok(sh) = SHARED_CAM_H264.lock() {
                    if !sh.2.is_empty() {
                        last_cam_idr = Some(sh.2.clone());
                    }
                    if !sh.1.is_empty() {
                        latest_cam_au = Some(sh.1.clone());
                    }
                }
            }
            let au_bytes: Vec<u8> = if cam_enabled.load(Ordering::Relaxed) {
                if let Some(au) = latest_cam_au {
                    if force_kf && !au_has_idr(&au) {
                        if let Some(cached_idr) = &last_cam_idr {
                            cached_idr.clone()
                        } else {
                            au
                        }
                    } else {
                        au
                    }
                } else if let Some(cached_idr) = &last_cam_idr {
                    if force_kf {
                        cached_idr.clone()
                    } else {
                        let p_nal = H264_PSLICES[vid_gop_pos.min(H264_PSLICES.len() - 1)];
                        vid_gop_pos = (vid_gop_pos + 1) % H264_PSLICES.len();
                        annexb_au(&[p_nal])
                    }
                } else {
                    let idr_nal = if (vid_idr_parity & 1) == 0 {
                        H264_IDR
                    } else {
                        H264_IDR_ALT
                    };
                    let p_nal = H264_PSLICES[vid_gop_pos.min(H264_PSLICES.len() - 1)];
                    if force_kf {
                        vid_idr_parity = vid_idr_parity.wrapping_add(1);
                        vid_gop_pos = 0;
                        annexb_au(&[H264_SPS, H264_PPS, idr_nal])
                    } else {
                        vid_gop_pos += 1;
                        annexb_au(&[p_nal])
                    }
                }
            } else {
                let idr_nal = if (vid_idr_parity & 1) == 0 {
                    H264_IDR
                } else {
                    H264_IDR_ALT
                };
                let p_nal = H264_PSLICES[vid_gop_pos.min(H264_PSLICES.len() - 1)];
                if force_kf {
                    vid_idr_parity = vid_idr_parity.wrapping_add(1);
                    vid_gop_pos = 0;
                    annexb_au(&[H264_SPS, H264_PPS, idr_nal])
                } else {
                    vid_gop_pos += 1;
                    annexb_au(&[p_nal])
                }
            };

            let payloads = packetize_zalo_au(&au_bytes, RTP_MAX_PAYLOAD);
            let last = payloads.len().saturating_sub(1);
            for (i, (vpt, payload)) in payloads.iter().enumerate() {
                let vpkt = build_rtp_packet(*vpt, i == last, vid_seq, vid_ts, vid_ssrc, payload);
                vid_seq = vid_seq.wrapping_add(1);
                vid_pkts_tx = vid_pkts_tx.wrapping_add(1);
                vid_octets_tx = vid_octets_tx.wrapping_add(payload.len() as u32);
                if let (Some(tsock), Some(target)) = (&udp_rtp_sock, rtp_sock_addr) {
                    if relay_token != 0 {
                        let type_byte: u8 = 0x0D; // `ZRTPPacket::initZRTPPacketVideo` (`isRtcp=false`)
                        let mut wire = Vec::with_capacity(5 + vpkt.len());
                        wire.push(type_byte);
                        wire.extend_from_slice(&relay_token.to_le_bytes());
                        wire.extend_from_slice(&vpkt);
                        send_dgram(tsock, &wire, target, &mut txs);
                        wl.rtp(">", type_byte, wire.len(), &rtp_addr, &vpkt);
                    } else {
                        send_dgram(tsock, &vpkt, target, &mut txs);
                        wl.rtp(">", vpkt[0], vpkt.len(), &rtp_addr, &vpkt);
                    }
                }
            }
            vid_frame_no = vid_frame_no.wrapping_add(1);
            vid_ts = vid_ts.wrapping_add(6000); // 90000 Hz / 15 fps
        }
        if last_rtcp.elapsed() >= Duration::from_secs(2) {
            last_rtcp = std::time::Instant::now();
            if let (Some(ref rtcp_s), Some(target)) = (&udp_rtcp_sock, rtcp_sock_addr) {
                // LSR/DLSR of the peer's last SR (RFC 3550 6.4.1); DLSR is in 1/65536 s.
                let (lsr, dlsr) = match last_peer_sr {
                    Some((m, at)) => (m, (at.elapsed().as_secs_f64() * 65536.0) as u32),
                    None => (0, 0),
                };
                let mut pkts: Vec<(u8, Vec<u8>)> = Vec::new();
                // Audio: RR (`0x05` = `initZRTPPacketAudio` with `isRtcp=true`) from our audio SSRC.
                let mut rr = vec![0x80u8, 201, 0, 1];
                rr.extend_from_slice(&ssrc.to_be_bytes());
                if let Some(b) = rx_audio.block(peer_ssrc, lsr, dlsr) {
                    rr[0] = 0x81;
                    rr[2..4].copy_from_slice(&7u16.to_be_bytes());
                    rr.extend_from_slice(&b);
                }
                pkts.push((0x05, rr));
                // Video: SR + SDES + REMB (`0x0F` = `initZRTPPacketVideo` with `isRtcp=true`) from our video SSRC.
                if is_video || cam_enabled.load(Ordering::Relaxed) {
                    let sr = build_sender_report(
                        vid_ssrc,
                        ntp_now(),
                        vid_ts,
                        vid_pkts_tx,
                        vid_octets_tx,
                        rx_video.block(peer_ssrc, lsr, dlsr),
                    );
                    let mut c = sr;
                    c.extend_from_slice(&build_sdes_empty(vid_ssrc));
                    if peer_ssrc != 0 {
                        c.extend_from_slice(&build_remb(vid_ssrc, peer_ssrc, 1_500_000));
                    }
                    pkts.push((0x0F, c));
                }
                // Also send ZRTP IncallNegotiateInfo (`H264DecCapacityInfo(720, 30)`) on RTP socket
                if (is_video || cam_enabled.load(Ordering::Relaxed)) && relay_token != 0 {
                    if let (Some(tsock), Some(rtarget)) = (&udp_rtp_sock, rtp_sock_addr) {
                        let nego_pkt = build_zrtp_incall_negotiate_packet(
                            partner_uid_u32,
                            relay_token,
                            nego_seq,
                        );
                        nego_seq = nego_seq.wrapping_add(1);
                        send_dgram(tsock, &nego_pkt, rtarget, &mut txs);
                    }
                }
                for (rtcp_type, rtcp_pkt) in pkts {
                    if relay_token != 0 {
                        let mut wire = Vec::with_capacity(5 + rtcp_pkt.len());
                        wire.push(rtcp_type);
                        wire.extend_from_slice(&relay_token.to_le_bytes());
                        wire.extend_from_slice(&rtcp_pkt);
                        send_dgram(rtcp_s, &wire, target, &mut txs);
                        wl.rtcp(
                            ">",
                            crate::voip_log::Via::RtcpSock,
                            rtcp_type,
                            wire.len(),
                            &rtcp_addr,
                            &rtcp_pkt,
                        );
                    } else {
                        send_dgram(rtcp_s, &rtcp_pkt, target, &mut txs);
                        wl.rtcp(
                            ">",
                            crate::voip_log::Via::RtcpSock,
                            rtcp_pkt[0],
                            rtcp_pkt.len(),
                            &rtcp_addr,
                            &rtcp_pkt,
                        );
                    }
                }
            }
        }

        // --- RTCP from the keepalive task's socket reader: PLI / FIR -> force a keyframe ---
        // --- Drain datagrams from the RTCP socket inbox + up to 64 datagrams from the RTP socket ---
        let mut incoming_dgrams: Vec<(crate::voip_log::Via, String, Vec<u8>)> = drain_rtcp_inbox()
            .into_iter()
            .map(|w| (crate::voip_log::Via::RtcpSock, rtcp_addr.clone(), w))
            .collect();
        let mut buf = [0u8; 4000];
        for _ in 0..64 {
            let recv_info = if let Some(tsock) = &udp_rtp_sock {
                tsock.try_recv_from(&mut buf).ok()
            } else if let Some(fsock) = &fallback_sock {
                fsock.recv_from(&mut buf).ok()
            } else {
                None
            };
            let Some((n, peer)) = recv_info else {
                break;
            };
            incoming_dgrams.push((
                crate::voip_log::Via::RtpSock,
                peer.to_string(),
                buf[..n].to_vec(),
            ));
        }
        for (via, peer_s, raw_pkt) in incoming_dgrams {
            let n = raw_pkt.len();
            if unwrap_zalo_relay_rtp(&raw_pkt).is_none() {
                wl.unknown(via, n, &peer_s, &raw_pkt);
            }
            if let Some(inner_rtp) = unwrap_zalo_relay_rtp(&raw_pkt) {
                // RTCP (RFC 5761 range 200..=207): react to PLI / FIR by forcing a keyframe.
                if matches!(inner_rtp.get(1), Some(b) if (200..=207).contains(b)) {
                    if let Some(m) = sr_middle_ntp(inner_rtp) {
                        last_peer_sr = Some((m, std::time::Instant::now()));
                    }
                    if wl.rtcp("<", via, raw_pkt[0], n, &peer_s, inner_rtp) {
                        want_keyframe.store(true, Ordering::Relaxed);
                        crate::call::trace_line(
                            "RTCP<",
                            format!("from={peer_s} PLI/FIR -> force IDR"),
                        );
                    }
                    continue;
                }
                let pt = inner_rtp.get(1).map(|b| b & 0x7F).unwrap_or(0);
                wl.rtp("<", raw_pkt[0], n, &peer_s, inner_rtp);
                if let Some(h) = crate::voip_log::parse_rtp(inner_rtp) {
                    if pt == 112 {
                        rx_audio.update(h.seq);
                        peer_ssrc = h.ssrc;
                    } else if pt == 96 || pt == 97 || pt == VIDEO_PT_FRAG {
                        if rx_video.update(h.seq) {
                            pli_gap = true; // hole in the peer's video: ask for a keyframe
                        }
                        peer_ssrc = h.ssrc;
                    }
                }
                static LOGGED_RX: std::sync::atomic::AtomicUsize =
                    std::sync::atomic::AtomicUsize::new(0);
                let cnt = LOGGED_RX.fetch_add(1, Ordering::Relaxed);
                if cnt < 16 || cnt % 100 == 0 {
                    crate::call::trace_line(
                        "RTP<",
                        format!("from={peer_s} len={n} outer=0x{:02x} pt={pt}", raw_pkt[0]),
                    );
                }
                if pt == 112 {
                    if let Some(payload) = extract_rtp_opus_payload(inner_rtp) {
                        let mut pcm = vec![0f32; FRAME_SAMPLES];
                        if decoder.decode_float(payload, &mut pcm, false).is_ok() {
                            push_peer_bridge_audio(slot_idx, &pcm);
                            if let Ok(mut b) = output_buf.lock() {
                                b.extend_from_slice(&pcm);
                            }
                        }
                    }
                } else if pt == 96 || pt == 97 || pt == VIDEO_PT_FRAG || raw_pkt[0] == 0x0E {
                    if let Some(payload) = extract_rtp_opus_payload(inner_rtp) {
                        depacketize_h264(payload, &mut fu_a_buf, &h264_tx);
                    }
                }
            }
        }
        wl.tick();
        // --- PLI towards the peer: its video is undecodable until a keyframe (SPS/IDR) reaches the decoder ---
        // Live log (call at 18:28 UTC): the peer sent only P frames, no SPS/IDR ever arrived, ffmpeg printed
        // "non-existing PPS 0 referenced" for all of them and decoded 0 frames; we never sent a PLI back.
        if is_video
            && decoder_active
            && rx_video.started
            && peer_ssrc != 0
            && (!video_synced.load(Ordering::Relaxed) || pli_gap)
            && last_pli.elapsed() >= Duration::from_millis(500)
        {
            last_pli = std::time::Instant::now();
            pli_gap = false;
            if let (Some(ref rtcp_s), Some(target)) = (&udp_rtcp_sock, rtcp_sock_addr) {
                // Compound packet: empty RR first (RFC 3550 6.1), then the PLI.
                let mut pkt = vec![0x80u8, 201, 0, 1];
                pkt.extend_from_slice(&ssrc.to_be_bytes());
                pkt.extend_from_slice(&build_pli(ssrc, peer_ssrc));
                if relay_token != 0 {
                    let rtcp_type: u8 = 0x0F; // `ZRTPPacket::initZRTPPacketVideo` (`isRtcp=true`)
                    let mut wire = Vec::with_capacity(5 + pkt.len());
                    wire.push(rtcp_type);
                    wire.extend_from_slice(&relay_token.to_le_bytes());
                    wire.extend_from_slice(&pkt);
                    send_dgram(rtcp_s, &wire, target, &mut txs);
                    wl.rtcp(
                        ">",
                        crate::voip_log::Via::RtcpSock,
                        rtcp_type,
                        wire.len(),
                        &rtcp_addr,
                        &pkt,
                    );
                } else {
                    send_dgram(rtcp_s, &pkt, target, &mut txs);
                    wl.rtcp(
                        ">",
                        crate::voip_log::Via::RtcpSock,
                        pkt[0],
                        pkt.len(),
                        &rtcp_addr,
                        &pkt,
                    );
                }
                pli_sent += 1;
            }
        }
        // --- send-side health: result counters of every UDP send + longest stall of this loop ---
        let gap = last_iter.elapsed();
        last_iter = std::time::Instant::now();
        if gap > max_loop_gap {
            max_loop_gap = gap;
        }
        if last_txst.elapsed() >= Duration::from_secs(5) {
            last_txst = std::time::Instant::now();
            crate::voip_log::line(
                "TXST",
                format!(
                    "sent_ok={} would_block={} short={} err={} last_err={} max_loop_gap_ms={} pli_sent={} video_synced={}",
                    txs.ok,
                    txs.would_block,
                    txs.short,
                    txs.err,
                    txs.last_err.as_deref().unwrap_or("-"),
                    max_loop_gap.as_millis(),
                    pli_sent,
                    video_synced.load(Ordering::Relaxed)
                ),
            );
            max_loop_gap = Duration::ZERO;
        }
        thread::sleep(Duration::from_millis(5));
    }
    crate::voip_log::line("CALL", "media stop");

    drop(input_stream);
    drop(output_stream);
}

/// Reassemble RFC 6184 H.264 RTP payloads (Single NAL 1..23, STAP-A 24, FU-A 28) into Annex-B (`00 00 00 01`) byte stream.
fn depacketize_rfc6184_h264(
    payload: &[u8],
    fu_a_buf: &mut Vec<u8>,
    h264_tx: &std::sync::mpsc::SyncSender<Vec<u8>>,
) {
    if payload.is_empty() {
        return;
    }
    let nal_type = payload[0] & 0x1F;
    match nal_type {
        1..=23 => {
            let mut annexb = Vec::with_capacity(4 + payload.len());
            annexb.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
            annexb.extend_from_slice(payload);
            let _ = h264_tx.try_send(annexb);
        }
        24 => {
            // STAP-A: multiple NAL units prefixed by u16 BE length
            let mut idx = 1usize;
            let mut annexb = Vec::new();
            while idx + 2 <= payload.len() {
                let sz = u16::from_be_bytes([payload[idx], payload[idx + 1]]) as usize;
                idx += 2;
                if sz > 0 && idx + sz <= payload.len() {
                    annexb.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
                    annexb.extend_from_slice(&payload[idx..idx + sz]);
                    idx += sz;
                } else {
                    break;
                }
            }
            if !annexb.is_empty() {
                let _ = h264_tx.try_send(annexb);
            }
        }
        28 if payload.len() >= 2 => {
            // FU-A fragmented NAL unit
            let fu_ind = payload[0];
            let fu_hdr = payload[1];
            let start_bit = (fu_hdr & 0x80) != 0;
            let end_bit = (fu_hdr & 0x40) != 0;
            let orig_nal_hdr = (fu_ind & 0xE0) | (fu_hdr & 0x1F);
            if start_bit {
                fu_a_buf.clear();
                fu_a_buf.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, orig_nal_hdr]);
                fu_a_buf.extend_from_slice(&payload[2..]);
            } else if !fu_a_buf.is_empty() {
                fu_a_buf.extend_from_slice(&payload[2..]);
                if end_bit {
                    let completed = std::mem::take(fu_a_buf);
                    let _ = h264_tx.try_send(completed);
                }
            }
        }
        _ => {}
    }
}

/// Access Unit Delimiter NAL (`primary_pic_type=7`), written before every access unit sent to ffmpeg.
const H264_AUD: &[u8] = &[0, 0, 0, 1, 0x09, 0xf0];

/// Spawn a headless `ffmpeg` pipe to decode incoming H.264 Annex-B NAL units into `480×360` RGBA frames.
/// Trim black letterbox/pillarbox borders added by `ffmpeg pad` so the returned RGBA buffer
/// has the exact aspect ratio of the active video content with zero black bars.
fn trim_black_borders(rgba: &[u8], w: usize, h: usize) -> (usize, usize, Vec<u8>) {
    if rgba.len() < w * h * 4 || w < 16 || h < 16 {
        return (w, h, rgba.to_vec());
    }
    let is_non_black = |x: usize, y: usize| -> bool {
        let idx = (y * w + x) * 4;
        rgba[idx] > 4 || rgba[idx + 1] > 4 || rgba[idx + 2] > 4
    };
    let mid_y = h / 2;
    let y1 = h / 4;
    let y2 = (h * 3) / 4;
    let mut left = 0usize;
    while left < w / 2
        && !is_non_black(left, mid_y)
        && !is_non_black(left, y1)
        && !is_non_black(left, y2)
    {
        left += 1;
    }
    let mut right = w;
    while right > w / 2
        && !is_non_black(right - 1, mid_y)
        && !is_non_black(right - 1, y1)
        && !is_non_black(right - 1, y2)
    {
        right -= 1;
    }
    let mid_x = w / 2;
    let x1 = w / 4;
    let x2 = (w * 3) / 4;
    let mut top = 0usize;
    while top < h / 2
        && !is_non_black(mid_x, top)
        && !is_non_black(x1, top)
        && !is_non_black(x2, top)
    {
        top += 1;
    }
    let mut bottom = h;
    while bottom > h / 2
        && !is_non_black(mid_x, bottom - 1)
        && !is_non_black(x1, bottom - 1)
        && !is_non_black(x2, bottom - 1)
    {
        bottom -= 1;
    }
    let sym_x = left.min(w.saturating_sub(right));
    let sym_y = top.min(h.saturating_sub(bottom));
    if sym_x < 4 && sym_y < 4 {
        return (w, h, rgba.to_vec());
    }
    let cw = w.saturating_sub(sym_x * 2).max(16);
    let ch = h.saturating_sub(sym_y * 2).max(16);
    let mut out = Vec::with_capacity(cw * ch * 4);
    for y in sym_y..(sym_y + ch) {
        let row_start = (y * w + sym_x) * 4;
        let row_end = row_start + cw * 4;
        out.extend_from_slice(&rgba[row_start..row_end]);
    }
    (cw, ch, out)
}

fn run_h264_ffmpeg_decoder(
    stop: Arc<AtomicBool>,
    h264_rx: std::sync::mpsc::Receiver<Vec<u8>>,
    gui_tx: std::sync::mpsc::Sender<crate::gui::GuiEvent>,
    egui_ctx: eframe::egui::Context,
    synced: Arc<AtomicBool>,
    tex_key: String,
) {
    use std::io::{Read, Write};
    use std::process::{Command, Stdio};

    const VID_W: usize = 640;
    const VID_H: usize = 480;
    const FRAME_BYTES: usize = VID_W * VID_H * 4;

    let mut cmd = Command::new("ffmpeg");
    cmd.args([
        "-hide_banner",
        "-loglevel",
        "error",
        // NOTE: no `-fflags nobuffer` here. Measured with a synthetic stream pushed through a pipe,
        // `nobuffer` made ffmpeg wait for ~33 access units before the first frame, which is more
        // than the ~22 the peer sent in call 5 (0 frames). Without it, plus an AUD before every AU
        // (see `H264_AUD`), the first frame appears at AU #2 (see re-notes/08_video_call.md).
        "-flags",
        "low_delay",
        // Without these ffmpeg probes ~5 MB / 5 s of stream before the first frame; with 15 fps video
        // that is ~3.8 s measured vs ~1.0 s (synthetic stream, see re-notes/08_video_call.md).
        "-probesize",
        "32",
        "-analyzeduration",
        "0",
        "-f",
        "h264",
        "-i",
        "pipe:0",
        "-vf",
        "scale=640:480:force_original_aspect_ratio=decrease:flags=lanczos,pad=640:480:(ow-iw)/2:(oh-ih)/2:black",
        "-pix_fmt",
        "rgba",
        "-f",
        "rawvideo",
        "pipe:1",
    ])
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            crate::voip_log::line("VDEC", format!("ffmpeg spawn failed: {e}"));
            return;
        }
    };
    crate::voip_log::line("VDEC", "ffmpeg decoder started");
    let Some(mut stdin) = child.stdin.take() else {
        let _ = child.kill();
        return;
    };
    let Some(mut stdout) = child.stdout.take() else {
        let _ = child.kill();
        return;
    };
    if let Some(mut stderr) = child.stderr.take() {
        // ffmpeg diagnostics (decode errors, missing reference frames) -> voip.log, bounded.
        thread::spawn(move || {
            let mut buf = [0u8; 512];
            let mut lines = 0u32;
            while let Ok(n) = stderr.read(&mut buf) {
                if n == 0 {
                    break;
                }
                lines += 1;
                if lines <= 20 {
                    crate::voip_log::line(
                        "VDEC",
                        format!("ffmpeg: {}", String::from_utf8_lossy(&buf[..n]).trim()),
                    );
                }
            }
        });
    }

    let stop_writer = stop.clone();
    thread::spawn(move || {
        let mut fed = 0u64;
        let mut dropped = 0u64;
        while !stop_writer.load(Ordering::Relaxed) {
            match h264_rx.recv_timeout(Duration::from_millis(100)) {
                Ok(nal) => {
                    // Until the first SPS/IDR access unit there is no PPS: ffmpeg would only print
                    // "non-existing PPS 0 referenced" for every P frame (seen in the live log), so hold them back.
                    if !synced.load(Ordering::Relaxed) {
                        if au_is_sync_point(&nal) {
                            synced.store(true, Ordering::Relaxed);
                            crate::voip_log::line(
                                "VDEC",
                                format!("first SPS/IDR access unit after {dropped} undecodable ones ({} bytes)", nal.len()),
                            );
                        } else {
                            dropped += 1;
                            if dropped == 1 || dropped % 100 == 0 {
                                crate::voip_log::line(
                                    "VDEC",
                                    format!(
                                        "waiting for SPS/IDR, dropped {dropped} P access units"
                                    ),
                                );
                            }
                            continue;
                        }
                    }
                    // The AUD marks the access-unit boundary so the parser can emit the previous
                    // frame immediately instead of waiting for the next slice header.
                    if stdin.write_all(H264_AUD).is_err() || stdin.write_all(&nal).is_err() {
                        crate::voip_log::line(
                            "VDEC",
                            format!("ffmpeg stdin closed after {fed} access units"),
                        );
                        break;
                    }
                    let _ = stdin.flush();
                    fed += 1;
                    if fed == 1 || fed % 100 == 0 {
                        crate::voip_log::line("VDEC", format!("fed {fed} access units to ffmpeg"));
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    });

    let mut raw_frame = vec![0u8; FRAME_BYTES];
    let mut decoded = 0u64;
    while !stop.load(Ordering::Relaxed) {
        if stdout.read_exact(&mut raw_frame).is_ok() {
            decoded += 1;
            if decoded == 1 || decoded % 100 == 0 {
                crate::voip_log::line("VDEC", format!("decoded frame #{decoded}"));
            }
            let (cw, ch, cropped) = trim_black_borders(&raw_frame, VID_W, VID_H);
            let color_img = eframe::egui::ColorImage::from_rgba_unmultiplied([cw, ch], &cropped);
            let _ = gui_tx.send(crate::gui::GuiEvent::StickerImageReady {
                key: tex_key.clone(),
                image: color_img,
            });
            egui_ctx.request_repaint();
        } else {
            break;
        }
    }
    crate::voip_log::line("VDEC", format!("decoder stop, {decoded} frames decoded"));
    let _ = child.kill();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unwrap_zalo_relay_rtp_all_formats() {
        let mut sample_rtp = vec![
            0x80, 112, 0x00, 0x01, 0x00, 0x00, 0x01, 0x40, 0x12, 0x34, 0x56, 0x78,
        ];
        sample_rtp.extend_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD]);

        // 1. Raw RTP (0x80)
        let r1 = unwrap_zalo_relay_rtp(&sample_rtp).expect("raw rtp");
        assert_eq!(
            extract_rtp_opus_payload(r1),
            Some(&[0xAA, 0xBB, 0xCC, 0xDD][..])
        );

        // 2. 1-byte relay header (0x04 incoming RTP)
        let mut p4 = vec![0x04];
        p4.extend_from_slice(&sample_rtp);
        let r2 = unwrap_zalo_relay_rtp(&p4).expect("0x04 relay rtp");
        assert_eq!(
            extract_rtp_opus_payload(r2),
            Some(&[0xAA, 0xBB, 0xCC, 0xDD][..])
        );

        // 3. 5-byte relay header (0x03 caller / 0x05 callee + 4-byte token LE)
        let mut p5 = vec![0x03, 0x78, 0x56, 0x34, 0x12];
        p5.extend_from_slice(&sample_rtp);
        let r3 = unwrap_zalo_relay_rtp(&p5).expect("0x03 relay rtp");
        assert_eq!(
            extract_rtp_opus_payload(r3),
            Some(&[0xAA, 0xBB, 0xCC, 0xDD][..])
        );

        // 4. 9-byte 0x7F relay header (cmd = 6)
        let mut p9 = vec![0x7F, 0x00, 0x00, 0x11, 0x22, 0x33, 0x44, 0x06, 0x00];
        p9.extend_from_slice(&sample_rtp);
        let r4 = unwrap_zalo_relay_rtp(&p9).expect("0x7F relay rtp");
        assert_eq!(
            extract_rtp_opus_payload(r4),
            Some(&[0xAA, 0xBB, 0xCC, 0xDD][..])
        );
    }

    #[test]
    fn fu_a_roundtrip_and_marker_layout() {
        let mut nal = vec![0x65u8];
        nal.extend((0..3000u32).map(|i| (i % 251) as u8));
        let parts = packetize_h264_nal(&nal, 1200);
        assert!(parts.len() >= 3);
        assert!(parts.iter().all(|p| p.len() <= 1200));
        assert_eq!(parts[0][0], (0x65 & 0xE0) | 28);
        assert_eq!(parts[0][1], 0x80 | 5);
        assert_eq!(parts.last().unwrap()[1], 0x40 | 5);
        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(16);
        let mut buf = Vec::new();
        for p in &parts {
            depacketize_rfc6184_h264(p, &mut buf, &tx);
        }
        let out = rx.try_recv().expect("reassembled NAL");
        let mut expect = vec![0, 0, 0, 1];
        expect.extend_from_slice(&nal);
        assert_eq!(out, expect);
    }

    #[test]
    fn small_nal_is_single_packet() {
        let parts = packetize_h264_nal(H264_PPS, 1200);
        assert_eq!(parts, vec![H264_PPS.to_vec()]);
    }

    #[test]
    fn detects_pli_and_fir_in_compound_rtcp() {
        // RR (len=1) followed by PLI (fmt 1, len=2)
        let mut c = vec![0x80, 201, 0, 1, 0, 0, 0, 1];
        c.extend_from_slice(&[0x81, 206, 0, 2, 0, 0, 0, 1, 0, 0, 0, 2]);
        assert!(rtcp_requests_keyframe(&c));
        let fir = [
            0x84, 206, 0, 4, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2, 1, 0, 0, 0,
        ];
        assert!(rtcp_requests_keyframe(&fir));
        let rr = [0x80, 201, 0, 1, 0, 0, 0, 1];
        assert!(!rtcp_requests_keyframe(&rr));
        // NACK (RTPFB 205) must not trigger a keyframe
        let nack = [0x81, 205, 0, 3, 0, 0, 0, 1, 0, 0, 0, 2, 0, 5, 0, 0];
        assert!(!rtcp_requests_keyframe(&nack));
    }

    #[test]
    fn rtp_marker_bit_only_when_requested() {
        let a = build_rtp_packet(97, true, 1, 2, 3, &[9]);
        assert_eq!(a[1], 0x80 | 97);
        let b = build_rtp_packet(97, false, 1, 2, 3, &[9]);
        assert_eq!(b[1], 97);
        assert_eq!(b.len(), 13);
    }

    #[test]
    fn wire_pli_through_relay_header_forces_keyframe() {
        let pli = [0x81u8, 206, 0, 2, 0, 0, 0, 1, 0, 0, 0, 2];
        let mut w = vec![0x0D, 0x78, 0x56, 0x34, 0x12];
        w.extend_from_slice(&pli);
        assert!(wire_requests_keyframe(&w));
        let mut raw = Vec::new();
        raw.extend_from_slice(&pli);
        assert!(wire_requests_keyframe(&raw));
        // RTP video packet and ZRTP-like junk must not trigger
        let rtp = build_rtp_packet(97, true, 1, 2, 3, &[0x41, 0x9A]);
        assert!(!wire_requests_keyframe(&rtp));
        assert!(!wire_requests_keyframe(&[0x00, 0x0B, 1, 2, 3]));
    }

    #[test]
    fn rtcp_inbox_is_bounded_and_drains() {
        clear_rtcp_inbox();
        for i in 0..(RTCP_INBOX_MAX + 10) {
            push_rtcp_inbox(&[i as u8]);
        }
        let v = drain_rtcp_inbox();
        assert_eq!(v.len(), RTCP_INBOX_MAX);
        assert_eq!(v[0], vec![10u8]);
        assert!(drain_rtcp_inbox().is_empty());
    }

    fn au_of(len: usize) -> Vec<u8> {
        let mut au = annexb_au(&[H264_SPS, H264_PPS]);
        au.extend_from_slice(&[0, 0, 0, 1, 0x65]);
        au.extend((0..len as u32).map(|i| (i % 249) as u8 | 1));
        au
    }

    #[test]
    fn small_au_uses_pt97_for_p_and_pt98_for_idr() {
        let p_au = annexb_au(&[H264_PSLICE]);
        assert_eq!(
            packetize_zalo_au(&p_au, RTP_MAX_PAYLOAD),
            vec![(VIDEO_PT, p_au)]
        );
        let idr_au = annexb_au(&[H264_SPS, H264_PPS, H264_IDR]);
        assert_eq!(
            packetize_zalo_au(&idr_au, RTP_MAX_PAYLOAD),
            vec![(VIDEO_PT_FRAG, idr_au)]
        );
    }

    #[test]
    fn large_au_fragments_like_the_peer_and_roundtrips() {
        let au = au_of(2800);
        let parts = packetize_zalo_au(&au, RTP_MAX_PAYLOAD);
        assert!(parts.len() >= 3);
        // keyframe (has an IDR slice) -> PT 98 fragments, each within the payload limit
        assert!(parts
            .iter()
            .all(|(pt, p)| *pt == VIDEO_PT_FRAG && p.len() <= RTP_MAX_PAYLOAD));
        // header bytes exactly as captured: 1c 80 / 1c 00 / 1c 40
        assert_eq!(&parts[0].1[..2], &[0x1C, 0x80]);
        assert_eq!(&parts[1].1[..2], &[0x1C, 0x00]);
        assert_eq!(&parts.last().unwrap().1[..2], &[0x1C, 0x40]);
        // even split: fragment sizes differ by at most 1 except possibly the last (peer: 1083/1083, 1062x3)
        let sizes: Vec<usize> = parts.iter().map(|(_, p)| p.len()).collect();
        let (mx, mn) = (sizes.iter().max().unwrap(), sizes.iter().min().unwrap());
        assert!(mx - mn <= 2, "uneven split {sizes:?}");
        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(16);
        let mut buf = Vec::new();
        for (_, p) in &parts {
            depacketize_h264(p, &mut buf, &tx);
        }
        assert_eq!(rx.try_recv().expect("reassembled AU"), au);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn peer_capture_shapes_are_decoded() {
        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(16);
        let mut buf = Vec::new();
        // PT 97 payload as captured: whole AU, Annex-B, NAL 0x21
        let whole = [0, 0, 0, 1, 0x21, 0xE1, 0x04, 0x06, 0x38];
        depacketize_h264(&whole, &mut buf, &tx);
        assert_eq!(rx.try_recv().unwrap(), whole.to_vec());
        // PT 98 fragments as captured: 1c 80 00 00 00 01 27 42 ..., 1c 00 ..., 1c 40 ...
        depacketize_h264(&[0x1C, 0x80, 0, 0, 0, 1, 0x27, 0x42, 0x00], &mut buf, &tx);
        depacketize_h264(&[0x1C, 0x00, 0x06, 0xF1], &mut buf, &tx);
        assert!(rx.try_recv().is_err());
        depacketize_h264(&[0x1C, 0x40, 0xDE, 0x3B], &mut buf, &tx);
        assert_eq!(
            rx.try_recv().unwrap(),
            vec![0, 0, 0, 1, 0x27, 0x42, 0x00, 0x06, 0xF1, 0xDE, 0x3B]
        );
    }

    #[test]
    fn lost_first_fragment_drops_the_access_unit() {
        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(16);
        let mut buf = Vec::new();
        depacketize_h264(&[0x1C, 0x00, 1, 2], &mut buf, &tx);
        depacketize_h264(&[0x1C, 0x40, 3, 4], &mut buf, &tx);
        assert!(rx.try_recv().is_err());
        // a new access unit still works afterwards
        depacketize_h264(&[0x1C, 0xC0, 0, 0, 0, 1, 0x41], &mut buf, &tx);
        assert_eq!(rx.try_recv().unwrap(), vec![0, 0, 0, 1, 0x41]);
    }

    #[test]
    fn rfc6184_still_accepted_on_receive() {
        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(16);
        let mut buf = Vec::new();
        depacketize_h264(&[0x41, 0x9A, 0x24], &mut buf, &tx);
        assert_eq!(rx.try_recv().unwrap(), vec![0, 0, 0, 1, 0x41, 0x9A, 0x24]);
    }

    #[test]
    fn p_frame_fragments_use_pt97_and_split_evenly() {
        // non-IDR access unit larger than one packet: fragments, but PT 97 (seen live)
        let mut au = vec![0, 0, 0, 1, 0x21];
        au.extend((0..3000u32).map(|i| (i % 249) as u8 | 1));
        let parts = packetize_zalo_au(&au, RTP_MAX_PAYLOAD);
        assert_eq!(parts.len(), 3);
        assert!(parts.iter().all(|(pt, _)| *pt == VIDEO_PT));
        assert_eq!(parts[0].1[..2], [0x1C, 0x80]);
        assert_eq!(parts[2].1[..2], [0x1C, 0x40]);
        let sizes: Vec<usize> = parts.iter().map(|(_, p)| p.len()).collect();
        assert!(
            sizes.iter().max().unwrap() - sizes.iter().min().unwrap() <= 2,
            "{sizes:?}"
        );
        // AU exactly at the limit stays whole
        let whole = vec![0u8; RTP_MAX_PAYLOAD];
        assert_eq!(packetize_zalo_au(&whole, RTP_MAX_PAYLOAD).len(), 1);
    }

    #[test]
    fn detects_idr_slice_in_annexb() {
        assert!(au_has_idr(&annexb_au(&[H264_SPS, H264_PPS, H264_IDR])));
        assert!(!au_has_idr(&annexb_au(&[H264_PSLICE])));
        assert!(!au_has_idr(&[]));
        // a short trailing NAL (4 bytes after the start code) must still be seen
        assert!(au_has_idr(&[0, 0, 0, 1, 0x65, 1, 2]));
    }

    #[test]
    fn sync_point_needs_sps_or_idr() {
        assert!(au_is_sync_point(&annexb_au(&[
            H264_SPS, H264_PPS, H264_IDR
        ])));
        assert!(au_is_sync_point(&annexb_au(&[H264_IDR])));
        assert!(!au_is_sync_point(&annexb_au(&[H264_PSLICE])));
    }

    #[test]
    fn pli_has_rtcp_shape_and_is_detected() {
        let pli = build_pli(0x0102_0304, 0x0A0B_0C0D);
        assert_eq!(pli.len(), 12);
        assert_eq!(&pli[..4], &[0x81, 206, 0, 2]);
        assert_eq!(&pli[8..], &0x0A0B_0C0Du32.to_be_bytes());
        assert!(rtcp_requests_keyframe(&pli));
        let mut compound = vec![0x80u8, 201, 0, 1, 1, 2, 3, 4];
        compound.extend_from_slice(&pli);
        assert!(rtcp_requests_keyframe(&compound));
    }

    #[test]
    fn rx_stats_update_reports_gaps() {
        let mut s = RxStats::default();
        assert!(!s.update(100)); // first packet
        assert!(!s.update(101));
        assert!(s.update(104)); // 102, 103 missing
        assert!(!s.update(103)); // late/reordered packet is not a new gap
        assert!(!s.update(105));
    }

    #[test]
    fn rx_stats_counts_loss_and_wraparound() {
        let mut s = RxStats::default();
        assert!(s.block(1, 0, 0).is_none());
        for seq in [10u16, 11, 12, 15, 16] {
            s.update(seq); // 13, 14 missing
        }
        let b = s.block(0xAABBCCDD, 0x1234, 7).unwrap();
        assert_eq!(&b[0..4], &0xAABBCCDDu32.to_be_bytes());
        assert_eq!(&b[5..8], &[0, 0, 2]); // cumulative lost = 2
        assert_eq!(u32::from_be_bytes([b[8], b[9], b[10], b[11]]), 16); // highest seq
        assert_eq!(b[4], 73); // fraction: 2 lost of 7 expected = 73/256
        assert_eq!(&b[16..20], &0x1234u32.to_be_bytes());
        assert_eq!(&b[20..24], &7u32.to_be_bytes());
        // sequence wrap
        let mut w = RxStats::default();
        for seq in [65534u16, 65535, 0, 1] {
            w.update(seq);
        }
        let b = w.block(1, 0, 0).unwrap();
        assert_eq!(u32::from_be_bytes([b[8], b[9], b[10], b[11]]), 65537);
        assert_eq!(&b[5..8], &[0, 0, 0]);
    }

    #[test]
    fn sender_report_has_peer_shape_and_parses() {
        let blk = RxStats {
            started: true,
            base_seq: 1,
            max_seq: 1,
            received: 1,
            ..Default::default()
        }
        .block(9, 0, 0);
        let sr = build_sender_report(0x1111_2222, (0xAABB_CCDD, 0x1122_3344), 90000, 5, 600, blk);
        assert_eq!(sr.len(), 52); // same length as the peer's SR (len=52)
        assert_eq!(&sr[..4], &[0x81, 200, 0, 12]);
        let sdes = build_sdes_empty(0x1111_2222);
        assert_eq!(sdes.len(), 12); // same as the peer's SDES (len=12)
        let mut compound = sr.clone();
        compound.extend_from_slice(&sdes);
        assert_eq!(sr_middle_ntp(&compound), Some(0xCCDD_1122));
        assert!(!rtcp_requests_keyframe(&compound));
        let (d, kf) = crate::voip_log::describe_rtcp(&compound);
        assert!(!kf && d.starts_with("SR(") && d.contains("+SDES("), "{d}");
        // no report block -> RC 0, 28 bytes
        let bare = build_sender_report(1, (0, 0), 0, 0, 0, None);
        assert_eq!((bare.len(), bare[0], bare[3]), (28, 0x80, 6));
    }

    #[test]
    fn remb_and_incall_negotiate_packets_have_valid_wire_format() {
        let remb = build_remb(0x19f5f926, 0x1bba1962, 1_500_000);
        assert_eq!(remb.len(), 24);
        assert_eq!(&remb[0..4], &[0x8F, 206, 0, 5]);
        assert_eq!(&remb[12..16], b"REMB");
        assert_eq!(remb[16], 1);
        let exp = remb[17] >> 2;
        let mant =
            (((remb[17] & 0x03) as u32) << 16) | (u16::from_be_bytes([remb[18], remb[19]]) as u32);
        let decoded_bps = mant << exp;
        assert!(
            (1_490_000..=1_500_000).contains(&decoded_bps),
            "decoded={decoded_bps}"
        );
        assert_eq!(&remb[20..24], &0x1bba1962u32.to_be_bytes());

        let nego = build_zrtp_incall_negotiate_packet(0x1bba1962, 0x3955e3d2, 7);
        assert_eq!(nego.len(), 36);
        assert_eq!(&nego[0..2], &[0x01, 0x01]);
        assert_eq!(u16::from_le_bytes([nego[18], nego[19]]), 0x0020);
        assert_eq!(nego[20], 0x14);
        assert_eq!(
            u32::from_le_bytes([nego[21], nego[22], nego[23], nego[24]]),
            11
        );
        assert_eq!(
            &nego[25..36],
            &[0, 0, 0, 7, 2, 0x0B, 0x02, 0xD0, 0x1E, 0x29, 0x01]
        );

        let mut fake_stream = H264_AUD.to_vec();
        fake_stream.extend_from_slice(&[0, 0, 0, 1, 0x67, 1, 2, 3]);
        let second_offset = fake_stream.len();
        fake_stream.extend_from_slice(H264_AUD);
        fake_stream.extend_from_slice(&[0, 0, 0, 1, 0x41, 9, 9]);
        assert_eq!(find_aud_delimiter(&fake_stream, 6), Some(second_offset));
        assert_eq!(
            strip_leading_aud(&fake_stream[..second_offset]),
            vec![0, 0, 0, 1, 0x67, 1, 2, 3]
        );
        // Pillarboxed 64x48 with 16px black bars on left and right -> crops to 32x48
        let mut padded = vec![0u8; 64 * 48 * 4];
        for y in 0..48 {
            for x in 16..48 {
                let i = (y * 64 + x) * 4;
                padded[i] = 120;
                padded[i + 1] = 140;
                padded[i + 2] = 160;
                padded[i + 3] = 255;
            }
        }
        let (cw, ch, cropped) = trim_black_borders(&padded, 64, 48);
        assert_eq!((cw, ch, cropped.len()), (32, 48, 32 * 48 * 4));
    }
}
