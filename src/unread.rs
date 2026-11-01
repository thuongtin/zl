//! Unread state follows server read cursors, never the size of a history batch.
use crate::chat::{ChatMessage, Thread};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnreadState {
    /// None means that no authoritative read point has been received.
    pub last_seen_id: Option<String>,
    #[serde(default)]
    pub unread_ids: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub mention_ids: BTreeSet<String>,
    #[serde(default)]
    pub muted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub muted_until: Option<i64>,
    #[serde(default)]
    pub marked_unread: bool,
    /// Network metadata copies may update state; ordinary history copies may not.
    #[serde(skip)]
    pub fresh: bool,
    #[serde(skip)]
    pub settings_fresh: bool,
}

fn numeric_id(id: &str) -> Option<&str> {
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let id = id.trim_start_matches('0');
    Some(if id.is_empty() { "0" } else { id })
}

pub fn compare_ids(a: &str, b: &str) -> Option<std::cmp::Ordering> {
    let (a, b) = (numeric_id(a)?, numeric_id(b)?);
    Some(a.len().cmp(&b.len()).then_with(|| a.cmp(b)))
}

impl UnreadState {
    pub fn has_unread_mentions(&self) -> bool {
        !self.mention_ids.is_empty()
    }
    pub fn count(&self) -> usize {
        self.unread_ids.len()
    }
    pub fn is_muted(&self) -> bool {
        self.muted
            && self
                .muted_until
                .is_none_or(|until| until > crate::login::unix_now())
    }
    pub fn has_badge(&self) -> bool {
        self.count() > 0 || self.marked_unread
    }
    pub fn badge(&self, is_group: bool) -> Option<String> {
        if !self.has_badge() {
            return None;
        }
        if self.is_muted() || self.count() == 0 {
            return Some(String::new());
        }
        Some(if is_group && self.count() > 5 {
            "5+".into()
        } else if self.count() > 99 {
            "99+".into()
        } else {
            self.count().to_string()
        })
    }
    pub fn merge_metadata(&mut self, incoming: &Self) -> bool {
        let before = self.clone();
        if incoming.fresh {
            if let Some(point) = &incoming.last_seen_id {
                self.advance_seen(point);
            }
            if let Some(point) = self.last_seen_id.as_deref() {
                for id in &incoming.unread_ids {
                    if compare_ids(id, point).is_some_and(|order| order.is_gt()) {
                        self.unread_ids.insert(id.clone());
                    }
                }
            }
            if incoming.settings_fresh {
                self.muted = incoming.muted;
                self.muted_until = incoming.muted_until;
                self.marked_unread = incoming.marked_unread;
            }
            self.fresh = true;
        } else if self.last_seen_id.is_none() && incoming.last_seen_id.is_some() {
            *self = incoming.clone();
        }
        if let Some(point) = self.last_seen_id.as_deref() {
            for id in &incoming.mention_ids {
                if self.unread_ids.contains(id)
                    && compare_ids(id, point).is_some_and(|order| order.is_gt())
                {
                    self.mention_ids.insert(id.clone());
                }
            }
        }
        *self != before
    }
    pub fn advance_seen(&mut self, point: &str) {
        if numeric_id(point).is_none() {
            return;
        }
        if self
            .last_seen_id
            .as_ref()
            .is_none_or(|old| compare_ids(point, old).is_some_and(|v| !v.is_lt()))
        {
            self.last_seen_id = Some(point.into());
            self.unread_ids
                .retain(|id| compare_ids(id, point).is_some_and(|v| v.is_gt()));
            self.mention_ids
                .retain(|id| compare_ids(id, point).is_some_and(|order| order.is_gt()));
        }
    }
    /// Called only for new live messages, or a read-only server-cursor snapshot.
    pub fn observe(&mut self, messages: &[ChatMessage]) {
        let Some(point) = &self.last_seen_id else {
            return;
        };
        for message in messages {
            if !message.is_outgoing
                && message.kind != crate::message_model::MessageKind::System
                && message.unread_policy.counts_unread()
                && compare_ids(&message.id, point).is_some_and(|v| v.is_gt())
            {
                self.unread_ids.insert(message.id.clone());
            }
        }
    }
    pub fn observe_for_account(&mut self, messages: &[ChatMessage], account: &str) {
        self.observe(messages);
        self.observe_mentions(messages, account);
    }
    /// Missing mention metadata never erases a previously verified off-screen mention.
    pub fn observe_mentions(&mut self, messages: &[ChatMessage], account: &str) {
        for message in messages {
            if message.is_outgoing
                || message.kind == crate::message_model::MessageKind::System
                || !message.unread_policy.counts_unread()
            {
                self.mention_ids.remove(&message.id);
                continue;
            }
            if !self.unread_ids.contains(&message.id) {
                continue;
            }
            match classify_message_mention(message, account) {
                MentionDetection::Mention => {
                    self.mention_ids.insert(message.id.clone());
                }
                MentionDetection::NoMention => {
                    self.mention_ids.remove(&message.id);
                }
                MentionDetection::Unknown => {}
            }
        }
    }
    /// Local foreground reading does not send a receipt or mutate the account.
    pub fn read_visible(&mut self, messages: &[ChatMessage]) {
        if let Some(last) = messages
            .iter()
            .filter(|m| numeric_id(&m.id).is_some())
            .max_by(|a, b| compare_ids(&a.id, &b.id).unwrap())
        {
            self.advance_seen(&last.id);
            self.marked_unread = false;
        }
        // Only the visible cursor is advanced; off-screen unread IDs remain.
    }
}

