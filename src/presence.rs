//! Actual last-online data, independent of friendship and message timestamps.
use crate::session::{obj, Session};
use anyhow::{bail, Result};
use serde_json::{json, Value};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Presence {
    pub last_online_ms: Option<i64>,
    pub hidden: bool,
}
impl Presence {
    pub fn parse(resp: &Value, now_ms: i64) -> Result<Self> {
        fn checked(v: &Value) -> Result<()> {
            if let Some(code) = v.get("error_code") {
                let code = code.as_i64().or_else(|| code.as_str()?.parse().ok());
                if code != Some(0) {
                    bail!("Last-online request failed");
                }
            }
            Ok(())
        }
        checked(resp)?;
        let raw = resp.get("data").unwrap_or(resp);
        let decoded;
        let data = if let Some(s) = raw.as_str() {
            decoded = serde_json::from_str::<Value>(s)?;
            &decoded
        } else {
            raw
        };
        checked(data)?;
        let hidden = data
            .pointer("/settings/show_online_status")
            .is_some_and(|v| v == &Value::Bool(false) || v.as_i64() == Some(0));
        let last = data
            .get("lastOnline")
            .and_then(|v| v.as_i64().or_else(|| v.as_str()?.parse().ok()));
        let last_online_ms = if hidden {
            None
        } else {
            last.and_then(|ts| {
                // The shipped client maps the explicit -1 sentinel to receipt time.
                let ts = if ts == -1 {
                    now_ms
                } else {
                    crate::clock::normalize_ts_ms(ts)
                };
                (ts >= 946_684_800_000 && ts <= now_ms.saturating_add(180_000)).then_some(ts)
            })
        };
        Ok(Self {
            last_online_ms,
            hidden,
        })
    }
    pub fn is_recent(&self, now_ms: i64) -> bool {
        self.is_recent_with_threshold(now_ms, DEFAULT_USER_ACTIVE_TIME_MS)
    }
    pub fn is_recent_with_threshold(&self, now_ms: i64, threshold_ms: i64) -> bool {
        !self.hidden
            && threshold_ms >= 0
            && self
                .last_online_ms
                .is_some_and(|ts| now_ms.saturating_sub(ts).abs() <= threshold_ms)
    }
    pub fn label(&self, now_ms: i64) -> String {
        if self.hidden {
            return "Không hiển thị trạng thái truy cập".into();
        }
        let Some(ts) = self.last_online_ms else {
            return "Chưa có thông tin truy cập".into();
        };
        let elapsed = now_ms.saturating_sub(ts).max(0) / 1000;
        if elapsed <= 180 {
            "Vừa mới truy cập".into()
        } else if elapsed < 3600 {
            format!("Truy cập {} phút trước", elapsed / 60)
        } else if elapsed < 86400 {
            format!("Truy cập {} giờ trước", elapsed / 3600)
        } else {
            crate::clock::local_datetime(ts)
                .map(|d| format!("Truy cập {}", d.format("%d/%m/%Y")))
                .unwrap_or_else(|| "Chưa có thông tin truy cập".into())
        }
    }
}

/// Shipped getOnlinStatus: uid, conv_type=1 (direct), imei; cmd 12090.
pub async fn get_presence(s: &Session, uid: &str) -> Result<Presence> {
    if s.login.is_none() || uid.is_empty() {
        bail!("No active conversation");
    }
    let body = obj(&[
        ("uid", json!(uid)),
        ("conv_type", json!(1)),
        ("imei", json!(s.imei)),
    ]);
    let params = s.encoded_params(&body)?;
    let url = format!(
        "{}/api/social/profile/lastOnline?{}&params={params}",
        s.domains.profile.trim_end_matches('/'),
        s.common_params()
    );
    let raw = s.get(&url).await?;
    let response = s.maybe_decrypt(raw, s.session_key_bytes()).await;
    Presence::parse(&response, crate::login::unix_now() * 1000)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parity_presence_uses_real_age_and_honors_missing_private_and_errors() {
        let now = 1793425680000_i64;
        let p = Presence::parse(
            &json!({"error_code":0,"data":{"lastOnline":now-8*3600*1000}}),
            now,
        )
        .unwrap();
        assert_eq!(p.label(now), "Truy cập 8 giờ trước");
        assert!(!p.is_recent(now));
        assert_eq!(
            Presence::parse(&json!({"data":{}}), now)
                .unwrap()
                .label(now),
            "Chưa có thông tin truy cập"
        );
        assert!(Presence::parse(&json!({"error_code":600,"data":{}}), now).is_err());
        let p = Presence::parse(
            &json!({"data":{"lastOnline":now,"settings":{"show_online_status":false}}}),
            now,
        )
        .unwrap();
        assert!(p.hidden);
        assert!(!p.is_recent(now));
        assert!(!p.label(now).contains("Vừa mới"));
        assert!(
            !Presence::parse(&json!({"data":{"lastOnline":now+3_600_000}}), now)
                .unwrap()
                .is_recent(now)
        );
        assert!(Presence::parse(&json!({"data":{"lastOnline":-1}}), now)
            .unwrap()
            .is_recent(now));
        assert!(
            Presence::parse(&json!({"data":{"lastOnline":10_000_000_000_i64}}), now)
                .unwrap()
                .last_online_ms
                .is_none()
        );
    }
}

/// Verified shipped online_configs.user_active_time = 18e4 milliseconds.
pub const DEFAULT_USER_ACTIVE_TIME_MS: i64 = 180_000;

pub fn configured_active_threshold(config: &Value) -> i64 {
    config
        .pointer("/online_configs/user_active_time")
        .and_then(Value::as_i64)
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_USER_ACTIVE_TIME_MS)
}

/// Request generation guard: failures leave the last confirmed state unchanged.
#[derive(Debug, Clone, Default)]
pub struct VersionedPresence {
    pub value: Presence,
    pub version: u64,
}
impl VersionedPresence {
    pub fn merge(&mut self, version: u64, response: Result<Presence>) -> bool {
        if version < self.version {
            return false;
        }
        self.version = version;
        let Ok(value) = response else {
            return false;
        };
        let changed = self.value != value;
        self.value = value;
        changed
    }
}
