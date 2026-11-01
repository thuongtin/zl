//! Control websocket (`zpw_ws`) - the real-time transport.
//!
//! [O] Decoded from Zalo PC 26.10.11 (`pc-dist_compact-app-pc.*.js`):
//!
//! * URL - `Q[i] + "?" + commonParams`, then the client appends `&t=<ms>`
//!   (`getSocketUrl`, `_connectSocket`). `Q` is the login-config `zpw_ws` list.
//! * Frame - 4-byte header + UTF-8 JSON: `u8 version`, `u16 LE cmd`, `u8 subCmd`
//!   (`_send`, `_getHeader`, `_processEventBlob`).
//! * The server speaks first: `cmd 1` (`AUTHEN`) with `{key, encrypt, data}`.
//!   `key` is base64 and becomes the AES-GCM key (`_cipherKey`).
//! * `encrypt`: 0 plain, 1 base64+zlib, 2 AES-GCM+zlib, 3 AES-GCM
//!   (`_decrypt`, `_decompress`). AES-GCM blob = `iv(16) | aad(16) | ciphertext||tag`.
//! * After auth the payload lists the queues to pull (`data.ctrl_qCmds`); each is pulled with
//!   `{cmd, subCmd, data:{first, reqId:"req_N", lastId, preIds}}` (`doGetOffline`).
//! * Ping - `{version:1, cmd:2, subCmd:1, data:{eventId}}`; `_onPong` matches `eventId`.
//!
//! HTTP long-polling only ever carries durable actions; live VoIP invites are socket-only,
//! which is why a polling-only callee sees `act:"answer" status:5` and never `act:"request"`.

