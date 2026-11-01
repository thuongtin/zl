//! Detailed RTP / RTCP wire log for real video-call captures (`voip.log`).
//!
//! Goal: settle the open questions from `re-notes/08_video_call.md` with data from a real call:
//! the video payload type, the effective MTU, and whether PLI / FIR actually arrive (and on which socket).
//!
//! Format: plain UTF-8 text, one event per line: `YYYY-MM-DD HH:MM:SS.mmm<TAB>TAG key=value ...` (UTC).
//! This is **not** the substitution-encoded `voip.log` that `ZaloCall.exe` writes, and the default path is
//! `./voip.log` (next to `call_trace.log`), so it never touches `%APPDATA%\ZaloData\cal\voip.log`.
//! Override with `ZALO_VOIP_LOG=<path>`; set `ZALO_VOIP_LOG=off` to disable. The file rotates at 2 MiB
//! (`voip.log` -> `voip.log.1`).
//!
//! Tags: `RTP>` / `RTP<` (per packet, rate-limited), `RTCP>` / `RTCP<` (every packet), `UNK<` (datagram that
//! `unwrap_zalo_relay_rtp` rejects), `STAT` (periodic per stream summary, includes the MTU hint), `SIG` (remote
//! codec / extendData from signaling).
//!
//! Content: headers, sizes, sequence numbers, a short hex prefix and `extendData`/`codec` JSON (addresses and ids
//! already present in `call_trace.log`). Media payloads are never logged beyond the first 16 bytes. Do not commit
//! the file.

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const ROTATE_BYTES: u64 = 2 * 1024 * 1024;
/// Detailed per-packet lines per (direction, payload type) before only the summary remains.
pub const DETAIL_FIRST: u32 = 32;
/// After `DETAIL_FIRST`, log one packet in this many.
pub const DETAIL_EVERY: u32 = 200;
/// Stop writing RTCP lines after this many (stays well below the rotation size).
const RTCP_MAX_LINES: u32 = 2000;
pub const STAT_INTERVAL: Duration = Duration::from_secs(5);

static FILE_LOCK: Mutex<()> = Mutex::new(());

fn log_path() -> Option<String> {
    match std::env::var("ZALO_VOIP_LOG") {
        Ok(v) if v.eq_ignore_ascii_case("off") || v == "0" => None,
        Ok(v) if !v.trim().is_empty() => Some(v),
        _ => Some("voip.log".to_string()),
    }
}

/// UTC timestamp `YYYY-MM-DD HH:MM:SS.mmm` for a Unix time in milliseconds.
pub fn format_ts(unix_ms: u64) -> String {
    let secs = unix_ms / 1000;
    let ms = unix_ms % 1000;
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}.{ms:03}",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Append one event. Failures are swallowed: logging must never break a call.
pub fn line(tag: &str, body: impl AsRef<str>) {
    let Some(path) = log_path() else { return };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let text = format!("{}\t{} {}\n", format_ts(now), tag, body.as_ref());
    let _guard = FILE_LOCK.lock();
    if let Ok(meta) = std::fs::metadata(&path) {
        if meta.len() >= ROTATE_BYTES {
            let _ = std::fs::rename(&path, format!("{path}.1"));
        }
    }
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = f.write_all(text.as_bytes());
    }
}

/// Lower-case hex of the first `max` bytes, with `..` appended if truncated.
pub fn hex_prefix(b: &[u8], max: usize) -> String {
    let mut s = String::with_capacity(max * 2 + 2);
    for x in b.iter().take(max) {
        s.push_str(&format!("{x:02x}"));
    }
    if b.len() > max {
        s.push_str("..");
    }
    s
}

/// NAL unit types (comma separated, at most 8) found after Annex-B start codes inside `p`.
pub fn annexb_nal_types(p: &[u8]) -> String {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 3 < p.len() && out.len() < 8 {
        if p[i] == 0 && p[i + 1] == 0 && p[i + 2] == 1 {
            out.push((p[i + 3] & 0x1F).to_string());
            i += 4;
        } else {
            i += 1;
        }
    }
    out.join(",")
}

