//! Visible-only read acknowledgement and the shipped seen-v2 transport.
//! Source: pc-dist/lazy/default-login-main-startup-shared-worker-znotification.d9141cdf23123a5c2330.js.
use crate::{message_model::WireMessageRef, session::Session};
use anyhow::{bail, Result};
use serde_json::{json, Value};

/// Logical egui focus may be stale during macOS startup; require native foreground truth.
#[cfg(target_os = "macos")]
pub fn native_visible_window_foreground() -> bool {
    use std::ffi::{c_char, c_void, CStr};
    #[link(name = "AppKit", kind = "framework")]
    extern "C" {}
    #[link(name = "objc")]
    extern "C" {
        fn objc_getClass(name: *const c_char) -> *mut c_void;
        fn sel_registerName(name: *const c_char) -> *mut c_void;
        fn objc_msgSend();
    }
    unsafe fn object(receiver: *mut c_void, selector: &'static CStr) -> *mut c_void {
        let send: unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        send(receiver, sel_registerName(selector.as_ptr()))
    }
    unsafe fn flag(receiver: *mut c_void, selector: &'static CStr) -> bool {
        let send: unsafe extern "C" fn(*mut c_void, *mut c_void) -> i8 =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        !receiver.is_null() && send(receiver, sel_registerName(selector.as_ptr())) != 0
    }
    // SAFETY: Called on the main GUI thread; selectors target AppKit getters with the declared signatures and perform no activation.
    unsafe {
        let workspace = object(objc_getClass(c"NSWorkspace".as_ptr()), c"sharedWorkspace");
        let front = object(workspace, c"frontmostApplication");
        if front.is_null() {
            return false;
        }
        let pid: unsafe extern "C" fn(*mut c_void, *mut c_void) -> i32 =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        if pid(front, sel_registerName(c"processIdentifier".as_ptr())) != std::process::id() as i32
        {
            return false;
        }
        let app = object(
            objc_getClass(c"NSApplication".as_ptr()),
            c"sharedApplication",
        );
        if !flag(app, c"isActive") {
            return false;
        }
        let window = object(app, c"keyWindow");
        flag(window, c"isKeyWindow")
            && flag(window, c"isVisible")
            && !flag(window, c"isMiniaturized")
    }
}
#[cfg(not(target_os = "macos"))]
pub fn native_visible_window_foreground() -> bool {
    true
}

pub fn verified_foreground(logical_focused: bool, native_foreground: bool) -> bool {
    logical_focused && native_foreground
}

struct ReadAudit {
    output: std::path::PathBuf,
    started: std::time::Instant,
    last_write: std::time::Instant,
    frames: u64,
    logical_focused_frames: u64,
    native_foreground_frames: u64,
    blocked_logical_focus_frames: u64,
    eligible_frames: u64,
    max_candidates: usize,
    enqueued: u64,
}
/// Optional aggregate-only audit. No thread IDs, content, credentials, or network payloads.
pub fn audit_visibility(
    logical: bool,
    native: bool,
    eligible: bool,
    candidates: usize,
    enqueued: bool,
) {
    static AUDIT: std::sync::OnceLock<Option<std::sync::Mutex<ReadAudit>>> =
        std::sync::OnceLock::new();
    let Some(audit) = AUDIT.get_or_init(|| {
        std::env::var_os("DAILY_READ_AUDIT_OUTPUT").map(|output| {
            let now = std::time::Instant::now();
            std::sync::Mutex::new(ReadAudit {
                output: output.into(),
                started: now,
                last_write: now - std::time::Duration::from_secs(2),
                frames: 0,
                logical_focused_frames: 0,
                native_foreground_frames: 0,
                blocked_logical_focus_frames: 0,
                eligible_frames: 0,
                max_candidates: 0,
                enqueued: 0,
            })
        })
    }) else {
        return;
    };
    let mut audit = audit.lock().unwrap_or_else(|error| error.into_inner());
    audit.frames += 1;
    audit.logical_focused_frames += u64::from(logical);
    audit.native_foreground_frames += u64::from(native);
    audit.blocked_logical_focus_frames += u64::from(logical && !native);
    audit.eligible_frames += u64::from(eligible);
    audit.max_candidates = audit.max_candidates.max(candidates);
    audit.enqueued += u64::from(enqueued);
    let now = std::time::Instant::now();
    if now.duration_since(audit.last_write).as_secs_f64() >= 1.0 || enqueued {
        let result = json!({"surface":"actual main-thread GUI seen eligibility; aggregate only", "pid": std::process::id(), "elapsed_seconds": audit.started.elapsed().as_secs_f64(), "frames": audit.frames, "logical_focused_frames": audit.logical_focused_frames, "native_foreground_frames": audit.native_foreground_frames, "blocked_logical_focus_frames": audit.blocked_logical_focus_frames, "eligible_frames": audit.eligible_frames, "max_wire_candidates": audit.max_candidates, "seen_commands_enqueued": audit.enqueued});
        let temp = audit.output.with_extension("tmp");
        if let Ok(bytes) = serde_json::to_vec_pretty(&result) {
            if std::fs::write(&temp, bytes).is_ok() {
                let _ = std::fs::rename(temp, &audit.output);
            }
        }
        audit.last_write = now;
    }
}

