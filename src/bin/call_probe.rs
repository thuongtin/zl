//! Headless VoIP signaling probe (dev tool, no GUI).
//!
//! Reads `session.json` from the current directory, so run it once inside each
//! instance directory (`C:\zalo-test\A`, `D:\KimiData`) or copy the exe there.
//!
//! Modes:
//!   call_probe info                              — session + domains summary
//!   call_probe watch [SECS]                      — print every decrypted VoIP control event
//!   call_probe call <UID> [--video] [--zrtp] [--secs N]
//!                                                — play the caller role (401 -> 416 -> listen)
//!   call_probe serve [--auto-answer SECS] [--reject] [--zrtp] [--secs N]
//!                                                — play the callee role (listen -> 402 -> listen)
//!
//! `--zrtp` also registers the ZRTP UDP relay session exactly like the GUI engine does;
//! without it only signaling is exercised (useful to isolate signaling from media).

use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use zalo_rust::call::{
    build_callee_extend_data, build_pure_rust_invite_fields_with_probe, dispatch_send_signal,
    event_media_params, extract_voip_controls, generate_call_session, official_imei,
    poll_voip_controls, start_zrtp_udp_relay_session_with_probe, trace_line, uid_str_to_u32,
    CallType, CtrlPollState,
};
use zalo_rust::session::Session;

const POLL_MS: u64 = 700;

fn val_i64(v: &Value, keys: &[&str]) -> i64 {
    for k in keys {
        if let Some(x) = v.get(*k) {
            if let Some(n) = x.as_i64() {
                return n;
            }
            if let Some(s) = x.as_str() {
                if let Ok(n) = s.trim().parse::<i64>() {
                    return n;
                }
            }
        }
    }
    0
}

fn val_str(v: &Value, keys: &[&str]) -> String {
    for k in keys {
        if let Some(x) = v.get(*k) {
            if let Some(s) = x.as_str() {
                if !s.is_empty() {
                    return s.to_string();
                }
            }
            if let Some(n) = x.as_u64() {
                return n.to_string();
            }
        }
    }
    String::new()
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string())
}

fn load_session() -> Result<Session> {
    let mut s = Session::new(false, false, false)?;
    if let Err(e) = s.load_from_file("session.json") {
        bail!("không đọc được session.json trong thư mục hiện tại ({e})");
    }
    if s.login.is_none() {
        bail!("session.json không có thông tin đăng nhập");
    }
    // Saved sessions carry the legacy 24-hex imei; the official client uses `uuid-md5(ua)`
    // (69 chars) and the voicecall backend treats the legacy shape as a degraded client.
    if s.imei.len() != 69 {
        println!("imei upgraded: {} -> {}", s.imei, official_imei());
        s.imei = official_imei();
    }
    Ok(s)
}

fn wants(args: &[String], flag: &str) -> bool {
    args.iter().any(|a| a == flag)
}

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    for i in 0..args.len() {
        if args[i] == flag {
            if let Some(v) = args.get(i + 1) {
                if !v.starts_with("--") {
                    return Some(v.clone());
                }
            }
        }
    }
    None
}