use aes::{Aes128, Aes256};
use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::AesGcm;
use anyhow::{anyhow, bail, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use futures_util::{SinkExt, StreamExt};
use generic_array::typenum::U16;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::{COOKIE, ORIGIN, USER_AGENT};
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;

use crate::session::Session;

// -- command codes (`A={AUTHEN:1,PING:2,...,CTRL_*}` in the bundle) -------------

/// Local lifecycle event, never serialized onto the socket.
pub const CMD_TRANSPORT_STATUS: u16 = 0;
pub const CMD_SYNC_V2_REQUEST: u16 = 590;
pub const CMD_SYNC_V2_DISPOSE: u16 = 591;
pub const CMD_SYNC_V2_WAKEUP: u16 = 592;
pub const CMD_AUTHEN: u16 = 1;
pub const CMD_PING: u16 = 2;
pub const CMD_PING_ACTIVE: u16 = 4;
pub const SUB_KEEP_CONNECTION: u8 = 1;
pub const SUB_KEEP_ACTIVE: u8 = 2;

pub const CMD_CTRL_PUSH_CTRL: u16 = 601;
pub const CMD_CTRL_ACTION: u16 = 602;
pub const CMD_CTRL_PULL_CTRL: u16 = 603;
pub const CMD_CTRL_PULL_VOIP: u16 = 604;

pub const CMD_PUSH_MSG_1_1: u16 = 501;
pub const CMD_PUSH_MSG_GROUP: u16 = 521;
pub const CMD_PULL_MSG_1_1: u16 = 510;
pub const CMD_PULL_MSG_GROUP: u16 = 511;
pub const CMD_PULL_MSG_513: u16 = 513;
pub const CMD_PULL_MSG_514: u16 = 514;
pub const CMD_PULL_MSG_515: u16 = 515;
pub const CMD_PULL_MSG_517: u16 = 517;
pub const CMD_PULL_MSG_518: u16 = 518;
pub const CMD_HAS_MSG: u16 = 533;
pub const CMD_PUSH_MISS_MSG: u16 = 534;
pub const CMD_GET_MSG: u16 = 570;
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                  (KHTML, like Gecko) ZaloPC/26.10.11 Chrome/122.0.0.0 Electron/29.0.0 Safari/537.36";

const PING_SECS: u64 = 15;

// -- wire level ----------------------------------------------------------------

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn trace_on() -> bool {
    std::env::var("ZCALL_WS_TRACE").ok().as_deref() == Some("1")
}

fn trace(msg: impl std::fmt::Display) {
    if trace_on() {
        eprintln!("[ws] {msg}");
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}

/// Encode one frame: `u8 version | u16 LE cmd | u8 subCmd | JSON body`.
pub fn encode_frame(cmd: u16, sub_cmd: u8, version: u8, data: Option<&Value>) -> Vec<u8> {
    let body = data.map(|d| d.to_string()).unwrap_or_default();
    let mut out = Vec::with_capacity(4 + body.len());
    out.push(version);
    out.extend_from_slice(&cmd.to_le_bytes());
    out.push(sub_cmd);
    out.extend_from_slice(body.as_bytes());
    out
}

/// Decode one frame into `(version, cmd, subCmd, JSON)`.
pub fn decode_frame(buf: &[u8]) -> Option<(u8, u16, u8, Value)> {
    if buf.len() < 4 {
        return None;
    }
    let version = buf[0];
    let cmd = u16::from_le_bytes([buf[1], buf[2]]);
    let sub_cmd = buf[3];
    let body = &buf[4..];
    if body.is_empty() {
        return Some((version, cmd, sub_cmd, Value::Null));
    }
    let value = serde_json::from_slice(body).ok()?;
    Some((version, cmd, sub_cmd, value))
}

/// `decodeURIComponent` for the AES branch, tolerating raw `+`/`/`/`=` base64.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (hi, lo) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Inflate a decrypted payload.
///
/// [O] The server sends **gzip** (`1f 8b 08 …`) for `encrypt` 2/3; the JS side calls a generic
/// `inflate` helper that sniffs the container. Older captures decoded zlib, so try gzip first
/// (by magic), then zlib, then raw deflate.
fn inflate(buf: &[u8]) -> Result<Vec<u8>> {
    use std::io::Read;
    if trace_on() {
        let head: String = buf.iter().take(12).map(|b| format!("{b:02x}")).collect();
        trace(format!("inflate input {} bytes head={head}", buf.len()));
    }
    type DecoderFactory = for<'a> fn(&'a [u8]) -> Box<dyn Read + 'a>;
    let attempts: [(&str, DecoderFactory); 2] = [
        ("gzip", |b| Box::new(flate2::read::GzDecoder::new(b))),
        ("zlib", |b| Box::new(flate2::read::ZlibDecoder::new(b))),
    ];
    let order: [usize; 2] = if buf.starts_with(&[0x1f, 0x8b]) {
        [0, 1]
    } else {
        [1, 0]
    };
    let mut last_err: Option<anyhow::Error> = None;
    for idx in order {
        let (name, make) = attempts[idx];
        let mut out = Vec::new();
        match make(buf).read_to_end(&mut out) {
            Ok(_) if !out.is_empty() => {
                trace(format!(
                    "inflate ok via {name}: {} -> {} bytes",
                    buf.len(),
                    out.len()
                ));
                return Ok(out);
            }
            Ok(_) => last_err = Some(anyhow!("{name}: empty output")),
            Err(e) => last_err = Some(anyhow!("{name}: {e}")),
        }
    }
    let mut out = Vec::new();
    flate2::read::DeflateDecoder::new(buf).read_to_end(&mut out)?;
    if out.is_empty() {
        return Err(last_err.unwrap_or_else(|| anyhow!("empty inflate output")));
    }
    Ok(out)
}

/// AES-GCM with a 16-byte nonce: `iv(16) | aad(16) | ciphertext||tag(16)`.
fn gcm_decrypt(key: &[u8], blob: &[u8]) -> Result<Vec<u8>> {
    if blob.len() < 48 {
        bail!("aes-gcm blob too short ({})", blob.len());
    }
    let nonce = aes_gcm::Nonce::<U16>::from_slice(&blob[..16]);
    let payload = Payload {
        msg: &blob[32..],
        aad: &blob[16..32],
    };
    let plain = match key.len() {
        32 => AesGcm::<Aes256, U16>::new_from_slice(key)
            .map_err(|e| anyhow!("aes key: {e}"))?
            .decrypt(nonce, payload)
            .map_err(|e| anyhow!("aes-gcm: {e}"))?,
        16 => AesGcm::<Aes128, U16>::new_from_slice(key)
            .map_err(|e| anyhow!("aes key: {e}"))?
            .decrypt(nonce, payload)
            .map_err(|e| anyhow!("aes-gcm: {e}"))?,
        n => bail!("unsupported AES key length {n}"),
    };
    Ok(plain)
}

/// Apply the envelope's `encrypt` scheme to its `data` field.
pub fn decrypt_payload(cipher_key: Option<&[u8]>, encrypt: u8, data: &str) -> Result<Vec<u8>> {
    match encrypt {
        0 => Ok(data.as_bytes().to_vec()),
        1 => inflate(&B64.decode(data)?),
        2 => {
            let key = cipher_key.ok_or_else(|| anyhow!("no cipher key"))?;
            let raw = B64.decode(percent_decode(data))?;
            inflate(&gcm_decrypt(key, &raw)?)
        }
        3 => {
            let key = cipher_key.ok_or_else(|| anyhow!("no cipher key"))?;
            let raw = B64.decode(percent_decode(data))?;
            gcm_decrypt(key, &raw)
        }
        other => bail!("unsupported encrypt type {other}"),
    }
}

// -- client --------------------------------------------------------------------

/// One decrypted socket event.
#[derive(Debug, Clone)]
pub struct WsEvent {
    pub cmd: u16,
    pub sub_cmd: u8,
    pub version: u8,
    /// Decrypted body when possible, otherwise the raw envelope.
    pub payload: Value,
}

/// Commands sent through the existing authenticated control connection.
#[derive(Debug, Clone)]
pub enum WsCommand {
    RequestDirectHistory {
        last_id: Option<String>,
    },
    RequestGroupHistory {
        last_id: Option<String>,
    },
    RequestSyncV2 {
        session_index: u32,
        sync_id: String,
        ek: String,
        ik: String,
        device_name: String,
        request: Value,
        request_id: String,
    },
    DisposeSyncV2 {
        sync_id: String,
    },
    WakeupSyncV2 {
        data: Value,
        request_id: String,
    },
}

#[derive(Default)]
struct HistoryQueue {
    requests: VecDeque<Option<String>>,
    in_flight: bool,
    selected: bool,
    sync_commands: VecDeque<WsCommand>,
    group_requests: VecDeque<Option<String>>,
    group_in_flight: bool,
    groups_selected: bool,
}

impl HistoryQueue {
    fn push(&mut self, command: WsCommand) {
        let last_id = match command {
            WsCommand::RequestDirectHistory { last_id } => last_id,
            WsCommand::RequestGroupHistory { last_id } => {
                self.groups_selected = true;
                if self.group_requests.front() == Some(&last_id) {
                    self.group_in_flight = false;
                } else if !self.group_requests.contains(&last_id) {
                    self.group_requests.push_back(last_id);
                }
                return;
            }
            command => {
                self.sync_commands.push_back(command);
                return;
            }
        };
        self.selected = true;
        if self.requests.front() == Some(&last_id) {
            // The caller owns the response timeout. An explicit retry re-arms
            // the pending cursor without growing the queue or retrying silently.
            self.in_flight = false;
            return;
        }
        if !self.requests.contains(&last_id) {
            self.requests.push_back(last_id);
        }
    }

    fn complete(&mut self) {
        if self.in_flight {
            self.requests.pop_front();
            self.in_flight = false;
        }
    }
}

/// Bounded, read-only replay of old reactions. Primary wire contract: zca-js
/// listen.ts at 2a7e360, requestOldReactions (610/611, sub-command 1).
struct ReactionHistoryPull {
    cmd: u16,
    cursor: Option<String>,
    pages: usize,
    pending: bool,
    in_flight: Option<std::time::Instant>,
}
impl ReactionHistoryPull {
    fn new(cmd: u16) -> Self {
        Self {
            cmd,
            cursor: None,
            pages: 0,
            pending: true,
            in_flight: None,
        }
    }
    fn frame(&mut self, req_id: &str) -> Option<Vec<u8>> {
        if !self.pending || self.in_flight.is_some() {
            return None;
        }
        self.pending = false;
        self.in_flight = Some(std::time::Instant::now());
        Some(encode_frame(
            self.cmd,
            1,
            1,
            Some(&json!({
                "first": true, "lastId": self.cursor, "preIds": [], "req_id": req_id,
            })),
        ))
    }
    fn accept(&mut self, payload: &Value) {
        if self.in_flight.take().is_none() {
            return;
        }
        self.pages += 1;
        let data = payload.get("data").unwrap_or(payload);
        if [payload, data].iter().any(|v| {
            v.get("error_code").is_some_and(|c| {
                c.as_i64()
                    .or_else(|| c.as_str().and_then(|s| s.parse().ok()))
                    != Some(0)
            })
        }) {
            return;
        }
        let bucket = if self.cmd == 610 {
            "reacts"
        } else {
            "reactGroups"
        };
        let Some(items) = data.get(bucket).and_then(Value::as_array) else {
            return;
        };
        fn cursor(v: &Value) -> Option<String> {
            match v {
                Value::String(s) if !s.is_empty() && s != "0" => Some(s.clone()),
                Value::Number(n) if n.as_u64().is_some_and(|n| n > 0) => Some(n.to_string()),
                _ => None,
            }
        }
        let next = items
            .last()
            .and_then(|item| item.get("msgId").and_then(cursor));
        if !items.is_empty() && self.pages < 20 && next.is_some() && next != self.cursor {
            self.cursor = next;
            self.pending = true;
        }
    }
    fn expire(&mut self) {
        if self
            .in_flight
            .is_some_and(|t| t.elapsed() >= Duration::from_secs(25))
        {
            self.in_flight = None;
            self.pending = false;
        }
    }
}

/// Direct history uses `req_id` and a nullable string cursor, unlike offline queues.
fn direct_history_frame(last_id: Option<&str>, req_id: &str) -> Vec<u8> {
    encode_frame(
        CMD_PULL_MSG_1_1,
        1,
        1,
        Some(&json!({
            "first": true,
            "lastId": last_id,
            "preIds": [],
            "req_id": req_id,
        })),
    )
}

fn group_history_frame(last_id: Option<&str>, req_id: &str) -> Vec<u8> {
    encode_frame(
        CMD_PULL_MSG_GROUP,
        1,
        1,
        Some(&json!({
            "first": true, "lastId": last_id, "preIds": [], "req_id": req_id,
        })),
    )
}

/// Serialize sync-v2 envelopes without touching their coordinator-owned keys or queries.
fn sync_v2_frame(command: &WsCommand) -> Option<Vec<u8>> {
    let (cmd, body) = match command {
        WsCommand::RequestSyncV2 {
            session_index,
            sync_id,
            ek,
            ik,
            device_name,
            request,
            request_id,
        } => (
            CMD_SYNC_V2_REQUEST,
            json!({"data": {
                "syncId": sync_id, "syncType": 0, "ek": ek, "ik": ik,
                "toDevice": 0, "tempKey": "", "deviceName": device_name,
                "req": request, "ver": 1, "ussidx": session_index,
            }, "reqId": request_id}),
        ),
        WsCommand::DisposeSyncV2 { sync_id } => (
            CMD_SYNC_V2_DISPOSE,
            json!({"data": {"syncId": sync_id, "toDevice": 0, "reason": 1}, "reqId": "req_disp"}),
        ),
        // The owning socket API passes wakeup data directly, unlike 590/591.
        WsCommand::WakeupSyncV2 { data, request_id } => {
            let mut body = data.as_object()?.clone();
            body.insert("reqId".into(), Value::String(request_id.clone()));
            (CMD_SYNC_V2_WAKEUP, Value::Object(body))
        }
        WsCommand::RequestDirectHistory { .. } | WsCommand::RequestGroupHistory { .. } => {
            return None
        }
    };
    Some(encode_frame(cmd, 0, 1, Some(&body)))
}

impl WsEvent {
    /// `true` for anything that may carry a VoIP invite (`601` push, `604` pull page).
    pub fn is_voip_ish(&self) -> bool {
        matches!(
            self.cmd,
            CMD_CTRL_PUSH_CTRL | CMD_CTRL_ACTION | CMD_CTRL_PULL_CTRL | CMD_CTRL_PULL_VOIP
        )
    }

    /// `true` for real-time or offline chat message frames (`501`, `521`, `510`, `511`, `515`, `570`, …).
    pub fn is_chat_ish(&self) -> bool {
        matches!(
            self.cmd,
            CMD_PUSH_MSG_1_1
                | CMD_PUSH_MSG_GROUP
                | CMD_PULL_MSG_1_1
                | CMD_PULL_MSG_GROUP
                | CMD_PULL_MSG_513
                | CMD_PULL_MSG_514
                | CMD_PULL_MSG_515
                | CMD_PULL_MSG_517
                | CMD_PULL_MSG_518
                | CMD_PUSH_MISS_MSG
                | CMD_GET_MSG
        )
    }
}

fn build_url(sess: &Session, endpoint_idx: usize) -> String {
    let endpoints = &sess.domains.ws;
    let base = std::env::var("ZCALL_WS")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            if endpoints.is_empty() {
                None
            } else {
                Some(endpoints[endpoint_idx % endpoints.len()].clone())
            }
        })
        .unwrap_or_else(|| "wss://ws1-msg.chat.zalo.me".to_string());
    // The server's `zlpws` front end answers `501 Not Implemented` to a request line with an
    // empty path (`GET ?zpw_ver=...`), so always send an explicit `/`.
    let (host, query) = match base.split_once('?') {
        Some((h, q)) => (h, Some(q)),
        None => (base.as_str(), None),
    };
    // Authority-only URLs (`wss://host`) need an explicit `/` before the query string.
    let after_scheme = host.split_once("://").map(|(_, rest)| rest).unwrap_or(host);
    let has_path = after_scheme.contains('/');
    let mut url = format!("{host}{}", if has_path { "" } else { "/" });
    let extra = format!("{}&t={}", sess.common_params(), now_ms());
    url.push('?');
    if let Some(q) = query {
        url.push_str(q);
        url.push('&');
    }
    url.push_str(&extra);
    url
}

