//! Compare a captured real `loginByPasswv2` request against what our client builds.
//!
//! Usage:
//!   1. Capture one request from Zalo PC (see re-notes/capture.md).
//!   2. Save the full URL (everything after `POST `) into `captured.txt`, one line.
//!   3. `cargo run --release --bin compare`
//!
//! It decrypts the captured payload (needs `enk` from the same URL) and prints every
//! field so the difference from our builder is mechanical, not guesswork.

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use md5::{Digest, Md5};
use std::collections::BTreeMap;

fn md5_hex(s: &str) -> String {
    let mut h = Md5::new();
    h.update(s.as_bytes());
    h.finalize().iter().fold(String::new(), |mut a, b| {
        use std::fmt::Write;
        let _ = write!(a, "{b:02x}");
        a
    })
}

fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("00");
                match u8::from_str_radix(hex, 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
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

fn aes_decrypt(key: &[u8], b64: &str) -> Option<String> {
    use aes::Aes256;
    use cbc::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
    let raw = B64.decode(b64.trim()).ok()?;
    let dec = cbc::Decryptor::<Aes256>::new_from_slices(key, &[0u8; 16]).ok()?;
    let mut buf = raw;
    let pt = dec.decrypt_padded_mut::<Pkcs7>(&mut buf).ok()?;
    String::from_utf8(pt.to_vec()).ok()
}

fn aes_decrypt_hex(key: &[u8], hex: &str) -> Option<String> {
    let raw: Vec<u8> = (0..hex.len() / 2)
        .filter_map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok())
        .collect();
    use aes::Aes256;
    use cbc::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
    let dec = cbc::Decryptor::<Aes256>::new_from_slices(key, &[0u8; 16]).ok()?;
    let mut buf = raw;
    let pt = dec.decrypt_padded_mut::<Pkcs7>(&mut buf).ok()?;
    String::from_utf8(pt.to_vec()).ok()
}

fn main() -> anyhow::Result<()> {
    let text = std::fs::read_to_string("captured.txt")
        .map_err(|_| anyhow::anyhow!("thiếu captured.txt — xem re-notes/capture.md"))?;
    let line = text
        .lines()
        .find(|l| l.contains("loginByPasswv2"))
        .unwrap_or(text.trim());
    let query = line.split_once('?').map(|(_, q)| q).unwrap_or(line);

    let mut f: BTreeMap<String, String> = BTreeMap::new();
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        f.insert(k.to_string(), url_decode(v));
    }

    println!("=== request fields (decoded) ===");
    for (k, v) in &f {
        let shown = if k == "enk" {
            zalo_rust::session::redact_secret(v)
        } else if v.len() > 70 {
            format!("{}… (len {})", &v[..40.min(v.len())], v.len())
        } else {
            v.clone()
        };
        println!("{k:<16} = {shown}");
    }

    // 1. Verify the captured signkey against our algorithm — this tells us whether the
    //    real client signs the same field set we do.
    let tags = ["loginbypasswv2"];
    let sets: [(&str, Vec<&str>); 4] = [
        (
            "all minus enk/ref",
            vec![
                "client_version",
                "enc_ver",
                "params",
                "type",
                "zcid",
                "zcid_ext",
            ],
        ),
        (
            "all minus ref",
            vec![
                "client_version",
                "enc_ver",
                "enk",
                "params",
                "type",
                "zcid",
                "zcid_ext",
            ],
        ),
        (
            "minus params",
            vec!["client_version", "enc_ver", "type", "zcid", "zcid_ext"],
        ),
        (
            "incl ref",
            vec![
                "client_version",
                "enc_ver",
                "params",
                "ref",
                "type",
                "zcid",
                "zcid_ext",
            ],
        ),
    ];

    println!("\n=== signkey check (against the captured value) ===");
    let want = f.get("signkey").cloned().unwrap_or_default();
    println!("captured signkey = {want}");
    for tag in tags {
        for (name, keys) in &sets {
            let mut s = String::from("zsecure");
            s.push_str(tag);
            for k in keys {
                s.push_str(f.get(*k).map(String::as_str).unwrap_or(""));
            }
            let got = md5_hex(&s);
            println!(
                "{:<18} {:<30} {} {}",
                tag,
                name,
                if got == want { "MATCH" } else { "no   " },
                got
            );
        }
    }

    // 2. Decrypt the payload so we can see the exact field set the server hashes.
    println!("\n=== payload (decrypted with enk) ===");
    let enk = f.get("enk").cloned().unwrap_or_default();
    if enk.is_empty() {
        println!("no enk in request — cannot decrypt");
    } else if let Some(pt) = f.get("params").and_then(|p| aes_decrypt(enk.as_bytes(), p)) {
        println!("{pt}");
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&pt) {
            if let Some(o) = v.as_object() {
                println!("\n--- field list ---");
                for (k, val) in o {
                    println!("{k:<18} {val}");
                }
            }
        }
    } else {
        println!(
            "decrypt failed with enk={}",
            zalo_rust::session::redact_secret(&enk)
        );
    }

    // 3. Show what zcid encodes.
    if let Some(zcid) = f.get("zcid") {
        if let Some(pt) = aes_decrypt_hex(b"3FC4F0D2AB50057BCE0D90D9187A22B1", zcid) {
            println!("\n=== zcid plaintext ===\n{pt}");
        }
    }

    Ok(())
}