/// Rewrite the `extendData` blob with `ZCALL_*` env overrides so that invite deltas can be
/// tested one at a time without editing `call.rs`. Every applied override is printed, so a
/// probe log always states exactly which shape was sent.
///
/// Supported (all optional, none set = byte-identical to `call.rs`):
///   ZCALL_NUMSERVERS=<n|auto>       `numServers`; `auto` = `serverResult.len()`
///   ZCALL_RECV=<n>                  `serverResult[*].recv`
///   ZCALL_RTT=<n>                   `serverResult[*].rtt`
///   ZCALL_P2P_PUBLIC=<ip:port>      append `{"ip","port","type":1}` to `p2p`
///   ZCALL_P2P_DROP_PRIVATE=1        drop every `p2p` entry with `type:0`
fn apply_extend_tweaks(extend_str: String) -> String {
    let mut obj: Value = match serde_json::from_str(extend_str.trim()) {
        Ok(v) => v,
        Err(e) => {
            println!("WARN: extendData is not JSON ({e}); sending it unchanged");
            return extend_str;
        }
    };
    let mut applied: Vec<String> = Vec::new();

    if let Some(raw) = std::env::var("ZCALL_NUMSERVERS")
        .ok()
        .filter(|s| !s.is_empty())
    {
        let n: usize = if raw.eq_ignore_ascii_case("auto") {
            obj.get("serverResult")
                .and_then(Value::as_array)
                .map(|a| a.len())
                .unwrap_or(0)
        } else {
            raw.trim().parse::<usize>().unwrap_or_else(|_| {
                println!("WARN: ZCALL_NUMSERVERS={raw:?} is not a number; ignored");
                0
            })
        };
        if let Some(o) = obj.as_object_mut() {
            o.insert("numServers".into(), json!(n));
        }
        applied.push(format!("numServers={n}"));
    }

    let recv = std::env::var("ZCALL_RECV")
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok());
    let rtt = std::env::var("ZCALL_RTT")
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok());
    if recv.is_some() || rtt.is_some() {
        let mut n = 0usize;
        if let Some(arr) = obj.get_mut("serverResult").and_then(Value::as_array_mut) {
            for entry in arr.iter_mut() {
                if let Some(e) = entry.as_object_mut() {
                    if let Some(r) = recv {
                        e.insert("recv".into(), json!(r));
                    }
                    if let Some(t) = rtt {
                        e.insert("rtt".into(), json!(t));
                    }
                    n += 1;
                }
            }
        }
        applied.push(format!("serverResult[*{n}].recv={recv:?} rtt={rtt:?}"));
    }

    if let Some(raw) = std::env::var("ZCALL_P2P_PUBLIC")
        .ok()
        .filter(|s| !s.is_empty())
    {
        // Accept `ip:port`, `ip,port` or `ip port`; a bare `ip` reuses the port already
        // published by the first private `p2p` entry (that is how the official client ships
        // its public candidate: same socket, second address).
        let private_port = obj
            .get("p2p")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(|e| e.get("port"))
            .and_then(Value::as_u64);
        let split = raw
            .rsplit_once(':')
            .or_else(|| raw.rsplit_once(','))
            .or_else(|| raw.rsplit_once(' '));
        let parsed =
            match split.and_then(|(ip, p)| p.trim().parse::<u16>().ok().map(|p| (ip.trim(), p))) {
                Some((ip, port)) => Some((ip.to_string(), port)),
                None => private_port.map(|p| (raw.trim().to_string(), p as u16)),
            };
        match parsed {
            Some((ip, port)) => {
                let entry = json!({ "ip": ip, "port": port, "type": 1 });
                if let Some(o) = obj.as_object_mut() {
                    match o.entry("p2p").or_insert_with(|| json!([])).as_array_mut() {
                        Some(arr) => arr.push(entry),
                        None => {
                            o.insert("p2p".into(), json!([entry]));
                        }
                    }
                    applied.push(format!("p2p += public {ip}:{port}"));
                }
            }
            None => println!(
                "WARN: ZCALL_P2P_PUBLIC={raw:?} is neither `ip:port` nor a bare ip with a private p2p port; ignored"
            ),
        }
    }

    if std::env::var("ZCALL_P2P_DROP_PRIVATE")
        .map(|v| v == "1")
        .unwrap_or(false)
    {
        let mut dropped = 0usize;
        if let Some(o) = obj.as_object_mut() {
            if let Some(arr) = o.get_mut("p2p").and_then(Value::as_array_mut) {
                let before = arr.len();
                arr.retain(|e| e.get("type").and_then(Value::as_i64) != Some(0));
                dropped = before - arr.len();
            }
        }
        applied.push(format!("p2p dropped {dropped} private entr(y/ies)"));
    }

    if applied.is_empty() {
        println!("invite tweaks: none (extendData untouched)");
        return extend_str;
    }
    println!("invite tweaks: {}", applied.join(" | "));
    format!("{obj}\n")
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().cloned().unwrap_or_else(|| "info".into());
    let s = load_session()?;
    let li = s.login.as_ref().expect("login checked");

    match mode.as_str() {
        "info" => {
            println!("=== CALL PROBE ===");
            println!("cwd:             {}", std::env::current_dir()?.display());
            println!("uin:             {}", li.uin);
            println!("display_name:    {}", li.display_name);
            println!(
                "imei:            {}",
                zalo_rust::session::redact_secret(&s.imei)
            );
            println!("voice_call:      {}", s.domains.voice_call);
            println!("lp_control:      {}", s.domains.lp_control);
            trace_line("PROBE", "info");
        }
        // `friends [UID]` - list friend ids; with UID checks whether that id is a friend.
        // Calling a non-friend answers `114`, so this distinguishes "not allowed" from
        // "bad payload" before blaming the payload.
        "friends" => {
            let want = args.get(1).cloned().unwrap_or_default();
            let list = zalo_rust::chat::get_friends(&s).await?;
            println!("=== FRIENDS: {} ===", list.len());
            for t in &list {
                println!("  {}  {}", t.id, t.name);
            }
            if !want.is_empty() {
                let hit = list.iter().any(|t| t.id.contains(&want));
                println!("is_friend({want}) = {hit}");
            }
        }
        "watch" => {
            let secs: f64 = args.get(1).and_then(|v| v.parse().ok()).unwrap_or(120.0);
            watch(&s, secs).await?;
        }
        // `friendreq <UID> [MSG]` - the desktop account refuses calls from non-friends with
        // `114`, so the probe needs a way to create the friendship the real client assumes.
        "friendreq" => {
            let uid = args.get(1).cloned().unwrap_or_default();
            if uid.is_empty() || uid.starts_with("--") {
                bail!("usage: call_probe friendreq <UID> [MSG]");
            }
            let msg = args.get(2).cloned().unwrap_or_else(|| "test call".into());
            let r = zalo_rust::chat::send_friend_request(&s, &uid, &msg).await?;
            println!("friendreq -> {r}");
        }
        "friendaccept" => {
            let uid = args.get(1).cloned().unwrap_or_default();
            if uid.is_empty() || uid.starts_with("--") {
                bail!("usage: call_probe friendaccept <UID>");
            }
            let r = zalo_rust::chat::accept_friend_request(&s, &uid).await?;
            println!("friendaccept -> {r}");
        }
        "call" => {
            let uid = args.get(1).cloned().unwrap_or_default();
            if uid.is_empty() || uid.starts_with("--") {
                bail!("usage: call_probe call <UID> [--video] [--zrtp] [--secs N]");
            }
            let video = wants(&args, "--video");
            let zrtp = !wants(&args, "--no-zrtp");
            let secs: f64 = flag_value(&args, "--secs")
                .and_then(|v| v.parse().ok())
                .unwrap_or(90.0);
            call(&s, &uid, video, zrtp, secs).await?;
        }
        "serve" => {
            let auto: Option<f64> = flag_value(&args, "--auto-answer")
                .and_then(|v| v.parse().ok())
                .map(Some)
                .unwrap_or_else(|| {
                    if wants(&args, "--auto-answer") {
                        Some(1.5)
                    } else {
                        None
                    }
                });
            let reject = wants(&args, "--reject");
            let zrtp = !wants(&args, "--no-zrtp");
            let secs: f64 = flag_value(&args, "--secs")
                .and_then(|v| v.parse().ok())
                .unwrap_or(180.0);
            // The 19-digit uid of the *caller* (a real callee knows it from its friend list).
            let caller_uid19 = flag_value(&args, "--caller-uid19").unwrap_or_default();
            serve(&s, auto, reject, zrtp, secs, caller_uid19).await?;
        }
        // `ws [--secs N]` - hold the control websocket open and print every decrypted frame.
        // This is the only transport that carries a live VoIP invite; HTTP polling sees just
        // the durable `act:"answer"` history record.
        "ws" => {
            let secs: f64 = flag_value(&args, "--secs")
                .and_then(|v| v.parse().ok())
                .unwrap_or(180.0);
            ws_watch(&s, secs).await?;
        }
        other => bail!("unknown mode `{other}` (info|watch|call|serve|ws)"),
    }
    Ok(())
}

