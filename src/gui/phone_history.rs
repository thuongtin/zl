//! Account-fenced phone-history gates and exact archived mention enrichment.
use super::Session;
use std::collections::{HashMap, HashSet};
use serde_json::{json, Value};
use anyhow::{bail, Context, Result};
fn text_id(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.is_empty() && s != "0" => Some(s.clone()),
        Value::Number(n) if n.as_u64().is_some_and(|x| x > 0) => Some(n.to_string()),
        _ => None,
    }
}
fn numeric_uid(v: &str) -> bool {
    !v.is_empty() && v != "0" && v.bytes().all(|x| x.is_ascii_digit())
}
fn code(v: Option<&Value>) -> Option<i64> {
    v.and_then(|v| {
        v.as_i64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
    })
}

async fn resolve_ids(
    s: &Session,
    ids: impl IntoIterator<Item = String>,
    mapping: &mut HashMap<String, String>,
) -> Result<()> {
    let missing: HashSet<String> = ids
        .into_iter()
        .filter(|id| !numeric_uid(id) && !id.is_empty() && !mapping.contains_key(id))
        .collect();
    let missing: Vec<_> = missing.into_iter().collect();
    for chunk in missing.chunks(50) {
        let params = s.encoded_params(&json!({"globalUids":serde_json::to_string(chunk)?}))?;
        let url = format!(
            "{}/api/gid/decrypt?{}&params={params}",
            s.domains.profile.trim_end_matches('/'),
            s.common_params()
        );
        let raw = s
            .get(&url)
            .await
            .map_err(|_| anyhow::anyhow!("Tra cứu định danh hội thoại thất bại"))?;
        let decoded = s.maybe_decrypt(raw, s.session_key_bytes()).await;
        if code(decoded.get("error_code")).is_some_and(|c| c != 0) {
            bail!("Máy chủ từ chối tra cứu định danh hội thoại");
        }
        let data = decoded.get("data").unwrap_or(&decoded);
        let data = data.get("data").unwrap_or(data);
        let data = data
            .as_object()
            .context("Phản hồi định danh hội thoại sai định dạng")?;
        for (gid, uid) in data {
            if let Some(uid) = text_id(uid).filter(|v| numeric_uid(v)) {
                mapping.insert(gid.clone(), uid);
            }
        }
    }
    Ok(())
}
fn mapped_id(id: &str, mapping: &HashMap<String, String>) -> Option<String> {
    if numeric_uid(id) {
        Some(id.into())
    } else {
        mapping.get(id).cloned()
    }
}

fn positive_mention_uid(id: &str) -> bool {
    numeric_uid(id) && id.bytes().any(|byte| byte != b'0')
}

fn archive_mention_ids(message: &crate::chat::ChatMessage) -> Vec<String> {
    message
        .history_source
        .as_ref()
        .and_then(|source| source.pointer("/meta/mentionsList"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|mention| mention.get("uid").and_then(text_id))
        .filter(|id| id != "-1" && !id.trim().is_empty() && !numeric_uid(id))
        .collect()
}

fn apply_archive_mention_mapping(
    message: &mut crate::chat::ChatMessage,
    mapping: &HashMap<String, String>,
) {
    for opaque in archive_mention_ids(message) {
        if let Some(uid) = mapping.get(&opaque).filter(|uid| positive_mention_uid(uid)) {
            message.resolved_mention_uids.insert(opaque, uid.clone());
        }
    }
}

/// Decode exact archive mention identities separately from immutable wire payloads.
/// The caller owns account/thread/request freshness checks and persistence.
pub(super) async fn enrich_archive_mentions(
    s: &Session,
    messages: &mut [crate::chat::ChatMessage],
) -> Result<()> {
    let unresolved: HashSet<_> = messages
        .iter()
        .flat_map(|message| {
            archive_mention_ids(message)
                .into_iter()
                .filter(move |opaque| {
                    !message
                        .resolved_mention_uids
                        .get(opaque)
                        .is_some_and(|uid| positive_mention_uid(uid))
                })
        })
        .collect();
    let mut mapping = HashMap::new();
    resolve_ids(s, unresolved, &mut mapping).await?;
    for message in messages {
        apply_archive_mention_mapping(message, &mapping);
    }
    Ok(())
}

pub(super) fn accepts_event(
    logged_in: bool,
    current_generation: u64,
    current_account: &str,
    generation: u64,
    account: &str,
) -> bool {
    logged_in && current_generation == generation && current_account == account
}