/// Queues the server told us to pull.
///
/// The auth payload lists the exact shard-local queue names (`603_1`, `610_1`, …); pulling a
/// queue the worker does not own (e.g. `604_0`) closes the socket with `3002 NOT_FOUND_WORKER`,
/// so only server-listed queues are used unless `ZCALL_WS_FORCE_VOIP=1` asks for `604_0`.
fn queues_from_auth(auth: &Value) -> Vec<(u16, u8)> {
    let mut out: Vec<(u16, u8)> = Vec::new();
    for key in ["ctrl_qCmds", "qCmds"] {
        if let Some(arr) = auth
            .pointer(&format!("/data/{key}"))
            .and_then(|v| v.as_array())
        {
            for q in arr {
                let cmd = q.get("cmd").and_then(|v| v.as_u64());
                let sub = q.get("subCmd").and_then(|v| v.as_u64()).unwrap_or(0);
                if let Some(cmd) = cmd {
                    let pair = (cmd as u16, sub as u8);
                    if !out.contains(&pair) {
                        out.push(pair);
                    }
                }
            }
        }
    }
    if std::env::var("ZCALL_WS_FORCE_VOIP").ok().as_deref() == Some("1")
        && !out.iter().any(|(c, _)| *c == CMD_CTRL_PULL_VOIP)
    {
        out.push((CMD_CTRL_PULL_VOIP, 0));
    }
    out
}