async fn poll_once(s: &Session, st: &mut CtrlPollState, t0: Instant) -> Vec<Value> {
    match poll_voip_controls(s, st).await {
        Ok(events) => {
            for ev in &events {
                let act = ev.get("act").and_then(Value::as_str).unwrap_or("?");
                let data = ev.get("data").unwrap_or(&Value::Null);
                println!(
                    "[{:>7.2}s] EVENT act={} call={} status={}",
                    t0.elapsed().as_secs_f64(),
                    act,
                    val_i64(data, &["callId", "id"]),
                    val_i64(data, &["status"])
                );
                println!("{}", pretty(ev));
            }
            events
        }
        Err(e) => {
            println!("[{:>7.2}s] poll error: {e}", t0.elapsed().as_secs_f64());
            Vec::new()
        }
    }
}

async fn watch(s: &Session, secs: f64) -> Result<()> {
    let t0 = Instant::now();
    let mut st = CtrlPollState::default();
    println!(
        "watching VoIP control events for {secs:.0}s (cwd = {:?})",
        std::env::current_dir()?
    );
    // First poll only syncs the cursor; historical replay is discarded.
    let _ = poll_once(s, &mut st, t0).await;
    while t0.elapsed().as_secs_f64() < secs {
        let _ = poll_once(s, &mut st, t0).await;
        tokio::time::sleep(Duration::from_millis(POLL_MS)).await;
    }
    println!("watch finished");
    Ok(())
}

/// Hold the control websocket open and dump everything the server pushes.
///
/// `ZCALL_WS_TRACE=1` additionally prints the handshake, the auth body and every frame
/// envelope. Without it only decrypted payloads that look Voip-related are shown.
async fn ws_watch(s: &Session, secs: f64) -> Result<()> {
    let t0 = Instant::now();
    println!(
        "=== WS: {} endpoint(s), {}s (cwd = {:?}) ===",
        s.domains.ws.len(),
        secs,
        std::env::current_dir()?
    );
    for (i, u) in s.domains.ws.iter().enumerate() {
        println!("  [{i}] {u}");
    }
    if std::env::var("ZCALL_WS_TRACE").ok().as_deref() != Some("1") {
        println!("tip: set ZCALL_WS_TRACE=1 for raw frames + auth body");
    }

    let mut rx = zalo_rust::socket::spawn(s.clone());
    let deadline = tokio::time::Instant::now() + Duration::from_secs_f64(secs);
    let mut seen = 0usize;

    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => break,
            ev = rx.recv() => {
                let Some(ev) = ev else {
                    println!("[{:>7.2}s] socket stream ended", t0.elapsed().as_secs_f64());
                    break;
                };
                seen += 1;
                println!(
                    "[{:>7.2}s] WS cmd={} sub={} v{}",
                    t0.elapsed().as_secs_f64(),
                    ev.cmd,
                    ev.sub_cmd,
                    ev.version
                );
                println!("{}", pretty(&ev.payload));
                // Live invites arrive through the ctrl push/pull commands; surface them loudly
                // so a dialing test is unambiguous even in a long trace.
                if ev.is_voip_ish() {
                    let txt = ev.payload.to_string();
                    if txt.contains("voip") || txt.contains("request") || txt.contains("callId") {
                        println!(">>> VOIP-CANDIDATE <<<");
                    }
                }
            }
        }
    }

    println!("ws watch finished ({seen} frame(s))");
    Ok(())
}

