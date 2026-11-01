//! ParamCipher v2 — per-request parameter encryption.
//!
//! Ported from `compact-app-pc-*.js` class `C`. Evidence: `re-notes/01_login.md` §2.1.
//!
//! The server rejects plain `params=` with `18060 Invalid encryption protocol`,
//! so every authenticated and login call must use this path.

use anyhow::Result;
use serde_json::{json, Map, Value};

use crate::crypto;

/// `zsecureSecrets().getParamCipherKey()` — 32 ASCII chars, used directly as the
/// AES-256 key (`{key:"utf-8"}`).
pub const PARAM_CIPHER_KEY: &[u8] = b"3FC4F0D2AB50057BCE0D90D9187A22B1";

/// Split a string into chars at even and odd indices. `processStr`.
fn split_even_odd(s: &str) -> (Vec<char>, Vec<char>) {
    let chars: Vec<char> = s.chars().collect();
    let even = chars.iter().step_by(2).copied().collect();
    let odd = chars.iter().skip(1).step_by(2).copied().collect();
    (even, odd)
}

/// `Math.random().toString(16).substr(2, n)` — 8 random hex chars would be 4 bytes;
/// the bundle produces 6..12 hex chars.
pub fn random_hex(len: usize) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let mut seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1)
        ^ (std::process::id() as u64) << 23;
    const HEX: &[u8] = b"0123456789abcdef";
    let mut out = String::with_capacity(len);
    for _ in 0..len {
        // xorshift64*
        seed ^= seed >> 12;
        seed ^= seed << 25;
        seed ^= seed >> 27;
        let r = seed.wrapping_mul(0x2545F4914F6CDD1D);
        out.push(HEX[(r >> 33) as usize % 16] as char);
    }
    out
}

/// `C.encodeAES(key, text, "hex", true)` — AES-256-CBC, zero IV, PKCS7, hex, uppercased.
fn encode_hex_upper(key: &[u8], text: &str) -> Result<String> {
    let b64 = crypto::encode_aes(key, text)?;
    use base64::{engine::general_purpose::STANDARD as B64, Engine};
    let raw = B64.decode(b64)?;
    Ok(raw.iter().fold(String::new(), |mut acc, b| {
        use std::fmt::Write;
        let _ = write!(acc, "{b:02X}");
        acc
    }))
}

/// One ParamCipher instance = one request's worth of encryption material.
pub struct ParamCipher {
    /// `zcid` — AES(key, "{type},{imei},{firstLaunchTime}") hex uppercase.
    pub zcid: String,
    /// `zcid_ext` — 6..12 random hex chars.
    pub zcid_ext: String,
    /// Derived AES key for the payload.
    pub encrypt_key: String,
    pub enc_ver: &'static str,
}

impl ParamCipher {
    /// `new C({type, imei, firstLaunchTime})`.
    pub fn new(api_type: &str, imei: &str, first_launch_time: i64) -> Result<Self> {
        let plain = format!("{api_type},{imei},{first_launch_time}");
        let zcid = encode_hex_upper(PARAM_CIPHER_KEY, &plain)?;
        let zcid_ext = random_hex(10);
        // createEncryptKey(): md5(zcid_ext) hex UPPER, then processStr.
        let md5_upper = crypto::md5_hex(&zcid_ext).to_uppercase();
        let (n_even, _) = split_even_odd(&md5_upper);
        let (a_even, mut a_odd) = split_even_odd(&zcid);
        a_odd.reverse();

        let key: String = n_even.iter().take(8).collect::<String>()
            + &a_even.iter().take(12).collect::<String>()
            + &a_odd.iter().take(12).collect::<String>();

        Ok(Self {
            zcid,
            zcid_ext,
            encrypt_key: key,
            enc_ver: "v2",
        })
    }

    /// `getParams()` → `{zcid, zcid_ext, enc_ver}`.
    pub fn params(&self) -> Value {
        json!({
            "zcid": self.zcid,
            "zcid_ext": self.zcid_ext,
            "enc_ver": self.enc_ver,
        })
    }

    /// Encrypt a payload object → base64(AES(encrypt_key, JSON)).
    pub fn encrypt_payload(&self, payload: &Value) -> Result<String> {
        crypto::encode_aes(self.encrypt_key.as_bytes(), &payload.to_string())
    }