/// What the payload of a video RTP packet looks like (Zalo Annex-B format, else H.264 RFC 6184).
pub fn describe_h264_payload(p: &[u8]) -> String {
    let Some(&b0) = p.first() else {
        return "empty".into();
    };
    // Zalo format (observed): PT 97 = whole Annex-B access unit, PT 98 = `1c <flags> <Annex-B chunk>`.
    if p.starts_with(&[0, 0, 0, 1]) || p.starts_with(&[0, 0, 1]) {
        return format!("annexb[{}]", annexb_nal_types(p));
    }
    if b0 & 0x1F == 28 && p.get(1).is_some_and(|f| f & 0x1F == 0) {
        let f = p[1];
        let se = match (f & 0x80 != 0, f & 0x40 != 0) {
            (true, true) => "S+E",
            (true, false) => "S",
            (false, true) => "E",
            _ => "M",
        };
        let first = if f & 0x80 != 0 {
            format!("/annexb[{}]", annexb_nal_types(&p[2..]))
        } else {
            String::new()
        };
        return format!("zfrag/{se}{first}");
    }
    let nri = (b0 >> 5) & 3;
    match b0 & 0x1F {
        t @ 1..=23 => format!("nal{t}/nri{nri}"),
        24 => {
            // STAP-A: list the aggregated NAL types.
            let mut i = 1usize;
            let mut kinds = Vec::new();
            while i + 2 < p.len() {
                let sz = u16::from_be_bytes([p[i], p[i + 1]]) as usize;
                if let Some(h) = p.get(i + 2) {
                    kinds.push((h & 0x1F).to_string());
                }
                i += 2 + sz;
            }
            format!("stapA[{}]", kinds.join(","))
        }
        28 | 29 => {
            let Some(&fu) = p.get(1) else {
                return "fu-trunc".into();
            };
            let kind = if b0 & 0x1F == 28 { "fuA" } else { "fuB" };
            let se = match (fu & 0x80 != 0, fu & 0x40 != 0) {
                (true, true) => "S+E",
                (true, false) => "S",
                (false, true) => "E",
                _ => "M",
            };
            format!("{kind}/nal{}/{se}", fu & 0x1F)
        }
        t => format!("type{t}"),
    }
}

/// Parsed view of an RTP v2 packet header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtpInfo {
    pub pt: u8,
    pub marker: bool,
    pub seq: u16,
    pub ts: u32,
    pub ssrc: u32,
    pub csrc: u8,
    pub ext: bool,
    pub payload_len: usize,
    /// Offset of the payload inside the packet (after CSRC list and header extension).
    pub payload_off: usize,
}

pub fn parse_rtp(p: &[u8]) -> Option<RtpInfo> {
    if p.len() < 12 || (p[0] & 0xC0) != 0x80 {
        return None;
    }
    let cc = p[0] & 0x0F;
    let ext = p[0] & 0x10 != 0;
    let mut off = 12 + cc as usize * 4;
    if ext {
        if p.len() < off + 4 {
            return None;
        }
        off += 4 + u16::from_be_bytes([p[off + 2], p[off + 3]]) as usize * 4;
    }
    if off > p.len() {
        return None;
    }
    Some(RtpInfo {
        pt: p[1] & 0x7F,
        marker: p[1] & 0x80 != 0,
        seq: u16::from_be_bytes([p[2], p[3]]),
        ts: u32::from_be_bytes([p[4], p[5], p[6], p[7]]),
        ssrc: u32::from_be_bytes([p[8], p[9], p[10], p[11]]),
        csrc: cc,
        ext,
        payload_len: p.len() - off,
        payload_off: off,
    })
}

fn rtcp_pt_name(pt: u8) -> &'static str {
    match pt {
        200 => "SR",
        201 => "RR",
        202 => "SDES",
        203 => "BYE",
        204 => "APP",
        205 => "RTPFB",
        206 => "PSFB",
        207 => "XR",
        _ => "?",
    }
}

fn rtcp_fmt_name(pt: u8, fmt: u8) -> &'static str {
    match (pt, fmt) {
        (205, 1) => "NACK",
        (205, 3) => "TMMBR",
        (205, 4) => "TMMBN",
        (205, 15) => "TWCC",
        (206, 1) => "PLI",
        (206, 2) => "SLI",
        (206, 3) => "RPSI",
        (206, 4) => "FIR",
        (206, 15) => "AFB/REMB",
        _ => "",
    }
}