async fn call(s: &Session, uid: &str, video: bool, zrtp: bool, secs: f64) -> Result<()> {
    let t0 = Instant::now();
    let call_type = CallType::from_flags(video, false) as u8;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(1_700_000_000_000);
    let call_id = ((now_ms % 1_800_000_000) + 100_000_000) as i64;

    // Keep the control cursor warm before dialing so we do not miss the answer event.
    let mut st = CtrlPollState::default();
    let _ = poll_once(s, &mut st, t0).await;

    println!("=== CALLER: 401 requestcall -> {uid} (callId={call_id}, type={call_type}) ===");
    // The official desktop dials in three steps for the same `callId`:
    //   1. a bare `requestcall` (`{calleeId, callId, codec:"[]", typeRequest, imei}`) that
    //      allocates the call record and returns the relay list,
    //   2. a second `requestcall` carrying the media description built from *that* relay
    //      (`{callId, calleeId, codec, extendData, rtcpAddress, rtpAddress, session}`),
    //   3. `request` (416) with `subCommand:3`.
    // Only the media-carrying 401 upgrades the callee invite (`params.status=0` + full
    // `extendData`). A 401 carrying a relay cached from a previous call leaves the invite
    // degraded (`status=5`, empty `extendData`) and the callee's `answer` (402) fails `-101`.
    let client_session = generate_call_session();
    let req_401_bare = json!({
        "calleeId": uid,
        "callId": call_id,
        "codec": "[]",
        "type": call_type,
    });
    let resp_401 = dispatch_send_signal(s, 401, &req_401_bare).await?;
    println!("401 (bare) resp:\n{}", pretty(&resp_401));
    let st_401 = val_i64(&resp_401, &["status"]);
    if st_401 != 0 {
        bail!(
            "401 refused: status={st_401} msg={}",
            val_str(&resp_401, &["msg"])
        );
    }
    let real_call_id = resp_401
        .get("id")
        .and_then(Value::as_i64)
        .unwrap_or(call_id);
    // Remember this relay for the next dial, like the official client does.
    if let Some(srv) = resp_401
        .get("servers")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
    {
        if let Ok(txt) = serde_json::to_string_pretty(srv) {
            let _ = std::fs::write("call_relay.json", txt);
        }
    }
    let from_uid = resp_401
        .get("fromId")
        .or_else(|| resp_401.get("callerId"))
        .and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_str().and_then(|x| x.parse().ok()))
        })
        .map(|v| v as u32)
        .unwrap_or_else(|| {
            s.zpw_sek
                .as_deref()
                .and_then(|sek| sek.split('.').nth(1))
                .and_then(|x| x.parse::<u32>().ok())
                .unwrap_or_else(|| {
                    uid_str_to_u32(&s.login.as_ref().map(|l| l.uin.clone()).unwrap_or_default())
                })
        });
    let partner_uid = resp_401
        .get("toId")
        .or_else(|| resp_401.get("calleeId"))
        .and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_str().and_then(|x| x.parse().ok()))
        })
        .map(|v| v as u32)
        .unwrap_or_else(|| uid_str_to_u32(uid));
    let sess_id_raw = val_str(&resp_401, &["sessId", "session"]);

    let mut bound_port = None;
    let mut zrtp_probe_opt = None;
    let (_rtp_hold, _rtcp_hold, _abort_hold) = if zrtp {
        let (rtp, rtcp, abort, probe) = start_zrtp_udp_relay_session_with_probe(
            &resp_401,
            from_uid,
            partner_uid,
            real_call_id as u32,
            true,
            &sess_id_raw,
        )
        .await;
        bound_port = Some(probe.local_port);
        println!(
            "ZRTP registered on local UDP port {} (public_p2p={:?}, token={}, echo_servers={})",
            probe.local_port,
            probe.public_p2p,
            probe.token,
            probe.echo_stats.len()
        );
        zrtp_probe_opt = Some(probe);
        (rtp, rtcp, abort)
    } else {
        (None, None, None)
    };

    let (rtp_ip, rtcp_ip, sess_from_401, codec_str, extend_str) =
        build_pure_rust_invite_fields_with_probe(
            &resp_401,
            video,
            bound_port,
            zrtp_probe_opt.as_ref(),
        );
    let extend_str = apply_extend_tweaks(extend_str);
    if let Err(e) = std::fs::write("invite_extend.json", &extend_str) {
        println!("WARN: could not write invite_extend.json ({e})");
    }
    let sess_id = if std::env::var("ZCALL_SESSION").ok().as_deref() == Some("random")
        || (sess_from_401.is_empty() && sess_id_raw.is_empty())
    {
        client_session.clone()
    } else if !sess_from_401.is_empty() {
        sess_from_401
    } else {
        sess_id_raw.clone()
    };
    println!(
        "invite fields: rtp={rtp_ip} rtcp={rtcp_ip} session_len={} port={bound_port:?}",
        sess_id.len()
    );

    // The official client has **no** media-carrying `requestcall`: `requestcall` only allocates
    // the record, and `request` (416) publishes the media. Keep the extra 401 behind a knob so
    // the two flows stay comparable (`ZCALL_FLOW=legacy`).

    let legacy_media_401 = std::env::var("ZCALL_FLOW")
        .map(|v| v == "legacy")
        .unwrap_or(false);
    if legacy_media_401 {
        println!("=== CALLER: 401 requestcall #2 (media, legacy) ===");
        let req_401_media = json!({
            "callId": real_call_id,
            "calleeId": uid,
            "codec": codec_str.clone(),
            "extendData": extend_str.clone(),
            "rtcpAddress": rtcp_ip.clone(),
            "rtpAddress": rtp_ip.clone(),
            "session": client_session.clone(),
            "type": call_type,
        });
        let resp_401_media = dispatch_send_signal(s, 401, &req_401_media).await?;
        println!("401 (media) resp:\n{}", pretty(&resp_401_media));
    }

    // `sess_id` is already resolved from `resp_401["sessId"]` above.

    println!("=== CALLER: 416 request (media + ringing) ===");
    // Official `sendRequestCall` (the only place the client publishes its media description):
    //   {calleeId, rtcpAddress, rtpAddress, codec, session, callId, imei}
    //   + {extendData, subCommand:3} — added *together*, and only when extendData is set.
    // Missing `subCommand:3` makes the record look like a legacy request: the callee still gets
    // an invite but the call never reaches a media-negotiated state, so its `answer` gets `-101`.
    let req_416 = json!({
        "calleeId": uid,
        "rtcpAddress": rtcp_ip,
        "rtpAddress": rtp_ip,
        "codec": codec_str,
        "session": sess_id,
        "callId": real_call_id,
        "imei": s.imei,
        "extendData": extend_str,
        "subCommand": 3,
    });
    let resp_416 = dispatch_send_signal(s, 416, &req_416).await?;
    println!("416 resp:\n{}", pretty(&resp_416));

    println!("listening for control events (WS + HTTP poll) for {secs:.0}s ...");
    let mut ws_rx = zalo_rust::socket::spawn(s.clone());
    while t0.elapsed().as_secs_f64() < secs {
        let mut events = Vec::new();
        while let Ok(ws_ev) = ws_rx.try_recv() {
            if ws_ev.is_voip_ish() {
                let data_node = ws_ev.payload.get("data").unwrap_or(&ws_ev.payload);
                for ev in extract_voip_controls(data_node) {
                    println!(
                        "[{:>7.2}s] WS-EVENT act={} call={} status={}",
                        t0.elapsed().as_secs_f64(),
                        ev.get("act").and_then(Value::as_str).unwrap_or("?"),
                        val_i64(ev.get("data").unwrap_or(&Value::Null), &["callId", "id"]),
                        val_i64(ev.get("data").unwrap_or(&Value::Null), &["status"])
                    );
                    events.push(ev);
                }
            }
        }
        events.extend(poll_once(s, &mut st, t0).await);
        for ev in &events {
            let act = ev.get("act").and_then(Value::as_str).unwrap_or("");
            let data = ev.get("data").unwrap_or(&Value::Null);
            let ev_call = val_i64(data, &["callId", "id"]);
            if ev_call != 0 && ev_call != real_call_id {
                println!("  (ignoring event for another call {ev_call})");
                continue;
            }
            if act == "answer" || act == "group_answer" {
                let status = val_i64(data, &["status"]);
                println!(">>> ANSWER status={status} (call {ev_call})");
                // Mirror the GUI engine: acknowledge with 408 answerack when accepted.
                if status == 0 {
                    let ack = json!({ "calleeId": uid, "callId": real_call_id });
                    let r = dispatch_send_signal(s, 408, &ack).await;
                    println!("408 answerack -> {r:?}");
                }
            }
            if matches!(
                act,
                "end_call" | "cancel" | "group_end_call" | "group_cancel"
            ) {
                println!(">>> CALL ENDED by peer (act={act})");
            }
        }
        tokio::time::sleep(Duration::from_millis(POLL_MS)).await;
    }
    Ok(())
}