fn string(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(|v| {
        v.as_str()
            .map(str::to_owned)
            .or_else(|| v.as_u64().map(|v| v.to_string()))
    })
}

/// Original desktop consumer ignores subchat cursors (`type=2`, `sct=1/2/3`).
pub fn merge_read_points(threads: &mut [Thread], data: &Value) {
    let entries = data
        .get("clearUnreads")
        .or_else(|| data.get("clearUnreadInfo"))
        .or_else(|| data.get("statusMap").and_then(|v| v.get("clearUnreads")))
        .and_then(Value::as_array);
    for entry in entries.into_iter().flatten() {
        if entry.get("type").and_then(Value::as_i64) == Some(2) && entry.get("sct").is_some() {
            continue;
        }
        let (Some(id), Some(point)) = (string(entry, "idTo"), string(entry, "lastMsgId")) else {
            continue;
        };
        if numeric_id(&point).is_none_or(|id| id == "0") || id.is_empty() || id == "0" {
            continue;
        }
        let group = entry
            .get("isGroup")
            .is_some_and(|v| v.as_bool() == Some(true) || v.as_i64() == Some(1));
        let tid = if group {
            format!("g{}", id.trim_start_matches('g'))
        } else {
            id
        };
        if let Some(thread) = threads.iter_mut().find(|t| {
            t.id == tid
                || (group
                    && t.is_group
                    && t.id.trim_start_matches('g') == tid.trim_start_matches('g'))
        }) {
            thread.unread.advance_seen(&point);
            thread.unread.fresh = true;
        }
    }
}