#[derive(Debug, Clone, Default)]
pub struct ReadVisibilityGate {
    thread_id: String,
    eligible_since_ms: Option<u64>,
}
impl ReadVisibilityGate {
    /// Call each frame, including background frames. This never changes unread state.
    pub fn observe(
        &mut self,
        thread_id: &str,
        active: bool,
        minimized: bool,
        visible_bottom: bool,
        now_ms: u64,
    ) -> bool {
        if self.thread_id != thread_id
            || !active
            || minimized
            || !visible_bottom
            || thread_id.is_empty()
        {
            self.thread_id = thread_id.into();
            self.eligible_since_ms = None;
        }
        if !active || minimized || !visible_bottom || thread_id.is_empty() {
            return false;
        }
        let since = self.eligible_since_ms.get_or_insert(now_ms);
        if now_ms < *since {
            *since = now_ms;
        }
        now_ms.saturating_sub(*since) >= 500
    }
}

/// Local reading is independent of whether an archive row can produce a wire receipt.
/// The caller supplies only actual viewport candidates after foreground/dwell gating.
pub fn read_local_visible(
    state: &mut crate::unread::UnreadState,
    messages: &[crate::chat::ChatMessage],
) -> bool {
    if messages.is_empty() {
        return false;
    }
    let before = state.clone();
    state.read_visible(messages);
    // A manual dot has no numeric cursor and can also apply to opaque archive IDs.
    state.marked_unread = false;
    *state != before
}

/// Validate rows independently so one partial/archive identity cannot poison a batch.
pub fn verified_visible_receipts(
    thread_id: &str,
    is_group: bool,
    messages: &[crate::chat::ChatMessage],
) -> Vec<crate::chat::ChatMessage> {
    let mut verified: Vec<_> = messages
        .iter()
        .rev()
        .filter(|message| {
            message.history_source.is_none()
                && !message.is_outgoing
                && SeenBatch::from_wire(
                    thread_id,
                    is_group,
                    std::slice::from_ref(&message.wire_ref),
                    "validation",
                )
                .is_ok()
        })
        .take(50)
        .cloned()
        .collect();
    verified.reverse();
    verified
}