    /// The full request body, per `encryptParam(tag)`.
    ///
    /// [O] Signed field set, confirmed byte-exact against a working live request:
    /// `{client_version, enc_ver, params, type, zcid, zcid_ext}` — sorted by key name.
    /// Anything added afterwards (`ref`, `nretry`) must stay out of the hash.
    ///
    /// The `tag` is the endpoint name lowercased with dashes removed:
    /// `/api/login/getLoginInfo` → `getlogininfo`.
    pub fn build_body(
        &self,
        api_type: &str,
        api_version: &str,
        payload: &Value,
        tag: &str,
    ) -> Result<Value> {
        let encrypted = self.encrypt_payload(payload)?;

        let mut m = Map::new();
        for (k, v) in self.params().as_object().cloned().unwrap_or_default() {
            m.insert(k, v);
        }
        m.insert("params".into(), json!(encrypted));
        m.insert("type".into(), json!(api_type));
        m.insert("client_version".into(), json!(api_version));

        let sig = crypto::sign_key(tag, &m);
        m.insert("signkey".into(), json!(sig));
        Ok(Value::Object(m))
    }

    /// The AES key for this request. **This is the only key**: it decrypts the response
    /// too. There is no separate `enk` query parameter in the real protocol.
    ///
    /// [O] Verified against a live Zalo Web session: `params` and the response `data`
    /// both decrypt with this value and zero IV.
    pub fn key(&self) -> &str {
        &self.encrypt_key
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zcid_is_aes_hex_upper() {
        let p = ParamCipher::new("30", "abc", 1000).unwrap();
        // "30,abc,1000" = 11 bytes → one 16-byte block → 32 hex chars.
        assert_eq!(p.zcid.len() % 32, 0);
        assert!(!p.zcid.is_empty());
        assert!(p
            .zcid
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_lowercase()));
    }

    #[test]
    fn encrypt_key_is_32_chars() {
        let p = ParamCipher::new("30", "abc", 1000).unwrap();
        assert_eq!(p.encrypt_key.chars().count(), 32);
    }

    #[test]
    fn payload_roundtrips_with_encrypt_key() {
        let p = ParamCipher::new("30", "abc", 1000).unwrap();
        let payload = json!({"phone_num": "0123"});
        let ct = p.encrypt_payload(&payload).unwrap();
        let pt = crypto::decode_aes(p.encrypt_key.as_bytes(), &ct).unwrap();
        assert_eq!(pt, payload.to_string());
    }

    #[test]
    fn body_carries_v2_markers() {
        let p = ParamCipher::new("30", "abc", 1000).unwrap();
        let b = p
            .build_body("30", "693", &json!({"a": 1}), "loginbypasswv2")
            .unwrap();
        assert_eq!(b["enc_ver"], "v2");
        assert_eq!(b["type"], "30");
        assert_eq!(b["client_version"], "693");
        assert!(b["signkey"].as_str().unwrap().len() == 32);
        assert!(b.get("params").is_some());
    }

    #[test]
    fn signkey_matches_js_semantics() {
        // Reproduce getSignKey exactly: sort keys, concat String(value).
        let mut m = serde_json::Map::new();
        m.insert("zcid".into(), json!("A"));
        m.insert("enc_ver".into(), json!("v2"));
        m.insert("type".into(), json!("30"));
        m.insert("params".into(), json!("P"));
        m.insert("client_version".into(), json!("693"));
        m.insert("zcid_ext".into(), json!("B"));
        // sorted: client_version, enc_ver, params, type, zcid, zcid_ext
        let expect = crypto::md5_hex("zsecureTAG693v2P30AB");
        assert_eq!(crypto::sign_key("TAG", &m), expect);
    }

    #[test]
    fn random_hex_length() {
        assert_eq!(random_hex(10).len(), 10);
        assert!(random_hex(10)
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }

    /// Real capture from a live login: the server answered error_code 0 and returned this
    /// blob. Decrypting with our `enk` must yield a JSON envelope, proving the key
    /// derivation is byte-exact even though the *signature* was rejected (18022).
    #[test]
    fn decrypts_real_login_response() {
        const ENK: &str = "0FCB6C6570D6E640AC3600DB0FED524F";
        const DATA: &str = "/AUIVJkkEIKTFZRhudPnX2ACqczgaUoCGbNa1ghAj/LnlDAFGlXg4ph8xovT2dKOY9sodSJji03BTHxGufRR5tjZsREykolY59EYa2/5xkoPmePwPeeIF3DgDZ4CD3xlopudNCKuD1K16Kre9yIUm+j1uhXbXbnwVvrYA8UAQnpMME3tjTAle2W4HaNd8YKt";
        let pt =
            crypto::decode_aes(ENK.as_bytes(), DATA).expect("enk must decrypt the real response");
        let v: Value = serde_json::from_str(&pt).expect("decrypted payload is JSON");
        // The envelope carries the real verdict — this is what the client must read.
        assert_eq!(
            v["error_code"], 18022,
            "captured reply was Invalid Signature"
        );
        assert_eq!(v["error_message"], "Invalid Signature");
    }
}