/// One-line description of a (compound) RTCP datagram, e.g. `RR(rc=1,ssrc=..)+PSFB/PLI(rc=1,ssrc=..,media=..)`.
/// `keyframe_req` is true when any block is a PLI or FIR.
pub fn describe_rtcp(rtcp: &[u8]) -> (String, bool) {
    let mut parts = Vec::new();
    let mut keyframe_req = false;
    let mut i = 0usize;
    while i + 4 <= rtcp.len() {
        if (rtcp[i] & 0xC0) != 0x80 {
            parts.push(format!("bad@{i}"));
            break;
        }
        let fmt = rtcp[i] & 0x1F;
        let pt = rtcp[i + 1];
        let words = u16::from_be_bytes([rtcp[i + 2], rtcp[i + 3]]) as usize;
        let end = (i + (words + 1) * 4).min(rtcp.len());
        let sender = rtcp
            .get(i + 4..i + 8)
            .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
        let media = rtcp
            .get(i + 8..i + 12)
            .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
        let sub = rtcp_fmt_name(pt, fmt);
        let name = if sub.is_empty() {
            rtcp_pt_name(pt).to_string()
        } else {
            format!("{}/{sub}", rtcp_pt_name(pt))
        };
        if pt == 206 && (fmt == 1 || fmt == 4) {
            keyframe_req = true;
        }
        let mut d = format!("{name}(pt={pt},fmt={fmt},len={}", end - i);
        if let Some(s) = sender {
            d.push_str(&format!(",ssrc={s:#010x}"));
        }
        if (pt == 205 || pt == 206) && media.is_some() {
            d.push_str(&format!(",media={:#010x}", media.unwrap_or(0)));
        }
        // SR (200) / RR (201): how the sender sees *our* streams (loss, highest seq, jitter).
        if pt == 200 || pt == 201 {
            let first = if pt == 200 { i + 28 } else { i + 8 };
            for k in 0..(fmt as usize) {
                let b = first + k * 24;
                if b + 24 > end {
                    break;
                }
                let u32at = |o: usize| {
                    u32::from_be_bytes([
                        rtcp[b + o],
                        rtcp[b + o + 1],
                        rtcp[b + o + 2],
                        rtcp[b + o + 3],
                    ])
                };
                let cum = u32at(4) & 0x00FF_FFFF;
                d.push_str(&format!(
                    ",blk[ssrc={:#010x},fl={},lost={},hi={},jit={},lsr={:#x},dlsr={}]",
                    u32at(0),
                    rtcp[b + 4],
                    cum,
                    u32at(8),
                    u32at(12),
                    u32at(16),
                    u32at(20),
                ));
            }
        }
        d.push(')');
        parts.push(d);
        if words == 0 && end <= i {
            break;
        }
        i = (i + (words + 1) * 4).max(i + 4);
    }
    (parts.join("+"), keyframe_req)
}

/// Where a datagram arrived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    RtpSock,
    RtcpSock,
}

impl Via {
    pub fn as_str(self) -> &'static str {
        match self {
            Via::RtpSock => "rtp-sock",
            Via::RtcpSock => "rtcp-sock",
        }
    }
}

#[derive(Default, Debug)]
struct StreamStat {
    pkts: u64,
    bytes: u64,
    min_len: usize,
    max_len: usize,
    first_seq: u16,
    last_seq: u16,
    lost: u64,
    marker: u64,
    nal: BTreeMap<String, u64>,
    last_bytes: u64,
    last_pkts: u64,
}

/// Per-call RTP statistics + log throttling. One instance lives inside the media loop.
pub struct WireLog {
    streams: BTreeMap<String, StreamStat>,
    detail_count: BTreeMap<String, u32>,
    rtcp_lines: u32,
    unk_lines: u32,
    /// Largest datagram seen as received / sent (relay header included) — MTU hint.
    max_wire_rx: usize,
    max_wire_tx: usize,
    last_stat: Instant,
    started: Instant,
}

impl Default for WireLog {
    fn default() -> Self {
        Self::new()
    }
}

impl WireLog {
    pub fn new() -> Self {
        Self {
            streams: BTreeMap::new(),
            detail_count: BTreeMap::new(),
            rtcp_lines: 0,
            unk_lines: 0,
            max_wire_rx: 0,
            max_wire_tx: 0,
            last_stat: Instant::now(),
            started: Instant::now(),
        }
    }