#[derive(Clone)]
pub struct SeenBatch {
    pub command: u32,
    pub path: &'static str,
    pub is_group: bool,
    // Private body prevents accidentally logging message/account identifiers.
    body: Value,
}
fn scalar(value: &Option<Value>) -> Option<Value> {
    value
        .as_ref()
        .filter(|v| {
            v.as_str().is_some_and(|s| !s.is_empty() && s != "0")
                || v.as_u64().is_some_and(|n| n > 0)
        })
        .cloned()
}
impl SeenBatch {
    /// Only pass source-verified wire refs. Archive/local IDs must never be adapted here.
    /// Reject a partial mapping instead of manufacturing receipt fields from display IDs.
    pub fn from_wire(
        thread_id: &str,
        is_group: bool,
        refs: &[WireMessageRef],
        imei: &str,
    ) -> Result<Self> {
        if thread_id.is_empty() || imei.is_empty() || refs.is_empty() || refs.len() > 50 {
            bail!("Invalid seen batch bounds");
        }
        let mut data = Vec::with_capacity(refs.len());
        for wire in refs {
            // Requiring original wire key msgId excludes the archive `id` fallback.
            if !wire.fields.contains_key("msgId") || wire.fields.contains_key("archive_source") {
                bail!("Unverified receipt mapping");
            }
            let (Some(cmi), Some(gmi), Some(si), Some(mut di), Some(mt)) = (
                scalar(&wire.client_id),
                scalar(&wire.server_id),
                scalar(&wire.sender),
                wire.destination
                    .as_ref()
                    .filter(|v| v.as_u64().is_some() || v.as_str().is_some_and(|s| !s.is_empty()))
                    .cloned(),
                scalar(&wire.original_type),
            ) else {
                bail!("Incomplete receipt mapping");
            };
            if wire.fields.get("msgId") != Some(&gmi) {
                bail!("Inconsistent receipt mapping");
            }
            if let Some(id) = di.as_str() {
                di = json!(id.strip_prefix('g').unwrap_or(id));
            }
            let key = |value: &Value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| value.to_string())
            };
            if (is_group && key(&di) != thread_id.strip_prefix('g').unwrap_or(thread_id))
                || (!is_group && key(&si) != thread_id)
            {
                bail!("Receipt conversation mismatch");
            }
            if !is_group && di == si {
                di = json!("0");
            }
            data.push(json!({"cmi":cmi,"gmi":gmi,"si":si,"di":di,"mt":mt,
                "st":wire.st.clone().unwrap_or(json!(-1)),"at":wire.at.clone().unwrap_or(json!(-1)),
                "cmd":wire.cmd.clone().unwrap_or(json!(-1)),"ts":wire.ts.clone().unwrap_or(json!(-1))}));
        }
        let mut infos = json!({"data":data});
        infos[if is_group { "grid" } else { "senderId" }] = json!(if is_group {
            thread_id.strip_prefix('g').unwrap_or(thread_id)
        } else {
            thread_id
        });
        Ok(Self {
            command: if is_group { 12124 } else { 12118 },
            path: if is_group {
                "/api/group/seenv2"
            } else {
                "/api/message/seenv2"
            },
            is_group,
            body: json!({"msgInfos":serde_json::to_string(&infos)?,"imei":imei}),
        })
    }
    pub fn len(&self) -> usize {
        self.body
            .get("msgInfos")
            .and_then(Value::as_str)
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
            .and_then(|v| v.get("data").and_then(Value::as_array).map(Vec::len))
            .unwrap_or(0)
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

pub fn parse_seen_response(response: &Value) -> Result<()> {
    let mut current = response.clone();
    for _ in 0..4 {
        if let Some(code) = current
            .get("error_code")
            .or_else(|| current.get("errorCode"))
        {
            if code.as_i64().or_else(|| code.as_str()?.parse().ok()) != Some(0) {
                bail!("Seen acknowledgement rejected");
            }
        }
        match current.get("data") {
            Some(Value::String(text)) => {
                current = serde_json::from_str(text)
                    .map_err(|_| anyhow::anyhow!("Invalid seen acknowledgement"))?
            }
            Some(Value::Object(_)) => current = current["data"].clone(),
            _ => {
                return if current.is_object() {
                    Ok(())
                } else {
                    bail!("Invalid seen acknowledgement")
                }
            }
        }
    }
    bail!("Nested seen acknowledgement limit")
}

/// No automatic retries: a transport failure may have delivered the write already.
/// This intentionally bypasses Session::post_form, which logs request bodies.
pub async fn send_seen(session: &Session, batch: &SeenBatch) -> Result<()> {
    if session.login.is_none() {
        bail!("No active session");
    }
    let domain = if batch.is_group {
        &session.domains.group
    } else {
        &session.domains.chat
    };
    let url = format!(
        "{}{}?{}",
        domain.trim_end_matches('/'),
        batch.path,
        session.common_params()
    );
    let parsed = reqwest::Url::parse(&url).map_err(|_| anyhow::anyhow!("Invalid seen endpoint"))?;
    if parsed.scheme() != "https"
        || !parsed
            .host_str()
            .is_some_and(|h| h.ends_with(".zalo.me") || h.ends_with(".zaloapp.com"))
    {
        bail!("Invalid seen endpoint");
    }
    let params = session.encoded_params(&batch.body)?;
    let mut request = session
        .http
        .post(url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(format!("params={params}"));
    if let Some(sek) = session.zpw_sek.as_ref().filter(|s| !s.is_empty()) {
        request = request
            .header("Cookie", format!("zpw_sek={sek}; _zlang=vi"))
            .header("zpw_sek", sek)
            .header("Origin", "https://chat.zalo.me")
            .header("Referer", "https://chat.zalo.me/");
    }
    let response = request
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("Seen delivery unknown"))?;
    if !response.status().is_success() {
        bail!("Seen HTTP failure");
    }
    let raw: Value = response
        .json()
        .await
        .map_err(|_| anyhow::anyhow!("Invalid seen response"))?;
    let decrypted = session
        .maybe_decrypt(raw, session.session_key_bytes())
        .await;
    parse_seen_response(&decrypted)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stale_logical_focus_never_authorizes_background_seen() {
        let mut gate = ReadVisibilityGate::default();
        for now in [0, 500, 1000, 10000] {
            assert!(!gate.observe(
                "fixture",
                verified_foreground(true, false),
                false,
                true,
                now
            ));
        }
        assert!(!gate.observe(
            "fixture",
            verified_foreground(false, true),
            false,
            true,
            11000
        ));
        assert!(!gate.observe(
            "fixture",
            verified_foreground(true, true),
            false,
            true,
            12000
        ));
        assert!(gate.observe(
            "fixture",
            verified_foreground(true, true),
            false,
            true,
            12500
        ));
        assert!(!gate.observe(
            "fixture",
            verified_foreground(true, false),
            false,
            true,
            12501
        ));
        assert!(!gate.observe(
            "fixture",
            verified_foreground(true, true),
            false,
            true,
            12502
        ));
    }
    #[test]
    fn source_seen_v2_body_preserves_types_zero_and_large_ids() {
        let wire = WireMessageRef::from_raw(
            &json!({"msgId":"9007199254740993001","cliMsgId":123,"uidFrom":"42","idTo":"42","msgType":"chat.text","st":0,"at":0,"cmd":501,"ts":1700000000000_i64}),
        );
        let batch = SeenBatch::from_wire("42", false, &[wire], "fixture").unwrap();
        let inner: Value = serde_json::from_str(batch.body["msgInfos"].as_str().unwrap()).unwrap();
        assert_eq!(
            inner,
            json!({"senderId":"42","data":[{"cmi":123,"gmi":"9007199254740993001","si":"42","di":"0","mt":"chat.text","st":0,"at":0,"cmd":501,"ts":1700000000000_i64}]})
        );
        assert_eq!(batch.body["imei"], "fixture");
    }
}