fn pull_frame(cmd: u16, sub_cmd: u8, req_id: &str) -> Vec<u8> {
    let data = json!({
        "first": true,
        "reqId": req_id,
        "lastId": 1,
        "preIds": [],
    });
    encode_frame(cmd, sub_cmd, 1, Some(&data))
}

fn pull_next_frame(cmd: u16, sub_cmd: u8, req_id: &str, last_id: Value, pre_ids: Value) -> Vec<u8> {
    let data = json!({
        "first": false,
        "reqId": req_id,
        "lastId": last_id,
        "preIds": pre_ids,
    });
    encode_frame(cmd, sub_cmd, 1, Some(&data))
}

/// Connect, authenticate, pull the control/voip queues, then stream events.
///
/// Reconnects forever with capped backoff. Dropping the returned receiver stops the task.
pub fn spawn(sess: Session) -> mpsc::UnboundedReceiver<WsEvent> {
    let (events, _commands) = spawn_with_history(sess);
    events
}

/// Stream events and accept bounded, caller-controlled direct-history requests.
/// Requests queued before authentication survive reconnects until a history response arrives.
pub fn spawn_with_history(
    sess: Session,
) -> (
    mpsc::UnboundedReceiver<WsEvent>,
    mpsc::UnboundedSender<WsCommand>,
) {
    let (tx, rx) = mpsc::unbounded_channel();
    let (commands, mut command_rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut backoff = 2u64;
        let mut endpoint_idx = 0usize;
        let mut history = HistoryQueue::default();
        loop {
            history.in_flight = false;
            history.group_in_flight = false;
            tokio::select! {
                biased;
                _ = tx.closed() => break,
                result = run_once(&sess, &tx, endpoint_idx, &mut command_rx, &mut history) => {
                    match result {
                        Ok(reason) => trace(format!("closed: {reason}")),
                        Err(e) => trace(format!("error: {e:#}")),
                    }
                }
            }
            // A disconnected sync round belongs to the coordinator, which must
            // restart it with fresh session keys. Never send a stale phone prompt.
            history.sync_commands.clear();
            while let Ok(command) = command_rx.try_recv() {
                if matches!(
                    command,
                    WsCommand::RequestDirectHistory { .. } | WsCommand::RequestGroupHistory { .. }
                ) {
                    history.push(command);
                }
            }
            let _ = tx.send(WsEvent {
                cmd: CMD_TRANSPORT_STATUS,
                sub_cmd: 0,
                version: 0,
                payload: json!({"transport": "disconnected"}),
            });
            // `3002 NOT_FOUND_WORKER` means this shard does not host the session; rotate.
            endpoint_idx = endpoint_idx.wrapping_add(1);
            tokio::select! {
                _ = tx.closed() => break,
                _ = tokio::time::sleep(Duration::from_secs(backoff)) => {}
            }
            backoff = (backoff * 2).min(30);
        }
    });
    (rx, commands)
}