async fn serve(
    s: &Session,
    auto_answer: Option<f64>,
    reject: bool,
    zrtp: bool,
    secs: f64,
    caller_uid19: String,
) -> Result<()> {
    let t0 = Instant::now();
    let mut st = CtrlPollState::default();
    // The control cursor replays recent events on the first poll, so events from a *previous*
    // probe run arrive as if they were fresh. Anything stamped before this process started is
    // stale and must not be mistaken for the invite we are waiting for.
    let start_ms: i64 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
        - 2_000;
    println!(
        "=== CALLEE: listening {secs:.0}s (auto_answer={:?}, reject={reject}, zrtp={zrtp}) ===",
        auto_answer
    );
    let _ = poll_once(s, &mut st, t0).await;
    let mut ws_rx = zalo_rust::socket::spawn(s.clone());
    let mut handled: Vec<i64> = Vec::new();
    let mut zrtp_holds = Vec::new();

    while t0.elapsed().as_secs_f64() < secs {
        let mut events = Vec::new();
        while let Ok(ws_ev) = ws_rx.try_recv() {
            if ws_ev.is_voip_ish() {
                let data_node = ws_ev.payload.get("data").unwrap_or(&ws_ev.payload);
                for ev in extract_voip_controls(data_node) {
                    let act = ev.get("act").and_then(Value::as_str).unwrap_or("?");
                    let data = ev.get("data").unwrap_or(&Value::Null);
                    println!(
                        "[{:>7.2}s] WS-EVENT act={} call={} status={}",
                        t0.elapsed().as_secs_f64(),
                        act,
                        val_i64(data, &["callId", "id"]),
                        val_i64(data, &["status"])
                    );
                    println!("{}", pretty(&ev));
                    events.push(ev);
                }
            }
        }
        events.extend(poll_once(s, &mut st, t0).await);
        for ev in &events {
            let act_type = ev.get("act_type").and_then(Value::as_str).unwrap_or("");
            let act = ev.get("act").and_then(Value::as_str).unwrap_or("");
            let data = ev.get("data").cloned().unwrap_or(Value::Null);
            if act_type != "voip" {
                continue;
            }
            // Only peer-originated call events. Our own 402 echo comes back with our uid.
            let self_uid_n = s.login.as_ref().map(|l| l.uin.clone()).unwrap_or_default();
            let uid_n = data.get("uidN").and_then(Value::as_str).unwrap_or("");
            if !self_uid_n.is_empty() && uid_n == self_uid_n {
                continue;
            }
            if !matches!(act, "request" | "group_request" | "answer" | "group_answer") {
                continue;
            }
            let ev_ts = val_i64(&data, &["ts"]);
            if ev_ts > 0 && ev_ts < start_ms {
                println!("  (skipping stale replayed event ts={ev_ts})");
                continue;
            }
            let call_id = val_i64(&data, &["callId", "id"]);
            let media_peek = event_media_params(&data);
            let invite_status = val_i64(&media_peek, &["status"]);
            let invite_ext = val_str(&media_peek, &["extendData"]);
            println!(
                ">>> INCOMING REQUEST act={act} status={} invite_status={invite_status} ext={} call={call_id}",
                val_i64(&data, &["status"]),
                invite_ext.len()
            );
            if call_id == 0 {
                println!("  (missing callId, skipped)");
                continue;
            }
            // A degraded invite (`params.status=5`, no `extendData`) is what the server emits for
            // a `requestcall` that carries no usable media description; answering it always fails
            // with `-101`. The media-carrying 401 that follows upgrades the same `callId` in
            // place, so wait for that one instead of answering the stub.
            let has_media_blob = media_peek
                .as_object()
                .map(|o| !o.is_empty())
                .unwrap_or(false);
            // A media-ready invite is identified by its non-empty `extendData`. `params.status`
            // is 5 on *every* fresh invite (the media-complete one included), so the old
            // `invite_status == 0` test rejected perfectly good invites.
            let upgraded = !has_media_blob || !invite_ext.is_empty();
            // `ZCALL_SERVE_ANSWER_STUB=1` runs the 402 variant sweep even on a degraded invite.
            // The server only pushes the media-carrying `act:"request"` to devices it has
            // registered for the voip queue, so a pure-Rust callee has to answer from the stub;
            // this knob is how the 402 field sweep gets executed.
            let force_stub = std::env::var("ZCALL_SERVE_ANSWER_STUB")
                .map(|v| v == "1")
                .unwrap_or(false);
            if !upgraded && !force_stub {
                println!("  degraded invite -> waiting for the media upgrade of call {call_id}");
                // Hypothesis: the stub invite is only a wake-up notification and the real,
                // media-carrying invite is pushed *after* the callee rings back (407). The old
                // gate refused to ring a degraded invite, which would deadlock that handshake.
                if std::env::var("ZCALL_SERVE_RING_DEGRADED")
                    .map(|v| v == "1")
                    .unwrap_or(false)
                {
                    let peer = if !uid_n.is_empty() {
                        uid_n.to_string()
                    } else {
                        caller_uid19.clone()
                    };
                    let ring = json!({
                        "callerId": peer,
                        "callId": call_id,
                        "status": 0,
                        "imei": s.imei,
                    });
                    let r = dispatch_send_signal(s, 407, &ring).await;
                    println!("  407 (degraded ringback) -> {r:?}");
                }
                continue;
            }
            if !upgraded {
                println!("  degraded invite -> answering anyway (ZCALL_SERVE_ANSWER_STUB=1)");
            }
            if handled.contains(&call_id) {
                println!("  (duplicate callId, skipped)");
                continue;
            }
            handled.push(call_id);
            let Some(delay) = auto_answer else {
                println!("  no --auto-answer: leaving it ringing");
                continue;
            };
            let caller_id = val_str(&data, &["uidFrom", "fromId", "callerId"]);
            // Media handshake for the *invite* is nested as a JSON string in `data.params`
            // (`sessId`, `rtpIP`, `codec`, `extendData`, ...), not in the outer event.
            let media = event_media_params(&data);
            let is_video = val_i64(&data, &["callType"]) == 3 || val_i64(&data, &["callType"]) == 6;
            let sess_id_raw = {
                let from_media = val_str(&media, &["sessId", "session"]);
                if from_media.is_empty() {
                    val_str(&data, &["session", "sessId"])
                } else {
                    from_media
                }
            };
            // `uidFrom` carries the peer's 32-bit id; our own 32-bit id lives in the `zpw_sek` prefix.
            let self_u32 = s
                .login
                .as_ref()
                .and_then(|l| {
                    l.zpw_sek
                        .split('.')
                        .nth(1)
                        .and_then(|x| x.parse::<u32>().ok())
                })
                .unwrap_or(0);
            let from_uid = self_u32;
            let partner_uid = val_i64(&data, &["uidFrom", "fromId", "callerId"]) as u32;

            println!(
                "  caller={caller_id} video={is_video} session_len={}",
                sess_id_raw.len()
            );

            let rtp_ip = {
                let v = val_str(&media, &["rtpIP", "rtpAddress", "rtpSerIp"]);
                if v.is_empty() {
                    val_str(&data, &["rtpAddress", "rtpIP"])
                } else {
                    v
                }
            };
            let rtcp_ip = {
                let v = val_str(&media, &["rtcpIP", "rtcpAddress"]);
                if v.is_empty() {
                    rtp_ip.clone()
                } else {
                    v
                }
            };
            let codec_str = {
                let v = val_str(&media, &["codec"]);
                if v.is_empty() {
                    val_str(&data, &["codec"])
                } else {
                    v
                }
            };
            let extend_str = val_str(&media, &["extendData"]);

            tokio::time::sleep(Duration::from_secs_f64(delay.max(0.0))).await;

            let mut zrtp_probe_opt = None;
            let mut bound_port = None;
            if zrtp {
                let relay_payload = if media.get("servers").and_then(Value::as_array).is_some() {
                    media.clone()
                } else {
                    json!({
                        "servers": [{ "rtpaddr": rtp_ip, "rtcpaddr": rtcp_ip }]
                    })
                };
                let (a, b, c, probe) = start_zrtp_udp_relay_session_with_probe(
                    &relay_payload,
                    from_uid,
                    partner_uid,
                    call_id as u32,
                    false,
                    &sess_id_raw,
                )
                .await;
                println!(
                    "  ZRTP registered on local UDP port {} (public_p2p={:?}, token={})",
                    probe.local_port, probe.public_p2p, probe.token
                );
                bound_port = Some(probe.local_port);
                zrtp_probe_opt = Some(probe);
                zrtp_holds.push((a, b, c));
            }

            let peer_thread_id = if !uid_n.is_empty() {
                uid_n.to_string()
            } else {
                caller_uid19.clone()
            };
            println!(
                "=== CALLEE: peer_thread_id={peer_thread_id} rtp={rtp_ip} rtcp={rtcp_ip} session_len={} ===",
                sess_id_raw.len()
            );

            // The official client rings back (407) before answering (`ringring` precedes `answer`).
            if !reject {
                let ring = json!({
                    "callerId": peer_thread_id.clone(),
                    "callId": call_id,
                    "status": 0,
                    "imei": s.imei,
                });
                let r = dispatch_send_signal(s, 407, &ring).await;
                println!("407 -> {r:?}");
            }

            let synth_extend = build_callee_extend_data(
                &rtp_ip,
                &rtcp_ip,
                is_video,
                bound_port,
                zrtp_probe_opt.as_ref(),
            );

            // The server error code is the oracle: `111` = a field is missing, `114` = bad id
            // form/size, `-101` = the value does not resolve to anything live. Sweep one field
            // at a time against a known-good base so the code tells us which field is at fault.
            let fake_sess: String = "A".repeat(sess_id_raw.len().max(152));
            let caller_num: Value = match peer_thread_id.parse::<i64>() {
                Ok(n) => json!(n),
                Err(_) => json!(0),
            };
            let mk = |cid: Value, st: Value, sess: &str, rtp: &str, rtcp: &str| -> Value {
                json!({
                    "callerId": cid,
                    "callId": call_id,
                    "status": st,
                    "codec": codec_str,
                    "rtcpAddress": rtcp,
                    "rtpAddress": rtp,
                    "session_id": sess,
                })
            };
            let base_cid = json!(peer_thread_id.clone());
            let target_status = if reject { 3 } else { 0 };
            let mut official_402 = mk(
                base_cid.clone(),
                json!(target_status),
                &sess_id_raw,
                &rtp_ip,
                &rtcp_ip,
            );
            if !reject {
                official_402["extendData"] = json!(synth_extend.clone());
                official_402["subCommand"] = json!(3);
            }
            let mut variants: Vec<(String, Value)> = vec![
                (
                    format!("official s{target_status}+calleeExtend+sub3"),
                    official_402,
                ),
                (
                    "status0".to_string(),
                    mk(base_cid.clone(), json!(0), &sess_id_raw, &rtp_ip, &rtcp_ip),
                ),
                (
                    "base uidN s1 sess=invite".to_string(),
                    mk(base_cid.clone(), json!(1), &sess_id_raw, &rtp_ip, &rtcp_ip),
                ),
                (
                    "callerId as number".to_string(),
                    mk(
                        caller_num.clone(),
                        json!(1),
                        &sess_id_raw,
                        &rtp_ip,
                        &rtcp_ip,
                    ),
                ),
                (
                    "session empty".to_string(),
                    mk(base_cid.clone(), json!(1), "", &rtp_ip, &rtcp_ip),
                ),
                (
                    "session garbage".to_string(),
                    mk(base_cid.clone(), json!(1), &fake_sess, &rtp_ip, &rtcp_ip),
                ),
                (
                    "rtp+rtcp empty".to_string(),
                    mk(base_cid.clone(), json!(1), &sess_id_raw, "", ""),
                ),
                (
                    "rtp 0.0.0.0:0".to_string(),
                    mk(
                        base_cid.clone(),
                        json!(1),
                        &sess_id_raw,
                        "0.0.0.0:0",
                        "0.0.0.0:0",
                    ),
                ),
                ("codec empty".to_string(), {
                    let mut v = mk(base_cid.clone(), json!(1), &sess_id_raw, &rtp_ip, &rtcp_ip);
                    v["codec"] = json!("");
                    v
                }),
                (
                    "callerId uid32".to_string(),
                    mk(
                        json!(caller_id.clone()),
                        json!(1),
                        &sess_id_raw,
                        &rtp_ip,
                        &rtcp_ip,
                    ),
                ),
                (
                    "callerId uid32 as str".to_string(),
                    mk(
                        json!(caller_id.to_string()),
                        json!(1),
                        &sess_id_raw,
                        &rtp_ip,
                        &rtcp_ip,
                    ),
                ),
                (
                    "callerId self19".to_string(),
                    mk(
                        json!(s.login.as_ref().map(|l| l.uin.clone()).unwrap_or_default()),
                        json!(1),
                        &sess_id_raw,
                        &rtp_ip,
                        &rtcp_ip,
                    ),
                ),
                (
                    "callerId uidN as number".to_string(),
                    mk(
                        caller_num.clone(),
                        json!(1),
                        &sess_id_raw,
                        &rtp_ip,
                        &rtcp_ip,
                    ),
                ),
            ];
            // The two optional extras the official builder attaches only when the callee has
            // its own media description to publish.
            {
                let mut v = mk(base_cid.clone(), json!(1), &sess_id_raw, &rtp_ip, &rtcp_ip);
                v["extendData"] = json!(if extend_str.is_empty() {
                    synth_extend.clone()
                } else {
                    extend_str.clone()
                });
                v["subCommand"] = json!(3);
                variants.push(("+extendData+subCommand3".to_string(), v));
            }
            {
                let mut v = mk(base_cid.clone(), json!(1), &sess_id_raw, &rtp_ip, &rtcp_ip);
                v["imei"] = json!(s.imei.clone());
                variants.push(("+imei".to_string(), v));
            }
            {
                let mut v = mk(base_cid.clone(), json!(1), &sess_id_raw, &rtp_ip, &rtcp_ip);
                v["callId"] = json!(call_id.to_string());
                variants.push(("callId as string".to_string(), v));
            }
            // Untested dimension: `status`. The official builder forwards whatever the native
            // engine computed (`n.status`); only 0 and 1 have been tried so far.
            for st in [2i64, 3, 4, 5] {
                variants.push((
                    format!("status{st}"),
                    mk(base_cid.clone(), json!(st), &sess_id_raw, &rtp_ip, &rtcp_ip),
                ));
            }
            // Untested dimension: whose session id the answer carries. The caller mints its own
            // `session` for its 401s; the callee may have to mint a *fresh* one rather than echo
            // the caller's `sessId` from the invite.
            {
                let callee_sess = generate_call_session();
                let v = mk(base_cid.clone(), json!(1), &callee_sess, &rtp_ip, &rtcp_ip);
                variants.push(("fresh callee session".to_string(), v));
                let mut v = mk(base_cid.clone(), json!(1), &callee_sess, &rtp_ip, &rtcp_ip);
                v["extendData"] = json!(if extend_str.is_empty() {
                    synth_extend.clone()
                } else {
                    extend_str.clone()
                });
                v["subCommand"] = json!(3);
                variants.push(("fresh session+extendData+subCommand3".to_string(), v));
            }
            // `ZCALL_CALLEE_EXTEND=<file>` injects a *foreign* `extendData` (the caller writes its
            // own to `invite_extend.json`). This answers the open question: does `402` need the
            // caller's media blob verbatim, or can the callee publish its own?
            if let Some(path) = std::env::var("ZCALL_CALLEE_EXTEND")
                .ok()
                .filter(|p| !p.is_empty())
            {
                match std::fs::read_to_string(&path) {
                    Ok(raw) if !raw.trim().is_empty() => {
                        let blob = raw.trim().to_string();
                        println!("  injected extendData from {path} ({} bytes)", blob.len());
                        for (label, with_sub) in [
                            ("+callerExtendData", false),
                            ("+callerExtendData+subCommand3", true),
                        ] {
                            let mut v =
                                mk(base_cid.clone(), json!(1), &sess_id_raw, &rtp_ip, &rtcp_ip);
                            v["extendData"] = json!(blob.clone());
                            if with_sub {
                                v["subCommand"] = json!(3);
                            }
                            variants.push((label.to_string(), v));
                        }
                    }
                    Ok(_) => println!("WARN: ZCALL_CALLEE_EXTEND={path} is empty; ignored"),
                    Err(e) => {
                        println!("WARN: ZCALL_CALLEE_EXTEND={path} unreadable ({e}); ignored")
                    }
                }
            }

            let mut answered = false;
            for (label, req_402) in variants {
                match dispatch_send_signal(s, 402, &req_402).await {
                    Ok(r) => {
                        println!("402 [{label}] OK: {}", pretty(&r));
                        answered = true;
                        break;
                    }
                    Err(e) => println!("402 [{label}] -> {e}"),
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            if !answered {
                println!("402: all variants rejected");
            }
        }
        tokio::time::sleep(Duration::from_millis(POLL_MS)).await;
    }
    println!("callee probe finished");
    Ok(())
}
