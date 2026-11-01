//! Offline decoder for captured `/api/voicecall/*` requests.
//!
//! The renderer encrypts `params` with `encodeAES(text)` (one argument), which uses the
//! login-derived `setDecryptKey`/`getSecretKey` value — the same AES-256-CBC key this crate
//! keeps in `secret_key`. So a capture can be read back byte-exactly without a proxy.
//!
//! Usage: `decrypt_capture <session.json> <capture.log> [endpoint-filter]`

use std::collections::BTreeSet;

use anyhow::Result;
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde_json::Value;

use zalo_rust::crypto::{decode_aes, parse_secret_key};

fn candidate_keys(session_path: &str) -> Result<Vec<(String, Vec<u8>)>> {
    let text = std::fs::read_to_string(session_path)?;
    let saved: Value = serde_json::from_str(&text)?;
    let mut out: Vec<(String, Vec<u8>)> = Vec::new();
    let mut seen: BTreeSet<Vec<u8>> = BTreeSet::new();
    let mut push = |label: &str, k: Vec<u8>, out: &mut Vec<(String, Vec<u8>)>| {
        if (k.len() == 16 || k.len() == 32) && seen.insert(k.clone()) {
            out.push((label.to_string(), k));
        }
    };

    if let Some(b64) = saved.get("secret_key_b64").and_then(Value::as_str) {
        if let Ok(raw) = B64.decode(b64) {
            push("secret_key_b64", raw, &mut out);
        }
    }
    if let Some(enk) = saved
        .get("login")
        .and_then(|l| l.get("zpw_enk"))
        .and_then(Value::as_str)
    {
        push("login.zpw_enk", parse_secret_key(enk), &mut out);
    }
    if let Some(enk) = saved.get("zpw_enk").and_then(Value::as_str) {
        push("zpw_enk", parse_secret_key(enk), &mut out);
    }
    Ok(out)
}

/// Pull the `params` query value out of a captured request URL.
fn params_of(url: &str) -> Option<String> {
    let i = url.find("params=")?;
    let rest = &url[i + "params=".len()..];
    let end = rest.find('&').unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                if let Ok(v) = u8::from_str_radix(hex, 16) {
                    out.push(v);
                    i += 3;
                    continue;
                }
                out.push(bytes[i]);
                i += 1;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let session_path = args.get(1).map(String::as_str).unwrap_or("session.json");
    let capture_path = args
        .get(2)
        .map(String::as_str)
        .unwrap_or("re-notes/voicecall_capture.log");
    let filter = args.get(3).map(String::as_str).unwrap_or("");

    let keys = candidate_keys(session_path)?;
    println!(
        "keys: {:?}",
        keys.iter().map(|(l, k)| (l, k.len())).collect::<Vec<_>>()
    );

    let text = std::fs::read_to_string(capture_path)?;
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("[REQ] GET ") else {
            continue;
        };
        let url = rest.trim();
        if !url.contains("/api/voicecall/") || (!filter.is_empty() && !url.contains(filter)) {
            continue;
        }
        let endpoint = url
            .split('?')
            .next()
            .unwrap_or("")
            .rsplit('/')
            .next()
            .unwrap_or("");
        let Some(enc) = params_of(url) else {
            println!("\n== {endpoint}: no params");
            continue;
        };
        let enc = url_decode(&enc);
        println!("\n== {endpoint}  (b64 {} chars)", enc.len());
        let mut decoded = false;
        for (label, key) in &keys {
            if let Ok(pt) = decode_aes(key, &enc) {
                println!("[{label}] {pt}");
                decoded = true;
                break;
            }
        }
        if !decoded {
            println!(
                "(could not decrypt with {:?})",
                keys.iter().map(|(l, _)| l).collect::<Vec<_>>()
            );
        }
    }
    Ok(())
}