async fn run_once(
    sess: &Session,
    tx: &mpsc::UnboundedSender<WsEvent>,
    endpoint_idx: usize,
    commands: &mut mpsc::UnboundedReceiver<WsCommand>,
    history: &mut HistoryQueue,
) -> Result<String> {
    let url = build_url(sess, endpoint_idx);
    trace(format!("connect {url}"));

    let mut req = url.clone().into_client_request()?;
    {
        let h = req.headers_mut();
        if let Some(sek) = sess.zpw_sek.as_deref().filter(|s| !s.is_empty()) {
            h.insert(
                COOKIE,
                HeaderValue::from_str(&format!("zpw_sek={sek}; _zlang=vi"))?,
            );
            h.insert("zpw_sek", HeaderValue::from_str(sek)?);
        }
        h.insert(ORIGIN, HeaderValue::from_static("https://chat.zalo.me"));
        h.insert(USER_AGENT, HeaderValue::from_static(UA));
    }

    let (mut ws, resp) =
        match tokio::time::timeout(Duration::from_secs(20), connect_async(req)).await? {
            Ok(pair) => pair,
            Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => {
                let mut detail = format!("handshake status {}", resp.status());
                for (k, v) in resp.headers() {
                    detail.push_str(&format!("\n  {k}: {}", v.to_str().unwrap_or("<bin>")));
                }
                bail!("{detail}");
            }
            Err(e) => return Err(e.into()),
        };
    trace(format!("handshake {}", resp.status()));

    let mut cipher_key: Option<Vec<u8>> = None;
    let mut authed = false;
    let mut reaction_pulls = [ReactionHistoryPull::new(610), ReactionHistoryPull::new(611)];
    let mut req_id = 0u64;
    let mut ping = tokio::time::interval(Duration::from_secs(PING_SECS));
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ping.tick().await; // consume the immediate first tick

    let auth_deadline = tokio::time::sleep(Duration::from_secs(30));
    tokio::pin!(auth_deadline);
    let mut commands_open = true;
    loop {
        // Drain already queued commands before handling auth, so explicit history
        // replaces a duplicate 510/1 offline pull even when both become ready together.
        while let Ok(command) = commands.try_recv() {
            history.push(command);
        }
        if authed {
            while let Some(command) = history.sync_commands.front() {
                let frame = sync_v2_frame(command)
                    .ok_or_else(|| anyhow!("invalid sync-v2 command body"))?;
                ws.send(Message::Binary(frame)).await?;
                // Sent sync sessions are never replayed after reconnect: a new
                // session requires fresh keys and an explicit coordinator command.
                history.sync_commands.pop_front();
            }
        }
        if authed {
            for pull in &mut reaction_pulls {
                pull.expire();
                let id = format!("req_{req_id}");
                if let Some(frame) = pull.frame(&id) {
                    req_id += 1;
                    ws.send(Message::Binary(frame)).await?;
                }
            }
        }
        if authed && !history.in_flight {
            if let Some(last_id) = history.requests.front() {
                let id = format!("req_{req_id}");
                req_id += 1;
                ws.send(Message::Binary(direct_history_frame(
                    last_id.as_deref(),
                    &id,
                )))
                .await?;
                history.in_flight = true;
            }
        }
        if authed && !history.group_in_flight {
            if let Some(last_id) = history.group_requests.front() {
                let id = format!("req_{req_id}");
                req_id += 1;
                ws.send(Message::Binary(group_history_frame(
                    last_id.as_deref(),
                    &id,
                )))
                .await?;
                history.group_in_flight = true;
            }
        }
        tokio::select! {
            _ = &mut auth_deadline, if !authed => bail!("auth timeout"),
            command = commands.recv(), if commands_open => {
                match command {
                    Some(command) => history.push(command),
                    None => commands_open = false,
                }
            }
            _ = ping.tick() => {
                if authed {
                    let body = json!({ "eventId": now_ms() });
                    let f = encode_frame(CMD_PING, SUB_KEEP_CONNECTION, 1, Some(&body));
                    ws.send(Message::Binary(f)).await?;
                    trace("-> ping");
                }
            }
            incoming = ws.next() => {
                let Some(incoming) = incoming else {
                    return Ok("eof".into());
                };
                let incoming = incoming?;
                let buf = match incoming {
                    Message::Binary(b) => b,
                    Message::Text(t) => {
                        trace(format!("<- text {}", truncate(t.as_str(), 200)));
                        continue;
                    }
                    Message::Ping(p) => {
                        ws.send(Message::Pong(p)).await?;
                        continue;
                    }
                    Message::Close(c) => return Ok(format!("close {c:?}")),
                    _ => continue,
                };

                let Some((version, cmd, sub_cmd, envelope)) = decode_frame(&buf) else {
                    trace(format!("<- undecodable {} bytes", buf.len()));
                    continue;
                };

                let encrypt = envelope.get("encrypt").and_then(|v| v.as_u64()).unwrap_or(0) as u8;
                let err = envelope.get("error_code").and_then(|v| v.as_i64());
                trace(format!(
                    "<- v{version} cmd={cmd} sub={sub_cmd} enc={encrypt} err={err:?} {}",
                    truncate(&envelope.to_string(), 300)
                ));

                if cmd == CMD_AUTHEN {
                    if let Some(key) = envelope.get("key").and_then(|v| v.as_str()) {
                        if !key.is_empty() {
                            cipher_key = Some(B64.decode(key)?);
                            trace(format!(
                                "cipher key {} bytes",
                                cipher_key.as_ref().map(|k| k.len()).unwrap_or(0)
                            ));
                        }
                    }
                    let raw = envelope.get("data").and_then(|v| v.as_str()).unwrap_or_default();
                    let plain = decrypt_payload(cipher_key.as_deref(), encrypt, raw)?;
                    let text = String::from_utf8_lossy(&plain).into_owned();
                    trace(format!("auth body {}", truncate(&text, 1200)));
                    let auth: Value = serde_json::from_str(&text)?;
                    if auth.get("error_code").and_then(|v| v.as_i64()) != Some(0) {
                        return Err(anyhow!("auth rejected: {text}"));
                    }
                    authed = true;
                    let _ = tx.send(WsEvent { cmd, sub_cmd, version, payload: auth.clone() });

                    while let Ok(command) = commands.try_recv() {
                        history.push(command);
                    }
                    for (qcmd, qsub) in queues_from_auth(&auth) {
                        // Replay coordinator owns the old reaction queues.
                        if matches!(qcmd, 610 | 611) && qsub == 1 { continue; }
                        if history.selected && (qcmd, qsub) == (CMD_PULL_MSG_1_1, 1)
                            || history.groups_selected && (qcmd, qsub) == (CMD_PULL_MSG_GROUP, 1) {
                            continue;
                        }
                        let id = format!("req_{req_id}");
                        req_id += 1;
                        ws.send(Message::Binary(pull_frame(qcmd, qsub, &id))).await?;
                        trace(format!("-> pull {qcmd}_{qsub} {id}"));
                    }
                    continue;
                }

                let payload = if encrypt == 0 {
                    envelope.clone()
                } else {
                    match envelope.get("data").and_then(|v| v.as_str()) {
                        Some(d) => decrypt_payload(cipher_key.as_deref(), encrypt, d)
                            .ok()
                            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
                            .unwrap_or_else(|| envelope.clone()),
                        None => envelope.clone(),
                    }
                };
                if cmd == CMD_HAS_MSG {
                    if let (Some(qid), Some(ids)) = (
                        payload.pointer("/data/queueId").cloned(),
                        payload.pointer("/data/ids").cloned(),
                    ) {
                        let id = format!("req_{req_id}");
                        req_id += 1;
                        let get_req = json!({
                            "queueId": qid,
                            "ids": ids,
                            "lastId": "1",
                            "reqId": id,
                        });
                        let _ = ws
                            .send(Message::Binary(
                                encode_frame(CMD_GET_MSG, 0, 1, Some(&get_req)),
                            ))
                            .await;
                        trace(format!("-> get_msg 570_0 {id}"));
                    }
                }
                if sub_cmd == 1 {
                    for pull in &mut reaction_pulls {
                        if pull.cmd == cmd { pull.accept(&payload); }
                    }
                }
                let direct_history = history.selected && (cmd, sub_cmd) == (CMD_PULL_MSG_1_1, 1);
                if direct_history {
                    history.complete();
                }
                let group_history = history.groups_selected && (cmd, sub_cmd) == (CMD_PULL_MSG_GROUP, 1);
                if group_history && history.group_in_flight {
                    history.group_requests.pop_front();
                    history.group_in_flight = false;
                }
                if !direct_history && !group_history && !matches!(cmd, 610 | 611)
                    && !matches!(cmd, CMD_SYNC_V2_REQUEST | CMD_SYNC_V2_DISPOSE | CMD_SYNC_V2_WAKEUP)
                    && payload.pointer("/data/more").and_then(|v| v.as_i64()) == Some(1)
                {
                    let qkey = format!("{cmd}_{sub_cmd}");
                    let last_id = payload
                        .pointer("/data/lastActionId")
                        .or_else(|| payload.pointer(&format!("/data/queueStatus/{qkey}/lastId")))
                        .cloned()
                        .unwrap_or(json!(1));
                    let pre_ids = payload
                        .pointer(&format!("/data/queueStatus/{qkey}/ids"))
                        .cloned()
                        .unwrap_or_else(|| json!([]));
                    let id = format!("req_{req_id}");
                    req_id += 1;
                    let _ = ws
                        .send(Message::Binary(
                            pull_next_frame(cmd, sub_cmd, &id, last_id, pre_ids),
                        ))
                        .await;
                    trace(format!("-> pull next {qkey} {id}"));
                }
                let _ = tx.send(WsEvent { cmd, sub_cmd, version, payload });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_sync_request() -> WsCommand {
        WsCommand::RequestSyncV2 {
            session_index: 0,
            sync_id: "sync_fixture".into(),
            ek: "public_ek".into(),
            ik: "public_ik".into(),
            device_name: "Zalo Rust".into(),
            request_id: "req_conv_sync_f".into(),
            request: json!({"type": "conversation", "priority": 0, "batchSize": 2000,
                "queries": [{"partition": "", "from": 0, "to": 123, "limit": 2147483647}]}),
        }
    }

    #[test]
    fn sync_v2_frames_match_owning_envelopes() {
        let command = fixture_sync_request();
        let (version, cmd, sub, body) = decode_frame(&sync_v2_frame(&command).unwrap()).unwrap();
        assert_eq!((version, cmd, sub), (1, 590, 0));
        assert_eq!(
            body,
            json!({"data": {
            "syncId": "sync_fixture", "syncType": 0, "ek": "public_ek", "ik": "public_ik",
            "toDevice": 0, "tempKey": "", "deviceName": "Zalo Rust", "ver": 1, "ussidx": 0,
            "req": {"type": "conversation", "priority": 0, "batchSize": 2000,
                "queries": [{"partition": "", "from": 0, "to": 123, "limit": 2147483647}]}
        }, "reqId": "req_conv_sync_f"})
        );
        assert!(body.get("req_id").is_none());
        let (_, cmd, sub, body) = decode_frame(
            &sync_v2_frame(&WsCommand::DisposeSyncV2 {
                sync_id: "sync_fixture".into(),
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!((cmd, sub), (591, 0));
        assert_eq!(
            body,
            json!({"data": {"syncId": "sync_fixture", "toDevice": 0, "reason": 1}, "reqId": "req_disp"})
        );
        let (_, cmd, sub, body) = decode_frame(
            &sync_v2_frame(&WsCommand::WakeupSyncV2 {
                data: json!({"syncId": "sync_fixture"}),
                request_id: "req_wake".into(),
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!((cmd, sub), (592, 0));
        assert_eq!(body, json!({"syncId": "sync_fixture", "reqId": "req_wake"}));
    }

    #[test]
    fn sync_v2_preserves_session_index_and_mobile_wakeup_destination() {
        let mut command = fixture_sync_request();
        if let WsCommand::RequestSyncV2 { session_index, .. } = &mut command {
            *session_index = 7;
        }
        let (_, _, _, body) = decode_frame(&sync_v2_frame(&command).unwrap()).unwrap();
        assert_eq!(body["data"]["ussidx"], 7);
        let (_, cmd, _, body) = decode_frame(
            &sync_v2_frame(&WsCommand::WakeupSyncV2 {
                data: json!({"syncId":"same-session","toDevice":0}),
                request_id: "req_wake".into(),
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(cmd, 592);
        assert_eq!(
            body,
            json!({"syncId":"same-session","toDevice":0,"reqId":"req_wake"})
        );
        assert!(body.get("data").is_none());
    }

    #[tokio::test]
    async fn sync_v2_pre_auth_fifo_and_events_share_connection() {
        assert!(std::env::var("ZCALL_WS").unwrap_or_default().is_empty());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut session = Session::new(false, false, false).unwrap();
        session.domains.ws = vec![format!("ws://{}", listener.local_addr().unwrap())];
        let (mut events, commands) = spawn_with_history(session);
        commands.send(fixture_sync_request()).unwrap();
        commands
            .send(WsCommand::DisposeSyncV2 {
                sync_id: "sync_fixture".into(),
            })
            .unwrap();
        let server = async {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            assert!(tokio::time::timeout(Duration::from_millis(40), ws.next())
                .await
                .is_err());
            let auth =
                json!({"encrypt": 0, "data": json!({"error_code": 0, "data": {}}).to_string()});
            ws.send(Message::Binary(encode_frame(1, 0, 1, Some(&auth))))
                .await
                .unwrap();
            for expected in [590, 591] {
                let frame = ws.next().await.unwrap().unwrap().into_data();
                assert_eq!(decode_frame(&frame).unwrap().1, expected);
            }
            for expected in [610, 611] {
                let frame = ws.next().await.unwrap().unwrap().into_data();
                let (_, cmd, sub, data) = decode_frame(&frame).unwrap();
                assert_eq!((cmd, sub), (expected, 1));
                assert_eq!(data["lastId"], Value::Null);
            }
            assert_eq!(events.recv().await.unwrap().cmd, CMD_AUTHEN);
            for cmd in [590, 601] {
                let mut payload = json!({"data": {"act": "transfer_sync2", "syncId": "sync_fixture", "status": 3}});
                if cmd == CMD_SYNC_V2_REQUEST {
                    payload["data"]["more"] = json!(1);
                }
                ws.send(Message::Binary(encode_frame(cmd, 0, 1, Some(&payload))))
                    .await
                    .unwrap();
                let event = events.recv().await.unwrap();
                assert_eq!(event.cmd, cmd);
                assert_eq!(event.payload, payload);
                assert!(tokio::time::timeout(Duration::from_millis(40), ws.next())
                    .await
                    .is_err());
            }
            ws.close(None).await.unwrap();
            let disconnected = events.recv().await.unwrap();
            assert_eq!(disconnected.cmd, CMD_TRANSPORT_STATUS);
            assert_eq!(disconnected.payload, json!({"transport": "disconnected"}));
            drop(events);
            tokio::time::timeout(Duration::from_secs(1), commands.closed())
                .await
                .unwrap();
        };
        tokio::time::timeout(Duration::from_secs(3), server)
            .await
            .unwrap();
    }

    #[test]
    fn direct_history_uses_nullable_cursor_and_req_id() {
        for cursor in [None, Some("9007199254740993")] {
            let (version, cmd, sub, data) =
                decode_frame(&direct_history_frame(cursor, "req_7")).unwrap();
            assert_eq!((version, cmd, sub), (1, 510, 1));
            assert_eq!(
                data,
                json!({
                    "first": true, "lastId": cursor, "preIds": [], "req_id": "req_7"
                })
            );
            assert!(data.get("reqId").is_none());
        }
        let (_, _, _, offline) = decode_frame(&pull_frame(510, 1, "req_8")).unwrap();
        assert_eq!(offline["lastId"], 1);
        assert_eq!(offline["reqId"], "req_8");
    }

    #[test]
    fn pending_history_survives_reconnect_and_completes_once() {
        let mut history = HistoryQueue::default();
        history.push(WsCommand::RequestDirectHistory { last_id: None });
        history.push(WsCommand::RequestDirectHistory {
            last_id: Some("42".into()),
        });
        assert!(history.selected);
        assert_eq!(history.requests.front(), Some(&None));
        history.in_flight = true;
        // Reconnect only resets the sent flag, never the pending request.
        history.in_flight = false;
        assert_eq!(history.requests.len(), 2);
        history.in_flight = true;
        history.complete();
        history.complete();
        assert_eq!(history.requests.len(), 1);
        assert_eq!(history.requests.front(), Some(&Some("42".into())));
    }

    #[test]
    fn explicit_retry_rearms_pending_history_without_queue_growth() {
        let mut history = HistoryQueue::default();
        history.push(WsCommand::RequestDirectHistory {
            last_id: Some("42".into()),
        });
        history.push(WsCommand::RequestDirectHistory {
            last_id: Some("41".into()),
        });
        for _ in 0..32 {
            history.in_flight = true;
            history.push(WsCommand::RequestDirectHistory {
                last_id: Some("42".into()),
            });
            assert!(!history.in_flight);
            assert_eq!(history.requests.len(), 2);
            assert_eq!(history.requests.front(), Some(&Some("42".into())));
            // Repeated queued cursors must not grow the queue or re-arm the front.
            history.in_flight = true;
            history.push(WsCommand::RequestDirectHistory {
                last_id: Some("41".into()),
            });
            assert!(history.in_flight);
            assert_eq!(history.requests.len(), 2);
        }
        history.complete();
        assert_eq!(history.requests.front(), Some(&Some("41".into())));

        let mut initial_page = HistoryQueue::default();
        initial_page.push(WsCommand::RequestDirectHistory { last_id: None });
        initial_page.in_flight = true;
        initial_page.push(WsCommand::RequestDirectHistory { last_id: None });
        assert!(!initial_page.in_flight);
        assert_eq!(initial_page.requests.len(), 1);
    }

    #[tokio::test]
    async fn pre_auth_request_uses_same_socket_without_auto_pagination() {
        // This test must never honor a live endpoint override.
        assert!(std::env::var("ZCALL_WS").unwrap_or_default().is_empty());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let mut session = Session::new(false, false, false).unwrap();
        session.domains.ws = vec![format!("ws://{address}")];
        let (events, commands) = spawn_with_history(session);
        commands
            .send(WsCommand::RequestDirectHistory { last_id: None })
            .unwrap();
        let server = async {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            // A command sent before authentication must not reach the wire yet.
            assert!(tokio::time::timeout(Duration::from_millis(40), ws.next())
                .await
                .is_err());
            let auth = json!({ "encrypt": 0, "data": json!({
                "error_code": 0, "data": { "qCmds": [{"cmd": 510, "subCmd": 1}] }
            }).to_string() });
            ws.send(Message::Binary(encode_frame(1, 0, 1, Some(&auth))))
                .await
                .unwrap();
            for expected in [610, 611] {
                let frame = ws.next().await.unwrap().unwrap().into_data();
                let (_, cmd, sub, data) = decode_frame(&frame).unwrap();
                assert_eq!((cmd, sub), (expected, 1));
                assert!(data["lastId"].is_null());
            }
            let frame = ws.next().await.unwrap().unwrap().into_data();
            let (_, cmd, sub, data) = decode_frame(&frame).unwrap();
            assert_eq!((cmd, sub), (510, 1));
            assert_eq!(
                data,
                json!({"first": true, "lastId": null, "preIds": [], "req_id": "req_2"})
            );
            let response = json!({"data": {"msgs": [{"msgId": "42"}], "more": 1}});
            ws.send(Message::Binary(encode_frame(510, 1, 1, Some(&response))))
                .await
                .unwrap();
            let mut events = events;
            assert_eq!(events.recv().await.unwrap().cmd, CMD_AUTHEN);
            assert_eq!(events.recv().await.unwrap().payload, response);
            // `more: 1` is surfaced unchanged, not turned into an automatic page.
            assert!(tokio::time::timeout(Duration::from_millis(40), ws.next())
                .await
                .is_err());
            drop(events);
            tokio::time::timeout(Duration::from_secs(1), commands.closed())
                .await
                .unwrap();
        };
        tokio::time::timeout(Duration::from_secs(3), server)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn dropping_receiver_stops_during_handshake() {
        assert!(std::env::var("ZCALL_WS").unwrap_or_default().is_empty());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut session = Session::new(false, false, false).unwrap();
        session.domains.ws = vec![format!("ws://{}", listener.local_addr().unwrap())];
        let (events, commands) = spawn_with_history(session);
        let (_stream, _) = listener.accept().await.unwrap();
        drop(events);
        tokio::time::timeout(Duration::from_secs(1), commands.closed())
            .await
            .unwrap();
    }

    #[test]
    fn frame_roundtrip() {
        let body = json!({ "a": 1, "b": "x" });
        let f = encode_frame(604, 0, 1, Some(&body));
        assert_eq!(f[0], 1);
        assert_eq!(u16::from_le_bytes([f[1], f[2]]), 604);
        assert_eq!(f[3], 0);
        let (v, cmd, sub, val) = decode_frame(&f).expect("decode");
        assert_eq!((v, cmd, sub), (1, 604, 0));
        assert_eq!(val, body);
    }

    #[test]
    fn frame_without_body() {
        let f = encode_frame(2, 1, 1, None);
        assert_eq!(f.len(), 4);
        let (_, cmd, sub, val) = decode_frame(&f).expect("decode");
        assert_eq!((cmd, sub), (2, 1));
        assert!(val.is_null());
    }

    #[test]
    fn queues_follow_auth_lists() {
        let auth = json!({ "error_code": 0, "data": {
            "ctrl_qCmds": [
                { "cmd": 610, "subCmd": 1 }, { "cmd": 610, "subCmd": 1 }, { "cmd": 603, "subCmd": 1 }
            ],
            "qCmds": [{ "cmd": 511, "subCmd": 1 }]
        } });
        assert_eq!(queues_from_auth(&auth), vec![(610, 1), (603, 1), (511, 1)]);

        // An empty list stays empty: injecting an unowned queue kills the socket.
        let auth = json!({ "error_code": 0, "data": {} });
        assert!(queues_from_auth(&auth).is_empty());
    }

    #[test]
    fn percent_decode_passthrough() {
        assert_eq!(percent_decode("abc+/="), "abc+/=");
        assert_eq!(percent_decode("a%2Bb"), "a+b");
    }

    #[test]
    fn gcm_rejects_short_blob() {
        let err = gcm_decrypt(&[0u8; 32], &[0u8; 16]).unwrap_err().to_string();
        assert!(err.contains("too short"), "{err}");
    }
}

#[cfg(test)]
mod reaction_history_tests {
    use super::*;
    #[test]
    fn reaction_replay_is_bounded_and_stops_on_repeated_cursor_or_errors() {
        let mut p = ReactionHistoryPull::new(610);
        let f = p.frame("r1").unwrap();
        let (_, cmd, sub, body) = decode_frame(&f).unwrap();
        assert_eq!((cmd, sub), (610, 1));
        assert!(body["lastId"].is_null());
        assert!(p.frame("r2").is_none());
        p.accept(&json!({"data":{"reacts":[{"msgId":"22"}]}}));
        assert_eq!(p.cursor.as_deref(), Some("22"));
        assert!(p.frame("r3").is_some());
        p.accept(&json!({"data":{"reacts":[{"msgId":"22"}]}}));
        assert!(!p.pending);
        let mut p = ReactionHistoryPull::new(611);
        for n in 0..20 {
            assert!(p.frame("r").is_some());
            p.accept(&json!({"data":{"reactGroups":[{"msgId":(100-n).to_string()}]}}));
        }
        assert!(!p.pending);
        assert_eq!(p.pages, 20);
        let mut p = ReactionHistoryPull::new(610);
        p.frame("r");
        p.accept(&json!({"error_code":114,"data":{"reacts":[{"msgId":"7"}]}}));
        assert!(!p.pending);
    }
    #[test]
    fn direct_and_group_history_queues_are_independent() {
        let mut q = HistoryQueue::default();
        q.push(WsCommand::RequestDirectHistory { last_id: None });
        q.push(WsCommand::RequestGroupHistory {
            last_id: Some("33".into()),
        });
        q.in_flight = true;
        q.group_in_flight = true;
        q.complete();
        assert!(q.requests.is_empty());
        assert_eq!(q.group_requests.len(), 1);
        assert!(q.group_in_flight);
        let (_, cmd, sub, body) = decode_frame(&group_history_frame(Some("33"), "r")).unwrap();
        assert_eq!((cmd, sub), (511, 1));
        assert_eq!(body["lastId"], "33");
    }
}
