//! Source-verified delivery receipts, never inferred from archive defaults or send ACKs.
//!
//! Owning source: pc-dist/lazy/default-login-main-startup-shared-worker-znotification.d9141cdf23123a5c2330.js
//! SHA256: 0a5a08a2a1d074c827d2b1826f8434fc04e243a1daa0a03a376353e54cdd2466.
//! Unicode code-point spans: command enum 7423900..7424300; group prefix
//! 1505622; getDeliveredsFromData 12307896..12308645; status application
//! 11750050..11751080; processSeenFromServer 7667818..7669200.
//! _processMessage at 12321078 ignores subcommand. No receipt version semantics
//! are established by these spans: retain raw version without inventing a clock.
use crate::{chat::Thread, message_model::DeliveryState};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeliveryEvent {
    pub thread_id: String,
    pub message_id: Option<String>,
    pub client_id: Option<String>,
    pub state: DeliveryState,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub version: Option<Value>,
    pub raw: Value,
}
fn present<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(d).map(Some)
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryUpdate {
    pub thread_id: String,
    pub message_id: String,
    pub client_id: String,
    pub state: DeliveryState,
}
fn id(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(s) if !s.is_empty() && s != "0" => Some(s.clone()),
        Value::Number(n) if n.as_u64().is_some_and(|n| n > 0) => Some(n.to_string()),
        _ => None,
    }
}
fn bit(v: Option<&Value>) -> Option<bool> {
    match v? {
        Value::Number(n) if n.as_u64() == Some(0) => Some(false),
        Value::Number(n) if n.as_u64() == Some(1) => Some(true),
        Value::String(s) if s == "0" => Some(false),
        Value::String(s) if s == "1" => Some(true),
        _ => None,
    }
}
fn own_only(record: &Value, own_uid: &str, key: &str) -> bool {
    record
        .get(key)
        .and_then(Value::as_array)
        .is_some_and(|uids| {
            !uids.is_empty() && uids.iter().all(|v| id(Some(v)).as_deref() == Some(own_uid))
        })
}

/// Accept only the original chat-handler command namespace, not control/ACK packets.
/// The shipped handler ignores subcommand for these commands.
pub fn parse_delivery_events(
    cmd: u16,
    _subcmd: u8,
    payload: &Value,
    own_uid: &str,
) -> Vec<DeliveryEvent> {
    if own_uid.is_empty() || !matches!(cmd, 501 | 502 | 521 | 522 | 534 | 570) {
        return vec![];
    }
    let mut body = payload;
    // Socket already decrypts payloads. Never parse an encrypted/string body here.
    for _ in 0..3 {
        if let Some(error) = body.get("error_code") {
            if error.as_i64() != Some(0) {
                return vec![];
            }
        }
        if body.get("delivereds").is_some()
            || body.get("groupSeens").is_some()
            || body.get("seens").is_some()
        {
            break;
        }
        match body.get("data").filter(|v| v.is_object()) {
            Some(data) => body = data,
            None => return vec![],
        }
    }
    let mut result = Vec::new();
    for key in ["delivereds", "groupSeens", "seens"] {
        let Some(records) = body.get(key).and_then(Value::as_array) else {
            continue;
        };
        for record in records {
            if !record.is_object() {
                continue;
            }
            // Explicit malformed recipient lists are not remote receipt evidence.
            if ["deliveredUids", "seenUids"].iter().any(|key| {
                record.get(*key).is_some_and(|v| {
                    v.as_array()
                        .is_none_or(|uids| uids.iter().any(|v| id(Some(v)).is_none()))
                })
            }) {
                continue;
            }
            let (thread_id, state) = if key == "seens" {
                let Some(target) = id(record.get("idTo")) else {
                    continue;
                };
                let Some(group) = bit(record.get("isGroup")) else {
                    continue;
                };
                (
                    if group { format!("g{target}") } else { target },
                    DeliveryState::Read,
                )
            } else {
                if own_only(record, own_uid, "deliveredUids") {
                    continue;
                }
                let group = id(record.get("groupId"));
                let thread = group.map(|id| format!("g{id}")).or_else(|| {
                    if key == "groupSeens" {
                        return None;
                    }
                    id(record.get("threadId")).or_else(|| {
                        record
                            .get("deliveredUids")
                            .and_then(Value::as_array)
                            .and_then(|uids| id(uids.first()))
                    })
                });
                let Some(thread) = thread else {
                    continue;
                };
                let read = if key == "groupSeens" {
                    true
                } else {
                    match record.get("seen") {
                        None => false,
                        Some(_) => match bit(record.get("seen")) {
                            Some(b) => b,
                            None => continue,
                        },
                    }
                };
                if read && own_only(record, own_uid, "seenUids") {
                    continue;
                }
                (
                    thread,
                    if read {
                        DeliveryState::Read
                    } else {
                        DeliveryState::Delivered
                    },
                )
            };
            if thread_id == own_uid {
                continue;
            }
            let message_id = id(record.get("msgId"));
            let client_id = id(record.get("cliMsgId"));
            if (key == "seens" && message_id.is_none())
                || (message_id.is_none() && client_id.is_none())
            {
                continue;
            }
            result.push(DeliveryEvent {
                thread_id,
                message_id,
                client_id,
                state,
                version: record.get("version").cloned(),
                raw: record.clone(),
            });
        }
    }
    result
}

/// Update only uniquely identified loaded outgoing messages. No range/time guesses.
pub fn apply_delivery_events(
    threads: &mut [Thread],
    events: &[DeliveryEvent],
) -> Vec<DeliveryUpdate> {
    let mut updates = Vec::new();
    for event in events {
        if !matches!(event.state, DeliveryState::Delivered | DeliveryState::Read) {
            continue;
        }
        let matching_threads: Vec<_> = threads
            .iter()
            .enumerate()
            .filter(|(_, t)| t.id == event.thread_id)
            .map(|(i, _)| i)
            .collect();
        if matching_threads.len() != 1 {
            continue;
        }
        let thread = &mut threads[matching_threads[0]];
        let exact: Vec<_> = thread
            .messages
            .iter()
            .enumerate()
            .filter(|(_, m)| m.is_outgoing && event.message_id.as_deref() == Some(m.id.as_str()))
            .map(|(i, _)| i)
            .collect();
        let index = if exact.len() == 1 {
            Some(exact[0])
        } else if exact.is_empty() {
            let clients: Vec<_> = thread
                .messages
                .iter()
                .enumerate()
                .filter(|(_, m)| {
                    m.is_outgoing
                        && event.client_id.as_deref() == Some(m.client_message_id.as_str())
                })
                .map(|(i, _)| i)
                .collect();
            if clients.len() == 1 {
                let m = &thread.messages[clients[0]];
                // A client ID cannot override a conflicting real server ID.
                (event.message_id.is_none() || m.id.starts_with("out_") || m.id.is_empty())
                    .then_some(clients[0])
            } else {
                None
            }
        } else {
            None
        };
        let Some(index) = index else {
            continue;
        };
        let message = &mut thread.messages[index];
        let state = message.delivery.merge(event.state);
        if state == message.delivery {
            continue;
        }
        message.delivery = state;
        updates.push(DeliveryUpdate {
            thread_id: thread.id.clone(),
            message_id: message.id.clone(),
            client_id: message.client_message_id.clone(),
            state,
        });
    }
    updates
}
