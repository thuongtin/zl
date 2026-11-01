//! Live probe: dump raw message JSON so media fields / msgType codes become visible.
//!
//! Run with: `cargo run --release --bin probe_media`
//!
//! Writes raw decrypted responses to `dump/probe-*.json` and prints a compact
//! per-message summary (msgType, media URLs) so we can classify photo/video/file.

use anyhow::Result;
use serde_json::Value;
use std::fs;
use zalo_rust::session::Session;

fn save(name: &str, v: &Value) {
    let _ = fs::create_dir_all("dump");
    let path = format!("dump/probe-{name}.json");
    let _ = fs::write(&path, serde_json::to_string_pretty(v).unwrap_or_default());
    println!("  → đã lưu {path}");
}

/// Pull the `data` payload out of a Zalo envelope, JSON-decoding string payloads.
fn unwrap_data(v: &Value) -> Value {
    let d = &v["data"];
    if let Some(s) = d.as_str() {
        if let Ok(parsed) = serde_json::from_str::<Value>(s) {
            return parsed;
        }
    }
    d.clone()
}

fn walk_msgs<'a>(v: &'a Value, out: &mut Vec<&'a Value>) {
    match v {
        Value::Array(a) => {
            for x in a {
                walk_msgs(x, out);
            }
        }
        Value::Object(m) => {
            // A message node has msgId and msgType.
            if m.contains_key("msgId") && m.contains_key("msgType") {
                out.push(v);
            }
            for (_k, x) in m {
                if x.is_array() || x.is_object() {
                    walk_msgs(x, out);
                }
            }
        }
        _ => {}
    }
}

fn summarize(m: &Value) {
    let msg_type = m["msgType"].as_i64().unwrap_or(-1);
    let msg_id = m["msgId"].as_str().unwrap_or("?");
    let from = m["uidFrom"].as_str().unwrap_or("?");
    let ts = m["ts"]
        .as_str()
        .or_else(|| m["sendDttm"].as_str())
        .unwrap_or("?");

    // `message` is usually a JSON-encoded string.
    let content = match &m["message"] {
        Value::String(s) => serde_json::from_str::<Value>(s).unwrap_or(Value::String(s.clone())),
        other => other.clone(),
    };

    let mut media: Vec<String> = Vec::new();
    for key in [
        "href",
        "thumb",
        "thumbUrl",
        "normalUrl",
        "oriUrl",
        "hdUrl",
        "rawUrl",
    ] {
        if let Some(u) = content[key].as_str() {
            if !u.is_empty() {
                media.push(format!("{key}={}", truncate(u, 110)));
            }
        }
    }
    if content.get("params").is_some() {
        media.push(format!(
            "params={}",
            truncate(&content["params"].to_string(), 90)
        ));
    }

    let text = content["title"]
        .as_str()
        .or_else(|| content["description"].as_str())
        .unwrap_or("");

    println!(
        "    msgId={msg_id} type={msg_type:<3} from={from} ts={ts}\n      content: {}",
        truncate(text, 100)
    );
    for md in &media {
        println!("      media:   {md}");
    }
    if media.is_empty() {
        println!("      media:   (none)");
    }
}

fn truncate(s: &str, n: usize) -> String {
    let clean: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if clean.chars().count() <= n {
        clean
    } else {
        let t: String = clean.chars().take(n).collect();
        format!("{t}…")
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("=== PROBE MEDIA / MSGTYPE / HISTORY ===");
    let mut s = Session::new(true, true, true)?;
    s.load_from_file("session.json")?;
    println!("Login: {:?}", s.login.as_ref().map(|l| &l.uin));

    // ---------- 1. get-last-msgs raw dump ----------
    println!("\n--- [1] get-last-msgs (raw) ---");
    let threads = match zalo_rust::chat::get_friends(&s).await {
        Ok(t) => {
            println!("  {} threads", t.len());
            t
        }
        Err(e) => {
            println!("  ! get_friends lỗi: {e}");
            Vec::new()
        }
    };
    let ids: Vec<String> = threads.iter().map(|t| t.id.clone()).collect();
    if let Ok(resp) = zalo_rust::chat::get_last_msgs(&s, &ids).await {
        save("last-msgs", &resp);
        let data = unwrap_data(&resp);
        let mut msgs = Vec::new();
        walk_msgs(&data, &mut msgs);
        println!("  tìm thấy {} message node", msgs.len());
        for m in msgs {
            summarize(m);
        }
    }

    // ---------- 2. group/history raw dump ----------
    println!("\n--- [2] group/history raw dump (thử mọi group thread) ---");
    let groups: Vec<_> = threads.iter().filter(|t| t.is_group).collect();
    if groups.is_empty() {
        println!("  không có group thread nào → endpoint này không áp dụng được cho chat 1-1");
    }
    for g in groups {
        println!("\n  group: {} ({})", g.name, g.id);
        match zalo_rust::chat::get_group_history_raw(&s, &g.id, 50).await {
            Ok(resp) => {
                save(&format!("group-history-{}", g.id), &resp);
                println!("  raw: {}", truncate(&resp.to_string(), 400));
                let data = unwrap_data(&resp);
                let mut msgs = Vec::new();
                walk_msgs(&data, &mut msgs);
                println!("  tìm thấy {} message node", msgs.len());
                for m in msgs.iter().take(30) {
                    summarize(m);
                }
            }
            Err(e) => println!("  ! lỗi: {e}"),
        }
    }

    Ok(())
}
