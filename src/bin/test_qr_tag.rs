use anyhow::Result;
use serde_json::json;
use zalo_rust::crypto;
use zalo_rust::param_cipher::ParamCipher;
use zalo_rust::session::{obj, urlencode, Session, ZPW_TYPE, ZPW_VER};

#[tokio::main]
async fn main() -> Result<()> {
    let mut s = Session::new(false, false, false)?;
    if let Ok(resp) = zalo_rust::login::get_server_info(&s).await {
        if let Some(d) = resp.get("data") {
            s.apply_server_info(d);
        }
    }

    let client_time = zalo_rust::login::unix_now();
    let payload = obj(&[
        ("language", json!("vi")),
        ("client_time", json!(client_time)),
        ("imei", json!(s.imei)),
        ("computer_name", json!(s.computer_name)),
        ("logged_uids", json!("")),
        ("self_g", json!(0)),
        ("qr_form", json!(0)),
    ]);

    let cipher = ParamCipher::new(ZPW_TYPE, &s.imei, client_time)?;
    let encrypted = cipher.encrypt_payload(&payload)?;

    let mut base_m = serde_json::Map::new();
    for (k, v) in cipher.params().as_object().cloned().unwrap_or_default() {
        base_m.insert(k, v);
    }
    base_m.insert("params".into(), json!(encrypted));
    base_m.insert("type".into(), json!(ZPW_TYPE));
    base_m.insert("client_version".into(), json!(ZPW_VER));

    let candidate_tags = vec![
        "reqqr",
        "11133",
        "getqrcode",
        "getQRCode",
        "login",
        "",
        "req_qr",
        "REQQR",
    ];

    for tag in candidate_tags {
        let mut m = base_m.clone();
        let sig = crypto::sign_key(tag, &m);
        m.insert("signkey".into(), json!(sig));

        let query = m
            .iter()
            .map(|(k, val)| {
                let sv = match val {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                format!("{}={}", k, urlencode(&sv))
            })
            .collect::<Vec<_>>()
            .join("&");

        let url = format!("{}/api/login/reqqr?{query}", s.domains.auth);
        let resp = s.get(&url).await?;
        let decrypted = s.maybe_decrypt(resp, cipher.encrypt_key.as_bytes()).await;
        let code = decrypted
            .get("error_code")
            .and_then(|c| c.as_i64())
            .unwrap_or(-1);
        let msg = decrypted
            .get("error_message")
            .and_then(|c| c.as_str())
            .unwrap_or("");
        println!("tag: '{:<12}' -> code: {:<6} msg: {}", tag, code, msg);

        if code == 0 {
            println!(
                "SUCCESS! Decrypted payload:\n{}",
                serde_json::to_string_pretty(&decrypted)?
            );
            break;
        }
    }

    Ok(())
}
