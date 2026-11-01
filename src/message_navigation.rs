//! Exact reply references and verified, UTF-16-indexed mention spans.
use crate::chat::ChatMessage;
use serde_json::Value;
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MentionSpan {
    pub user_id: String,
    pub byte_range: Range<usize>,
}

fn exact_id(value: &Value) -> Option<String> {
    match value {
        Value::String(id) if !id.is_empty() => Some(id.clone()),
        Value::Number(id) if id.is_u64() || id.is_i64() => Some(id.to_string()),
        _ => None,
    }
}

/// No numeric coercion, text/time heuristics, or preference between colliding namespaces.
pub fn unique_reference_index(messages: &[ChatMessage], reference: &str) -> Option<usize> {
    if reference.is_empty() {
        return None;
    }
    let mut matches = messages.iter().enumerate().filter(|(_, message)| {
        message.id == reference
            || message.client_message_id == reference
            || [&message.wire_ref.server_id, &message.wire_ref.client_id]
                .into_iter()
                .flatten()
                .any(|id| exact_id(id).as_deref() == Some(reference))
    });
    let (index, _) = matches.next()?;
    matches.next().is_none().then_some(index)
}

/// JS substring offsets are UTF-16 code units, not bytes or Unicode scalars.
pub fn utf16_byte_range(text: &str, position: usize, length: usize) -> Option<Range<usize>> {
    if length == 0 {
        return None;
    }
    let end = position.checked_add(length)?;
    let boundary = |wanted| {
        let mut units = 0;
        for (byte, ch) in text.char_indices() {
            if units == wanted {
                return Some(byte);
            }
            units += ch.len_utf16();
            if units > wanted {
                return None;
            }
        }
        (units == wanted).then_some(text.len())
    };
    let range = boundary(position)?..boundary(end)?;
    let is_grapheme_boundary =
        |byte| byte == text.len() || text.grapheme_indices(true).any(|(index, _)| index == byte);
    (is_grapheme_boundary(range.start) && is_grapheme_boundary(range.end)).then_some(range)
}

/// Original shared-worker-original.js CP143496: mentions[].pos/len are passed to
/// message.substring(pos, pos + len). Archive meta.mentionsList uses the same spans;
/// opaque archive UIDs require the exact-key sidecar from the identity decoder.
pub fn message_mentions(message: &ChatMessage) -> Vec<MentionSpan> {
    let wire = message.wire_ref.fields.get("mentions");
    let archive = message
        .history_source
        .as_ref()
        .and_then(|source| source.pointer("/meta/mentionsList"));
    let Some(mentions) = wire.or(archive).and_then(Value::as_array) else {
        return vec![];
    };
    let mut result = Vec::new();
    for mention in mentions {
        let Some(mut user_id) = mention.get("uid").and_then(exact_id) else {
            continue;
        };
        if user_id.trim().is_empty() || matches!(user_id.as_str(), "0" | "-1") {
            continue;
        }
        if wire.is_none() {
            if !user_id.bytes().all(|byte| byte.is_ascii_digit()) {
                let Some(resolved) = message.resolved_mention_uids.get(&user_id) else {
                    continue;
                };
                user_id = resolved.clone();
            }
            if user_id.is_empty()
                || !user_id.bytes().all(|byte| byte.is_ascii_digit())
                || !user_id.bytes().any(|byte| byte != b'0')
            {
                continue;
            }
        }
        let Some(position) = mention
            .get("pos")
            .and_then(Value::as_u64)
            .and_then(|number| usize::try_from(number).ok())
        else {
            continue;
        };
        let Some(length) = mention
            .get("len")
            .and_then(Value::as_u64)
            .and_then(|number| usize::try_from(number).ok())
        else {
            continue;
        };
        let Some(byte_range) = utf16_byte_range(&message.text, position, length) else {
            continue;
        };
        result.push(MentionSpan {
            user_id,
            byte_range,
        });
    }
    result.sort_by_key(|span| (span.byte_range.start, span.byte_range.end));
    result.dedup();
    // Conflicting/overlapping claims do not establish a clickable identity.
    result
        .iter()
        .enumerate()
        .filter(|(index, span)| {
            !result.iter().enumerate().any(|(other, candidate)| {
                other != *index
                    && span.byte_range.start < candidate.byte_range.end
                    && candidate.byte_range.start < span.byte_range.end
            })
        })
        .map(|(_, span)| span.clone())
        .collect()
}

/// A later selection/page/jump invalidates every earlier response, including away/back.
pub fn accepts_page_result(
    current_thread: Option<&str>,
    current_epoch: u64,
    thread: &str,
    epoch: u64,
) -> bool {
    current_thread == Some(thread) && current_epoch == epoch
}

pub fn reply_reference(quote: &Value) -> Option<String> {
    ["cliMsgId", "globalMsgId", "msgId"]
        .into_iter()
        .filter_map(|key| quote.get(key).and_then(exact_id))
        .find(|id| !id.trim().is_empty() && !id.bytes().all(|b| b == b'0'))
}

pub fn message_reply_reference(message: &ChatMessage) -> Option<String> {
    message
        .reply_to_id
        .as_ref()
        .filter(|id| !id.trim().is_empty() && !id.bytes().all(|b| b == b'0'))
        .cloned()
        .or_else(|| {
            message
                .wire_ref
                .fields
                .get("quote")
                .and_then(reply_reference)
        })
        .or_else(|| {
            message
                .history_source
                .as_ref()
                .and_then(|raw| raw.pointer("/meta/quote"))
                .and_then(reply_reference)
        })
}