/// Complete read-only mute snapshots use mode 1 and duration -1 for indefinite mute.
pub fn merge_mute_metadata(threads: &mut [Thread], data: &Value) {
    let (Some(peers), Some(groups)) = (
        data.get("chatEntries").and_then(Value::as_array),
        data.get("groupChatEntries").and_then(Value::as_array),
    ) else {
        return;
    };
    for thread in threads {
        let entries = if thread.is_group { groups } else { peers };
        let entry = entries
            .iter()
            .find(|e| string(e, "id").is_some_and(|id| id == thread.id.trim_start_matches('g')));
        let mode = entry
            .and_then(|e| e.get("muteMode"))
            .and_then(Value::as_i64);
        if mode.is_some_and(|m| m != 0 && m != 1) {
            continue;
        }
        let duration = entry
            .and_then(|e| e.get("duration"))
            .and_then(Value::as_i64);
        let until = entry
            .and_then(|e| e.get("startTime"))
            .and_then(Value::as_i64)
            .zip(duration)
            .and_then(|(start, duration)| {
                if duration >= 0 {
                    start.checked_add(duration)
                } else {
                    None
                }
            });
        thread.unread.muted = mode == Some(1);
        thread.unread.muted_until = until;
        thread.unread.settings_fresh = true;
        thread.unread.fresh = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn message(id: &str, outgoing: bool) -> ChatMessage {
        ChatMessage {
            id: id.into(),
            is_outgoing: outgoing,
            ..Default::default()
        }
    }
    #[test]
    fn unread_unknown_is_not_fabricated_from_history() {
        let mut state = UnreadState::default();
        state.observe(&[message("22", false)]);
        assert_eq!(state.count(), 0);
        assert!(state.last_seen_id.is_none());
    }
    #[test]
    fn unread_cursor_deduplicates_and_never_regresses() {
        let mut state = UnreadState::default();
        state.advance_seen("20");
        for _ in 0..3 {
            state.observe(&[
                message("21", false),
                message("22", true),
                message("19", false),
            ]);
        }
        assert_eq!(state.count(), 1);
        state.advance_seen("21");
        state.advance_seen("19");
        assert_eq!(state.count(), 0);
        assert_eq!(state.last_seen_id.as_deref(), Some("21"));
    }
    #[test]
    fn unread_large_ids_remain_exact_and_muted_is_dot() {
        let mut state = UnreadState::default();
        state.advance_seen("9007199254740993000");
        for n in 1..=6 {
            state.observe(&[message(&format!("900719925474099300{n}"), false)]);
        }
        assert_eq!(state.badge(true).as_deref(), Some("5+"));
        state.muted = true;
        assert_eq!(state.badge(true).as_deref(), Some(""));
        let saved = serde_json::to_string(&state).unwrap();
        assert_eq!(serde_json::from_str::<UnreadState>(&saved).unwrap(), state);
    }
    #[test]
    fn unread_mute_snapshot_handles_indefinite_expiry_and_omitted_entries() {
        let mut threads = vec![Thread {
            presentation: Default::default(),
            id: "42".into(),
            name: String::new(),
            preview: String::new(),
            time: String::new(),
            avatar: None,
            is_group: false,
            messages: vec![],
            unread: Default::default(),
        }];
        merge_mute_metadata(
            &mut threads,
            &serde_json::json!({"chatEntries":[{"id":"42","muteMode":1,"duration":-1,"startTime":10}],"groupChatEntries":[]}),
        );
        assert!(threads[0].unread.is_muted());
        merge_mute_metadata(
            &mut threads,
            &serde_json::json!({"chatEntries":[{"id":"42","muteMode":1,"duration":10,"startTime":10}],"groupChatEntries":[]}),
        );
        assert!(!threads[0].unread.is_muted());
        merge_mute_metadata(
            &mut threads,
            &serde_json::json!({"chatEntries":[],"groupChatEntries":[]}),
        );
        assert!(!threads[0].unread.muted);
    }
    #[test]
    fn unread_zero_and_subchat_cursors_do_not_fabricate_read_points() {
        let mut threads = Vec::new();
        merge_read_points(
            &mut threads,
            &serde_json::json!({"clearUnreads":[{"idTo":"42","lastMsgId":"0","isGroup":0},{"idTo":"43","lastMsgId":"99","isGroup":1,"type":2,"sct":1}]}),
        );
        assert!(threads.is_empty());
    }
    #[test]
    fn unread_history_defaults_preserve_state_and_foreground_read_clears() {
        let mut state = UnreadState::default();
        state.advance_seen("10");
        state.observe(&[message("11", false)]);
        state.merge_metadata(&UnreadState::default());
        assert_eq!(state.count(), 1);
        state.read_visible(&[message("11", false)]);
        assert!(!state.has_badge());
    }
}

/// Complete read-only manual-mark snapshot. No conversations are created by this adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManualUnreadMark {
    pub thread_id: String,
    pub client_message_id: String,
    pub from_uid: String,
}

pub fn parse_manual_unread_marks(response: &Value) -> anyhow::Result<Vec<ManualUnreadMark>> {
    fn checked(value: &Value) -> anyhow::Result<()> {
        if let Some(code) = value.get("error_code") {
            if code.as_i64().or_else(|| code.as_str()?.parse().ok()) != Some(0) {
                anyhow::bail!("Manual unread request failed");
            }
        }
        Ok(())
    }
    checked(response)?;
    let raw = response.get("data").unwrap_or(response);
    let decoded;
    let data = if let Some(text) = raw.as_str() {
        decoded = serde_json::from_str::<Value>(text)
            .map_err(|_| anyhow::anyhow!("Invalid manual unread payload"))?;
        &decoded
    } else {
        raw
    };
    checked(data)?;
    let (Some(groups), Some(users)) = (
        data.get("convsGroup").and_then(Value::as_array),
        data.get("convsUser").and_then(Value::as_array),
    ) else {
        anyhow::bail!("Incomplete manual unread snapshot");
    };
    let mut marks = Vec::new();
    for (rows, group) in [(groups, true), (users, false)] {
        for row in rows {
            let id = string(row, "id")
                .filter(|id| !id.is_empty() && id != "0")
                .ok_or_else(|| anyhow::anyhow!("Invalid manual unread identity"))?;
            marks.push(ManualUnreadMark {
                thread_id: if group {
                    format!("g{}", id.trim_start_matches('g'))
                } else {
                    id
                },
                client_message_id: string(row, "cliMsgId").unwrap_or_default(),
                from_uid: string(row, "fromUid").unwrap_or_default(),
            });
        }
    }
    Ok(marks)
}