    /// Record one RTP packet. `dir` is `">"` (sent) or `"<"` (received); `wire_len` includes the relay header;
    /// `outer` is the first byte of the datagram (relay type, or `0x80` for raw RTP); `peer` is the remote address.
    pub fn rtp(&mut self, dir: &str, outer: u8, wire_len: usize, peer: &str, inner: &[u8]) {
        let Some(h) = parse_rtp(inner) else {
            return;
        };
        if dir == "<" {
            self.max_wire_rx = self.max_wire_rx.max(wire_len);
        } else {
            self.max_wire_tx = self.max_wire_tx.max(wire_len);
        }
        let payload = &inner[h.payload_off..];
        let is_video = h.pt != 112 && h.pt != 0;
        let desc = if is_video {
            describe_h264_payload(payload)
        } else {
            String::new()
        };
        let ext_hex = if h.ext {
            hex_prefix(&inner[12 + h.csrc as usize * 4..h.payload_off], 16)
        } else {
            "-".to_string()
        };
        let key = format!("{dir}pt{}/ssrc{:08x}", h.pt, h.ssrc);
        let st = self.streams.entry(key).or_default();
        if st.pkts == 0 {
            st.first_seq = h.seq;
            st.min_len = wire_len;
        } else {
            let gap = h.seq.wrapping_sub(st.last_seq);
            // 1 = in order; 2..=1000 = forward gap (loss); larger = reorder/duplicate/restart.
            if (2..=1000).contains(&gap) {
                st.lost += gap as u64 - 1;
            }
        }
        st.last_seq = h.seq;
        st.pkts += 1;
        st.bytes += wire_len as u64;
        st.min_len = st.min_len.min(wire_len);
        st.max_len = st.max_len.max(wire_len);
        if h.marker {
            st.marker += 1;
        }
        if !desc.is_empty() {
            *st.nal.entry(desc.clone()).or_default() += 1;
        }

        let n = self
            .detail_count
            .entry(format!("{dir}{}", h.pt))
            .or_default();
        *n += 1;
        if *n <= DETAIL_FIRST || *n % DETAIL_EVERY == 0 {
            line(
                if dir == "<" { "RTP<" } else { "RTP>" },
                format!(
                    "peer={peer} outer={outer:#04x} wire={wire_len} pt={} m={} seq={} ts={} ssrc={:#010x} cc={} x={} ext={ext_hex} payload={} {desc} head={}",
                    h.pt,
                    h.marker as u8,
                    h.seq,
                    h.ts,
                    h.ssrc,
                    h.csrc,
                    h.ext as u8,
                    h.payload_len,
                    hex_prefix(payload, 16),
                ),
            );
        }
    }

    /// Record an RTCP datagram (sent `">"` or received `"<"`). Returns true if it contains PLI/FIR.
    pub fn rtcp(
        &mut self,
        dir: &str,
        via: Via,
        outer: u8,
        wire_len: usize,
        peer: &str,
        inner: &[u8],
    ) -> bool {
        let (desc, kf) = describe_rtcp(inner);
        if dir == "<" {
            self.max_wire_rx = self.max_wire_rx.max(wire_len);
        } else {
            self.max_wire_tx = self.max_wire_tx.max(wire_len);
        }
        self.rtcp_lines += 1;
        if self.rtcp_lines <= RTCP_MAX_LINES || kf {
            line(
                if dir == "<" { "RTCP<" } else { "RTCP>" },
                format!(
                    "via={} peer={peer} outer={outer:#04x} wire={wire_len} kf_req={} {desc} head={}",
                    via.as_str(),
                    kf as u8,
                    hex_prefix(inner, 32),
                ),
            );
        }
        kf
    }

    /// Record a datagram that the relay unwrapper rejected (unknown framing).
    pub fn unknown(&mut self, via: Via, wire_len: usize, peer: &str, wire: &[u8]) {
        self.max_wire_rx = self.max_wire_rx.max(wire_len);
        self.unk_lines += 1;
        if self.unk_lines <= 40 || self.unk_lines % 100 == 0 {
            line(
                "UNK<",
                format!(
                    "via={} peer={peer} wire={wire_len} n={} head={}",
                    via.as_str(),
                    self.unk_lines,
                    hex_prefix(wire, 32),
                ),
            );
        }
    }