pub fn merge_manual_unread_marks(threads: &mut [Thread], marks: &[ManualUnreadMark]) {
    for thread in threads {
        thread.unread.marked_unread = marks.iter().any(|mark| mark.thread_id == thread.id);
        thread.unread.settings_fresh = true;
        thread.unread.fresh = true;
    }
}

/// Shipped static getMarkAsUnread, command 12011. Reading does not modify account state.
pub async fn fetch_manual_unread_marks(
    session: &crate::session::Session,
) -> anyhow::Result<Vec<ManualUnreadMark>> {
    if session.login.is_none() {
        anyhow::bail!("No active session");
    }
    let params = session.encoded_params(&serde_json::json!({}))?;
    let url = format!(
        "{}/api/conv/getUnreadMark?{}&params={params}",
        session.domains.conversation.trim_end_matches('/'),
        session.common_params()
    );
    let raw = session.get(&url).await?;
    let response = session
        .maybe_decrypt(raw, session.session_key_bytes())
        .await;
    parse_manual_unread_marks(&response)
}

/// Unknown is distinct from a verified empty/nonmatching raw mentions array.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MentionDetection {
    Mention,
    NoMention,
    Unknown,
}

/// Verified original desktop MENTION_ALL_ID, not a display-name/text heuristic.
pub const MENTION_ALL_ID: &str = "-1";

pub fn detect_message_mention(message: &ChatMessage, account: &str) -> bool {
    classify_message_mention(message, account) == MentionDetection::Mention
}

pub fn classify_message_mention(message: &ChatMessage, account: &str) -> MentionDetection {
    if account.is_empty() || account == "0" {
        return MentionDetection::Unknown;
    }
    if message.is_outgoing {
        return MentionDetection::NoMention;
    }
    // Original lazy/default-login-main-startup-shared-worker-znotification.d914...:
    // U(e) tests mentions[].uid == session UID || uid === MENTION_ALL_ID; ji="-1".
    // Original shared-worker.06caa1ffa528545b8f43.js syncMessage2SVMessage:
    // meta.mentionsList becomes mentions; opaque UIDs are decoded, while "-1" stays.
    if ["uidFrom", "fromUid"]
        .iter()
        .any(|key| message.wire_ref.fields.get(*key).and_then(Value::as_str) == Some("0"))
    {
        return MentionDetection::NoMention;
    }
    let wire = message.wire_ref.fields.get("mentions");
    let archive = message
        .history_source
        .as_ref()
        .and_then(|source| source.pointer("/meta/mentionsList"));
    let Some(raw) = wire.or(archive) else {
        return MentionDetection::Unknown;
    };
    let Some(mentions) = raw.as_array() else {
        return MentionDetection::Unknown;
    };
    let mut unknown = false;
    for mention in mentions {
        let Some(uid) = mention.get("uid") else {
            unknown = true;
            continue;
        };
        let target = uid
            .as_str()
            .map(str::to_owned)
            .or_else(|| uid.as_u64().map(|uid| uid.to_string()));
        if uid.as_str() == Some(MENTION_ALL_ID) || target.as_deref() == Some(account) {
            return MentionDetection::Mention;
        }
        if target.is_none() {
            unknown = true;
        }
        // An archive noised UID that does not exactly match the account is not a
        // verified negative without the original decoder's identity mapping.
        if archive.is_some()
            && target
                .as_ref()
                .is_some_and(|uid| !uid.bytes().all(|b| b.is_ascii_digit()))
        {
            unknown = true;
        }
    }
    if unknown {
        MentionDetection::Unknown
    } else {
        MentionDetection::NoMention
    }
}