    /// Emit the per-stream summary when `STAT_INTERVAL` elapsed (cheap to call every loop turn).
    pub fn tick(&mut self) {
        if self.last_stat.elapsed() < STAT_INTERVAL {
            return;
        }
        let secs = self.last_stat.elapsed().as_secs_f64().max(0.001);
        self.last_stat = Instant::now();
        let up = self.started.elapsed().as_secs();
        for (key, st) in self.streams.iter_mut() {
            let kbps = (st.bytes - st.last_bytes) as f64 * 8.0 / secs / 1000.0;
            let pps = (st.pkts - st.last_pkts) as f64 / secs;
            st.last_bytes = st.bytes;
            st.last_pkts = st.pkts;
            let nal = st
                .nal
                .iter()
                .map(|(k, v)| format!("{k}:{v}"))
                .collect::<Vec<_>>()
                .join(",");
            line(
                "STAT",
                format!(
                    "up={up}s stream={key} pkts={} bytes={} {pps:.1}pps {kbps:.1}kbps wire[min={},max={}] seq={}..{} lost={} marker={} nal{{{nal}}}",
                    st.pkts, st.bytes, st.min_len, st.max_len, st.first_seq, st.last_seq, st.lost, st.marker,
                ),
            );
        }
        line(
            "STAT",
            format!(
                "up={up}s mtu_hint wire_rx_max={} wire_tx_max={} (largest datagram incl. relay header)",
                self.max_wire_rx, self.max_wire_tx
            ),
        );
    }
}

/// Log the remote media parameters that settle the payload type: `codec` and `extendData`.
/// `params` is the parsed `data.params` object of a control event (see `call::event_media_params`).
pub fn log_signal_media(act: &str, params: &serde_json::Value) {
    if params.is_null() {
        return;
    }
    let cut = |v: Option<&serde_json::Value>| -> String {
        let s = match v {
            None => "-".to_string(),
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(other) => other.to_string(),
        };
        s.chars().take(1500).collect()
    };
    line(
        "SIG",
        format!(
            "act={act} codec={} extendData={} rtpIP={}",
            cut(params.get("codec")),
            cut(params.get("extendData")),
            cut(params.get("rtpIP")),
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rtp(pt: u8, marker: bool, seq: u16, payload: &[u8]) -> Vec<u8> {
        let mut p = vec![0x80, if marker { 0x80 | pt } else { pt }];
        p.extend_from_slice(&seq.to_be_bytes());
        p.extend_from_slice(&1000u32.to_be_bytes());
        p.extend_from_slice(&0xAABBCCDDu32.to_be_bytes());
        p.extend_from_slice(payload);
        p
    }

    #[test]
    fn timestamp_format_is_utc_civil() {
        assert_eq!(format_ts(0), "1970-01-01 00:00:00.000");
        assert_eq!(format_ts(1_000_000_000_123), "2001-09-09 01:46:40.123");
        assert_eq!(format_ts(1_709_164_800_000), "2024-02-29 00:00:00.000"); // leap day
    }

    #[test]
    fn parses_rtp_header_with_csrc_and_extension() {
        let h = parse_rtp(&rtp(97, true, 7, &[0x65, 1, 2])).unwrap();
        assert_eq!(
            (h.pt, h.marker, h.seq, h.ts, h.ssrc),
            (97, true, 7, 1000, 0xAABBCCDD)
        );
        assert_eq!((h.payload_off, h.payload_len), (12, 3));
        // X bit + 1 extension word + 1 CSRC
        let mut p = vec![0x91, 96, 0, 1, 0, 0, 0, 2, 0, 0, 0, 3];
        p.extend_from_slice(&[0, 0, 0, 9]); // CSRC
        p.extend_from_slice(&[0xBE, 0xDE, 0, 1, 1, 2, 3, 4]); // ext header + 1 word
        p.extend_from_slice(&[0x41, 0x9A]);
        let h = parse_rtp(&p).unwrap();
        assert_eq!(
            (h.csrc, h.ext, h.payload_off, h.payload_len),
            (1, true, 24, 2)
        );
        assert!(parse_rtp(&[0x40; 20]).is_none());
        assert!(parse_rtp(&[0x80; 5]).is_none());
    }

    #[test]
    fn describes_h264_payload_kinds() {
        assert_eq!(describe_h264_payload(&[0x67, 0x42]), "nal7/nri3");
        assert_eq!(describe_h264_payload(&[0x41, 0x9A]), "nal1/nri2");
        assert_eq!(describe_h264_payload(&[0x7C, 0x85, 0xAA]), "fuA/nal5/S");
        assert_eq!(describe_h264_payload(&[0x7C, 0x45, 0xAA]), "fuA/nal5/E");
        assert_eq!(describe_h264_payload(&[0x7C, 0x05, 0xAA]), "fuA/nal5/M");
        // STAP-A carrying SPS (type 7) and PPS (type 8)
        let stap = [0x78, 0, 2, 0x67, 0xAA, 0, 2, 0x68, 0xBB];
        assert_eq!(describe_h264_payload(&stap), "stapA[7,8]");
        assert_eq!(describe_h264_payload(&[]), "empty");
        // Zalo format as captured
        assert_eq!(
            describe_h264_payload(&[0, 0, 0, 1, 0x21, 0xE1]),
            "annexb[1]"
        );
        assert_eq!(
            describe_h264_payload(&[0x1C, 0x80, 0, 0, 0, 1, 0x27, 0x42, 0, 0, 1, 0x28]),
            "zfrag/S/annexb[7,8]"
        );
        assert_eq!(describe_h264_payload(&[0x1C, 0x00, 0x06]), "zfrag/M");
        assert_eq!(describe_h264_payload(&[0x1C, 0x40, 0xDE]), "zfrag/E");
    }

    #[test]
    fn describes_compound_rtcp_and_flags_pli_fir() {
        // RR (rc=0, 1 word) + PSFB PLI (fmt 1, 2 words)
        let mut p = vec![0x80, 201, 0, 1, 0, 0, 0, 5];
        p.extend_from_slice(&[0x81, 206, 0, 2]);
        p.extend_from_slice(&1u32.to_be_bytes());
        p.extend_from_slice(&2u32.to_be_bytes());
        let (d, kf) = describe_rtcp(&p);
        assert!(kf, "{d}");
        assert!(d.starts_with("RR(") && d.contains("+PSFB/PLI("), "{d}");
        assert!(d.contains("media=0x00000002"), "{d}");

        let fir = [0x84, 206, 0, 2, 0, 0, 0, 1, 0, 0, 0, 2];
        assert!(describe_rtcp(&fir).1);
        let nack = [0x81, 205, 0, 2, 0, 0, 0, 1, 0, 0, 0, 2];
        let (d, kf) = describe_rtcp(&nack);
        assert!(!kf && d.contains("RTPFB/NACK"), "{d}");
        let remb = [0x8F, 206, 0, 2, 0, 0, 0, 1, 0, 0, 0, 0];
        assert!(describe_rtcp(&remb).0.contains("PSFB/AFB/REMB"));
        // garbage must not loop or panic
        assert!(describe_rtcp(&[0x00, 0x01, 0x02, 0x03]).0.contains("bad@0"));
    }

    #[test]
    fn rtcp_agrees_with_media_keyframe_detector() {
        let pli = [0x81, 206, 0, 2, 0, 0, 0, 1, 0, 0, 0, 2];
        assert!(describe_rtcp(&pli).1);
        assert!(crate::media::rtcp_requests_keyframe(&pli));
    }

    #[test]
    fn stats_track_loss_marker_nal_and_mtu_hint() {
        std::env::set_var("ZALO_VOIP_LOG", "off");
        let mut w = WireLog::new();
        w.rtp(
            "<",
            0x0E,
            1201,
            "1.2.3.4:5",
            &rtp(97, false, 10, &[0x7C, 0x85, 1]),
        );
        w.rtp(
            "<",
            0x0E,
            1500,
            "1.2.3.4:5",
            &rtp(97, false, 11, &[0x7C, 0x05, 1]),
        );
        // seq 12, 13 lost
        w.rtp(
            "<",
            0x0E,
            300,
            "1.2.3.4:5",
            &rtp(97, true, 14, &[0x7C, 0x45, 1]),
        );
        let st = w.streams.values().next().unwrap();
        assert_eq!((st.pkts, st.lost, st.marker), (3, 2, 1));
        assert_eq!((st.min_len, st.max_len), (300, 1500));
        assert_eq!(st.nal.get("fuA/nal5/S"), Some(&1));
        assert_eq!(w.max_wire_rx, 1500);
        assert_eq!(w.max_wire_tx, 0);
        assert!(w.rtcp(
            "<",
            Via::RtcpSock,
            0x04,
            40,
            "p",
            &[0x81, 206, 0, 2, 0, 0, 0, 1, 0, 0, 0, 2]
        ));
    }

    #[test]
    fn hex_prefix_truncates() {
        assert_eq!(hex_prefix(&[1, 2, 3], 2), "0102..");
        assert_eq!(hex_prefix(&[1, 2], 4), "0102");
    }
}
