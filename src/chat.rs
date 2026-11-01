//! Chat data: friend/conversation list + last messages, all over REST.
//!
//! Notes: `re-notes/02_chat_data.md`.

use anyhow::Result;
use serde_json::{json, Value};

use crate::session::{obj, Session};

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ChatMessage {
    #[serde(default)]
    pub kind: crate::message_model::MessageKind,
    #[serde(default)]
    pub wire_ref: crate::message_model::WireMessageRef,
    #[serde(default)]
    pub unread_policy: crate::message_model::UnreadPolicy,
    #[serde(default)]
    pub delivery: crate::message_model::DeliveryState,
    pub id: String,
    pub sender: String,
    pub text: String,
    pub time: String,
    pub is_outgoing: bool,
    pub ts: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sticker_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sticker_cate_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sticker_type: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to_sender: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to_text: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reactions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reaction_summaries: Vec<crate::reactions::ReactionSummary>,
    #[serde(default)]
    pub reaction_snapshot: bool,
    #[serde(default)]
    pub reaction_authoritative_snapshot: bool,
    #[serde(skip)]
    pub reaction_snapshot_fresh: bool,
    #[serde(default)]
    pub reaction_snapshot_provenance: crate::reactions::ReactionSnapshotProvenance,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub reaction_versions: std::collections::BTreeMap<String, crate::reactions::ReactionVersion>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub client_message_id: String,
    #[serde(default)]
    pub is_pinned: bool,
    #[serde(default)]
    pub is_starred: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_desc: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_thumb: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_src: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_source: Option<Value>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub resolved_mention_uids: std::collections::BTreeMap<String, String>,
}

impl ChatMessage {
    pub fn merge_message_metadata(&mut self, incoming: &Self) -> bool {
        let previous_mentions = self.resolved_mention_uids.clone();
        self.resolved_mention_uids
            .extend(incoming.resolved_mention_uids.clone());
        let before = (
            self.kind,
            self.wire_ref.clone(),
            self.unread_policy.clone(),
            self.delivery,
            self.history_source.clone(),
        );
        if incoming.kind != crate::message_model::MessageKind::Unknown {
            self.kind = incoming.kind;
        }
        if !incoming.wire_ref.fields.is_empty() {
            let mut fields = self.wire_ref.fields.clone();
            fields.extend(incoming.wire_ref.fields.clone());
            self.wire_ref = crate::message_model::WireMessageRef::from_raw(&Value::Object(
                fields.into_iter().collect(),
            ));
            if incoming.unread_policy.params_ext_present {
                self.unread_policy = incoming.unread_policy.clone();
            }
        }
        if incoming.history_source.is_some() {
            self.history_source = incoming.history_source.clone();
        }
        self.delivery = self.delivery.merge(incoming.delivery);
        previous_mentions != self.resolved_mention_uids
            || before
                != (
                    self.kind,
                    self.wire_ref.clone(),
                    self.unread_policy.clone(),
                    self.delivery,
                    self.history_source.clone(),
                )
    }

    /// Keep aggregate counts and event replay cursors across cache/network merges.
    pub fn merge_reaction_metadata(&mut self, incoming: &Self) -> bool {
        let model_changed = self.merge_message_metadata(incoming);
        let previous_provenance = self.reaction_snapshot_provenance.clone();
        let before = (
            self.reaction_summaries.clone(),
            self.reactions.clone(),
            self.client_message_id.clone(),
            self.reaction_snapshot,
            self.reaction_authoritative_snapshot,
            self.reaction_versions.clone(),
        );
        if self.client_message_id.is_empty() {
            self.client_message_id = incoming.client_message_id.clone();
        }
        let accept_snapshot = incoming.reaction_authoritative_snapshot
            && (incoming.reaction_snapshot_fresh || !self.reaction_authoritative_snapshot)
            && self.reaction_snapshot_provenance.accepts_snapshot(
                &incoming.reaction_snapshot_provenance,
                &self.reaction_versions,
                incoming.reaction_snapshot_fresh,
            );

        if accept_snapshot {
            self.reaction_snapshot_provenance = incoming.reaction_snapshot_provenance.clone();
            self.reaction_summaries = incoming.reaction_summaries.clone();
            self.reaction_authoritative_snapshot = true;
            self.reaction_snapshot = true;
            for (user, version) in &mut self.reaction_versions {
                version.covered_events.append(&mut version.events);
                version.baseline = incoming
                    .reaction_summaries
                    .iter()
                    .filter_map(|s| Some((s.r_type?, *s.reactor_counts.get(user)?)))
                    .collect();
                version.ledger_initialized = true;
                version.snapshot_baseline = true;
            }
            for (user, incoming_version) in &incoming.reaction_versions {
                let covered = self
                    .reaction_versions
                    .get(user)
                    .map(|v| v.covered_events.clone())
                    .unwrap_or_default();
                let mut version = incoming_version.clone();
                version.covered_events.extend(covered);
                if incoming.reaction_snapshot_fresh {
                    version.covered_events.append(&mut version.events);
                    version.baseline = incoming
                        .reaction_summaries
                        .iter()
                        .filter_map(|s| Some((s.r_type?, *s.reactor_counts.get(user)?)))
                        .collect();
                    version.ledger_initialized = true;
                    version.snapshot_baseline = true;
                }
                self.reaction_versions.insert(user.clone(), version);
            }
        }
        if !self.reaction_authoritative_snapshot
            && !incoming.reaction_authoritative_snapshot
            && (incoming.reaction_snapshot || !incoming.reaction_summaries.is_empty())
            && self.reaction_versions.is_empty()
        {
            self.reaction_summaries = incoming.reaction_summaries.clone();
            self.reactions = crate::reactions::legacy_emojis(&self.reaction_summaries);
            self.reaction_snapshot = incoming.reaction_snapshot;
        }
        if !self.reaction_authoritative_snapshot
            && !incoming.reaction_authoritative_snapshot
            && !self.reaction_versions.is_empty()
            && (incoming.reaction_snapshot || !incoming.reaction_summaries.is_empty())
        {
            crate::reactions::merge_untracked_snapshot(
                &mut self.reaction_summaries,
                &incoming.reaction_summaries,
                &self.reaction_versions,
                incoming.reaction_snapshot && incoming.reaction_versions.is_empty(),
            );
        }
        for (user, version) in &incoming.reaction_versions {
            if accept_snapshot {
                continue;
            }
            if !self.reaction_versions.contains_key(user) {
                if self.reaction_authoritative_snapshot {
                    for event in &version.events {
                        if event.r_type >= -1 {
                            self.apply_reaction(event);
                        }
                    }
                    continue;
                }
                self.reaction_versions.insert(user.clone(), version.clone());
                if version.ledger_initialized {
                    version.rebuild(&mut self.reaction_summaries, user);
                }
            } else {
                for event in &version.events {
                    if event.r_type >= -1 {
                        self.apply_reaction(event);
                    }
                }
            }
        }
        if !self.reaction_summaries.is_empty()
            || incoming.reaction_snapshot
            || !incoming.reaction_versions.is_empty()
        {
            self.reactions = crate::reactions::legacy_emojis(&self.reaction_summaries);
        } else if self.reactions.is_empty()
            && self.reaction_versions.is_empty()
            && !self.reaction_snapshot
        {
            self.reactions = incoming.reactions.clone();
        }
        model_changed
            || previous_provenance != self.reaction_snapshot_provenance
            || before
                != (
                    self.reaction_summaries.clone(),
                    self.reactions.clone(),
                    self.client_message_id.clone(),
                    self.reaction_snapshot,
                    self.reaction_authoritative_snapshot,
                    self.reaction_versions.clone(),
                )
    }

    pub fn apply_reaction(&mut self, event: &crate::reactions::ReactionEvent) -> bool {
        if event.sender_id.is_empty() || event.r_type < -1 {
            return false;
        }
        if self.reaction_summaries.is_empty() && !self.reactions.is_empty() {
            self.reaction_summaries =
                crate::reactions::parse_reactions(&json!({"reactions":self.reactions}));
        }
        let changed = self
            .reaction_versions
            .entry(event.sender_id.clone())
            .or_default()
            .record(
                &mut self.reaction_summaries,
                event,
                self.reaction_authoritative_snapshot,
            );
        self.reactions = crate::reactions::legacy_emojis(&self.reaction_summaries);
        self.reaction_snapshot = true;
        changed
    }

    /// Repair cached media metadata without changing captions or signed URL bytes.
    pub fn hydrate_media_metadata(&mut self) -> bool {
        if self.text == "[Tin nhắn đã thu hồi]" {
            return false;
        }
        let before = (
            self.link_url.clone(),
            self.link_thumb.clone(),
            self.link_src.clone(),
        );
        let source_kind = self.history_source.as_ref().and_then(|source| {
            let kind = source
                .get("msgType")
                .and_then(|v| v.as_i64().or_else(|| v.as_str()?.parse().ok()));
            match kind {
                Some(2..=4) => Some("Hình ảnh Zalo"),
                Some(19) => Some("Video Zalo"),
                _ => None,
            }
        });
        if let Some(source) = &self.history_source {
            if let Some(attachment) = source
                .get("meta")
                .and_then(|m| m.get("attachsList"))
                .and_then(Value::as_array)
                .and_then(|a| a.first())
            {
                let (url, _, _, thumb, _) = extract_link_info(attachment);
                if self.link_url.is_none() {
                    self.link_url = url;
                }
                if self.link_thumb.is_none() {
                    self.link_thumb = thumb;
                }
            }
        }
        let label_kind = match self.text.trim() {
            "[Photo]" | "[Hình ảnh]" => Some("Hình ảnh Zalo"),
            "[Video]" => Some("Video Zalo"),
            _ => None,
        };
        if let Some(kind) = source_kind.or(label_kind) {
            self.link_src = Some(kind.into());
            if kind == "Hình ảnh Zalo" && self.link_thumb.is_none() {
                self.link_thumb = self.link_url.clone();
            }
        }
        before
            != (
                self.link_url.clone(),
                self.link_thumb.clone(),
                self.link_src.clone(),
            )
    }

    /// Fill independently missing card fields when an existing URL already exists.
    fn merge_media_metadata(&mut self, incoming: &Self) -> bool {
        if self.text == "[Tin nhắn đã thu hồi]" {
            return false;
        }
        let before = (
            self.link_url.clone(),
            self.link_title.clone(),
            self.link_desc.clone(),
            self.link_thumb.clone(),
            self.link_src.clone(),
        );
        if self.link_title.is_none() {
            self.link_title = incoming.link_title.clone();
        }
        if self.link_desc.is_none() {
            self.link_desc = incoming.link_desc.clone();
        }
        if self.link_thumb.is_none() {
            self.link_thumb = incoming.link_thumb.clone();
        }
        if self.link_src.is_none()
            || matches!(
                incoming.link_src.as_deref(),
                Some("Hình ảnh Zalo" | "Video Zalo")
            )
        {
            self.link_src = incoming.link_src.clone().or_else(|| self.link_src.clone());
        }
        self.hydrate_media_metadata();
        before
            != (
                self.link_url.clone(),
                self.link_title.clone(),
                self.link_desc.clone(),
                self.link_thumb.clone(),
                self.link_src.clone(),
            )
    }

    /// Compute the static PNG URL for a Zalo sticker (`compact-app-pc.b7bec208c4c8f73e09f8.js:7739583` & `18589292`).
    pub fn sticker_image_url(&self) -> Option<String> {
        let sid = self.sticker_id?;
        if sid == 0 {
            return None;
        }
        if self.sticker_cate_id == Some(0) && (1..=30).contains(&sid) {
            Some(format!(
                "https://stc-chat.zdn.vn/images/stickers/default/thumb/{sid}.png"
            ))
        } else {
            Some(format!(
                "https://api.zaloapp.com/api/emoticon/sticker/webpc?eid={sid}&size=130"
            ))
        }
    }

    pub fn sticker_cache_key(&self) -> Option<String> {
        let sid = self.sticker_id?;
        if sid == 0 {
            return None;
        }
        let cid = self.sticker_cate_id.unwrap_or(0);
        Some(format!("stk_{cid}_{sid}"))
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Thread {
    #[serde(default)]
    pub presentation: SidebarPresentation,
    pub id: String,
    pub name: String,
    pub preview: String,
    pub time: String,
    pub avatar: Option<String>,
    pub is_group: bool,
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub unread: crate::unread::UnreadState,
}

/// Durable sidebar metadata, including metadata-only startup snapshots.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct SidebarPresentation {
    pub members: Vec<SidebarMember>,
    /// Server totalMember only; never inferred from message senders or avatar samples.
    pub member_count: Option<u64>,
    pub last_message_ts: i64,
    pub last_message_id: String,
    pub last_sender: String,
    pub last_sender_id: String,
    pub last_outgoing: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct SidebarMember {
    pub id: String,
    pub name: String,
    pub avatar: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarFontStyle {
    Regular,
    Semibold,
}

impl Thread {
    pub fn sidebar_font_style(&self) -> SidebarFontStyle {
        if self.unread.has_badge() {
            SidebarFontStyle::Semibold
        } else {
            SidebarFontStyle::Regular
        }
    }

    pub fn refresh_sidebar_presentation(&mut self) {
        if let Some(message) = self
            .messages
            .iter()
            .max_by_key(|m| crate::clock::normalize_ts_ms(m.ts))
        {
            let ts = crate::clock::normalize_ts_ms(message.ts);
            if ts >= crate::clock::normalize_ts_ms(self.presentation.last_message_ts) {
                let sender_id = sidebar_sender_id(message)
                    .or_else(|| {
                        (message.id == self.presentation.last_message_id
                            && ts
                                == crate::clock::normalize_ts_ms(self.presentation.last_message_ts))
                        .then(|| self.presentation.last_sender_id.clone())
                    })
                    .unwrap_or_default();
                let keep_resolved = message.id == self.presentation.last_message_id
                    && ts == crate::clock::normalize_ts_ms(self.presentation.last_message_ts)
                    && sender_id == self.presentation.last_sender_id
                    && !self.presentation.last_sender.is_empty()
                    && !self
                        .presentation
                        .last_sender
                        .bytes()
                        .all(|b| b.is_ascii_digit())
                    && message.sender.bytes().all(|b| b.is_ascii_digit());
                self.presentation.last_message_ts = message.ts;
                self.presentation.last_message_id = message.id.clone();
                if !keep_resolved {
                    self.presentation.last_sender = message.sender.clone();
                }
                self.presentation.last_sender_id = sender_id;
                self.presentation.last_outgoing = message.is_outgoing;
            }
        }
    }

    pub fn sidebar_preview(&self) -> String {
        let latest = self
            .messages
            .iter()
            .max_by_key(|m| crate::clock::normalize_ts_ms(m.ts))
            .filter(|m| {
                crate::clock::normalize_ts_ms(m.ts)
                    > crate::clock::normalize_ts_ms(self.presentation.last_message_ts)
                    || self.presentation.last_sender.is_empty()
            });
        let (sender, outgoing) = latest
            .map(|m| (m.sender.as_str(), m.is_outgoing))
            .unwrap_or((
                &self.presentation.last_sender,
                self.presentation.last_outgoing,
            ));
        let sender = self
            .presentation
            .members
            .iter()
            .find(|member| member.id == sender && !member.name.trim().is_empty())
            .map(|member| member.name.as_str())
            .unwrap_or(sender);
        let text = self.preview.replace('\n', " ");
        if text.is_empty() {
            return text;
        }
        if outgoing {
            if text.starts_with("Bạn: ") {
                text
            } else {
                format!("Bạn: {text}")
            }
        } else if self.is_group
            && !sender.trim().is_empty()
            && !sender.trim().chars().all(|c| c.is_ascii_digit())
        {
            let prefix = format!("{}: ", sender.trim());
            if text.starts_with(&prefix) {
                text
            } else {
                format!("{prefix}{text}")
            }
        } else {
            text
        }
    }

    pub fn sidebar_activity_ts(&self) -> i64 {
        self.messages
            .iter()
            .map(|m| crate::clock::normalize_ts_ms(m.ts))
            .chain(std::iter::once(crate::clock::normalize_ts_ms(
                self.presentation.last_message_ts,
            )))
            .max()
            .unwrap_or(0)
    }

    pub fn sidebar_timestamp(&self, now_ms: i64) -> String {
        let ts = self.sidebar_activity_ts();
        if ts > 0 {
            crate::clock::sidebar_time_at(ts, now_ms)
        } else {
            self.time.clone()
        }
    }

    /// Parse only group roster metadata observed in the original getmg-v2 consumer.
    pub fn apply_group_sidebar_metadata(&mut self, info: &Value) {
        if let Some(count) = info
            .get("totalMember")
            .and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()))
        {
            self.presentation.member_count = Some(count);
        }
        let mut ids = Vec::new();
        if let Some(list) = info.get("memVerList").and_then(Value::as_array) {
            for item in list.iter().filter_map(Value::as_str) {
                let id = item.split('_').next().unwrap_or_default();
                if !id.is_empty() && !ids.iter().any(|s| s == id) {
                    ids.push(id.to_owned());
                }
            }
        } else if let Some(map) = info.get("memberVerMap").and_then(Value::as_object) {
            ids.extend(map.keys().cloned());
        }
        let mut leaders = Vec::new();
        if let Some(creator) = pick_str(info, &["creatorId"]) {
            leaders.push(creator);
        }
        if let Some(admins) = info.get("adminIds").and_then(Value::as_array) {
            leaders.extend(admins.iter().filter_map(Value::as_str).map(str::to_owned));
        }
        let mut ordered = Vec::new();
        for id in leaders.into_iter().chain(ids.iter().cloned()) {
            if ids.contains(&id) && !ordered.contains(&id) {
                ordered.push(id);
            }
        }
        ids = ordered;
        let top = info.get("topMember").and_then(Value::as_array);
        if let Some(top) = top {
            for profile in top {
                if let Some(id) = pick_str(profile, &["id", "userId", "uid"]) {
                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                }
            }
        }
        if !ids.is_empty() {
            let previous = std::mem::take(&mut self.presentation.members);
            self.presentation.members =
                ids.into_iter()
                    .map(|id| {
                        let profile = top.and_then(|list| {
                            list.iter().find(|p| {
                                pick_str(p, &["id", "userId", "uid"]).as_ref() == Some(&id)
                            })
                        });
                        let mut member = previous.iter().find(|m| m.id == id).cloned().unwrap_or(
                            SidebarMember {
                                id,
                                ..Default::default()
                            },
                        );
                        if let Some(profile) = profile {
                            if let Some(name) =
                                pick_str(profile, &["displayName", "zaloName", "name"])
                            {
                                member.name = name;
                            }
                            if let Some(avatar) = pick_str(profile, &["avatar", "avt"]) {
                                member.avatar = Some(avatar);
                            }
                        }
                        member
                    })
                    .collect();
        }
    }
}

pub fn sidebar_sender_id(message: &ChatMessage) -> Option<String> {
    let raw = message.wire_ref.sender.as_ref().or_else(|| {
        message
            .history_source
            .as_ref()
            .and_then(|v| v.get("senderId"))
    })?;
    let id = match raw {
        Value::String(v) => v.clone(),
        Value::Number(v) if v.is_u64() => v.to_string(),
        _ => return None,
    };
    (!id.is_empty() && id != "0" && id.bytes().all(|b| b.is_ascii_digit())).then_some(id)
}

/// Return `true` if a thread name is empty or a generic placeholder (`"Người dùng ..."`, `"Nhóm ..."`).
pub fn is_generic_thread_name(name: &str) -> bool {
    let t = name.trim();
    t.is_empty()
        || t.starts_with("Người dùng ")
        || t.starts_with("Nhóm ")
        || t.chars().all(|c| c.is_ascii_digit())
}

/// Extract the stable avatar filename (e.g. `"00000000000000000000000000000001.jpg"`) from a Zalo CDN URL.
pub fn avatar_file_key(avatar: Option<&str>) -> Option<String> {
    let url = avatar?.trim();
    if url.is_empty() {
        return None;
    }
    let path = url.split('?').next()?;
    let fname = path.rsplit('/').next()?.trim();
    if fname.len() >= 12
        && (fname.ends_with(".jpg") || fname.ends_with(".png") || fname.ends_with(".webp"))
    {
        Some(fname.to_ascii_lowercase())
    } else {
        None
    }
}

/// Extract the `?time=...` expiry timestamp from a Zalo avatar URL so newer session avatars win.
pub fn avatar_url_time(avatar: Option<&str>) -> i64 {
    let Some(url) = avatar else {
        return 0;
    };
    let mut max_ts = 0i64;
    for seg in url.split(';') {
        let Some(query) = seg.split('?').nth(1) else {
            continue;
        };
        for pair in query.split('&') {
            if let Some(val) = pair.strip_prefix("time=") {
                let digits: String = val.chars().take_while(|c| c.is_ascii_digit()).collect();
                if let Ok(ts) = digits.parse::<i64>() {
                    if ts > max_ts {
                        max_ts = ts;
                    }
                }
            }
        }
    }
    max_ts
}

/// Count how many real `http(s)://` image URLs (excluding `/default`) an avatar string contains.
pub fn avatar_http_url_count(avatar: Option<&str>) -> usize {
    let Some(raw) = avatar.map(str::trim).filter(|s| !s.is_empty()) else {
        return 0;
    };
    if let Some(rest) = raw.strip_prefix("multi:") {
        rest.split(';')
            .filter_map(|seg| seg.split_once('|').map(|(_, u)| u.trim()))
            .filter(|u| {
                (u.starts_with("http://") || u.starts_with("https://")) && !u.ends_with("/default")
            })
            .count()
    } else if (raw.starts_with("http://") || raw.starts_with("https://"))
        && !raw.ends_with("/default")
    {
        1
    } else {
        0
    }
}

/// Normalize an avatar string by extracting stable image filenames (`avatar_file_key`) so `s120`
/// vs `s160` CDN subdomains and rotating `?key=...&time=...` tokens compare equal.
fn normalize_avatar_signature(avatar: &str) -> String {
    let raw = avatar.trim();
    if let Some(rest) = raw.strip_prefix("multi:") {
        let mut out = String::from("multi:");
        for (idx, seg) in rest.split(';').enumerate() {
            if idx > 0 {
                out.push(';');
            }
            if let Some((name, url)) = seg.split_once('|') {
                let u_trim = url.trim();
                let file_key = avatar_file_key(Some(u_trim))
                    .unwrap_or_else(|| u_trim.split('?').next().unwrap_or("").to_string());
                out.push_str(name.trim());
                out.push('|');
                out.push_str(&file_key);
            } else {
                out.push_str(seg.trim());
            }
        }
        out
    } else {
        avatar_file_key(Some(raw))
            .unwrap_or_else(|| raw.split('?').next().unwrap_or(raw).to_string())
    }
}

/// Decide whether `incoming` should replace `current` avatar without downgrading a rich
/// `multi:` composite avatar (with member photo URLs) or flipping between `s120` and `s160` URLs.
pub fn should_replace_avatar(current: Option<&str>, incoming: Option<&str>) -> bool {
    let Some(inc) = incoming.map(str::trim).filter(|s| !s.is_empty()) else {
        return false;
    };
    let Some(cur) = current.map(str::trim).filter(|s| !s.is_empty()) else {
        return true;
    };
    let cur_urls = avatar_http_url_count(Some(cur));
    let inc_urls = avatar_http_url_count(Some(inc));
    if inc_urls < cur_urls {
        return false;
    }
    if inc_urls > cur_urls {
        return true;
    }
    if cur.starts_with("multi:") && inc.starts_with("multi:") && cur_urls > 0 {
        return false;
    }
    if normalize_avatar_signature(cur) == normalize_avatar_signature(inc) {
        return false;
    }
    avatar_url_time(Some(inc)) > avatar_url_time(Some(cur))
}

/// Identity requires an exact nonempty ID. Names and avatar URLs are presentation only.
pub fn threads_represent_same_contact(a: &Thread, b: &Thread) -> bool {
    !a.id.is_empty() && a.id != "0" && a.id == b.id && a.is_group == b.is_group
}

/// Find an exact message, or a uniquely identified pending client echo.
pub fn matching_message_index(messages: &[ChatMessage], incoming: &ChatMessage) -> Option<usize> {
    if !incoming.id.is_empty() {
        if let Some(index) = messages.iter().position(|m| m.id == incoming.id) {
            return Some(index);
        }
    }
    if incoming.client_message_id.is_empty() {
        return None;
    }
    let mut candidates = messages
        .iter()
        .enumerate()
        .filter(|(_, m)| m.client_message_id == incoming.client_message_id);
    let (index, candidate) = candidates.next()?;
    if candidates.next().is_some() {
        return None;
    }
    let pending = !candidate.id.is_empty()
        && !incoming.id.is_empty()
        && candidate.id.starts_with("out_") != incoming.id.starts_with("out_");
    (pending && candidate.is_outgoing && incoming.is_outgoing).then_some(index)
}

/// Merge `incoming` thread into `target`. When `incoming_id_is_authoritative` is `true`,
/// `target.id` is migrated to `incoming.id` so actions (chat/VoIP call) always target the active session's `userId`.
pub fn merge_thread_into(
    target: &mut Thread,
    mut incoming: Thread,
    incoming_id_is_authoritative: bool,
) -> bool {
    let previous_presentation = target.presentation.clone();
    target.refresh_sidebar_presentation();
    incoming.refresh_sidebar_presentation();
    let previous_latest_ts = crate::clock::normalize_ts_ms(target.presentation.last_message_ts);
    let incoming_latest_ts = crate::clock::normalize_ts_ms(incoming.presentation.last_message_ts);
    let mut changed = previous_presentation != target.presentation;
    macro_rules! set {
        ($field:expr, $value:expr) => {{
            let value = $value;
            if $field != value {
                $field = value;
                changed = true;
            }
        }};
    }
    if incoming_id_is_authoritative && !incoming.id.is_empty() && incoming.id != "0" {
        set!(target.id, incoming.id);
    }
    if !incoming.name.is_empty()
        && (!is_generic_thread_name(&incoming.name) || is_generic_thread_name(&target.name))
    {
        set!(target.name, incoming.name);
    }
    if should_replace_avatar(target.avatar.as_deref(), incoming.avatar.as_deref()) {
        set!(target.avatar, incoming.avatar);
    }
    set!(target.is_group, target.is_group || incoming.is_group);
    changed |= target.unread.merge_metadata(&incoming.unread);
    if incoming.presentation.member_count.is_some() {
        set!(
            target.presentation.member_count,
            incoming.presentation.member_count
        );
    }
    if !incoming.presentation.members.is_empty() {
        set!(target.presentation.members, incoming.presentation.members);
    }
    if crate::clock::normalize_ts_ms(incoming.presentation.last_message_ts)
        > crate::clock::normalize_ts_ms(target.presentation.last_message_ts)
    {
        set!(
            target.presentation.last_message_ts,
            incoming.presentation.last_message_ts
        );
        set!(
            target.presentation.last_message_id,
            incoming.presentation.last_message_id.clone()
        );
        set!(
            target.presentation.last_sender,
            incoming.presentation.last_sender.clone()
        );
        set!(
            target.presentation.last_sender_id,
            incoming.presentation.last_sender_id.clone()
        );
        set!(
            target.presentation.last_outgoing,
            incoming.presentation.last_outgoing
        );
    }

    if incoming_latest_ts == previous_latest_ts
        && incoming_latest_ts > 0
        && (target.presentation.last_message_id.is_empty()
            || target.presentation.last_message_id == incoming.presentation.last_message_id)
        && (target.presentation.last_sender.is_empty()
            || target
                .presentation
                .last_sender
                .bytes()
                .all(|b| b.is_ascii_digit()))
        && !incoming.presentation.last_sender.is_empty()
        && !incoming
            .presentation
            .last_sender
            .bytes()
            .all(|b| b.is_ascii_digit())
        && (target.presentation.last_sender_id.is_empty()
            || target.presentation.last_sender_id == incoming.presentation.last_sender_id)
    {
        set!(
            target.presentation.last_sender,
            incoming.presentation.last_sender
        );
        set!(
            target.presentation.last_sender_id,
            incoming.presentation.last_sender_id
        );
    }
    let mut ids = std::collections::HashMap::new();
    let mut clients: std::collections::HashMap<String, Vec<usize>> =
        std::collections::HashMap::new();
    for (index, message) in target.messages.iter().enumerate() {
        if !message.id.is_empty() {
            ids.entry(message.id.clone()).or_insert(index);
        }
        if !message.client_message_id.is_empty() {
            clients
                .entry(message.client_message_id.clone())
                .or_default()
                .push(index);
        }
    }
    for msg in incoming.messages {
        if msg.text == "sendBubbleMessage" {
            continue;
        }
        let exact = ids.get(&msg.id).copied().filter(|_| !msg.id.is_empty());
        let echo = clients
            .get(&msg.client_message_id)
            .filter(|indices| indices.len() == 1)
            .and_then(|indices| {
                let index = indices[0];
                let existing = &target.messages[index];
                (!existing.id.is_empty()
                    && !msg.id.is_empty()
                    && existing.is_outgoing
                    && msg.is_outgoing
                    && (existing.id.starts_with("out_") != msg.id.starts_with("out_")))
                .then_some(index)
            });
        if let Some(index) = exact.or(echo) {
            let existing_msg = &mut target.messages[index];
            changed |= existing_msg.merge_reaction_metadata(&msg);
            changed |= existing_msg.merge_media_metadata(&msg);
            if msg.history_source.is_some() {
                set!(existing_msg.history_source, msg.history_source);
            }
            if existing_msg.id.starts_with("out_")
                && !msg.id.is_empty()
                && !msg.id.starts_with("out_")
            {
                set!(existing_msg.id, msg.id.clone());
                ids.entry(msg.id.clone()).or_insert(index);
            }
            if msg.ts > 0 && existing_msg.ts != msg.ts {
                set!(existing_msg.ts, msg.ts);
                set!(existing_msg.time, msg.time);
            }
            if msg.text.trim() == "[Tin nhắn đã thu hồi]" {
                set!(existing_msg.text, "[Tin nhắn đã thu hồi]".to_string());
                set!(existing_msg.sticker_id, None);
                set!(existing_msg.link_url, None);
                set!(existing_msg.link_thumb, None);
            }
            if existing_msg.sticker_id.is_none() && msg.sticker_id.is_some() {
                set!(existing_msg.sticker_id, msg.sticker_id);
                set!(existing_msg.sticker_cate_id, msg.sticker_cate_id);
                set!(existing_msg.sticker_type, msg.sticker_type);
            }
            if existing_msg.reply_to_text.is_none() && msg.reply_to_text.is_some() {
                set!(existing_msg.reply_to_id, msg.reply_to_id);
                set!(existing_msg.reply_to_sender, msg.reply_to_sender);
                set!(existing_msg.reply_to_text, msg.reply_to_text);
            }
            if existing_msg.link_url.is_none() && msg.link_url.is_some() {
                set!(existing_msg.link_url, msg.link_url);
                set!(existing_msg.link_title, msg.link_title);
                set!(existing_msg.link_desc, msg.link_desc);
                set!(existing_msg.link_thumb, msg.link_thumb);
                set!(existing_msg.link_src, msg.link_src);
                if !msg.text.is_empty() {
                    set!(existing_msg.text, msg.text);
                }
            }
            if existing_msg.reactions.is_empty() && !msg.reactions.is_empty() {
                set!(existing_msg.reactions, msg.reactions);
            }
            set!(
                existing_msg.is_pinned,
                existing_msg.is_pinned || msg.is_pinned
            );
            set!(
                existing_msg.is_starred,
                existing_msg.is_starred || msg.is_starred
            );
        } else {
            let index = target.messages.len();
            if !msg.id.is_empty() {
                ids.entry(msg.id.clone()).or_insert(index);
            }
            if !msg.client_message_id.is_empty() {
                clients
                    .entry(msg.client_message_id.clone())
                    .or_default()
                    .push(index);
            }
            target.messages.push(msg);
            changed = true;
        }
    }

    let previous_len = target.messages.len();
    target.messages.retain(|m| m.text != "sendBubbleMessage");
    changed |= target.messages.len() != previous_len;
    changed |= target
        .messages
        .windows(2)
        .any(|pair| pair[0].ts > pair[1].ts);
    target.messages.sort_by_key(|m| m.ts);

    let newest_known_ts = previous_latest_ts.max(incoming_latest_ts);
    if let Some(newest) = target
        .messages
        .last()
        .filter(|m| crate::clock::normalize_ts_ms(m.ts) >= newest_known_ts)
    {
        if !newest.text.is_empty() {
            set!(target.preview, newest.text.replace('\n', " "));
            set!(target.time, newest.time.clone());
        }
    } else if (incoming_latest_ts > previous_latest_ts || target.preview.is_empty())
        && incoming_latest_ts >= previous_latest_ts
        && !incoming.preview.is_empty()
    {
        set!(target.preview, incoming.preview);
        if !incoming.time.is_empty() {
            set!(target.time, incoming.time);
        }
    } else if target.time.is_empty()
        && incoming_latest_ts >= previous_latest_ts
        && !incoming.time.is_empty()
    {
        set!(target.time, incoming.time);
    }
    let before = target.presentation.clone();
    target.refresh_sidebar_presentation();
    changed |= before != target.presentation;
    changed
}

/// Deduplicate `threads` in-place, merging entries that represent the same contact/group
/// by exact ID. Display names and avatars never establish identity.
pub fn deduplicate_threads(
    threads: &mut Vec<Thread>,
    authoritative_ids: Option<&std::collections::HashSet<String>>,
) {
    let mut merged: Vec<Thread> = Vec::with_capacity(threads.len());
    for mut t in threads.drain(..) {
        if t.id.is_empty() || t.id == "0" {
            continue;
        }
        t.messages.retain(|m| m.text != "sendBubbleMessage");
        if let Some(newest) = t.messages.last() {
            if !newest.text.is_empty() && (t.preview.is_empty() || t.preview == "sendBubbleMessage")
            {
                t.preview = newest.text.replace('\n', " ");
                t.time = newest.time.clone();
            }
        }
        if let Some(existing) = merged.iter_mut().find(|e| {
            if e.id == t.id {
                return true;
            }
            if let Some(auth) = authoritative_ids {
                if auth.contains(&e.id) && auth.contains(&t.id) {
                    return false;
                }
            }
            threads_represent_same_contact(e, &t)
        }) {
            let incoming_auth = if let Some(auth) = authoritative_ids {
                auth.contains(&t.id) && !auth.contains(&existing.id)
            } else {
                // In cache order, later entries or entries with newer avatar URL timestamps come from newer sessions
                let t_time = avatar_url_time(t.avatar.as_deref());
                let e_time = avatar_url_time(existing.avatar.as_deref());
                if t_time != e_time {
                    t_time > e_time
                } else {
                    true
                }
            };
            merge_thread_into(existing, t, incoming_auth);
        } else {
            merged.push(t);
        }
    }
    *threads = merged;
}

/// Load locally cached conversations and message history.
pub fn load_threads_cache() -> Vec<Thread> {
    if let Ok(text) = std::fs::read_to_string("threads_cache.json") {
        if let Ok(mut threads) = serde_json::from_str::<Vec<Thread>>(&text) {
            for thread in &mut threads {
                for message in &mut thread.messages {
                    if message.ts > 0 {
                        message.time = crate::clock::message_time(message.ts);
                    }
                }
                if let Some(last) = thread.messages.iter().max_by_key(|m| m.ts) {
                    thread.time = last.time.clone();
                }
            }
            deduplicate_threads(&mut threads, None);
            return threads;
        }
    }
    Vec::new()
}

/// Snapshot of the thread list for an instant first paint. Only the newest
/// `PRELOAD_MSGS` messages per thread are kept: full history lives in `zalo_history.db`, and
/// rewriting every message as pretty JSON on each change cost more than the SQLite write.
pub fn save_threads_cache(threads: &[Thread]) {
    if let Err(error) = try_save_threads_cache(threads) {
        crate::logger::log_error("chat::save_threads_cache", &error.to_string());
    }
}

/// Checked atomic persistence for UI callers that must surface storage failures.
pub fn try_save_threads_cache(threads: &[Thread]) -> Result<()> {
    let keep = crate::store::MessageStore::PRELOAD_MSGS;
    let slim: Vec<Thread> = threads
        .iter()
        .map(|thread| Thread {
            messages: thread.messages[thread.messages.len().saturating_sub(keep)..].to_vec(),
            presentation: thread.presentation.clone(),
            unread: thread.unread.clone(),
            id: thread.id.clone(),
            name: thread.name.clone(),
            preview: thread.preview.clone(),
            time: thread.time.clone(),
            avatar: thread.avatar.clone(),
            is_group: thread.is_group,
        })
        .collect();
    let text = serde_json::to_vec(&slim)?;
    crate::data_dir::write_private_atomic(std::path::Path::new("threads_cache.json"), &text)
}

/// Preserve application errors after decrypting an HTTP sync envelope.
fn checked_sync_response(mut response: Value) -> Result<Value> {
    fn check(value: &Value) -> Result<()> {
        let code = value
            .get("error_code")
            .and_then(|v| {
                v.as_i64()
                    .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            })
            .unwrap_or(0);
        if code != 0 {
            // Do not include request URLs, credentials, or response bodies in UI errors.
            return Err(crate::session::SessionError::new(code, "sync request rejected").into());
        }
        Ok(())
    }
    check(&response)?;
    if let Some(text) = response.get("data").and_then(Value::as_str) {
        let parsed: Value = serde_json::from_str(text)
            .map_err(|_| anyhow::anyhow!("sync response could not be decrypted"))?;
        check(&parsed)?;
        response["data"] = parsed;
    }
    if let Some(data) = response.get("data") {
        check(data)?;
    }
    Ok(response)
}

/// `/api/social/friend/getfriends` — the friend list.
///
/// Uses `s.domains.friend` (defaulting to `tt-friend-wpa.chat.zalo.me`), falling back
/// to `s.domains.profile` only if `domains.friend` is empty.
pub async fn get_friends(s: &Session) -> Result<Vec<Thread>> {
    get_friends_impl(s, true).await
}

/// Fetch remote metadata only. Account-owned GUI storage is handled by its caller.
pub async fn get_friends_remote(s: &Session) -> Result<Vec<Thread>> {
    get_friends_impl(s, false).await
}

async fn get_friends_impl(s: &Session, legacy_storage: bool) -> Result<Vec<Thread>> {
    if s.login.is_none() {
        anyhow::bail!("chưa đăng nhập");
    }
    let body = obj(&[
        ("incInvalid", json!(1)),
        ("page", json!(1)),
        ("count", json!(20000)),
        ("avatar_size", json!(160)),
        ("actiontime", json!(0)),
        ("imei", json!(s.imei)),
    ]);
    let params = match s.encoded_params(&body) {
        Ok(p) => format!("params={p}"),
        Err(e) => {
            crate::logger::log_error("chat::get_friends::encoded_params", &e.to_string());
            eprintln!("! get_friends: no session key for encoded_params ({e}), falling back to plain_params");
            Session::plain_params(&body)
        }
    };
    // `friend` is the canonical host for `social/friend/getfriends`; `profile` is
    // only a fallback for very old sessions where `friend` was never populated.
    let hosts = [&s.domains.friend, &s.domains.profile];
    let mut resp_opt = None;
    let mut last_err = None;
    for host in hosts {
        if host.is_empty() {
            continue;
        }
        let url = format!(
            "{}/api/social/friend/getfriends?{}{}",
            host,
            s.common_params(),
            if params.is_empty() {
                String::new()
            } else {
                format!("&{params}")
            }
        );
        match s.get(&url).await {
            Ok(r) => {
                resp_opt = Some(s.maybe_decrypt(r, s.session_key_bytes()).await);
                break;
            }
            Err(e) => {
                crate::logger::log_error(
                    "chat::get_friends",
                    &format!("host {} failed: {e}", host),
                );
                last_err = Some(e);
            }
        }
    }
    let resp = match resp_opt {
        Some(r) => r,
        None => {
            let err = last_err.unwrap_or_else(|| anyhow::anyhow!("get_friends failed"));
            return Err(err.context("không tải được danh sách bạn bè"));
        }
    };
    let resp = checked_sync_response(resp)?;
    let data_root = resp.get("data").unwrap_or(&resp);
    let mut threads: Vec<Thread> = match data_root {
        Value::Array(items) => items.iter().filter_map(parse_thread).collect(),
        Value::Object(o) if o.contains_key("friends") => o["friends"]
            .as_array()
            .map(|a| a.iter().filter_map(parse_thread).collect())
            .unwrap_or_default(),
        Value::Object(o) if o.get("data").and_then(Value::as_array).is_some() => o["data"]
            .as_array()
            .map(|a| a.iter().filter_map(parse_thread).collect())
            .unwrap_or_default(),
        _ => Vec::new(),
    };

    // If Cloud của tôi (send2me) is available, place it at the top
    if let Some(li) = &s.login {
        if !li.send2me_id.is_empty() && !threads.iter().any(|t| t.id == li.send2me_id) {
            threads.insert(
                0,
                Thread {
                    presentation: Default::default(),
                    unread: Default::default(),
                    id: li.send2me_id.clone(),
                    name: "Cloud của tôi (Lưu trữ)".into(),
                    preview: "".into(),
                    time: crate::ui::clock(),
                    avatar: None,
                    is_group: false,
                    messages: Vec::new(),
                },
            );
        }
    }

    // Merge user groups (`getlg/v4` + `getmg-v2`) into threads list, upgrading any placeholder group names
    if let Ok(mut groups) = get_groups(s).await {
        for g in groups.drain(..) {
            if let Some(existing) = threads.iter_mut().find(|t| t.id == g.id) {
                merge_thread_into(existing, g, true);
            } else {
                threads.push(g);
            }
        }
    }

    // Hydrate with locally persisted history from SQLite (`zalo_history.db`)
    if legacy_storage {
        crate::store::load_history_into_threads("zalo_history.db", &mut threads, 200);
    }
    resolve_thread_names(s, &mut threads).await;

    Ok(threads)
}

fn parse_thread(v: &Value) -> Option<Thread> {
    let id = pick_str(v, &["userId", "uid", "id", "friendId", "globalId"])?;
    let name = pick_str(v, &["displayName", "zaloName", "display_name", "name"])
        .or_else(|| {
            v.get("profile_displayName")
                .or_else(|| v.get("profile").and_then(|p| p.get("displayName")))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| id.clone());
    let avatar = pick_str(v, &["avatar", "avt", "profile_avatar"]);
    let is_group = is_group_flag(v);
    // lastActionTime belongs to contact activity, not conversation history.
    let time = String::new();
    Some(Thread {
        presentation: Default::default(),
        unread: Default::default(),
        id,
        name,
        preview: String::new(),
        time,
        avatar,
        is_group,
        messages: Vec::new(),
    })
}

fn pick_str(v: &Value, keys: &[&str]) -> Option<String> {
    for k in keys {
        match v.get(*k) {
            Some(Value::String(s)) if !s.is_empty() => return Some(s.clone()),
            Some(Value::Number(n)) => return Some(n.to_string()),
            _ => {}
        }
    }
    None
}

/// Preserve the protocol's integer clientId without rounding through a float.
pub async fn send_message(s: &Session, to_id: &str, text: &str) -> Result<Value> {
    if s.login.is_none() {
        anyhow::bail!("chưa đăng nhập");
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let body = obj(&[
        ("toid", json!(to_id)),
        ("message", json!(text)),
        ("clientId", json!(now)),
        ("imei", json!(s.imei)),
    ]);
    let params = s.encoded_params(&body)?;

    let host = if s.domains.chat.contains("-msg-chat.")
        || s.domains.chat.contains("tt-chat2-wpa")
        || s.domains.chat.is_empty()
    {
        "https://tt-chat3-wpa.api.zaloapp.com"
    } else {
        &s.domains.chat
    };

    let url = format!("{host}/api/message/sms?{}", s.common_params());
    let form = format!("params={params}");
    let resp = s.post_form(&url, form).await?;
    let resp = s.maybe_decrypt(resp, s.session_key_bytes()).await;
    if let Some(code) = resp.get("error_code").and_then(Value::as_i64) {
        if code != 0 {
            let msg = resp
                .get("error_message")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            return Err(crate::session::SessionError::new(code, msg).into());
        }
    }
    Ok(resp)
}

/// Send a group text message: `POST {domains.group}/api/group/sendmsg?{common}`
pub async fn send_group_message(s: &Session, grid: &str, text: &str) -> Result<Value> {
    if s.login.is_none() {
        anyhow::bail!("chưa đăng nhập");
    }
    let clean_grid = grid
        .trim_start_matches("group_")
        .trim_start_matches('g')
        .trim_start_matches('G');
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let body = obj(&[
        ("grid", json!(clean_grid)),
        ("message", json!(text)),
        ("clientId", json!(now)),
        ("imei", json!(s.imei)),
        ("visibility", json!(0)),
    ]);
    let params = s.encoded_params(&body)?;
    let url = format!(
        "{}/api/group/sendmsg?{}",
        s.domains.group,
        s.common_params()
    );
    let form = format!("params={params}");
    let resp = s.post_form(&url, form).await?;
    let resp = s.maybe_decrypt(resp, s.session_key_bytes()).await;
    if let Some(code) = resp.get("error_code").and_then(Value::as_i64) {
        if code != 0 {
            let msg = resp
                .get("error_message")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            return Err(crate::session::SessionError::new(code, msg).into());
        }
    }
    Ok(resp)
}

/// Format a byte count into a human-readable size string (e.g. `"1.4 MB"`, `"320 KB"`).
pub fn format_file_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

fn md5_hex_bytes(bytes: &[u8]) -> String {
    use md5::{Digest, Md5};
    let mut h = Md5::new();
    h.update(bytes);
    let out = h.finalize();
    out.iter().fold(String::with_capacity(32), |mut acc, b| {
        use std::fmt::Write;
        let _ = write!(acc, "{b:02x}");
        acc
    })
}

/// Upload and send a photo, video, or general file attachment to a 1-1 or group conversation.
///
/// Implements Zalo PC 26.10.11's two-stage chunked upload (`photo_original/upload` -> `photo_original/send`
/// for images; `asyncfile/upload` -> `asyncfile/msg` for videos/files) with automatic fallback to
/// single-shot `message/photo` (`11152`) and `message/sharefile_full` (`11146`).
pub async fn send_attachment(
    s: &Session,
    to_id: &str,
    is_group: bool,
    file_path: &std::path::Path,
    caption: &str,
) -> Result<(Value, ChatMessage)> {
    if s.login.is_none() {
        anyhow::bail!("chưa đăng nhập");
    }
    let file_bytes = std::fs::read(file_path)?;
    if file_bytes.is_empty() {
        anyhow::bail!("Tập tin rỗng (0 byte)");
    }
    let file_name = file_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("attachment.bin")
        .to_string();
    let ext = file_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    let is_image = matches!(
        ext.as_str(),
        "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp"
    );
    let is_video = matches!(ext.as_str(), "mp4" | "mov" | "mkv" | "webm" | "avi");

    let file_host = if s.domains.file.is_empty() {
        "https://tt-files-wpa.zaloapp.com"
    } else {
        s.domains.file.trim_end_matches('/')
    };
    let clean_target = if is_group {
        to_id
            .trim_start_matches("group_")
            .trim_start_matches('g')
            .trim_start_matches('G')
    } else {
        to_id
    };
    let client_id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(1793165000000);
    let total_size = file_bytes.len() as u64;
    let chunk_size: usize = 1_048_576; // 1 MiB chunks
    let total_chunks = file_bytes.len().div_ceil(chunk_size).max(1);
    let target_type = if is_group { 11 } else { 2 };

    if is_image {
        let (width, height) = image::load_from_memory(&file_bytes)
            .map(|img| (img.width(), img.height()))
            .unwrap_or((800, 600));
        let mime = match ext.as_str() {
            "png" => "image/png",
            "webp" => "image/webp",
            "gif" => "image/gif",
            "bmp" => "image/bmp",
            _ => "image/jpeg",
        };
        let upload_ep = if is_group {
            "/api/group/photo_original/upload"
        } else {
            "/api/message/photo_original/upload"
        };

        let mut last_up_dec = Value::Null;
        for (idx, chunk) in file_bytes.chunks(chunk_size).enumerate() {
            let chunk_id = idx + 1;
            let mut m = serde_json::Map::new();
            m.insert("totalChunk".into(), json!(total_chunks));
            m.insert("fileName".into(), json!(file_name));
            m.insert("clientId".into(), json!(client_id));
            m.insert("totalSize".into(), json!(total_size));
            m.insert("imei".into(), json!(s.imei));
            m.insert("isE2EE".into(), json!(0));
            m.insert("jxl".into(), json!(0));
            m.insert("chunkId".into(), json!(chunk_id));
            if is_group {
                m.insert("grid".into(), json!(clean_target));
            } else {
                m.insert("toid".into(), json!(clean_target));
            }
            let params = s.encoded_params(&Value::Object(m))?;
            let url = format!(
                "{file_host}{upload_ep}?{}&params={params}&type={target_type}",
                s.common_params()
            );
            let up_raw = s
                .post_multipart(&url, "chunkContent", &file_name, mime, chunk)
                .await?;
            last_up_dec = s.maybe_decrypt(up_raw, s.session_key_bytes()).await;
        }

        let data_node = last_up_dec.get("data").unwrap_or(&last_up_dec);
        let photo_id = pick_str(data_node, &["photoId", "clientFileId"]);
        let normal_url =
            pick_str(data_node, &["normalUrl", "hdUrl", "oriUrl", "url"]).unwrap_or_default();
        let hd_url = pick_str(data_node, &["hdUrl", "normalUrl", "oriUrl"])
            .unwrap_or_else(|| normal_url.clone());
        let thumb_url = pick_str(data_node, &["thumbUrl", "normalUrl", "hdUrl"])
            .unwrap_or_else(|| normal_url.clone());

        let send_resp = if let Some(pid) = photo_id.filter(|_| !normal_url.is_empty()) {
            let send_ep = if is_group {
                "/api/group/photo_original/send"
            } else {
                "/api/message/photo_original/send"
            };
            let pid_val = pid
                .parse::<u64>()
                .map(|n| json!(n))
                .unwrap_or_else(|_| json!(pid));
            let mut send_obj = serde_json::Map::new();
            send_obj.insert("photoId".into(), pid_val);
            send_obj.insert("clientId".into(), json!(client_id));
            send_obj.insert("desc".into(), json!(caption));
            send_obj.insert("width".into(), json!(width));
            send_obj.insert("height".into(), json!(height));
            send_obj.insert("rawUrl".into(), json!(normal_url));
            send_obj.insert("hdUrl".into(), json!(hd_url));
            send_obj.insert("thumbUrl".into(), json!(thumb_url));
            send_obj.insert("oriUrl".into(), json!(normal_url));
            send_obj.insert("normalUrl".into(), json!(normal_url));
            send_obj.insert("hdSize".into(), json!(total_size));
            send_obj.insert("zsource".into(), json!(-1));
            send_obj.insert("jcp".into(), json!("{\"convertible\":true}"));
            send_obj.insert("imei".into(), json!(s.imei));
            if is_group {
                send_obj.insert("grid".into(), json!(clean_target));
                send_obj.insert("visibility".into(), json!(0));
            } else {
                send_obj.insert("toid".into(), json!(clean_target));
            }
            let params = s.encoded_params(&Value::Object(send_obj))?;
            let url = format!("{file_host}{send_ep}?{}", s.common_params());
            let r = s.post_form(&url, format!("params={params}")).await?;
            s.maybe_decrypt(r, s.session_key_bytes()).await
        } else {
            // Fallback: single-shot `/api/message/photo` or `/api/group/sendpt`
            let single_ep = if is_group {
                "/api/group/sendpt"
            } else {
                "/api/message/photo"
            };
            let mut body_obj = serde_json::Map::new();
            body_obj.insert("clientId".into(), json!(client_id));
            body_obj.insert("desc".into(), json!(caption));
            body_obj.insert("width".into(), json!(width));
            body_obj.insert("height".into(), json!(height));
            body_obj.insert("imei".into(), json!(s.imei));
            if is_group {
                body_obj.insert("grid".into(), json!(clean_target));
                body_obj.insert("visibility".into(), json!(0));
            } else {
                body_obj.insert("toid".into(), json!(clean_target));
            }
            let params = s.encoded_params(&Value::Object(body_obj))?;
            let url = format!(
                "{file_host}{single_ep}?{}&params={params}&type={target_type}",
                s.common_params()
            );
            let r = s
                .post_multipart(&url, "fileContent", &file_name, mime, &file_bytes)
                .await?;
            s.maybe_decrypt(r, s.session_key_bytes()).await
        };

        if let Some(code) = send_resp.get("error_code").and_then(Value::as_i64) {
            if code != 0 {
                let msg = send_resp
                    .get("error_message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                return Err(crate::session::SessionError::new(code, msg).into());
            }
        }

        let final_thumb = if !thumb_url.is_empty() {
            thumb_url
        } else {
            format!("local://{}", file_path.display())
        };
        let final_url = if !hd_url.is_empty() {
            hd_url
        } else {
            file_path.display().to_string()
        };
        // Pre-populate `stickers_cache/thumb_<md5>.png` with the local image bytes so the GUI
        // renders the sent photo immediately without an extra network round-trip.
        let _ = std::fs::create_dir_all("stickers_cache");
        let cache_key = format!("thumb_{}", crate::crypto::md5_hex(&final_thumb));
        let _ = std::fs::write(format!("stickers_cache/{cache_key}.png"), &file_bytes);

        let msg_id = send_resp
            .get("data")
            .and_then(|d| pick_str(d, &["msgId", "cliMsgId"]))
            .unwrap_or_else(|| format!("out_{client_id}"));
        let now_str = crate::ui::clock();
        let display_text = if !caption.trim().is_empty() {
            caption.trim().to_string()
        } else {
            format!("[Hình ảnh] {file_name}")
        };
        let cm = ChatMessage {
            id: msg_id,
            sender: "Tôi".into(),
            text: display_text.clone(),
            time: now_str,
            is_outgoing: true,
            ts: client_id as i64,
            link_url: Some(final_url),
            link_title: Some(format!(
                "{file_name} ({width}×{height} · {})",
                format_file_size(total_size)
            )),
            link_desc: if caption.trim().is_empty() {
                None
            } else {
                Some(caption.trim().to_string())
            },
            link_thumb: Some(final_thumb),
            link_src: Some("Hình ảnh Zalo".into()),
            ..Default::default()
        };
        return Ok((send_resp, cm));
    }

    // Video / Audio / Document / Archive upload (`asyncfile/upload` -> `asyncfile/msg`, fallback `sharefile_full`)
    let mime = match ext.as_str() {
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "txt" | "md" | "log" => "text/plain",
        _ => "application/octet-stream",
    };
    let checksum = md5_hex_bytes(&file_bytes);
    let upload_ep = if is_group {
        "/api/group/asyncfile/upload"
    } else {
        "/api/message/asyncfile/upload"
    };

    let mut last_up_dec = Value::Null;
    for (idx, chunk) in file_bytes.chunks(chunk_size).enumerate() {
        let chunk_id = idx + 1;
        let mut m = serde_json::Map::new();
        m.insert("totalChunk".into(), json!(total_chunks));
        m.insert("fileName".into(), json!(file_name));
        m.insert("clientId".into(), json!(client_id));
        m.insert("totalSize".into(), json!(total_size));
        m.insert("imei".into(), json!(s.imei));
        m.insert("isE2EE".into(), json!(0));
        m.insert("chunkId".into(), json!(chunk_id));
        if is_group {
            m.insert("grid".into(), json!(clean_target));
        } else {
            m.insert("toid".into(), json!(clean_target));
        }
        let params = s.encoded_params(&Value::Object(m))?;
        let url = format!(
            "{file_host}{upload_ep}?{}&params={params}&type={target_type}",
            s.common_params()
        );
        if let Ok(up_raw) = s
            .post_multipart(&url, "chunkContent", &file_name, mime, chunk)
            .await
        {
            last_up_dec = s.maybe_decrypt(up_raw, s.session_key_bytes()).await;
        } else {
            break;
        }
    }

    let data_node = last_up_dec.get("data").unwrap_or(&last_up_dec);
    let file_id = pick_str(data_node, &["fileId", "clientFileId"]);
    let file_url = pick_str(data_node, &["fileUrl", "url", "normalUrl"]).unwrap_or_default();

    let mut send_resp = if let Some(fid) = file_id {
        let msg_ep = if is_group {
            "/api/group/asyncfile/msg"
        } else {
            "/api/message/asyncfile/msg"
        };
        let fid_val = fid
            .parse::<u64>()
            .map(|n| json!(n))
            .unwrap_or_else(|_| json!(fid));
        let mut msg_obj = serde_json::Map::new();
        msg_obj.insert("fileId".into(), fid_val);
        if !file_url.is_empty() {
            msg_obj.insert("fileUrl".into(), json!(file_url));
        }
        msg_obj.insert("checksum".into(), json!(checksum));
        msg_obj.insert("checksumSha".into(), json!(""));
        msg_obj.insert("extension".into(), json!(ext));
        msg_obj.insert("totalSize".into(), json!(total_size));
        msg_obj.insert("fileName".into(), json!(file_name));
        msg_obj.insert("clientId".into(), json!(client_id));
        msg_obj.insert("fType".into(), json!(if is_video { 2 } else { 1 }));
        msg_obj.insert("fileCount".into(), json!(0));
        msg_obj.insert("fdata".into(), json!("{}"));
        msg_obj.insert("imei".into(), json!(s.imei));
        if is_group {
            msg_obj.insert("grid".into(), json!(clean_target));
        } else {
            msg_obj.insert("toid".into(), json!(clean_target));
        }
        let params = s.encoded_params(&Value::Object(msg_obj))?;
        let url = format!("{file_host}{msg_ep}?{}", s.common_params());
        let r = s.post_form(&url, format!("params={params}")).await?;
        s.maybe_decrypt(r, s.session_key_bytes()).await
    } else {
        json!({ "error_code": -1 })
    };

    if send_resp
        .get("error_code")
        .and_then(Value::as_i64)
        .unwrap_or(-1)
        != 0
    {
        // Fallback to single-shot `/api/message/sharefile_full` or `/api/group/sharefile_full`
        let share_ep = if is_group {
            "/api/group/sharefile_full"
        } else {
            "/api/message/sharefile_full"
        };
        let body = if is_group {
            obj(&[
                ("grid", json!(clean_target)),
                ("clientId", json!(client_id)),
            ])
        } else {
            obj(&[
                ("toid", json!(clean_target)),
                ("clientId", json!(client_id)),
            ])
        };
        let params = s.encoded_params(&body)?;
        let url = format!(
            "{file_host}{share_ep}?{}&params={params}&type={target_type}",
            s.common_params()
        );
        let r = s
            .post_multipart(&url, "fileContent", &file_name, mime, &file_bytes)
            .await?;
        send_resp = s.maybe_decrypt(r, s.session_key_bytes()).await;
    }

    if let Some(code) = send_resp.get("error_code").and_then(Value::as_i64) {
        if code != 0 {
            let msg = send_resp
                .get("error_message")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            return Err(crate::session::SessionError::new(code, msg).into());
        }
    }

    let final_url = if !file_url.is_empty() {
        file_url
    } else {
        send_resp
            .get("data")
            .and_then(|d| pick_str(d, &["fileUrl", "url", "href"]))
            .unwrap_or_default()
    };
    let msg_id = send_resp
        .get("data")
        .and_then(|d| pick_str(d, &["msgId", "cliMsgId"]))
        .unwrap_or_else(|| format!("out_{client_id}"));
    let kind_label = if is_video { "Video" } else { "Tập tin" };
    let display_text = if !caption.trim().is_empty() {
        caption.trim().to_string()
    } else {
        format!("[{kind_label}] {file_name}")
    };
    let cm = ChatMessage {
        id: msg_id,
        sender: "Tôi".into(),
        text: display_text,
        time: crate::ui::clock(),
        is_outgoing: true,
        ts: client_id as i64,
        link_url: if final_url.is_empty() {
            None
        } else {
            Some(final_url)
        },
        link_title: Some(format!("{file_name} ({})", format_file_size(total_size))),
        link_desc: if caption.trim().is_empty() {
            None
        } else {
            Some(caption.trim().to_string())
        },
        link_src: Some(format!("{kind_label} Zalo")),
        ..Default::default()
    };
    Ok((send_resp, cm))
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct LinkMetadata {
    pub url: String,
    pub title: String,
    pub desc: String,
    pub thumb: String,
    pub src: String,
}

/// Extract the first `https://` or `http://` URL from a message text string.
pub fn extract_first_url(text: &str) -> Option<String> {
    let trimmed = text.trim();
    let pos = trimmed
        .find("https://")
        .or_else(|| trimmed.find("http://"))?;
    let after = &trimmed[pos..];
    let end = after.find(char::is_whitespace).unwrap_or(after.len());
    let url = after[..end].trim_end_matches([')', ']', ',', ';']);
    if url.len() >= 10 {
        Some(url.to_string())
    } else {
        None
    }
}

fn unescape_html_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
        .trim()
        .to_string()
}

fn extract_meta_attr(tag: &str, attr_name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    for pat in [
        format!("{attr_name}=\""),
        format!("{attr_name}='"),
        format!("{attr_name} = \""),
    ] {
        if let Some(idx) = lower.find(&pat) {
            let quote_char = pat.chars().last().unwrap_or('"');
            let rest = &tag[idx + pat.len()..];
            if let Some(end) = rest.find(quote_char) {
                let val = unescape_html_entities(&rest[..end]);
                if !val.is_empty() {
                    return Some(val);
                }
            }
        }
    }
    None
}

/// Scrape OpenGraph / Twitter Card / HTML `<meta>` tags directly from a webpage URL.
pub async fn scrape_opengraph_metadata(url: &str) -> Option<LinkMetadata> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(3800))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .ok()?;
    let resp = client
        .get(url)
        .header(
            "User-Agent",
            "Mozilla/5.0 (compatible; Discordbot/2.0; +https://discordapp.com)",
        )
        .header("Accept-Language", "vi-VN,vi;q=0.9,en-US;q=0.8")
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let bytes =
        crate::media_resources::read_bounded_response(resp, crate::media_resources::HTML_LIMIT)
            .await
            .ok()?;
    let html = String::from_utf8_lossy(&bytes);

    let mut og_title = String::new();
    let mut og_desc = String::new();
    let mut og_thumb = String::new();
    let mut og_site = String::new();

    let mut cursor = html.as_ref();
    while let Some(start) = cursor.find('<') {
        let after = &cursor[start..];
        let Some(end) = after.find('>') else {
            break;
        };
        let tag = &after[..=end];
        cursor = &after[end + 1..];

        if tag.len() >= 5 && tag[1..5].eq_ignore_ascii_case("meta") {
            let key = extract_meta_attr(tag, "property")
                .or_else(|| extract_meta_attr(tag, "name"))
                .unwrap_or_default()
                .to_ascii_lowercase();
            let Some(content) = extract_meta_attr(tag, "content") else {
                continue;
            };
            match key.as_str() {
                "og:title" | "twitter:title" | "lark:url:video_title" if og_title.is_empty() => {
                    og_title = content;
                }
                "og:description" | "twitter:description" | "description" if og_desc.is_empty() => {
                    og_desc = content;
                }
                "og:image" | "twitter:image" | "lark:url:video_cover_image_url"
                    if og_thumb.is_empty() && content.starts_with("http") =>
                {
                    og_thumb = content;
                }
                "og:site_name" if og_site.is_empty() => {
                    og_site = content;
                }
                _ => {}
            }
        } else if og_title.is_empty() && tag.len() >= 6 && tag[1..6].eq_ignore_ascii_case("title") {
            if let Some(close) = cursor.find("</") {
                let raw_t = unescape_html_entities(&cursor[..close]);
                if !raw_t.is_empty() {
                    og_title = raw_t;
                }
            }
        }
    }

    let domain = url
        .split("://")
        .nth(1)
        .and_then(|r| r.split('/').next())
        .unwrap_or("Liên kết")
        .to_string();
    Some(LinkMetadata {
        url: url.to_string(),
        title: og_title,
        desc: og_desc,
        thumb: og_thumb,
        src: if !og_site.is_empty() { og_site } else { domain },
    })
}

fn is_placeholder_zalo_thumb(thumb: &str) -> bool {
    thumb.is_empty()
        || thumb.contains("feed_thumb_link")
        || thumb.contains("im_parselink")
        || thumb.contains("aHR0cHM6Ly9yZXMtemFsby56YWRuLnZuL3VwbG9hZC9tZWRpYS8yMDE5LzEwLzE1L2ZlZWRfdGh1bWJfbGlua19fMV9fMTU3MTEzMzEyMjc3OF85Mzc4MC5wbmc=")
}

/// Parse rich link metadata (`title`, `desc`, `thumb`, `src`) via Zalo's `/api/message/parselink`
/// (`cmd 11285`) combined with direct OpenGraph HTML scraping fallback.
pub async fn parse_link_metadata(s: &Session, raw_url: &str) -> Result<LinkMetadata> {
    let url = raw_url.trim().to_string();
    let domain = url
        .split("://")
        .nth(1)
        .and_then(|r| r.split('/').next())
        .unwrap_or("Liên kết")
        .to_string();
    let mut meta = LinkMetadata {
        url: url.clone(),
        title: String::new(),
        desc: String::new(),
        thumb: String::new(),
        src: domain.clone(),
    };

    if s.login.is_some() {
        let file_host = if s.domains.file.is_empty() {
            "https://tt-files-wpa.chat.zalo.me"
        } else {
            s.domains.file.trim_end_matches('/')
        };
        let body = obj(&[
            ("link", json!(url)),
            ("version", json!(1)),
            ("imei", json!(s.imei)),
        ]);
        if let Ok(params) = s.encoded_params(&body) {
            let parse_url = format!(
                "{file_host}/api/message/parselink?{}&params={params}",
                s.common_params()
            );
            if let Ok(resp) = s.get(&parse_url).await {
                let dec = s.maybe_decrypt(resp, s.session_key_bytes()).await;
                let d = dec
                    .get("data")
                    .and_then(|x| x.get("data").or(Some(x)))
                    .unwrap_or(&dec);
                if let Some(t) = d.get("title").and_then(Value::as_str).map(str::trim) {
                    if !t.is_empty() && t != url {
                        meta.title = t.to_string();
                    }
                }
                if let Some(ds) = d.get("desc").and_then(Value::as_str).map(str::trim) {
                    if !ds.is_empty() && ds != url {
                        meta.desc = ds.to_string();
                    }
                }
                if let Some(th) = d.get("thumb").and_then(Value::as_str).map(str::trim) {
                    if !is_placeholder_zalo_thumb(th) {
                        meta.thumb = th.to_string();
                    }
                }
                if let Some(sr) = d.get("src").and_then(Value::as_str).map(str::trim) {
                    if !sr.is_empty() {
                        meta.src = sr.to_string();
                    }
                }
            }
        }
    }

    if meta.title.is_empty() || meta.thumb.is_empty() || meta.desc.is_empty() {
        if let Some(og) = scrape_opengraph_metadata(&url).await {
            if meta.title.is_empty() && !og.title.is_empty() {
                meta.title = og.title;
            }
            if meta.desc.is_empty() && !og.desc.is_empty() {
                meta.desc = og.desc;
            }
            if meta.thumb.is_empty() && !og.thumb.is_empty() {
                meta.thumb = og.thumb;
            }
            if (meta.src.is_empty() || meta.src == domain) && !og.src.is_empty() {
                meta.src = og.src;
            }
        }
    }

    if meta.title.is_empty() {
        meta.title = domain;
    }
    Ok(meta)
}

/// Send a rich link message (`sendLinkMessage` @ `compact-app-pc.js:14319196`):
/// - 1-1 (`cmd 11286`): `POST {domains.chat}/api/message/link?{common}`
/// - Group (`cmd 11193`): `POST {domains.group}/api/group/sendlink?{common}`
pub async fn send_link_message(
    s: &Session,
    to_id: &str,
    text: &str,
    meta: &LinkMetadata,
    is_group: bool,
) -> Result<Value> {
    if s.login.is_none() {
        anyhow::bail!("chưa đăng nhập");
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let msg_str = if text.trim().is_empty() {
        meta.url.as_str()
    } else {
        text.trim()
    };
    let title_str = if meta.title.trim().is_empty() {
        meta.url.as_str()
    } else {
        meta.title.trim()
    };
    let media_json = json!({
        "type": 0,
        "count": 0,
        "mediaTitle": "",
        "artist": "",
        "streamUrl": "",
        "stream_icon": ""
    })
    .to_string();

    let res: Result<Value> = async {
        if is_group {
            let clean_grid = to_id
                .trim_start_matches("group_")
                .trim_start_matches('g')
                .trim_start_matches('G');
            let body = obj(&[
                ("grid", json!(clean_grid)),
                ("msg", json!(msg_str)),
                ("href", json!(meta.url)),
                ("src", json!(meta.src)),
                ("title", json!(title_str)),
                ("desc", json!(meta.desc)),
                ("thumb", json!(meta.thumb)),
                ("type", json!(0)),
                ("media", json!(media_json)),
                ("clientId", json!(now)),
                ("mentionInfo", json!("[]")),
                ("language", json!("vi")),
                ("ttl", json!(0)),
                ("imei", json!(s.imei)),
            ]);
            let params = s.encoded_params(&body)?;
            let url = format!(
                "{}/api/group/sendlink?{}",
                s.domains.group,
                s.common_params()
            );
            let resp = s.post_form(&url, format!("params={params}")).await?;
            let dec = s.maybe_decrypt(resp, s.session_key_bytes()).await;
            if let Some(code) = dec.get("error_code").and_then(Value::as_i64) {
                if code != 0 {
                    anyhow::bail!("sendlink group error {code}");
                }
            }
            Ok(dec)
        } else {
            let host = if s.domains.chat.contains("-msg-chat.")
                || s.domains.chat.contains("tt-chat2-wpa")
                || s.domains.chat.is_empty()
            {
                "https://tt-chat3-wpa.api.zaloapp.com"
            } else {
                &s.domains.chat
            };
            let body = obj(&[
                ("toId", json!(to_id)),
                ("msg", json!(msg_str)),
                ("href", json!(meta.url)),
                ("src", json!(meta.src)),
                ("title", json!(title_str)),
                ("desc", json!(meta.desc)),
                ("thumb", json!(meta.thumb)),
                ("type", json!(0)),
                ("media", json!(media_json)),
                ("clientId", json!(now)),
                ("mentionInfo", json!("")),
                ("language", json!("vi")),
                ("ttl", json!(0)),
                ("imei", json!(s.imei)),
            ]);
            let params = s.encoded_params(&body)?;
            let url = format!("{host}/api/message/link?{}", s.common_params());
            let resp = s.post_form(&url, format!("params={params}")).await?;
            let dec = s.maybe_decrypt(resp, s.session_key_bytes()).await;
            if let Some(code) = dec.get("error_code").and_then(Value::as_i64) {
                if code != 0 {
                    anyhow::bail!("sendlink error {code}");
                }
            }
            Ok(dec)
        }
    }
    .await;

    match res {
        Ok(v) => Ok(v),
        Err(_) => {
            if is_group {
                send_group_message(s, to_id, msg_str).await
            } else {
                send_message(s, to_id, msg_str).await
            }
        }
    }
}

/// Send a sticker (`sendSticker` in `compact-app-pc.b7bec208c4c8f73e09f8.js:14325410`):
/// - 1-1 (`cmd 11154`): `POST {domains.chat}/api/message/sticker?{common}`
/// - Group (`cmd 11165`): `POST {domains.group}/api/group/sticker?{common}`
pub async fn send_sticker(
    s: &Session,
    to_id: &str,
    sticker_id: u64,
    cate_id: u64,
    sticker_type: u32,
    is_group: bool,
) -> Result<Value> {
    if s.login.is_none() {
        anyhow::bail!("chưa đăng nhập");
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let url = if is_group {
        let clean_grid = to_id
            .trim_start_matches("group_")
            .trim_start_matches('g')
            .trim_start_matches('G');
        let body = obj(&[
            ("grid", json!(clean_grid)),
            ("stickerId", json!(sticker_id)),
            ("cateId", json!(cate_id)),
            ("type", json!(sticker_type)),
            ("clientId", json!(now)),
            ("imei", json!(s.imei)),
            ("zsource", json!(101)),
        ]);
        let params = s.encoded_params(&body)?;
        let url = format!(
            "{}/api/group/sticker?{}",
            s.domains.group,
            s.common_params()
        );
        let form = format!("params={params}");
        let resp = s.post_form(&url, form).await?;
        let resp = s.maybe_decrypt(resp, s.session_key_bytes()).await;
        if let Some(code) = resp.get("error_code").and_then(Value::as_i64) {
            if code != 0 {
                let msg = resp
                    .get("error_message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                return Err(crate::session::SessionError::new(code, msg).into());
            }
        }
        return Ok(resp);
    } else {
        let host = if s.domains.chat.contains("-msg-chat.")
            || s.domains.chat.contains("tt-chat2-wpa")
            || s.domains.chat.is_empty()
        {
            "https://tt-chat3-wpa.api.zaloapp.com"
        } else {
            &s.domains.chat
        };
        format!("{host}/api/message/sticker?{}", s.common_params())
    };

    let body = obj(&[
        ("toid", json!(to_id)),
        ("stickerId", json!(sticker_id)),
        ("cateId", json!(cate_id)),
        ("type", json!(sticker_type)),
        ("clientId", json!(now)),
        ("imei", json!(s.imei)),
        ("zsource", json!(101)),
    ]);
    let params = s.encoded_params(&body)?;
    let form = format!("params={params}");
    let resp = s.post_form(&url, form).await?;
    let resp = s.maybe_decrypt(resp, s.session_key_bytes()).await;
    if let Some(code) = resp.get("error_code").and_then(Value::as_i64) {
        if code != 0 {
            let msg = resp
                .get("error_message")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            return Err(crate::session::SessionError::new(code, msg).into());
        }
    }
    Ok(resp)
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StickerPackInfo {
    pub id: u64,
    pub name: String,
    pub total_image: u32,
    pub icon_url: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StickerItem {
    pub sticker_id: u64,
    pub cate_id: u64,
    pub sticker_type: u32,
    pub label: String,
}

/// Fetch the complete Zalo Sticker Store catalog (`getStickerCatsV2` -> `getAllCate` in `compact-app-pc.js:14288962`).
///
/// Returns ~390 official Zalo sticker packs sorted by newest/featured first and caches them to
/// `stickers_cache/store_packs.json`.
pub async fn fetch_sticker_store_packs(s: &Session) -> Result<Vec<StickerPackInfo>> {
    let cache_dir = std::path::Path::new("stickers_cache");
    let _ = std::fs::create_dir_all(cache_dir);
    let cache_path = cache_dir.join("store_packs.json");
    if let Ok(raw_cache) = std::fs::read_to_string(&cache_path) {
        if let Ok(cached) = serde_json::from_str::<Vec<StickerPackInfo>>(&raw_cache) {
            if cached.len() >= 20 {
                return Ok(cached);
            }
        }
    }

    if s.login.is_none() {
        anyhow::bail!("chưa đăng nhập");
    }
    let sticker_host = if s.domains.sticker.is_empty() {
        "https://tt-sticker-wpa.chat.zalo.me"
    } else {
        s.domains.sticker.trim_end_matches('/')
    };
    let key = s.session_key_bytes();
    if key.is_empty() {
        anyhow::bail!("thiếu session key để tải kho nhãn dán");
    }
    let enc_empty = crate::crypto::encode_aes(key, "")?;
    let mut encoded_param = String::with_capacity(enc_empty.len() + 16);
    for b in enc_empty.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded_param.push(b as char);
            }
            _ => {
                use std::fmt::Write;
                let _ = write!(encoded_param, "%{b:02X}");
            }
        }
    }

    let list_url = format!(
        "{sticker_host}/api/message/sticker/category/list/v2?{}&params={encoded_param}",
        s.common_params()
    );
    let resp = s.get(&list_url).await?;
    let dec = s.maybe_decrypt(resp, key).await;
    let data = dec.get("data").unwrap_or(&dec);

    let ex_ids: std::collections::HashSet<u64> = data
        .get("exCateIds")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(Value::as_u64).collect())
        .unwrap_or_default();

    let data_url = data
        .get("dataUrl")
        .and_then(Value::as_str)
        .unwrap_or("https://zalo-api.zadn.vn/api/emoticon/pcweb/getAllCate")
        .replace("http://", "https://");

    let cdn_resp = s.http.get(&data_url).send().await?;
    let raw_bytes = cdn_resp.bytes().await?;
    let json_text = if raw_bytes.len() >= 2 && raw_bytes[0] == 0x1f && raw_bytes[1] == 0x8b {
        use std::io::Read;
        let mut gz = flate2::read::GzDecoder::new(&raw_bytes[..]);
        let mut out = String::new();
        gz.read_to_string(&mut out)?;
        out
    } else {
        String::from_utf8_lossy(&raw_bytes).into_owned()
    };

    let arr: Vec<Value> = serde_json::from_str(&json_text)?;
    let mut packs = Vec::with_capacity(arr.len());
    for item in arr {
        let Some(id) = item.get("id").and_then(Value::as_u64) else {
            continue;
        };
        if id == 0 || ex_ids.contains(&id) {
            continue;
        }
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if name.is_empty() || name.to_lowercase().contains("test") {
            continue;
        }
        let total_image = item.get("totalImage").and_then(Value::as_u64).unwrap_or(16) as u32;
        let icon_url = item
            .get("iconUrl")
            .or_else(|| item.get("thumbImg"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .replace("http://", "https://");
        packs.push(StickerPackInfo {
            id,
            name,
            total_image,
            icon_url,
        });
    }

    // Sort newest packs first (`id` descending) so modern Zalo packs appear at the top of the store
    packs.sort_by(|a, b| b.id.cmp(&a.id));
    if let Ok(txt) = serde_json::to_string(&packs) {
        let _ = std::fs::write(&cache_path, txt);
    }
    Ok(packs)
}

/// Fetch all stickers belonging to a specific category (`getStickerCategoryDetail` — `cmd 11284`).
pub async fn fetch_sticker_pack_items(
    s: &Session,
    cate_id: u64,
    pack_name: &str,
) -> Result<Vec<StickerItem>> {
    if cate_id == 0 {
        return Ok((1..=30)
            .map(|id| StickerItem {
                sticker_id: id,
                cate_id: 0,
                sticker_type: 7,
                label: format!("Củ hành #{id}"),
            })
            .collect());
    }

    let cache_dir = std::path::Path::new("stickers_cache");
    let _ = std::fs::create_dir_all(cache_dir);
    let cache_path = cache_dir.join(format!("pack_{cate_id}.json"));
    if let Ok(raw_cache) = std::fs::read_to_string(&cache_path) {
        if let Ok(cached) = serde_json::from_str::<Vec<StickerItem>>(&raw_cache) {
            if !cached.is_empty() {
                return Ok(cached);
            }
        }
    }

    if s.login.is_none() {
        anyhow::bail!("chưa đăng nhập");
    }
    let sticker_host = if s.domains.sticker.is_empty() {
        "https://tt-sticker-wpa.chat.zalo.me"
    } else {
        s.domains.sticker.trim_end_matches('/')
    };
    let params = s.encoded_params(&json!({ "cid": cate_id.to_string() }))?;
    let url = format!(
        "{sticker_host}/api/message/sticker/category/sticker_detail?{}&params={params}",
        s.common_params()
    );
    let resp = s.get(&url).await?;
    let dec = s.maybe_decrypt(resp, s.session_key_bytes()).await;
    let arr = dec
        .get("data")
        .and_then(Value::as_array)
        .or_else(|| dec.as_array())
        .cloned()
        .unwrap_or_default();

    let mut items = Vec::with_capacity(arr.len());
    for (idx, it) in arr.iter().enumerate() {
        let Some(sid) = it
            .get("id")
            .or_else(|| it.get("stickerId"))
            .and_then(Value::as_u64)
        else {
            continue;
        };
        let cid = it.get("cateId").and_then(Value::as_u64).unwrap_or(cate_id);
        let stype = it.get("type").and_then(Value::as_u64).unwrap_or(7) as u32;
        items.push(StickerItem {
            sticker_id: sid,
            cate_id: cid,
            sticker_type: stype,
            label: format!("{pack_name} #{}", idx + 1),
        });
    }

    if !items.is_empty() {
        if let Ok(txt) = serde_json::to_string(&items) {
            let _ = std::fs::write(&cache_path, txt);
        }
    }
    Ok(items)
}

/// Normalize a sticker keyword by converting non-breaking spaces (`\u{a0}`), collapsing
/// consecutive whitespace to a single ASCII space, and converting to lowercase.
pub fn normalize_sticker_keyword(s: &str) -> String {
    s.replace('\u{a0}', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Strip Vietnamese diacritics to lowercase ASCII (`simpleStripVietnamese` in `compact-app-pc.js:12955017`).
pub fn strip_vn_for_sticker_kw(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for raw in s.chars() {
        for c in raw.to_lowercase() {
            let mapped = match c {
                'à' | 'á' | 'ạ' | 'ả' | 'ã' | 'â' | 'ầ' | 'ấ' | 'ậ' | 'ẩ' | 'ẫ' | 'ă' | 'ằ'
                | 'ắ' | 'ặ' | 'ẳ' | 'ẵ' => 'a',
                'è' | 'é' | 'ẹ' | 'ẻ' | 'ẽ' | 'ê' | 'ề' | 'ế' | 'ệ' | 'ể' | 'ễ' => {
                    'e'
                }
                'ì' | 'í' | 'ị' | 'ỉ' | 'ĩ' => 'i',
                'ò' | 'ó' | 'ọ' | 'ỏ' | 'õ' | 'ô' | 'ồ' | 'ố' | 'ộ' | 'ổ' | 'ỗ' | 'ơ' | 'ờ'
                | 'ớ' | 'ợ' | 'ở' | 'ỡ' => 'o',
                'ù' | 'ú' | 'ụ' | 'ủ' | 'ũ' | 'ư' | 'ừ' | 'ứ' | 'ự' | 'ử' | 'ữ' => {
                    'u'
                }
                'ỳ' | 'ý' | 'ỵ' | 'ỷ' | 'ỹ' => 'y',
                'đ' => 'd',
                '\u{a0}' => ' ',
                other => other,
            };
            out.push(mapped);
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Map common unaccented Vietnamese keywords, English terms, or chat slang to the accented
/// Vietnamese form indexed by Zalo's `/api/message/sticker/suggest/stickers` (`cmd 11741`) and
/// `/api/message/sticker/search` (`cmd 11747`) endpoints.
pub fn canonical_accented_vn_for_sticker(stripped: &str) -> Option<&'static str> {
    match stripped {
        "nho" | "nho em" | "nho anh" | "nho qua" | "miss" | "miss u" | "miss you" => Some("nhớ"),
        "khoc" | "huhu" | "huhuhu" | "hic" | "cry" => Some("khóc"),
        "buon" | "buon qua" | "sad" | "tui than" => Some("buồn"),
        "vui" | "vui ve" | "vui qua" | "happy" => Some("vui"),
        "cuoi" | "mac cuoi" | "buon cuoi" => Some("cười"),
        "yeu" | "iu" | "yeu em" | "yeu anh" | "iu em" | "iu anh" | "love" | "love u"
        | "love you" | "tha tim" | "tim" => Some("yêu"),
        "thuong" | "thuong em" | "thuong anh" | "thuong qua" => Some("thương"),
        "gian" | "gian doi" | "doi hon" | "tuc" | "buc" | "buc minh" | "cay" | "angry" => {
            Some("giận")
        }
        "met" | "met qua" | "met moi" | "duoi" | "duoi qua" | "tired" => Some("mệt"),
        "doi" | "doi bung" | "doi qua" | "hungry" => Some("đói"),
        "an" | "an com" | "di an" => Some("ăn cơm"),
        "ca phe" | "cf" | "cafe" | "coffee" => Some("cà phê"),
        "tra sua" | "trasua" => Some("trà sữa"),
        "nhau" | "di nhau" | "bia" | "dzoo" | "dzo" => Some("nhậu"),
        "ngu" | "di ngu" | "buon ngu" | "ngu ngon" | "ngu ngon nhe" | "ngu ngon nha"
        | "good night" | "goodnight" | "gn" | "g9" => Some("ngủ"),
        "chao" | "xin chao" | "chao ban" | "chao anh" | "chao chi" | "chao em" | "he lo"
        | "helo" => Some("xin chào"),
        "cam on" | "cam on ban" | "cam on anh" | "cam on chi" | "cam on em" | "cam on nhieu"
        | "thank" | "thanks" | "thank you" | "tks" | "tkss" | "thank kiu" => Some("cảm ơn"),
        "xin loi" | "xin loi nha" | "xin loi nhe" | "sorry" | "sr" | "so ry" => Some("xin lỗi"),
        "tam biet" | "tam biet nhe" | "bye" | "byebye" | "good bye" | "goodbye" | "bibi" | "pp" => {
            Some("tạm biệt")
        }
        "chuc mung" | "happy birthday" | "hpbd" | "sinh nhat" | "cmsn" => Some("chúc mừng"),
        "tet" | "chuc mung nam moi" | "nam moi" => Some("tết"),
        "co len" | "co gang" | "fighting" => Some("cố lên"),
        "tuyet" | "tuyet voi" | "xuat sac" | "dinh" | "dinh qua" | "chuan" | "duoc" => {
            Some("tuyệt vời")
        }
        "ngai" | "ngai qua" | "ngai nhi" | "xau ho" => Some("ngại"),
        "so" | "so qua" | "scared" => Some("sợ"),
        "lanh" | "lanh qua" | "lanh khong" | "ret" | "ret qua" => Some("lạnh"),
        "nong" | "nong qua" | "nong buc" => Some("nóng"),
        "tien" | "luong" => Some("tiền"),
        "het tien" | "ngheo" | "ngheo qua" | "vi rong" => Some("hết tiền"),
        "quy" | "lay" | "xin" | "van xin" => Some("lạy"),
        "ban" | "ban qua" | "busy" => Some("bận"),
        "chan" | "chan qua" | "chan doi" | "boring" => Some("chán"),
        "cho" | "cho chut" | "cho xiu" | "doi chut" | "doi xiu" | "wait" => Some("chờ chút"),
        "di choi" | "choi" | "dao pho" => Some("đi chơi"),
        "di lam" | "lam viec" | "deadline" | "code" => Some("làm"),
        "hong" | "hong chuyen" | "ngac nhien" => Some("ngạc nhiên"),
        _ => None,
    }
}

/// Build the deduplicated list of query variants (accented + unaccented + canonical synonym)
/// to maximize recall across both Zalo Sticker Suggest (`cmd 11741`) and Search (`cmd 11747`).
pub fn canonical_vn_sticker_queries(keyword: &str) -> Vec<String> {
    let norm = normalize_sticker_keyword(keyword);
    if norm.is_empty() {
        return Vec::new();
    }
    let stripped = strip_vn_for_sticker_kw(&norm);
    let mut queries = vec![norm.clone()];
    if !stripped.is_empty() && stripped != norm && !queries.contains(&stripped) {
        queries.push(stripped.clone());
    }
    if let Some(accented) = canonical_accented_vn_for_sticker(&stripped) {
        let acc_s = accented.to_string();
        if !queries.contains(&acc_s) {
            queries.push(acc_s.clone());
        }
        let acc_stripped = strip_vn_for_sticker_kw(&acc_s);
        if !acc_stripped.is_empty() && !queries.contains(&acc_stripped) {
            queries.push(acc_stripped);
        }
    }
    queries.truncate(4);
    queries
}

/// Fetch official Zalo contextual sticker trigger keywords (`getStickerListKeyWords` — `cmd 11740`).
/// Includes both normalized accented/raw forms and ASCII-folded forms so typing accented or
/// unaccented Vietnamese triggers suggestions reliably.
pub async fn fetch_sticker_suggest_keywords(s: &Session) -> Result<Vec<String>> {
    let cache_dir = std::path::Path::new("stickers_cache");
    let _ = std::fs::create_dir_all(cache_dir);
    let cache_path = cache_dir.join("suggest_keywords_v2.json");
    if let Ok(raw_cache) = std::fs::read_to_string(&cache_path) {
        if let Ok(cached) = serde_json::from_str::<Vec<String>>(&raw_cache) {
            if cached.len() >= 100 {
                return Ok(cached);
            }
        }
    }
    if s.login.is_none() {
        anyhow::bail!("chưa đăng nhập");
    }
    let sticker_host = if s.domains.sticker.is_empty() {
        "https://tt-sticker-wpa.chat.zalo.me"
    } else {
        s.domains.sticker.trim_end_matches('/')
    };
    let params = s.encoded_params(&json!({ "imei": s.imei }))?;
    let url = format!(
        "{sticker_host}/api/message/sticker/suggest/keywords?{}&params={params}",
        s.common_params()
    );
    let resp = match s.get(&url).await {
        Ok(r) => r,
        Err(_) => {
            let fallback_url = format!(
                "https://tt-sticker-wpa.zaloapp.com/api/message/sticker/suggest/keywords?{}&params={params}",
                s.common_params()
            );
            s.get(&fallback_url).await?
        }
    };
    let dec = s.maybe_decrypt(resp, s.session_key_bytes()).await;
    let mut seen = std::collections::HashSet::new();
    let mut kws: Vec<String> = Vec::new();
    if let Some(arr) = dec
        .get("data")
        .or(Some(&dec))
        .and_then(|d| d.get("keywords"))
        .and_then(Value::as_array)
    {
        for raw_v in arr {
            let Some(raw_str) = raw_v.as_str() else {
                continue;
            };
            let norm = normalize_sticker_keyword(raw_str);
            if norm.is_empty() || norm.starts_with("zkwd_") {
                continue;
            }
            if seen.insert(norm.clone()) {
                kws.push(norm.clone());
            }
            let stripped = strip_vn_for_sticker_kw(&norm);
            if !stripped.is_empty() && seen.insert(stripped.clone()) {
                kws.push(stripped);
            }
        }
    }
    if !kws.is_empty() {
        if let Ok(txt) = serde_json::to_string(&kws) {
            let _ = std::fs::write(&cache_path, txt);
        }
    }
    Ok(kws)
}

async fn query_sticker_endpoints_once(
    s: &Session,
    sticker_host: &str,
    queries: &[String],
) -> Vec<(u64, u64, u32, u8)> {
    let mut futs = Vec::with_capacity(queries.len() * 2);
    for (q_idx, q) in queries.iter().enumerate() {
        // 1) Contextual suggest endpoint (`getStickerSuggestion` — `cmd 11741`)
        let sugg_body = obj(&[
            ("keyword", json!(q)),
            ("gif", json!(1)),
            ("guggy", json!(0)),
            ("imei", json!(s.imei)),
        ]);
        if let Ok(params) = s.encoded_params(&sugg_body) {
            let url = format!(
                "{sticker_host}/api/message/sticker/suggest/stickers?{}&params={params}",
                s.common_params()
            );
            let is_primary = q_idx == 0;
            futs.push(async move {
                let Ok(resp) = s.get(&url).await else {
                    return Vec::new();
                };
                let dec = s.maybe_decrypt(resp, s.session_key_bytes()).await;
                let arr = dec
                    .get("data")
                    .or(Some(&dec))
                    .and_then(|d| d.get("sugg_sticker"))
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let mut items = Vec::with_capacity(arr.len());
                for it in arr {
                    let Some(sid) = it
                        .get("sticker_id")
                        .or_else(|| it.get("id"))
                        .and_then(Value::as_u64)
                    else {
                        continue;
                    };
                    let cid = it
                        .get("cate_id")
                        .or_else(|| it.get("cateId"))
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    let stype = it.get("type").and_then(Value::as_u64).unwrap_or(7) as u32;
                    items.push((cid, sid, stype, if is_primary { 3u8 } else { 2u8 }));
                }
                items
            });
        }
    }

    let mut search_futs = Vec::with_capacity(queries.len());
    for (q_idx, q) in queries.iter().enumerate() {
        // 2) Full sticker tag search endpoint (`searchSticker` — `cmd 11747`)
        let search_body = obj(&[
            ("keyword", json!(q)),
            ("limit", json!(30)),
            ("srcType", json!(0)),
            ("imei", json!(s.imei)),
        ]);
        if let Ok(params) = s.encoded_params(&search_body) {
            let url = format!(
                "{sticker_host}/api/message/sticker/search?{}&params={params}",
                s.common_params()
            );
            let is_primary = q_idx == 0;
            search_futs.push(async move {
                let Ok(resp) = s.get(&url).await else {
                    return Vec::new();
                };
                let dec = s.maybe_decrypt(resp, s.session_key_bytes()).await;
                let arr = dec
                    .get("data")
                    .and_then(Value::as_array)
                    .or_else(|| dec.as_array())
                    .cloned()
                    .unwrap_or_default();
                let mut items = Vec::with_capacity(arr.len());
                for it in arr {
                    let Some(sid) = it
                        .get("sticker_id")
                        .or_else(|| it.get("id"))
                        .and_then(Value::as_u64)
                    else {
                        continue;
                    };
                    let cid = it
                        .get("cate_id")
                        .or_else(|| it.get("cateId"))
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    let stype = it.get("type").and_then(Value::as_u64).unwrap_or(7) as u32;
                    items.push((cid, sid, stype, if is_primary { 3u8 } else { 2u8 }));
                }
                items
            });
        }
    }

    let (sugg_batches, search_batches) = tokio::join!(
        futures_util::future::join_all(futs),
        futures_util::future::join_all(search_futs)
    );

    // Merge and boost stickers that matched across multiple endpoints/queries
    let mut score_map: std::collections::HashMap<u64, (u64, u32, u8)> =
        std::collections::HashMap::new();
    for batch in sugg_batches.into_iter().chain(search_batches.into_iter()) {
        for (cid, sid, stype, weight) in batch {
            score_map
                .entry(sid)
                .and_modify(|e| {
                    e.2 = e.2.saturating_add(weight);
                    if e.0 == 0 && cid > 0 {
                        e.0 = cid;
                    }
                })
                .or_insert((cid, stype, weight));
        }
    }

    score_map
        .into_iter()
        .map(|(sid, (cid, stype, score))| (cid, sid, stype, score))
        .collect()
}

/// Search Zalo stickers by Vietnamese, English, or emoji keyword by querying both
/// `/api/message/sticker/suggest/stickers` (`cmd 11741`) and `/api/message/sticker/search` (`cmd 11747`)
/// concurrently across accented and unaccented Vietnamese variants.
pub async fn search_stickers_by_keyword(s: &Session, keyword: &str) -> Result<Vec<StickerItem>> {
    let kw = normalize_sticker_keyword(keyword);
    if kw.is_empty() {
        return Ok(Vec::new());
    }
    let cache_dir = std::path::Path::new("stickers_cache");
    let _ = std::fs::create_dir_all(cache_dir);
    let cache_path = cache_dir.join(format!("suggest_v2_{}.json", crate::crypto::md5_hex(&kw)));
    if let Ok(raw_cache) = std::fs::read_to_string(&cache_path) {
        if let Ok(cached) = serde_json::from_str::<Vec<StickerItem>>(&raw_cache) {
            if !cached.is_empty() {
                return Ok(cached);
            }
        }
    }
    if s.login.is_none() {
        anyhow::bail!("chưa đăng nhập");
    }
    let sticker_host = if s.domains.sticker.is_empty() {
        "https://tt-sticker-wpa.chat.zalo.me"
    } else {
        s.domains.sticker.trim_end_matches('/')
    };

    let queries = canonical_vn_sticker_queries(&kw);
    let mut raw_items = query_sticker_endpoints_once(s, sticker_host, &queries).await;

    // If a multi-word sentence/phrase (e.g. "anh ơi đi chơi", "hôm nay mệt quá") had no direct match,
    // automatically fallback to its trailing 2-word phrase (`tail2`) or trailing word (`last_word`).
    if raw_items.is_empty() {
        let words: Vec<&str> = kw.split_whitespace().collect();
        if words.len() >= 2 && words.len() <= 5 {
            let mut fallback_phrases = Vec::new();
            if words.len() >= 3 {
                fallback_phrases.push(format!(
                    "{} {}",
                    words[words.len() - 2],
                    words[words.len() - 1]
                ));
            }
            let last_w = words[words.len() - 1];
            if last_w.chars().count() >= 2 {
                fallback_phrases.push(last_w.to_string());
            }
            for fb in fallback_phrases {
                let fb_queries = canonical_vn_sticker_queries(&fb);
                raw_items = query_sticker_endpoints_once(s, sticker_host, &fb_queries).await;
                if !raw_items.is_empty() {
                    break;
                }
            }
        }
    }

    // Prioritize modern illustrated Zalo character packs (`cate_id` descending, matching Zalo PC's
    // `pointCate` ranking) and exclude legacy round-emoji/photo-meme packs when character packs exist.
    const LEGACY_NON_CHAR_PACKS: &[u64] = &[
        0, 50, 10788, 11190, 11984, 11998, 12002, 12003, 12024, 12658,
    ];
    let has_modern_char = raw_items
        .iter()
        .any(|(cid, _, _, _)| *cid >= 10000 && !LEGACY_NON_CHAR_PACKS.contains(cid));
    if has_modern_char {
        raw_items.retain(|(cid, _, _, _)| *cid >= 10000 && !LEGACY_NON_CHAR_PACKS.contains(cid));
    }
    // Sort by relevance score first (matched in both search + suggest), then newest pack (`cate_id` desc)
    raw_items.sort_by(|a, b| b.3.cmp(&a.3).then_with(|| b.0.cmp(&a.0)));

    // Keep at most 2 stickers from the same pack in the first pass so suggestions show diverse characters,
    // then fill up to 24 items if needed for Sticker Picker search grids.
    let mut pack_counts: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
    let mut out = Vec::with_capacity(24);
    let mut deferred = Vec::new();
    for &(cid, sid, stype, _) in &raw_items {
        let cnt = pack_counts.entry(cid).or_insert(0);
        if *cnt >= 2 {
            deferred.push((cid, sid, stype));
            continue;
        }
        *cnt += 1;
        let idx = out.len() + 1;
        out.push(StickerItem {
            sticker_id: sid,
            cate_id: cid,
            sticker_type: stype,
            label: format!("{kw} #{idx}"),
        });
        if out.len() >= 24 {
            break;
        }
    }
    for (cid, sid, stype) in deferred {
        if out.len() >= 24 {
            break;
        }
        let idx = out.len() + 1;
        out.push(StickerItem {
            sticker_id: sid,
            cate_id: cid,
            sticker_type: stype,
            label: format!("{kw} #{idx}"),
        });
    }

    if !out.is_empty() {
        if let Ok(txt) = serde_json::to_string(&out) {
            let _ = std::fs::write(&cache_path, txt);
        }
    }
    Ok(out)
}

/// Send a quote/reply message (`sendQuoteMessage` / `sendGroupQuoteMessage` in `compact-app-pc.b7bec208c4c8f73e09f8.js:14317494`):
/// - 1-1 (`cmd 11771`): `POST {domains.chat}/api/message/quote?{common}`
/// - Group (`cmd 11197`): `POST {domains.group}/api/group/quote?{common}`
///
/// Automatically falls back to `send_message` / `send_group_message` if the quoted message lacks a server ID.
pub async fn send_quote_message(
    s: &Session,
    to_id: &str,
    text: &str,
    quoted: &ChatMessage,
    is_group: bool,
) -> Result<Value> {
    if s.login.is_none() {
        anyhow::bail!("chưa đăng nhập");
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let my_uid = s
        .login
        .as_ref()
        .map(|l| l.uin.clone())
        .unwrap_or_else(|| "0".into());

    let qmsg_owner = if quoted.is_outgoing {
        my_uid.as_str()
    } else if !quoted.sender.is_empty() && quoted.sender.chars().all(|c| c.is_ascii_digit()) {
        quoted.sender.as_str()
    } else if !is_group {
        to_id
    } else {
        "0"
    };

    let qmsg_id_num = quoted.id.parse::<u64>().unwrap_or(now.saturating_sub(1000));
    let qmsg_ts = if quoted.ts > 0 {
        quoted.ts as u64
    } else {
        now.saturating_sub(1000)
    };
    let qmsg_cli_id = qmsg_ts;

    let quote_res: Result<Value> = async {
        if quoted.id.starts_with("out_") || quoted.id.starts_with("local_") || quoted.id.is_empty()
        {
            anyhow::bail!("local quote id");
        }
        if is_group {
            let clean_grid = to_id
                .trim_start_matches("group_")
                .trim_start_matches('g')
                .trim_start_matches('G');
            let body = obj(&[
                ("grid", json!(clean_grid)),
                ("message", json!(text)),
                ("clientId", json!(now)),
                ("language", json!("vi")),
                ("qmsgOwner", json!(qmsg_owner)),
                ("qmsgId", json!(qmsg_id_num)),
                ("qmsgCliId", json!(qmsg_cli_id)),
                ("qmsgType", json!(1)),
                ("qmsg", json!(quoted.text)),
                ("qmsgTs", json!(qmsg_ts)),
                ("visibility", json!(0)),
                ("ttl", json!(0)),
                ("imei", json!(s.imei)),
                ("zsource", json!(101)),
            ]);
            let params = s.encoded_params(&body)?;
            let url = format!("{}/api/group/quote?{}", s.domains.group, s.common_params());
            let resp = s.post_form(&url, format!("params={params}")).await?;
            let resp = s.maybe_decrypt(resp, s.session_key_bytes()).await;
            if let Some(code) = resp.get("error_code").and_then(Value::as_i64) {
                if code != 0 {
                    anyhow::bail!("quote error {code}");
                }
            }
            Ok(resp)
        } else {
            let host = if s.domains.chat.contains("-msg-chat.")
                || s.domains.chat.contains("tt-chat2-wpa")
                || s.domains.chat.is_empty()
            {
                "https://tt-chat3-wpa.api.zaloapp.com"
            } else {
                &s.domains.chat
            };
            let body = obj(&[
                ("toid", json!(to_id)),
                ("message", json!(text)),
                ("clientId", json!(now)),
                ("qmsgOwner", json!(qmsg_owner)),
                ("qmsgId", json!(qmsg_id_num)),
                ("qmsgCliId", json!(qmsg_cli_id)),
                ("qmsgType", json!(1)),
                ("qmsgTs", json!(qmsg_ts)),
                ("qmsg", json!(quoted.text)),
                ("imei", json!(s.imei)),
                ("ttl", json!(0)),
                ("zsource", json!(101)),
            ]);
            let params = s.encoded_params(&body)?;
            let url = format!("{host}/api/message/quote?{}", s.common_params());
            let resp = s.post_form(&url, format!("params={params}")).await?;
            let resp = s.maybe_decrypt(resp, s.session_key_bytes()).await;
            if let Some(code) = resp.get("error_code").and_then(Value::as_i64) {
                if code != 0 {
                    anyhow::bail!("quote error {code}");
                }
            }
            Ok(resp)
        }
    }
    .await;

    match quote_res {
        Ok(r) => Ok(r),
        Err(_) => {
            if is_group {
                send_group_message(s, to_id, text).await
            } else {
                send_message(s, to_id, text).await
            }
        }
    }
}

/// Forward a message or sticker to another conversation (`forwardMultiMessage` @ `compact-app-pc.js:14323900`).
pub async fn forward_message(
    s: &Session,
    target_id: &str,
    target_is_group: bool,
    msg: &ChatMessage,
) -> Result<Value> {
    if let Some(sid) = msg.sticker_id {
        if sid > 0 {
            let cid = msg.sticker_cate_id.unwrap_or(0);
            let stype = msg.sticker_type.unwrap_or(7);
            return send_sticker(s, target_id, sid, cid, stype, target_is_group).await;
        }
    }
    if target_is_group {
        send_group_message(s, target_id, &msg.text).await
    } else {
        send_message(s, target_id, &msg.text).await
    }
}

/// Convert Zalo reaction icon code to visual emoji.
pub fn icon_to_emoji(icon: &str) -> &'static str {
    match icon {
        "/-strong" => "👍",
        "/-heart" => "❤️",
        ":>" => "😆",
        ":o" => "😮",
        ":-((" | ":-(( " => "😢",
        ":-h" => "😠",
        _ => "❤️",
    }
}

/// Map a visual reaction emoji or Zalo reaction code to `(rIcon, rType)` (`compact-app-pc.js:10848900`).
pub fn reaction_code_and_type(emoji_or_code: &str) -> (&'static str, i32) {
    match emoji_or_code {
        "👍" | "/-strong" => ("/-strong", 3),
        "❤️" | "❤" | "/-heart" => ("/-heart", 5),
        "😆" | ":>" => (":>", 0),
        "😮" | ":o" => (":o", 32),
        "😢" | ":-(( " | ":-((" => (":-((", 2),
        "😠" | ":-h" => (":-h", 20),
        _ => ("/-heart", 5),
    }
}

/// Send a message reaction (`sendReactionMessage` in `compact-app-pc.b7bec208c4c8f73e09f8.js:14317097`, cmd `12410` / `12411`).
pub async fn send_reaction(
    s: &Session,
    to_id: &str,
    is_group: bool,
    msg: &ChatMessage,
    reaction: &str,
) -> Result<Value> {
    if s.login.is_none() {
        anyhow::bail!("chưa đăng nhập");
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let (r_icon, r_type) = reaction_code_and_type(reaction);
    let is_local_id =
        msg.id.is_empty() || msg.id.starts_with("out_") || msg.id.starts_with("local_");
    let g_msg_id_val = if is_local_id {
        json!("")
    } else {
        json!(msg.id)
    };
    let c_msg_id = if msg.ts > 0 {
        msg.ts as u64
    } else {
        msg.id.parse::<u64>().unwrap_or(now)
    };

    let inner_msg = json!({
        "rMsg": [{
            "gMsgID": g_msg_id_val,
            "cMsgID": c_msg_id,
            "msgType": if msg.sticker_id.is_some() { 5 } else { 1 }
        }],
        "rIcon": r_icon,
        "rType": r_type,
        "source": 6
    })
    .to_string();

    let react_list = json!([{
        "message": inner_msg,
        "clientId": now
    }]);

    let (url, body) = if is_group {
        let clean_grid = to_id
            .trim_start_matches("group_")
            .trim_start_matches('g')
            .trim_start_matches('G');
        (
            format!(
                "https://reaction.api.zaloapp.com/api/group/reaction?{}",
                s.common_params()
            ),
            obj(&[
                ("react_list", react_list),
                ("grid", json!(clean_grid)),
                ("imei", json!(s.imei)),
            ]),
        )
    } else {
        (
            format!(
                "https://reaction.api.zaloapp.com/api/message/reaction?{}",
                s.common_params()
            ),
            obj(&[("react_list", react_list), ("toid", json!(to_id))]),
        )
    };

    let params = s.encoded_params(&body)?;
    let resp = s.post_form(&url, format!("params={params}")).await?;
    Ok(s.maybe_decrypt(resp, s.session_key_bytes()).await)
}

/// Delete a message for me (`deleteOneOneMessageV2` / `deleteGroupMessageV2` @ `compact-app-pc.js:14373319`, cmd `11779` / `11834`).
pub async fn delete_message_for_me(
    s: &Session,
    to_id: &str,
    is_group: bool,
    msg: &ChatMessage,
) -> Result<Value> {
    if s.login.is_none() {
        anyhow::bail!("chưa đăng nhập");
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let my_uid = s
        .login
        .as_ref()
        .map(|l| l.uin.clone())
        .unwrap_or_else(|| "0".into());
    let owner_id = if msg.is_outgoing {
        my_uid.as_str()
    } else if !is_group {
        to_id
    } else {
        &msg.sender
    };
    let cli_id = if msg.ts > 0 {
        msg.ts.to_string()
    } else {
        now.to_string()
    };

    let msgs_arr = json!([{
        "cliMsgId": cli_id,
        "globalMsgId": msg.id,
        "ownerId": owner_id,
        "destId": to_id
    }]);

    if is_group {
        let clean_grid = to_id
            .trim_start_matches("group_")
            .trim_start_matches('g')
            .trim_start_matches('G');
        let body = obj(&[
            ("grid", json!(clean_grid)),
            ("cliMsgId", json!(now)),
            ("msgs", msgs_arr),
            ("onlyMe", json!(1)),
        ]);
        let params = s.encoded_params(&body)?;
        let url = format!(
            "{}/api/group/deletemsg?{}",
            s.domains.group,
            s.common_params()
        );
        let resp = s.post_form(&url, format!("params={params}")).await?;
        Ok(s.maybe_decrypt(resp, s.session_key_bytes()).await)
    } else {
        let host = if s.domains.chat.contains("-msg-chat.")
            || s.domains.chat.contains("tt-chat2-wpa")
            || s.domains.chat.is_empty()
        {
            "https://tt-chat3-wpa.api.zaloapp.com"
        } else {
            &s.domains.chat
        };
        let body = obj(&[
            ("toid", json!(to_id)),
            ("cliMsgId", json!(now)),
            ("msgs", msgs_arr),
            ("onlyMe", json!(1)),
            ("imei", json!(s.imei)),
        ]);
        let params = s.encoded_params(&body)?;
        let url = format!("{host}/api/message/delete?{}", s.common_params());
        let resp = s.post_form(&url, format!("params={params}")).await?;
        Ok(s.maybe_decrypt(resp, s.session_key_bytes()).await)
    }
}

/// Recall / undo an outgoing message for everyone (`undoMessage` @ `compact-app-pc.js:14376599`, cmd `11281` / `11192`).
pub async fn recall_message(
    s: &Session,
    to_id: &str,
    is_group: bool,
    msg: &ChatMessage,
) -> Result<Value> {
    if s.login.is_none() {
        anyhow::bail!("chưa đăng nhập");
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let cli_id = if msg.ts > 0 {
        msg.ts.to_string()
    } else {
        now.to_string()
    };

    if is_group {
        let clean_grid = to_id
            .trim_start_matches("group_")
            .trim_start_matches('g')
            .trim_start_matches('G');
        let body = obj(&[
            ("msgId", json!(msg.id)),
            ("grid", json!(clean_grid)),
            ("clientId", json!(now)),
            ("visibility", json!(0)),
            ("cliMsgIdUndo", json!(cli_id)),
            ("imei", json!(s.imei)),
        ]);
        let params = s.encoded_params(&body)?;
        let url = format!(
            "{}/api/group/undomsg?{}",
            s.domains.group,
            s.common_params()
        );
        let resp = s.post_form(&url, format!("params={params}")).await?;
        Ok(s.maybe_decrypt(resp, s.session_key_bytes()).await)
    } else {
        let host = if s.domains.chat.contains("-msg-chat.")
            || s.domains.chat.contains("tt-chat2-wpa")
            || s.domains.chat.is_empty()
        {
            "https://tt-chat3-wpa.api.zaloapp.com"
        } else {
            &s.domains.chat
        };
        let body = obj(&[
            ("msgId", json!(msg.id)),
            ("toid", json!(to_id)),
            ("clientId", json!(now)),
            ("cliMsgIdUndo", json!(cli_id)),
        ]);
        let params = s.encoded_params(&body)?;
        let url = format!("{host}/api/message/undo?{}", s.common_params());
        let resp = s.post_form(&url, format!("params={params}")).await?;
        Ok(s.maybe_decrypt(resp, s.session_key_bytes()).await)
    }
}

/// `GET /api/preloadconvers/get-last-msgs` (cmd 10053).
///
/// [O] request: GET `{convers_api}/api/preloadconvers/get-last-msgs?{common}&params={encodeAES(json)}`
/// where payload is `{ threadIdLocalMsgId: map_object, imei }`.
pub async fn get_last_msgs(s: &Session, thread_ids: &[String]) -> Result<Value> {
    if s.login.is_none() {
        anyhow::bail!("chưa đăng nhập");
    }
    let mut map = serde_json::Map::new();
    // [O] `getLastMessagesForPreview(e)` in `compact-app-pc.js:14385933` passes `{}` on preload
    // to fetch all recent conversation previews without overflowing URL length.
    if thread_ids.len() <= 25 {
        for id in thread_ids {
            let clean = id.trim_start_matches("group_").trim_start_matches('g');
            let key = if id.starts_with("group_") || id.starts_with('g') {
                format!("{clean}_1")
            } else {
                format!("{clean}_0")
            };
            map.insert(key, json!("1"));
        }
    }
    let map_str = serde_json::to_string(&map).unwrap_or_else(|_| "{}".into());
    let body = obj(&[
        ("threadIdLocalMsgId", json!(map_str)),
        ("imei", json!(s.imei)),
    ]);
    let params = match s.encoded_params(&body) {
        Ok(p) => format!("params={p}"),
        Err(e) => {
            crate::logger::log_error("chat::get_last_msgs::encoded_params", &e.to_string());
            eprintln!("! get_last_msgs: no session key for encoded_params ({e}), falling back to plain_params");
            Session::plain_params(&body)
        }
    };

    let url = format!(
        "{}/api/preloadconvers/get-last-msgs?{}&{params}",
        s.domains.convers_api,
        s.common_params()
    );
    let resp = s.get(&url).await?;
    let resp = s.maybe_decrypt(resp, s.session_key_bytes()).await;
    checked_sync_response(resp)
}

/// Metadata requests deliberately bypass request/payload dumps, including developer mode.
async fn get_private_metadata(s: &Session, url: String) -> Result<Value> {
    anyhow::ensure!(
        s.login.is_some(),
        "Metadata requires an authenticated session"
    );
    let parsed = reqwest::Url::parse(&url)?;
    anyhow::ensure!(
        parsed.host_str().is_some_and(|h| h == "zalo.me"
            || h.ends_with(".zalo.me")
            || h == "zaloapp.com"
            || h.ends_with(".zaloapp.com")),
        "Unexpected metadata host"
    );
    let mut request = s
        .http
        .get(parsed)
        .timeout(std::time::Duration::from_secs(15));
    if let Some(token) = s.zpw_sek.as_ref() {
        request = request
            .header("Cookie", format!("zpw_sek={token}; _zlang=vi"))
            .header("zpw_sek", token)
            .header("Origin", "https://chat.zalo.me")
            .header("Referer", "https://chat.zalo.me/");
    }
    let response = request
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("Metadata transport failed"))?;
    anyhow::ensure!(
        response.status().is_success(),
        "Metadata HTTP request failed"
    );
    let mut value: Value = response
        .json()
        .await
        .map_err(|_| anyhow::anyhow!("Invalid metadata response"))?;
    if let Some(encoded) = value.get("data").and_then(Value::as_str) {
        if !encoded.trim_start().starts_with('{') && !encoded.trim_start().starts_with('[') {
            let plaintext = crate::crypto::decode_aes(s.session_key_bytes(), encoded)
                .map_err(|_| anyhow::anyhow!("Metadata decryption failed"))?;
            value = serde_json::from_str(&plaintext)
                .map_err(|_| anyhow::anyhow!("Invalid decrypted metadata"))?;
        }
    }
    checked_sync_response(value)
}

/// Read-only desktop command 12009. It fetches account read cursors, not receipts.
pub async fn get_last_seen(s: &Session, threads: &[Thread]) -> Result<Value> {
    let groups: Vec<_> = threads
        .iter()
        .filter(|t| t.is_group)
        .map(|t| t.id.trim_start_matches('g').to_string())
        .collect();
    let peers: Vec<_> = threads
        .iter()
        .filter(|t| !t.is_group)
        .map(|t| t.id.clone())
        .collect();
    let body = json!({"param": serde_json::to_string(&json!({"group":groups,"oneone":peers}))?});
    let params = s.encoded_params(&body)?;
    get_private_metadata(
        s,
        format!(
            "{}/api/preloadconvers/get-last-seen?{}&params={params}",
            s.domains.chat,
            s.common_params()
        ),
    )
    .await
}

/// Read-only desktop command 11280, used to render muted unread dots.
pub async fn get_mute_metadata(s: &Session) -> Result<Value> {
    let params = s.encoded_params(&json!({"imei":s.imei}))?;
    get_private_metadata(
        s,
        format!(
            "{}/api/social/profile/getmute?{}&params={params}",
            s.domains.profile,
            s.common_params()
        ),
    )
    .await
}

/// Fetch read cursors without opening another socket or sending any seen mutation.
fn unread_metadata_targets(threads: &[Thread]) -> Vec<Thread> {
    threads
        .iter()
        .filter(|thread| !thread.id.trim().is_empty() && thread.id != "0")
        .map(|thread| Thread {
            presentation: Default::default(),
            id: thread.id.clone(),
            is_group: thread.is_group,
            name: String::new(),
            preview: String::new(),
            time: String::new(),
            avatar: None,
            messages: Vec::new(),
            unread: Default::default(),
        })
        .collect()
}

pub async fn sync_unread_metadata(s: &Session, threads: &mut [Thread]) -> Result<()> {
    // Match the original batching contract and avoid URL overflow on large contact lists.
    let known = unread_metadata_targets(threads);
    for batch in known.chunks(100) {
        let response = get_last_seen(s, batch).await?;
        let mut data = response;
        for _ in 0..4 {
            match data.get("data") {
                Some(Value::String(text)) => {
                    data = serde_json::from_str(text)?;
                }
                Some(value) => {
                    data = value.clone();
                }
                None => break,
            }
        }
        crate::unread::merge_read_points(threads, &data);
    }
    for thread in threads.iter_mut().filter(|t| t.unread.fresh) {
        thread.unread.observe(&thread.messages);
    }
    if let Ok(response) = get_mute_metadata(s).await {
        let mut data = response;
        for _ in 0..4 {
            match data.get("data") {
                Some(Value::String(text)) => {
                    if let Ok(value) = serde_json::from_str(text) {
                        data = value;
                    } else {
                        break;
                    }
                }
                Some(value) => {
                    data = value.clone();
                }
                None => break,
            }
        }
        crate::unread::merge_mute_metadata(threads, &data);
    }
    Ok(())
}

/// `GET /api/pinconvers/list` (cmd 12120) — pinned threads.
pub async fn get_pinned(s: &Session) -> Result<Value> {
    if s.login.is_none() {
        anyhow::bail!("chưa đăng nhập");
    }
    let url = format!(
        "{}/api/pinconvers/list?{}",
        s.domains.conversation,
        s.common_params()
    );
    s.get(&url).await
}

/// `GET /api/group/history` (cmd 11240) — group message history.
pub async fn get_group_history(s: &Session, group_id: &str, count: u32) -> Result<Value> {
    get_group_history_raw(s, group_id, count).await
}

/// Byte-exact replica of the shipped bundle's `getHistoryMessage(e, t)`.
///
/// [O] `compact-app-pc:14364564`:
/// `n = {grid: e, count: t}`
/// `getGroupDomain() + "/api/group/history?" + _getCommonParams() + "&params=" + encodeURIComponent(encodeAES(JSON.stringify(n)))`
/// Signing tag: **11240**.
///
/// Note: the bundle sends **only** `grid` and `count` — no `imei`, no `signkey` on this
/// path. Kept as a separate raw probe so the guessed `get_group_history` above can be
/// compared against it without breaking existing callers.
pub async fn get_group_history_raw(s: &Session, grid: &str, count: u32) -> Result<Value> {
    if s.login.is_none() {
        anyhow::bail!("chưa đăng nhập");
    }
    // [O] getGroupIdFromConversationId: strip the group-id prefix if present.
    let grid = grid
        .trim_start_matches("group_")
        .trim_start_matches('g')
        .trim_start_matches('G');
    let body = obj(&[("grid", json!(grid)), ("count", json!(count))]);
    let params = match s.encoded_params(&body) {
        Ok(p) => format!("params={p}"),
        Err(e) => {
            crate::logger::log_error(
                "chat::get_group_history_raw::encoded_params",
                &e.to_string(),
            );
            Session::plain_params(&body)
        }
    };
    let url = format!(
        "{}/api/group/history?{}&{params}",
        s.domains.group,
        s.common_params()
    );
    let resp = s.get(&url).await?;
    let resp = s.maybe_decrypt(resp, s.session_key_bytes()).await;
    Ok(resp)
}

/// Fetch group metadata (`name`, `avt`/`fullAvt`, `totalMember`, `removedsGroup`) from
/// `POST /api/group/getmg-v2` by sending `gridVerMap: {"<grid>": 0}` so the server always
/// returns `gridInfoMap` instead of putting groups into `unchangedsGroup`.
pub async fn fetch_group_profiles(
    s: &Session,
    group_ids: &[String],
) -> Result<(
    serde_json::Map<String, Value>,
    std::collections::HashSet<String>,
)> {
    if s.login.is_none() || group_ids.is_empty() {
        return Ok((serde_json::Map::new(), std::collections::HashSet::new()));
    }
    let mut gv_zero = serde_json::Map::new();
    for gid in group_ids {
        let clean = gid
            .trim_start_matches("group_")
            .trim_start_matches('g')
            .trim_start_matches('G')
            .trim();
        if !clean.is_empty() {
            gv_zero.insert(clean.to_string(), json!(0));
        }
    }
    let gv_str = serde_json::to_string(&gv_zero)?;
    let body = obj(&[("gridVerMap", json!(gv_str)), ("imei", json!(s.imei))]);
    let params = s.encoded_params(&body)?;
    let mg_url = format!(
        "{}/api/group/getmg-v2?{}",
        s.domains.group,
        s.common_params()
    );
    let mg_resp = s.post_form(&mg_url, format!("params={params}")).await?;
    let mut mg_dec = s.maybe_decrypt(mg_resp, s.session_key_bytes()).await;
    if let Some(d_str) = mg_dec.get("data").and_then(Value::as_str) {
        if let Ok(parsed) = serde_json::from_str::<Value>(d_str.trim()) {
            mg_dec["data"] = parsed;
        }
    }
    let data_node = mg_dec
        .pointer("/data/data")
        .or_else(|| mg_dec.get("data"))
        .unwrap_or(&mg_dec);
    let info_map = data_node
        .get("gridInfoMap")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let removed: std::collections::HashSet<String> = data_node
        .get("removedsGroup")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    Ok((info_map, removed))
}

/// Extract up to 4 sorted member UIDs (`creatorId` first, then `adminIds`, then `memVerList`)
/// and total member count from a `getmg-v2` group info object.
fn extract_group_member_ids(info: &Value) -> (Vec<String>, usize) {
    let mut raw_ids: Vec<String> = Vec::new();
    if let Some(arr) = info.get("memVerList").and_then(Value::as_array) {
        for item in arr {
            if let Some(s) = item.as_str() {
                let uid = s.split('_').next().unwrap_or("").trim();
                if !uid.is_empty() && !raw_ids.iter().any(|x| x == uid) {
                    raw_ids.push(uid.to_string());
                }
            }
        }
    }
    let mut sorted: Vec<String> = Vec::new();
    if let Some(creator) = pick_str(info, &["creatorId"]) {
        if raw_ids.iter().any(|x| x == &creator) {
            sorted.push(creator);
        }
    }
    if let Some(admins) = info.get("adminIds").and_then(Value::as_array) {
        for a in admins {
            let aid = a
                .as_str()
                .map(str::to_string)
                .or_else(|| a.as_u64().map(|n| n.to_string()))
                .unwrap_or_default();
            if !aid.is_empty()
                && raw_ids.iter().any(|x| x == &aid)
                && !sorted.iter().any(|x| x == &aid)
            {
                sorted.push(aid);
            }
        }
    }
    for uid in &raw_ids {
        if !sorted.iter().any(|x| x == uid) {
            sorted.push(uid.clone());
        }
    }
    let total = info
        .get("totalMember")
        .and_then(Value::as_u64)
        .map(|n| n as usize)
        .unwrap_or_else(|| sorted.len())
        .max(sorted.len());
    sorted.truncate(4);
    (sorted, total)
}

/// Format a `multi:<total>;<name1>|<url1>;...` composite group avatar string.
fn format_multi_group_avatar(
    group_name: &str,
    mem_ids: &[String],
    total_members: usize,
    profiles: &serde_json::Map<String, Value>,
) -> Option<String> {
    let fallback_names: Vec<&str> = group_name
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    let mut slots: Vec<String> = Vec::new();
    for (idx, uid) in mem_ids.iter().take(4).enumerate() {
        let mut m_name = fallback_names
            .get(idx)
            .copied()
            .unwrap_or("Thành viên")
            .to_string();
        let mut m_ava = String::new();
        if let Some(p) = profiles.get(uid) {
            if let Some(n) = pick_str(p, &["zaloName", "displayName", "name", "display_name"]) {
                if !n.is_empty() {
                    m_name = n;
                }
            }
            if let Some(a) = pick_str(p, &["avatar", "avt", "profile_avatar"]) {
                if !a.is_empty() && !a.ends_with("/default") {
                    m_ava = a;
                }
            }
        }
        let clean_name = m_name.replace([';', '|'], " ");
        let clean_ava = m_ava.replace([';', '|'], "");
        slots.push(format!("{clean_name}|{clean_ava}"));
    }
    if slots.is_empty() {
        return None;
    }
    // If the group name is a comma-separated list of member names (e.g. "Đỗ Thành, Đtc, Hoang"),
    // order the slots to match `fallback_names` so API-resolved and name-derived slots never swap positions.
    if fallback_names.len() >= 2 {
        slots.sort_by_key(|slot| {
            let s_name = slot.split('|').next().unwrap_or("").trim();
            let s_fold = strip_vn_for_sticker_kw(s_name);
            fallback_names
                .iter()
                .position(|cand| {
                    let c_fold = strip_vn_for_sticker_kw(cand);
                    !c_fold.is_empty()
                        && (c_fold == s_fold
                            || s_fold.contains(&c_fold)
                            || c_fold.contains(&s_fold))
                })
                .unwrap_or(99)
        });
    }
    let tot = total_members.max(slots.len());
    Some(format!("multi:{tot};{}", slots.join(";")))
}

/// `GET /api/group/getlg/v4` (cmd 12027) — list of all groups user belongs to.
pub async fn get_groups(s: &Session) -> Result<Vec<Thread>> {
    if s.login.is_none() {
        return Ok(Vec::new());
    }
    let url = format!(
        "{}/api/group/getlg/v4?{}",
        s.domains.group,
        s.common_params()
    );
    let resp = s.get(&url).await?;
    let mut resp = s.maybe_decrypt(resp, s.session_key_bytes()).await;
    if let Some(d_str) = resp.get("data").and_then(Value::as_str) {
        if let Ok(parsed) = serde_json::from_str::<Value>(d_str.trim()) {
            resp["data"] = parsed;
        }
    }
    let data_node = resp
        .pointer("/data/data")
        .or_else(|| resp.get("data"))
        .unwrap_or(&resp);

    let mut groups = Vec::new();
    if let Some(map) = data_node.get("gridVerMap").and_then(Value::as_object) {
        for (grid, _ver) in map {
            groups.push(Thread {
                presentation: Default::default(),
                unread: Default::default(),
                id: grid.clone(),
                name: format!("Nhóm {}", grid),
                preview: String::new(),
                time: crate::ui::clock(),
                avatar: None,
                is_group: true,
                messages: Vec::new(),
            });
        }
        if !groups.is_empty() {
            let gids: Vec<String> = groups.iter().map(|g| g.id.clone()).collect();
            if let Ok((info_map, rem_ids)) = fetch_group_profiles(s, &gids).await {
                let mut need_mem_uids: Vec<String> = Vec::new();
                for g in &mut groups {
                    if let Some(info) = info_map.get(&g.id) {
                        g.apply_group_sidebar_metadata(info);
                        if let Some(name) = pick_str(info, &["name", "groupName", "displayName"]) {
                            g.name = name;
                        }
                        if let Some(avt) = pick_str(info, &["fullAvt", "avt", "avatar"])
                            .filter(|u| !u.is_empty() && !u.ends_with("/default"))
                        {
                            g.avatar = Some(avt);
                        } else {
                            let (mids, _) = extract_group_member_ids(info);
                            for uid in mids {
                                if !need_mem_uids.iter().any(|x| x == &uid) {
                                    need_mem_uids.push(uid);
                                }
                            }
                        }
                        if g.preview.is_empty() {
                            if let Some(n_mem) = info.get("totalMember").and_then(Value::as_u64) {
                                if n_mem > 0 {
                                    g.preview = format!("Nhóm · {n_mem} thành viên");
                                }
                            }
                        }
                    }
                }
                if !need_mem_uids.is_empty() {
                    let mut all_profiles = serde_json::Map::new();
                    for chunk in need_mem_uids.chunks(50) {
                        if let Ok(mp) = get_mini_profiles(s, chunk).await {
                            for (k, v) in mp {
                                all_profiles.insert(k, v);
                            }
                        }
                    }
                    for g in &mut groups {
                        if g.avatar.is_none() {
                            if let Some(info) = info_map.get(&g.id) {
                                let (mids, tot) = extract_group_member_ids(info);
                                g.avatar =
                                    format_multi_group_avatar(&g.name, &mids, tot, &all_profiles);
                            }
                        }
                    }
                }
                if !rem_ids.is_empty() {
                    groups.retain(|g| !rem_ids.contains(&g.id));
                }
            }
        }
    }
    resolve_thread_names(s, &mut groups).await;
    Ok(groups)
}

/// `POST /api/social/group/members` (cmd 12030) — fetch member display names and avatars for user IDs.
///
/// Zalo's `/api/social/friend/getminiprofiles` returns `error_code = 114` for raw UID lists,
/// whereas `/api/social/group/members` with `friend_pversion_map: ["<uid>_0", ...]` returns
/// `data.profiles.<uid>.{displayName, zaloName, avatar}` with `error_code = 0`.
pub async fn get_mini_profiles(
    s: &Session,
    user_ids: &[String],
) -> Result<serde_json::Map<String, Value>> {
    if s.login.is_none() || user_ids.is_empty() {
        return Ok(serde_json::Map::new());
    }
    let versioned_ids: Vec<String> = user_ids
        .iter()
        .map(|uid| {
            let clean = uid.trim();
            if clean.contains('_') {
                clean.to_string()
            } else {
                format!("{clean}_0")
            }
        })
        .collect();
    let body = obj(&[("friend_pversion_map", json!(versioned_ids))]);
    let params = s.encoded_params(&body)?;
    let url = format!(
        "{}/api/social/group/members?{}",
        s.domains.profile,
        s.common_params()
    );
    let form = format!("params={params}");
    let resp = s.post_form(&url, form).await?;
    let mut resp = s.maybe_decrypt(resp, s.session_key_bytes()).await;
    if let Some(d_str) = resp.get("data").and_then(Value::as_str) {
        if let Ok(parsed) = serde_json::from_str::<Value>(d_str.trim()) {
            resp["data"] = parsed;
        }
    }
    if let Some(profiles) = resp
        .pointer("/data/profiles")
        .or_else(|| resp.pointer("/data/data/profiles"))
        .or_else(|| resp.get("data"))
        .and_then(Value::as_object)
    {
        return Ok(profiles.clone());
    }
    Ok(serde_json::Map::new())
}

/// Automatically resolve real display names and avatars for 1-1 and group threads that only have placeholder names.
pub async fn resolve_thread_names(s: &Session, threads: &mut [Thread]) {
    let unknown_ids: Vec<String> = threads
        .iter()
        .filter(|t| {
            !t.is_group
                && !t.name.contains("Cloud của tôi")
                && (t.name.starts_with("Người dùng ")
                    || t.name.is_empty()
                    || t.avatar.as_deref().unwrap_or("").is_empty())
        })
        .map(|t| t.id.clone())
        .collect();

    let member_ids: std::collections::BTreeSet<String> = threads
        .iter()
        .filter(|t| t.is_group)
        .flat_map(|t| t.presentation.members.iter().take(4))
        .filter(|member| member.avatar.is_none() || member.name.is_empty())
        .map(|member| member.id.clone())
        .collect();
    let mut member_ids = member_ids;
    for thread in threads.iter_mut().filter(|t| t.is_group) {
        thread.refresh_sidebar_presentation();
        if !thread.presentation.last_outgoing
            && !thread.presentation.last_sender_id.is_empty()
            && (thread.presentation.last_sender.is_empty()
                || thread
                    .presentation
                    .last_sender
                    .bytes()
                    .all(|b| b.is_ascii_digit()))
        {
            member_ids.insert(thread.presentation.last_sender_id.clone());
        }
    }
    let member_ids: Vec<_> = member_ids.into_iter().collect();
    for chunk in member_ids.chunks(50) {
        if let Ok(profiles) = get_mini_profiles(s, chunk).await {
            for thread in threads.iter_mut().filter(|t| t.is_group) {
                if let Some(profile) = profiles.get(&thread.presentation.last_sender_id) {
                    if let Some(name) = pick_str(profile, &["zaloName", "displayName", "name"]) {
                        thread.presentation.last_sender = name.clone();
                        for message in &mut thread.messages {
                            if sidebar_sender_id(message).as_deref()
                                == Some(&thread.presentation.last_sender_id)
                            {
                                message.sender = name.clone();
                            }
                        }
                    }
                }

                for member in &mut thread.presentation.members {
                    if let Some(profile) = profiles.get(&member.id) {
                        if let Some(name) = pick_str(profile, &["zaloName", "displayName", "name"])
                        {
                            member.name = name;
                        }
                        if let Some(avatar) = pick_str(profile, &["avatar", "avt"]) {
                            member.avatar = Some(avatar);
                        }
                    }
                }
            }
        }
    }

    if !unknown_ids.is_empty() {
        for chunk in unknown_ids.chunks(50) {
            if let Ok(profiles) = get_mini_profiles(s, chunk).await {
                for t in threads.iter_mut() {
                    if let Some(p) = profiles.get(&t.id) {
                        if t.name.starts_with("Người dùng ") || t.name.is_empty() {
                            if let Some(name) =
                                pick_str(p, &["zaloName", "displayName", "name", "display_name"])
                            {
                                if !name.is_empty() {
                                    t.name = name;
                                }
                            }
                        }
                        if let Some(ava) = pick_str(p, &["avatar", "avt", "profile_avatar"]) {
                            if !ava.is_empty() {
                                t.avatar = Some(ava);
                            }
                        }
                    }
                }
            }
        }
    }

    let unknown_group_ids: Vec<String> = threads
        .iter()
        .filter(|t| {
            t.is_group
                && (is_generic_thread_name(&t.name)
                    || t.preview.is_empty()
                    || t.avatar.as_deref().unwrap_or("").is_empty())
        })
        .map(|t| t.id.clone())
        .collect();

    if !unknown_group_ids.is_empty() {
        for chunk in unknown_group_ids.chunks(50) {
            if let Ok((info_map, _)) = fetch_group_profiles(s, chunk).await {
                let mut need_mem_uids: Vec<String> = Vec::new();
                for t in threads.iter_mut() {
                    let clean_id =
                        t.id.trim_start_matches("group_")
                            .trim_start_matches('g')
                            .trim_start_matches('G');
                    if let Some(info) = info_map.get(&t.id).or_else(|| info_map.get(clean_id)) {
                        t.apply_group_sidebar_metadata(info);
                        if let Some(name) = pick_str(info, &["name", "groupName", "displayName"]) {
                            if !name.is_empty() {
                                t.name = name;
                            }
                        }
                        if let Some(avt) = pick_str(info, &["fullAvt", "avt", "avatar"])
                            .filter(|u| !u.is_empty() && !u.ends_with("/default"))
                        {
                            t.avatar = Some(avt);
                        } else {
                            let (mids, _) = extract_group_member_ids(info);
                            for uid in mids {
                                if !need_mem_uids.iter().any(|x| x == &uid) {
                                    need_mem_uids.push(uid);
                                }
                            }
                        }
                        if t.preview.is_empty() {
                            if let Some(n_mem) = info.get("totalMember").and_then(Value::as_u64) {
                                if n_mem > 0 {
                                    t.preview = format!("Nhóm · {n_mem} thành viên");
                                }
                            }
                        }
                    }
                }
                if !need_mem_uids.is_empty() {
                    let mut all_profiles = serde_json::Map::new();
                    for mchunk in need_mem_uids.chunks(50) {
                        if let Ok(mp) = get_mini_profiles(s, mchunk).await {
                            for (k, v) in mp {
                                all_profiles.insert(k, v);
                            }
                        }
                    }
                    for t in threads.iter_mut() {
                        if !t.is_group {
                            continue;
                        }
                        let clean_id =
                            t.id.trim_start_matches("group_")
                                .trim_start_matches('g')
                                .trim_start_matches('G');
                        if let Some(info) = info_map.get(&t.id).or_else(|| info_map.get(clean_id)) {
                            let cur = t.avatar.as_deref().unwrap_or("");
                            if cur.is_empty() || cur.starts_with("multi:") {
                                let (mids, tot) = extract_group_member_ids(info);
                                if let Some(multi_s) =
                                    format_multi_group_avatar(&t.name, &mids, tot, &all_profiles)
                                {
                                    if should_replace_avatar(t.avatar.as_deref(), Some(&multi_s)) {
                                        t.avatar = Some(multi_s);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Offline / local contact fallback for composite group avatars: match comma-separated member names
    // against known 1-1 contact threads so group avatars display real member photos even without network.
    let known_contacts: Vec<(String, String)> = threads
        .iter()
        .filter(|t| {
            !t.is_group
                && !t.name.contains("Cloud của tôi")
                && !t.avatar.as_deref().unwrap_or("").is_empty()
                && !t.avatar.as_deref().unwrap_or("").ends_with("/default")
        })
        .map(|t| (t.name.clone(), t.avatar.clone().unwrap_or_default()))
        .collect();

    for t in threads.iter_mut() {
        if !t.is_group {
            continue;
        }
        let cur_ava = t.avatar.as_deref().unwrap_or("");
        if avatar_http_url_count(Some(cur_ava)) > 0 {
            continue;
        }
        let parts: Vec<&str> = t
            .name
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        if parts.len() >= 2 {
            let mut slots: Vec<String> = Vec::new();
            for p in parts.iter().take(4) {
                let p_fold = strip_vn_for_sticker_kw(p);
                let matched_url = known_contacts
                    .iter()
                    .find(|(cname, _)| {
                        let c_fold = strip_vn_for_sticker_kw(cname);
                        !p_fold.is_empty() && c_fold == p_fold
                    })
                    .map(|(_, url)| url.as_str())
                    .unwrap_or("");
                slots.push(format!("{}|{}", p.replace([';', '|'], " "), matched_url));
            }
            let new_multi = format!("multi:{};{}", parts.len(), slots.join(";"));
            if should_replace_avatar(t.avatar.as_deref(), Some(&new_multi)) {
                t.avatar = Some(new_multi);
            }
        }
    }
}

/// Per-queue cursor for `getChatFromPolling` (`MultiQueueId` in module `1iR4`, `sync-v2-sub-worker.js:1620000`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct QueueCursor {
    #[serde(rename = "lastId")]
    pub last_id: String,
    pub ids: Vec<String>,
}

/// State machine for Zalo's native HTTP long-polling message receiver (`getChatFromPolling`, cmd `99100`).
///
/// [O] Observed in `sync-v2-sub-worker.b7bec208c4c8f73e09f8.js` (offsets `1620000`, `3498103`, `14557981`):
/// - `R = new y({ type: "MSG", qosId: 91021, queueIds: [1, 2, 3], ... })`
/// - `getLastActionIds()` builds:
///   `"{lastActionId},{lastActionIdOther}," + encodeURIComponent(btoa(JSON.stringify(queues)) + ",1")`
/// - `getChatFromPolling(params)`:
///   `GET {zpw_chts_new[0]}/?zpw_ver=694&zpw_type=30&params={params}&ts={unix_sec}`
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PollState {
    pub last_action_id: String,
    pub last_action_id_other: String,
    pub queues: std::collections::BTreeMap<String, QueueCursor>,
}

impl Default for PollState {
    fn default() -> Self {
        let mut queues = std::collections::BTreeMap::new();
        for qid in ["1", "2", "3"] {
            queues.insert(
                qid.to_string(),
                QueueCursor {
                    last_id: "1".into(),
                    ids: Vec::new(),
                },
            );
        }
        Self {
            last_action_id: "1".into(),
            last_action_id_other: "1".into(),
            queues,
        }
    }
}

fn max_action_id(a: &str, b: &str) -> String {
    let na = a.parse::<u128>().unwrap_or(0);
    let nb = b.parse::<u128>().unwrap_or(0);
    if nb >= na && nb > 0 {
        b.to_string()
    } else if na > 0 {
        a.to_string()
    } else {
        "1".into()
    }
}

impl PollState {
    pub fn encode_params(&self) -> String {
        use base64::Engine;
        let q_json = serde_json::to_string(&self.queues).unwrap_or_else(|_| "{}".into());
        let b64 = base64::engine::general_purpose::STANDARD.encode(q_json.as_bytes());
        let tail = crate::session::urlencode(&format!("{b64},1"));
        format!(
            "{},{},{}",
            self.last_action_id, self.last_action_id_other, tail
        )
    }

    pub fn update_from_response(&mut self, resp: &Value) {
        let mut data_node = resp.get("data").unwrap_or(resp);
        if let Some(inner) = data_node.get("data").filter(|v| v.is_object()) {
            data_node = inner;
        }
        if let Some(reset_id) = pick_str(data_node, &["resetLastActionId"]) {
            if reset_id != "0" && reset_id.parse::<u128>().unwrap_or(0) > 0 {
                self.last_action_id = reset_id.clone();
                self.last_action_id_other = "1".into();
                for q in self.queues.values_mut() {
                    q.last_id = reset_id.clone();
                    q.ids.clear();
                }
                return;
            }
        }
        if let Some(lai) = pick_str(data_node, &["lastActionId"]) {
            if lai != "0" {
                self.last_action_id = max_action_id(&self.last_action_id, &lai);
            }
        }
        if let Some(laio) = pick_str(data_node, &["lastActionIdOther"]) {
            if laio != "0" {
                self.last_action_id_other = max_action_id(&self.last_action_id_other, &laio);
            }
        }
        if let Some(qs) = data_node.get("queueStatus").and_then(Value::as_object) {
            for (qid, qval) in qs {
                // Only update configured HTTP long-polling MSG queues ("1", "2", "3").
                // Adding socket-only queues ("15", "16", "7", "8") causes lp_chat to reject subsequent requests.
                let Some(entry) = self.queues.get_mut(qid) else {
                    continue;
                };
                if let Some(lid) = pick_str(qval, &["lastId"]) {
                    if lid != "0" {
                        entry.last_id = max_action_id(&entry.last_id, &lid);
                    }
                }
                if let Some(ids_arr) = qval.get("ids").and_then(Value::as_array) {
                    let max_lid = entry.last_id.parse::<u128>().unwrap_or(u128::MAX);
                    let mut new_ids: Vec<String> = ids_arr
                        .iter()
                        .filter_map(|v| match v {
                            Value::String(s) if !s.is_empty() => Some(s.clone()),
                            Value::Number(n) => Some(n.to_string()),
                            _ => None,
                        })
                        .filter(|s| s.parse::<u128>().map(|n| n <= max_lid).unwrap_or(true))
                        .collect();
                    if !new_ids.is_empty() {
                        // [O] `QueueControl.getLastActionIdsArray` (`sync-v2-sub-worker.js:1606950`):
                        // sort descending, take at most 10 <= lastId, then reverse so `ids` is ascending.
                        new_ids.sort_by_key(|s| std::cmp::Reverse(s.parse::<u128>().unwrap_or(0)));
                        new_ids.dedup();
                        new_ids.truncate(10);
                        new_ids.reverse();
                        entry.ids = new_ids;
                    }
                }
            }
        }
    }
}

/// Fetch cloud message history (`getCM` in `sync-v2-sub-worker.b7bec208c4c8f73e09f8.js:14595433`, cmd `12506` / `12507`).
///
/// [O] Observed in module `1EWQ` (`sync-v2-sub-worker.js:1545933`):
/// Cloud message history (`/api/cm/getrecentv2` and `/api/cm/getoldv2` on `group_cloud_message`)
/// is enabled for Group conversations (`g<groupId>`) and My Cloud (`sendToMeId`).
pub async fn get_cloud_messages(
    s: &Session,
    conv_id: &str,
    global_msg_id: &str,
    is_old: bool,
    count: u32,
) -> Result<Value> {
    if s.login.is_none() {
        anyhow::bail!("chưa đăng nhập");
    }
    let clean_id = conv_id
        .trim_start_matches("group_")
        .trim_start_matches('g')
        .trim_start_matches('G');
    let is_send2me = s
        .login
        .as_ref()
        .map(|l| !l.send2me_id.is_empty() && l.send2me_id == clean_id)
        .unwrap_or(false);
    let mut map = serde_json::Map::new();
    map.insert("groupId".into(), json!(clean_id));
    map.insert(
        "globalMsgId".into(),
        json!(if global_msg_id.is_empty() || global_msg_id == "0" {
            "9223372036854775808"
        } else {
            global_msg_id
        }),
    );
    map.insert("count".into(), json!(count));
    map.insert("msgIds".into(), json!([]));
    map.insert("imei".into(), json!(s.imei));
    if is_send2me {
        map.insert("isOA".into(), json!(1));
    }
    map.insert("src".into(), json!(if is_old { 3 } else { 1 }));

    let body = Value::Object(map);
    let params = s.encoded_params(&body)?;
    let ep = if is_old {
        "/api/cm/getoldv2"
    } else {
        "/api/cm/getrecentv2"
    };
    let host = if s.domains.cloud_msg.is_empty() {
        "https://tt-group-cm.api.zaloapp.com"
    } else {
        &s.domains.cloud_msg
    };
    let url = format!(
        "{}{ep}?{}&params={params}&nretry=0",
        host.trim_end_matches('/'),
        s.common_params()
    );
    let resp = s.get(&url).await?;
    let mut dec = s.maybe_decrypt(resp, s.session_key_bytes()).await;
    if let Some(d_str) = dec.get("data").and_then(Value::as_str) {
        let trimmed = d_str.trim();
        if (trimmed.starts_with('{') && trimmed.ends_with('}'))
            || (trimmed.starts_with('[') && trimmed.ends_with(']'))
        {
            if let Ok(parsed_data) = serde_json::from_str::<Value>(trimmed) {
                dec["data"] = parsed_data;
            }
        }
    }
    Ok(dec)
}

/// Call Zalo's native HTTP long-polling message endpoint (`getChatFromPolling`, cmd `99100`).
///
/// Uses a 12-second timeout matching `lp_chat` long-polling windows (`sync-v2-sub-worker.js:3498030`).
pub async fn get_chat_from_polling(s: &Session, state: &mut PollState) -> Result<Value> {
    if s.login.is_none() {
        anyhow::bail!("chưa đăng nhập");
    }
    let host = if s.domains.lp_chat.is_empty() {
        "https://p4-msg-chat.zaloapp.com"
    } else {
        &s.domains.lp_chat
    };
    let ts = crate::login::unix_now();
    let url = format!(
        "{}/?{}&params={}&ts={ts}",
        host.trim_end_matches('/'),
        s.common_params(),
        state.encode_params()
    );
    let fut = s.get(&url);
    match tokio::time::timeout(std::time::Duration::from_millis(12000), fut).await {
        Ok(Ok(resp)) => {
            let dec = s.maybe_decrypt(resp, s.session_key_bytes()).await;
            if let Some(code) = dec.get("error_code").and_then(Value::as_i64) {
                if code != 0 {
                    let msg = dec
                        .get("error_message")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown");
                    return Err(crate::session::SessionError::new(code, msg).into());
                }
            }
            state.update_from_response(&dec);
            Ok(dec)
        }
        Ok(Err(e)) => Err(e),
        Err(_) => Ok(json!({"error_code": 0, "data": {}})),
    }
}

/// Poll both `getChatFromPolling` (`lp_chat`) and `get-last-msgs` (`convers_api`),
/// merge all incoming/outgoing/group messages into `threads`, resolve unknown names,
/// and persist to `zalo_history.db` and `threads_cache.json`.
pub async fn poll_new_messages(
    s: &Session,
    state: &mut PollState,
    threads: &mut Vec<Thread>,
) -> Result<()> {
    poll_new_messages_impl(s, state, threads, true).await
}

/// Merge remote updates without reading or writing global legacy history/cache.
pub async fn poll_new_messages_remote(
    s: &Session,
    state: &mut PollState,
    threads: &mut Vec<Thread>,
) -> Result<()> {
    poll_new_messages_impl(s, state, threads, false).await
}

async fn poll_new_messages_impl(
    s: &Session,
    state: &mut PollState,
    threads: &mut Vec<Thread>,
    legacy_storage: bool,
) -> Result<()> {
    if s.login.is_none() {
        anyhow::bail!("chưa đăng nhập");
    }
    // 1. Poll live/offline message queues via `getChatFromPolling` (follow `more == 1` / `resetLastActionId` up to 3 batches)
    for _ in 0..3 {
        let Ok(lp_resp) = get_chat_from_polling(s, state).await else {
            break;
        };
        merge_previews(threads, &lp_resp);
        let data_node = lp_resp.get("data").unwrap_or(&lp_resp);
        let more = data_node.get("more").and_then(Value::as_i64).unwrap_or(0);
        let has_reset = pick_str(data_node, &["resetLastActionId"])
            .map(|r| r != "0" && !r.is_empty())
            .unwrap_or(false);
        if more == 0 && !has_reset {
            break;
        }
    }
    // 2. Poll conversation previews via `get-last-msgs` (both empty map and populated known thread IDs)
    if let Ok(preview_resp) = get_last_msgs(s, &[]).await {
        merge_previews(threads, &preview_resp);
    }
    let known_ids: Vec<String> = threads
        .iter()
        .filter(|t| !t.id.is_empty())
        .map(|t| {
            if t.is_group && !t.id.starts_with('g') && !t.id.starts_with("group_") {
                format!("g{}", t.id)
            } else {
                t.id.clone()
            }
        })
        .collect();
    for chunk in known_ids.chunks(25) {
        if let Ok(preview_resp) = get_last_msgs(s, chunk).await {
            merge_previews(threads, &preview_resp);
        }
    }
    // 2b. Sync Cloud của tôi (`send2me_id`) and group conversations via `/api/cm/getrecentv2`
    let send2me_id = s
        .login
        .as_ref()
        .map(|l| l.send2me_id.clone())
        .unwrap_or_default();
    if !send2me_id.is_empty() {
        if let Ok(cm_resp) = get_cloud_messages(s, &send2me_id, "0", false, 50).await {
            if let Some(t) = threads.iter_mut().find(|t| t.id == send2me_id) {
                merge_group_history(t, &cm_resp);
            } else {
                merge_previews(threads, &cm_resp);
            }
        }
    }
    let group_ids: Vec<String> = threads
        .iter()
        .filter(|t| t.is_group && !t.id.is_empty())
        .map(|t| t.id.clone())
        .collect();
    for gid in group_ids {
        if let Ok(cm_resp) = get_cloud_messages(s, &gid, "0", false, 50).await {
            if let Some(t) = threads.iter_mut().find(|t| t.id == gid) {
                merge_group_history(t, &cm_resp);
            }
        }
    }
    let _ = sync_unread_metadata(s, threads).await;
    // 3. Resolve display names for any newly discovered threads
    resolve_thread_names(s, threads).await;
    threads.retain(|t| !t.messages.is_empty() || !t.name.starts_with("Người dùng "));
    deduplicate_threads(threads, None);
    if legacy_storage {
        if let Ok(mut store) = crate::store::MessageStore::open("zalo_history.db") {
            let _ = crate::store::persist_threads(&mut store, threads);
        }
        save_threads_cache(threads);
    }
    Ok(())
}

fn extract_content_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => {
            let trimmed = s.trim();
            if trimmed == "sendBubbleMessage" {
                return None;
            }
            if trimmed.starts_with('{') && trimmed.ends_with('}') {
                if let Ok(parsed) = serde_json::from_str::<Value>(trimmed) {
                    if let Some(inner) = extract_content_text(&parsed) {
                        if !inner.is_empty() {
                            return Some(inner);
                        }
                    }
                    // It is a JSON object with no text/caption fields (like sticker {catId, id} or photo {href}).
                    // Return None so parse_chat_message can assign a clean descriptive label like [Nhãn dán] or [Hình ảnh]
                    // instead of displaying the raw JSON string in chat.
                    return None;
                }
            }
            Some(s.clone())
        }
        Value::Object(o) => {
            let href = o
                .get("href")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|h| {
                    !h.is_empty() && (h.starts_with("http://") || h.starts_with("https://"))
                });
            for k in ["description", "caption", "text", "msg", "title"] {
                if let Some(Value::String(s)) = o.get(k) {
                    let t = s.trim();
                    if !t.is_empty() && t != "sendBubbleMessage" {
                        return Some(s.clone());
                    }
                }
            }
            if let Some(Value::String(s)) = o.get("fileName").or_else(|| o.get("name")) {
                if !s.trim().is_empty() {
                    return Some(format!("[Tập tin] {s}"));
                }
            }
            if let Some(link) = href {
                return Some(link.to_string());
            }
            None
        }
        _ => None,
    }
}

fn normalize_msg_type(it: &Value) -> String {
    let raw = it
        .get("msgType")
        .or_else(|| it.get("messageType"))
        .or_else(|| it.get("type"));
    match raw {
        Some(Value::String(s)) if s.parse::<i64>().is_ok() => {
            normalize_msg_type(&json!({"msgType": s.parse::<i64>().unwrap()}))
        }
        Some(Value::String(s)) => s.to_ascii_lowercase(),
        Some(Value::Number(n)) => match n.as_i64() {
            Some(1) => "chat.text".into(),
            Some(2) => "chat.photo".into(),
            Some(3) => "chat.voice".into(),
            Some(4) => "chat.video".into(),
            Some(5) => "chat.sticker".into(),
            Some(19) => "chat.file".into(),
            Some(24) => "chat.location".into(),
            Some(31) => "chat.undo".into(),
            _ => n.to_string(),
        },
        _ => String::new(),
    }
}

fn extract_sticker_info(v: &Value) -> Option<(u64, u64, u32)> {
    match v {
        Value::String(s) => {
            let trimmed = s.trim();
            if trimmed.starts_with('{') && trimmed.ends_with('}') {
                if let Ok(parsed) = serde_json::from_str::<Value>(trimmed) {
                    return extract_sticker_info(&parsed);
                }
            }
            None
        }
        Value::Object(o) => {
            let parse_u64 = |val: &Value| -> Option<u64> {
                match val {
                    Value::Number(n) => n.as_u64(),
                    Value::String(s) => s.parse::<u64>().ok(),
                    _ => None,
                }
            };
            let cat_id = o
                .get("catId")
                .or_else(|| o.get("cateId"))
                .and_then(parse_u64);
            let stk_id = o
                .get("stickerId")
                .or_else(|| o.get("id"))
                .and_then(parse_u64);
            let stk_type = o
                .get("type")
                .and_then(parse_u64)
                .map(|t| t as u32)
                .unwrap_or(7);
            if let Some(sid) = stk_id {
                if cat_id.is_some()
                    || o.contains_key("stickerId")
                    || o.contains_key("catId")
                    || o.contains_key("cateId")
                {
                    return Some((sid, cat_id.unwrap_or(0), stk_type));
                }
            }
            None
        }
        _ => None,
    }
}

/// Extract rich link/media card metadata (`link_url`, `link_title`, `link_desc`, `link_thumb`, `link_src`)
/// from a Zalo `chat.recommended` (`recommened.link`) or rich-object `content`.
type LinkInfo = (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

fn extract_link_info(v: &Value) -> LinkInfo {
    match v {
        Value::String(s) => {
            let trimmed = s.trim();
            if trimmed.starts_with('{') && trimmed.ends_with('}') {
                if let Ok(parsed) = serde_json::from_str::<Value>(trimmed) {
                    return extract_link_info(&parsed);
                }
            }
            (None, None, None, None, None)
        }
        Value::Object(o) => {
            let href = o
                .get("href")
                .and_then(Value::as_str)
                .filter(|h| h.starts_with("http://") || h.starts_with("https://"))
                .map(str::to_string);
            let Some(url) = href else {
                return (None, None, None, None, None);
            };
            let thumb = ["thumb", "hd"]
                .iter()
                .filter_map(|key| o.get(*key).and_then(Value::as_str))
                .find(|t| t.starts_with("http://") || t.starts_with("https://"))
                .map(str::to_string);
            let mut title = o
                .get("title")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(str::to_string);
            let desc = o
                .get("description")
                .or_else(|| o.get("caption"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|d| !d.is_empty())
                .map(str::to_string);
            let mut src = None;
            if let Some(params_str) = o.get("params").and_then(Value::as_str) {
                if let Ok(p_json) = serde_json::from_str::<Value>(params_str.trim()) {
                    if title.is_none() {
                        title = p_json
                            .get("mediaTitle")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(str::to_string);
                    }
                    src = p_json
                        .get("src")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string);
                }
            }
            if src.is_none() {
                src = url
                    .split("://")
                    .nth(1)
                    .and_then(|rest| rest.split('/').next())
                    .filter(|h| !h.is_empty())
                    .map(str::to_string);
            }
            (Some(url), title, desc, thumb, src)
        }
        _ => (None, None, None, None, None),
    }
}
pub fn parse_chat_message(it: &Value) -> Option<ChatMessage> {
    let msg_type = normalize_msg_type(it);
    if msg_type == "chat.reaction" {
        return None;
    }

    let raw_content_val = it
        .get("content")
        .or_else(|| {
            it.get("message")
                .and_then(|m| m.get("content"))
                .or_else(|| it.get("message"))
        })
        .or_else(|| it.get("msg"));

    let mut sticker_meta = raw_content_val.and_then(extract_sticker_info);
    if sticker_meta.is_none()
        && (msg_type == "chat.sticker" || msg_type == "sticker" || msg_type == "5")
    {
        sticker_meta = extract_sticker_info(it);
    }
    let (link_url, link_title, link_desc, mut link_thumb, mut link_src) = raw_content_val
        .map(extract_link_info)
        .unwrap_or((None, None, None, None, None));

    match msg_type.as_str() {
        "chat.photo" | "photo" | "2" => {
            link_src = Some("Hình ảnh Zalo".into());
            if link_thumb.is_none() {
                link_thumb = link_url.clone();
            }
        }
        "chat.video" | "video" | "4" => link_src = Some("Video Zalo".into()),
        _ => {}
    }

    let mut content = raw_content_val
        .and_then(extract_content_text)
        .unwrap_or_default();
    let is_undo_type = matches!(msg_type.as_str(), "chat.undo" | "undo" | "31");
    if is_undo_type {
        content = "[Tin nhắn đã thu hồi]".to_string();
        sticker_meta = None;
    } else if sticker_meta.is_some() && (content.is_empty() || content.starts_with('{')) {
        content = "[Nhãn dán]".to_string();
    }

    if content.is_empty() {
        content = match msg_type.as_str() {
            "chat.sticker" | "sticker" | "5" => "[Nhãn dán]".to_string(),
            "chat.photo" | "photo" | "2" => "[Hình ảnh]".to_string(),
            "chat.voice" | "voice" | "3" => "[Tin nhắn thoại]".to_string(),
            "chat.video" | "video" | "4" => "[Video]".to_string(),
            "chat.file" | "file" | "19" => "[Tập tin]".to_string(),
            "chat.location" | "location" | "24" => "[Vị trí]".to_string(),
            "chat.contact" | "contact" => "[Danh thiếp]".to_string(),
            "chat.undo" | "undo" | "31" => "[Tin nhắn đã thu hồi]".to_string(),
            "chat.recommended" | "share.file" => "[Chia sẻ]".to_string(),
            _ => {
                let obj = it.get("content").or_else(|| it.get("message"));
                if let Some(o) = obj {
                    if o.get("catId").is_some()
                        || o.get("stickerId").is_some()
                        || o.get("id").is_some()
                    {
                        "[Nhãn dán]".to_string()
                    } else if o.get("href").is_some() || o.get("thumb").is_some() {
                        "[Hình ảnh]".to_string()
                    } else if o.is_object() {
                        "[Tin nhắn]".to_string()
                    } else {
                        String::new()
                    }
                } else {
                    String::new()
                }
            }
        };
    }

    let undo_target_id = if is_undo_type {
        raw_content_val.and_then(|rv| match rv {
            Value::Object(_) => pick_str(rv, &["globalMsgId", "msgId", "cliMsgId"]),
            Value::String(s) if s.trim().starts_with('{') => {
                serde_json::from_str::<Value>(s.trim())
                    .ok()
                    .and_then(|parsed| pick_str(&parsed, &["globalMsgId", "msgId", "cliMsgId"]))
            }
            _ => None,
        })
    } else {
        None
    };

    let msg_id = undo_target_id
        .or_else(|| {
            it.get("msgId")
                .or_else(|| it.get("cliMsgId"))
                .or_else(|| it.get("globalMsgId"))
                .or_else(|| it.get("lastMsgId"))
                .or_else(|| it.get("id"))
                .and_then(|v| match v {
                    Value::String(s) => Some(s.clone()),
                    Value::Number(n) => Some(n.to_string()),
                    _ => None,
                })
        })
        .unwrap_or_default();

    if content.is_empty() && msg_id.is_empty() {
        return None;
    }

    let uid_from = it
        .get("uidFrom")
        .or_else(|| it.get("fromUid"))
        .or_else(|| it.get("fromId"))
        .or_else(|| it.get("senderId"))
        .or_else(|| it.get("sender"))
        .and_then(|v| match v {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        })
        .unwrap_or_default();

    let display_sender = it
        .get("dName")
        .or_else(|| it.get("displayName"))
        .or_else(|| it.get("fromName"))
        .or_else(|| it.get("senderName"))
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| uid_from.clone());

    let is_outgoing = uid_from == "0"
        || uid_from.is_empty()
        || it
            .get("isOutgoing")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        || it
            .get("is_outgoing")
            .and_then(Value::as_i64)
            .map(|n| n == 1)
            .unwrap_or(false);

    let ts = it
        .get("ts")
        .or_else(|| it.get("sendTime"))
        .or_else(|| it.get("serverTime"))
        .or_else(|| it.get("time"))
        .and_then(|v| match v {
            Value::Number(n) => n.as_i64(),
            Value::String(s) => s.parse::<i64>().ok(),
            _ => None,
        })
        .unwrap_or(0);

    let ts = crate::clock::normalize_ts_ms(ts);
    let time = crate::clock::message_time(ts);

    let (reply_to_id, reply_to_sender, reply_to_text) =
        if let Some(q) = it.get("quote").filter(|v| v.is_object()) {
            let q_id = crate::message_navigation::reply_reference(q);
            let q_sender = pick_str(q, &["fromD", "dName", "ownerId", "fromUid", "uidFrom"]);
            let q_text = pick_str(q, &["msg", "message", "content"]).or_else(|| {
                q.get("msg")
                    .or_else(|| q.get("content"))
                    .and_then(extract_content_text)
            });
            (q_id, q_sender, q_text)
        } else {
            (None, None, None)
        };

    Some(ChatMessage {
        resolved_mention_uids: Default::default(),
        kind: crate::message_model::MessageKind::from_wire_name(&msg_type),
        wire_ref: crate::message_model::WireMessageRef::from_raw(it),
        unread_policy: crate::message_model::UnreadPolicy::from_raw(it),
        delivery: crate::message_model::DeliveryState::Unknown,
        id: msg_id,
        sender: display_sender,
        text: content,
        time,
        is_outgoing,
        ts,
        sticker_id: sticker_meta.map(|(sid, _, _)| sid),
        sticker_cate_id: sticker_meta.map(|(_, cid, _)| cid),
        sticker_type: sticker_meta.map(|(_, _, st)| st),
        reply_to_id,
        reply_to_sender,
        reply_to_text,
        reactions: crate::reactions::legacy_emojis(&crate::reactions::parse_reactions(it)),
        reaction_summaries: crate::reactions::parse_reactions(it),
        reaction_snapshot: it.get("reacts").is_some_and(|v| !v.is_null())
            || it.get("reactions").is_some_and(|v| !v.is_null()),
        reaction_authoritative_snapshot: it
            .get("reacts")
            .and_then(|v| v.get("reactions"))
            .or_else(|| it.get("reactions"))
            .is_some_and(Value::is_object),
        reaction_snapshot_fresh: it
            .get("reacts")
            .and_then(|v| v.get("reactions"))
            .or_else(|| it.get("reactions"))
            .is_some_and(Value::is_object),
        reaction_snapshot_provenance: it
            .get("reaction_snapshot_provenance")
            .and_then(|value| serde_json::from_value(value.clone()).ok())
            .unwrap_or_default(),
        reaction_versions: Default::default(),
        client_message_id: pick_str(it, &["cliMsgId", "clientId", "client_message_id"])
            .unwrap_or_default(),
        is_pinned: false,
        is_starred: false,
        link_url,
        link_title,
        link_desc,
        link_thumb,
        link_src,
        history_source: it.get("historySource").cloned(),
    })
}

fn is_group_flag(v: &Value) -> bool {
    v.get("isGroup")
        .map(|g| g.as_bool() == Some(true) || g.as_i64() == Some(1))
        .unwrap_or(false)
}

fn extract_message_thread_id_ext(
    entry: &Value,
    m_val: &Value,
    in_group_bucket: bool,
    cloud_thread_id: Option<&str>,
) -> Option<(String, bool)> {
    let is_group = in_group_bucket || is_group_flag(m_val) || is_group_flag(entry);

    // 1. Explicit `grid` field or `groupMsgs` bucket (`e.msgKey = GROUPID_PREFIX + e.idTo` in `sync-v2-sub-worker.js:3500340`)
    for obj in [m_val, entry] {
        if let Some(grid) = pick_str(obj, &["grid", "groupId"]) {
            if !grid.is_empty() && grid != "0" {
                let clean = grid.trim_start_matches("group_").to_string();
                return Some((clean, true));
            }
        }
    }
    if is_group {
        if let Some(gid) = pick_str(m_val, &["idTo", "toid", "toId", "threadId", "conversId"])
            .or_else(|| pick_str(entry, &["idTo", "toid", "toId", "threadId", "conversId"]))
        {
            if !gid.is_empty() && gid != "0" {
                let clean = gid.trim_start_matches("group_").to_string();
                return Some((clean, true));
            }
        }
    }

    // 2. 1-on-1 or self ("Cloud của tôi") message routing
    // [O] `sync-v2-sub-worker.js:3500260`:
    //   `"0" == e.idTo && "0" == e.uidFrom && (e.idTo = "" + this.userId)`
    // [O] `sync-v2-sub-worker.js:829500` (`_filterDuplicatedMessages`):
    //   `e.isGroup ? e.msgKey : "0" == e.idTo ? e.uidFrom : e.idTo`
    let uid_from = pick_str(m_val, &["uidFrom", "fromUid", "fromId", "senderId"])
        .or_else(|| pick_str(entry, &["uidFrom", "fromUid", "fromId", "senderId"]))
        .unwrap_or_default();
    let id_to = pick_str(m_val, &["idTo", "toid", "toId"])
        .or_else(|| pick_str(entry, &["idTo", "toid", "toId"]))
        .unwrap_or_default();

    if (id_to.is_empty() || id_to == "0") && (uid_from.is_empty() || uid_from == "0") {
        if let Some(cid) = cloud_thread_id {
            if !cid.is_empty() {
                return Some((cid.to_string(), false));
            }
        }
    }

    if !uid_from.is_empty()
        && uid_from != "0"
        && (id_to.is_empty() || id_to == "0" || Some(id_to.as_str()) == cloud_thread_id)
    {
        // Incoming 1-1 message: peer sent it to us -> thread ID is `uid_from`
        Some((uid_from, false))
    } else if !id_to.is_empty() && id_to != "0" {
        // Outgoing 1-1 message or conversation wrapper with explicit `idTo` -> thread ID is `id_to`
        Some((id_to, false))
    } else if !uid_from.is_empty() && uid_from != "0" {
        Some((uid_from, false))
    } else {
        pick_str(
            m_val,
            &["threadId", "conversId", "userId", "friendId", "id"],
        )
        .or_else(|| {
            pick_str(
                entry,
                &["threadId", "conversId", "userId", "friendId", "id"],
            )
        })
        .filter(|s| s != "0" && !s.is_empty())
        .map(|s| (s, is_group))
    }
}

/// Fold a `get-last-msgs` or `getChatFromPolling` response into thread previews and conversation messages.
pub fn merge_previews(threads: &mut Vec<Thread>, resp: &Value) {
    let mut entries: Vec<(&Value, bool)> = Vec::new();
    let parsed_str_data: Option<Value> = resp
        .get("data")
        .and_then(Value::as_str)
        .and_then(|s| serde_json::from_str::<Value>(s.trim()).ok());
    let mut data_node = parsed_str_data
        .as_ref()
        .unwrap_or_else(|| resp.get("data").unwrap_or(resp));
    if let Some(inner_data) = data_node
        .get("data")
        .filter(|v| v.is_object() || v.is_array())
    {
        data_node = inner_data;
    }
    crate::unread::merge_read_points(threads, data_node);
    let cloud_tid: Option<String> = threads
        .iter()
        .find(|t| t.name.contains("Cloud của tôi"))
        .map(|t| t.id.clone());

    if let Some(arr) = data_node.as_array() {
        for item in arr {
            entries.push((item, is_group_flag(item)));
        }
    } else if let Some(obj) = data_node.as_object() {
        let mut found_list = false;
        for (key, is_grp) in [
            ("msgs", false),
            ("groupMsgs", true),
            ("pageMsgs", false),
            ("conversations", false),
            ("threads", false),
            ("allmsgs", false),
        ] {
            if let Some(arr) = obj.get(key).and_then(Value::as_array) {
                for item in arr {
                    entries.push((item, is_grp || is_group_flag(item)));
                }
                found_list = true;
            }
        }
        if let Some(cu_arr) = obj.get("clearUnreads").and_then(Value::as_array) {
            for cu in cu_arr {
                let Some(tid) = pick_str(cu, &["idTo", "threadId", "userId"]) else {
                    continue;
                };
                if tid.is_empty() || tid == "0" {
                    continue;
                }
                let is_grp = is_group_flag(cu);
                // Only auto-create 1-1 conversation threads from `clearUnreads`; disbanded groups
                // may linger in `clearUnreads` after removal, whereas active groups come from `get_groups`.
                if !is_grp && !threads.iter().any(|t| t.id == tid) {
                    threads.push(Thread {
                        presentation: Default::default(),
                        unread: Default::default(),
                        id: tid.clone(),
                        name: format!("Người dùng {tid}"),
                        preview: String::new(),
                        time: String::new(),
                        avatar: None,
                        is_group: false,
                        messages: Vec::new(),
                    });
                }
            }
            found_list = true;
        }
        if !found_list {
            for (_, val) in obj {
                if val.is_object() {
                    entries.push((val, is_group_flag(val)));
                }
            }
        }
    }

    for (entry, is_grp_bucket) in entries {
        let mut raw_msgs: Vec<(&Value, bool)> = Vec::new();
        if let Some(arr) = entry.get("messages").and_then(Value::as_array) {
            for m in arr {
                raw_msgs.push((m, is_grp_bucket || is_group_flag(m)));
            }
        }
        if let Some(arr) = entry.get("msgs").and_then(Value::as_array) {
            for m in arr {
                raw_msgs.push((m, is_grp_bucket || is_group_flag(m)));
            }
        }
        if let Some(arr) = entry.get("groupMsgs").and_then(Value::as_array) {
            for m in arr {
                raw_msgs.push((m, true));
            }
        }
        if let Some(obj) = entry.get("lastMsg").filter(|v| v.is_object()) {
            raw_msgs.push((obj, is_grp_bucket || is_group_flag(obj)));
        }
        if let Some(obj) = entry.get("message").filter(|v| v.is_object()) {
            raw_msgs.push((obj, is_grp_bucket || is_group_flag(obj)));
        }
        if raw_msgs.is_empty()
            && (entry.get("content").is_some()
                || entry.get("msg").is_some()
                || entry.get("msgId").is_some())
        {
            raw_msgs.push((entry, is_grp_bucket));
        }

        for (m_val, is_grp) in raw_msgs {
            let Some(parsed) = parse_chat_message(m_val) else {
                continue;
            };

            let Some((tid, thread_is_group)) =
                extract_message_thread_id_ext(entry, m_val, is_grp, cloud_tid.as_deref())
            else {
                continue;
            };

            if let Some(t) = threads.iter_mut().find(|t| t.id == tid) {
                if thread_is_group {
                    t.is_group = true;
                }
                if let Some(index) = matching_message_index(&t.messages, &parsed) {
                    let existing = &mut t.messages[index];
                    existing.merge_reaction_metadata(&parsed);
                    existing.merge_media_metadata(&parsed);
                    if parsed.history_source.is_some() {
                        existing.history_source = parsed.history_source.clone();
                    }
                    if existing.id.starts_with("out_")
                        && !parsed.id.is_empty()
                        && !parsed.id.starts_with("out_")
                    {
                        existing.id = parsed.id;
                    }
                    if parsed.ts > 0 && existing.ts != parsed.ts {
                        existing.ts = parsed.ts;
                        existing.time = parsed.time;
                    }
                    if parsed.text.trim() == "[Tin nhắn đã thu hồi]" {
                        existing.text = "[Tin nhắn đã thu hồi]".to_string();
                        existing.sticker_id = None;
                        existing.link_url = None;
                        existing.link_thumb = None;
                    }
                    if existing.sticker_id.is_none() && parsed.sticker_id.is_some() {
                        existing.sticker_id = parsed.sticker_id;
                        existing.sticker_cate_id = parsed.sticker_cate_id;
                        existing.sticker_type = parsed.sticker_type;
                    }
                    if existing.reply_to_text.is_none() && parsed.reply_to_text.is_some() {
                        existing.reply_to_id = parsed.reply_to_id;
                        existing.reply_to_sender = parsed.reply_to_sender;
                        existing.reply_to_text = parsed.reply_to_text;
                    }
                    if existing.link_url.is_none() && parsed.link_url.is_some() {
                        existing.link_url = parsed.link_url;
                        existing.link_title = parsed.link_title;
                        existing.link_desc = parsed.link_desc;
                        existing.link_thumb = parsed.link_thumb;
                        existing.link_src = parsed.link_src;
                        if !parsed.text.is_empty() {
                            existing.text = parsed.text;
                        }
                    }
                    if !parsed.reactions.is_empty() {
                        existing.reactions = parsed.reactions;
                    }
                    existing.is_pinned = existing.is_pinned || parsed.is_pinned;
                    existing.is_starred = existing.is_starred || parsed.is_starred;
                } else {
                    t.messages.push(parsed);
                }
            } else {
                // New thread detected from incoming message!
                let default_name = if thread_is_group {
                    format!("Nhóm {}", tid)
                } else {
                    format!("Người dùng {}", tid)
                };
                threads.push(Thread {
                    presentation: Default::default(),
                    unread: Default::default(),
                    id: tid,
                    name: default_name,
                    preview: parsed.text.clone(),
                    time: parsed.time.clone(),
                    avatar: None,
                    is_group: thread_is_group,
                    messages: vec![parsed],
                });
            }
        }
    }

    for t in threads.iter_mut() {
        t.refresh_sidebar_presentation();
        if !t.messages.is_empty() {
            t.messages.sort_by_key(|m| m.ts);
            if let Some(newest) = t.messages.last().filter(|m| {
                crate::clock::normalize_ts_ms(m.ts)
                    >= crate::clock::normalize_ts_ms(t.presentation.last_message_ts)
            }) {
                if !newest.text.is_empty() {
                    t.preview = newest.text.replace('\n', " ");
                    t.time = newest.time.clone();
                }
            }
        }
    }
}

/// Merge group history response into a thread's messages, sorted by timestamp and deduplicated by msg_id.
pub fn merge_group_history(thread: &mut Thread, resp: &Value) {
    let mut raw_msgs: Vec<&Value> = Vec::new();
    let parsed_str_data: Option<Value> = resp
        .get("data")
        .and_then(Value::as_str)
        .and_then(|s| serde_json::from_str::<Value>(s.trim()).ok());
    let mut data_node = parsed_str_data
        .as_ref()
        .unwrap_or_else(|| resp.get("data").unwrap_or(resp));
    if let Some(inner_data) = data_node
        .get("data")
        .filter(|v| v.is_object() || v.is_array())
    {
        data_node = inner_data;
    }

    if let Some(arr) = data_node.as_array() {
        raw_msgs.extend(arr);
    } else if let Some(obj) = data_node.as_object() {
        for key in ["msgs", "groupMsgs", "messages"] {
            if let Some(arr) = obj.get(key).and_then(Value::as_array) {
                raw_msgs.extend(arr);
            }
        }
    }

    let clean_thread_id = thread
        .id
        .trim_start_matches("group_")
        .trim_start_matches('g')
        .trim_start_matches('G')
        .to_string();

    let mut ids = std::collections::HashMap::new();
    let mut clients: std::collections::HashMap<String, Vec<usize>> =
        std::collections::HashMap::new();
    for (index, message) in thread.messages.iter().enumerate() {
        if !message.id.is_empty() {
            ids.entry(message.id.clone()).or_insert(index);
        }
        if !message.client_message_id.is_empty() {
            clients
                .entry(message.client_message_id.clone())
                .or_default()
                .push(index);
        }
    }
    for it in raw_msgs {
        if !clean_thread_id.is_empty() {
            if let Some(id_to) = pick_str(it, &["idTo", "toId", "grid", "groupId"]) {
                let clean_to = id_to
                    .trim_start_matches("group_")
                    .trim_start_matches('g')
                    .trim_start_matches('G');
                if !clean_to.is_empty() && clean_to != "0" && clean_to != clean_thread_id {
                    continue;
                }
            }
        }
        if let Some(m) = parse_chat_message(it) {
            let exact = ids.get(&m.id).copied().filter(|_| !m.id.is_empty());
            let echo = clients
                .get(&m.client_message_id)
                .filter(|indices| indices.len() == 1)
                .and_then(|indices| {
                    let index = indices[0];
                    let existing = &thread.messages[index];
                    (!existing.id.is_empty()
                        && !m.id.is_empty()
                        && existing.is_outgoing
                        && m.is_outgoing
                        && (existing.id.starts_with("out_") != m.id.starts_with("out_")))
                    .then_some(index)
                });
            if let Some(index) = exact.or(echo) {
                let existing = &mut thread.messages[index];
                if existing.id.starts_with("out_") && !m.id.starts_with("out_") {
                    existing.id = m.id.clone();
                    ids.entry(m.id.clone()).or_insert(index);
                }
                if existing.client_message_id.is_empty() && !m.client_message_id.is_empty() {
                    clients
                        .entry(m.client_message_id.clone())
                        .or_default()
                        .push(index);
                }
                existing.merge_reaction_metadata(&m);
                existing.merge_media_metadata(&m);
                if existing.link_url.is_none() && m.link_url.is_some() {
                    existing.link_url = m.link_url;
                    existing.link_title = m.link_title;
                    existing.link_desc = m.link_desc;
                    existing.link_thumb = m.link_thumb;
                    existing.link_src = m.link_src;
                    if !m.text.is_empty() {
                        existing.text = m.text;
                    }
                }
            } else {
                let index = thread.messages.len();
                if !m.id.is_empty() {
                    ids.entry(m.id.clone()).or_insert(index);
                }
                if !m.client_message_id.is_empty() {
                    clients
                        .entry(m.client_message_id.clone())
                        .or_default()
                        .push(index);
                }
                thread.messages.push(m);
            }
        }
    }

    thread.refresh_sidebar_presentation();
    thread.messages.sort_by_key(|m| m.ts);

    if let Some(newest) = thread.messages.last().filter(|m| {
        crate::clock::normalize_ts_ms(m.ts)
            >= crate::clock::normalize_ts_ms(thread.presentation.last_message_ts)
    }) {
        if !newest.text.is_empty() {
            thread.preview = newest.text.replace('\n', " ");
            thread.time = newest.time.clone();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum FriendshipStatus {
    #[default]
    Unknown,
    Friend,
    OutgoingPending,
    IncomingPending,
    Stranger,
}

/// Parse `/api/friend/reqstatus` (`cmd 11430`) JSON into `FriendshipStatus`
/// (`compact-app-pc.js:8351970`: `is_friend`, `is_requested` = I requested you, `is_requesting` = you requested me).
pub fn parse_friendship_status_response(resp: &Value) -> FriendshipStatus {
    let d = resp
        .get("data")
        .and_then(|x| x.get("data").or(Some(x)))
        .unwrap_or(resp);
    let is_friend = d
        .get("is_friend")
        .or_else(|| d.get("isFriend"))
        .and_then(|v| {
            v.as_i64()
                .or_else(|| v.as_bool().map(|b| if b { 1 } else { 0 }))
        })
        .unwrap_or(0);
    if is_friend == 1 {
        return FriendshipStatus::Friend;
    }
    let is_requesting = d
        .get("is_requesting")
        .or_else(|| d.get("isRequesting"))
        .and_then(|v| {
            v.as_i64()
                .or_else(|| v.as_bool().map(|b| if b { 1 } else { 0 }))
        })
        .unwrap_or(0);
    if is_requesting == 1 {
        return FriendshipStatus::IncomingPending;
    }
    let is_requested = d
        .get("is_requested")
        .or_else(|| d.get("isRequested"))
        .and_then(|v| {
            v.as_i64()
                .or_else(|| v.as_bool().map(|b| if b { 1 } else { 0 }))
        })
        .unwrap_or(0);
    if is_requested == 1 {
        return FriendshipStatus::OutgoingPending;
    }
    FriendshipStatus::Stranger
}

/// Query `/api/friend/reqstatus` (`cmd 11430`) and return the typed `FriendshipStatus`.
pub async fn check_friendship_status(s: &Session, fid: &str) -> Result<FriendshipStatus> {
    let resp = get_friend_request_status(s, fid).await?;
    Ok(parse_friendship_status_response(&resp))
}
/// Check friend request status with a target user (`/api/friend/reqstatus`, cmd `11430`).
pub async fn get_friend_request_status(s: &Session, fid: &str) -> Result<Value> {
    if s.login.is_none() {
        anyhow::bail!("chưa đăng nhập");
    }
    let body = obj(&[("fid", json!(fid)), ("imei", json!(s.imei))]);
    let enc = s.encoded_params(&body)?;
    let host = if s.domains.friend.is_empty() {
        "https://tt-friend-wpa.chat.zalo.me"
    } else {
        &s.domains.friend
    };
    let url = format!(
        "{}/api/friend/reqstatus?{}&params={enc}",
        host.trim_end_matches('/'),
        s.common_params()
    );
    let resp = s.get(&url).await?;
    Ok(s.maybe_decrypt(resp, s.session_key_bytes()).await)
}

/// Query `/api/social/profile/lastOnline` (`cmd 12090`) and return `(is_online, label)`
/// matching Zalo PC's `getLastActiveText` (`user_active_time = 5 minutes`).
pub async fn get_user_online_status(s: &Session, uid: &str) -> Result<(bool, String)> {
    if s.login.is_none() || uid.trim().is_empty() {
        return Ok((false, String::new()));
    }
    let body = obj(&[
        ("uid", json!(uid.trim())),
        ("conv_type", json!(1)),
        ("imei", json!(s.imei)),
    ]);
    let enc = s.encoded_params(&body)?;
    // Session domain defaults provide the canonical profile host.
    let host = &s.domains.profile;
    let url = format!(
        "{}/api/social/profile/lastOnline?{}&params={enc}",
        host.trim_end_matches('/'),
        s.common_params()
    );
    let resp = s.get(&url).await?;
    let mut dec = s.maybe_decrypt(resp, s.session_key_bytes()).await;
    if let Some(d_str) = dec.get("data").and_then(Value::as_str) {
        if let Ok(parsed) = serde_json::from_str::<Value>(d_str.trim()) {
            dec["data"] = parsed;
        }
    }
    let d = dec
        .pointer("/data/data")
        .or_else(|| dec.get("data"))
        .unwrap_or(&dec);
    let show_online = d
        .pointer("/settings/show_online_status")
        .and_then(|v| v.as_bool().or_else(|| v.as_i64().map(|n| n != 0)))
        .unwrap_or(true);
    if !show_online {
        return Ok((false, String::new()));
    }
    let last_onl_raw = d
        .get("lastOnline")
        .and_then(|v| {
            v.as_i64()
                .or_else(|| v.as_str().and_then(|s| s.trim().parse::<i64>().ok()))
        })
        .unwrap_or(0);
    if last_onl_raw <= 0 {
        return Ok((false, String::new()));
    }
    let last_sec = if last_onl_raw > 10_000_000_000 {
        last_onl_raw / 1000
    } else {
        last_onl_raw
    };
    let now_sec = crate::login::unix_now();
    let diff_sec = (now_sec - last_sec).max(0);
    if diff_sec <= 300 {
        return Ok((true, "Vừa mới truy cập".into()));
    }
    let mins = diff_sec / 60;
    if mins < 60 {
        return Ok((false, format!("Truy cập {mins} phút trước")));
    }
    let hours = diff_sec / 3600;
    if hours < 24 {
        return Ok((false, format!("Truy cập {hours} giờ trước")));
    }
    let days = (diff_sec / 86400).min(7);
    Ok((false, format!("Truy cập {days} ngày trước")))
}

/// Send a friend request to a target user (`/api/friend/sendreq`, cmd `11424`).
pub async fn send_friend_request(s: &Session, to_id: &str, msg: &str) -> Result<Value> {
    if s.login.is_none() {
        anyhow::bail!("chưa đăng nhập");
    }
    let body = obj(&[
        ("toid", json!(to_id)),
        ("msg", json!(msg)),
        ("reqsrc", json!(30)),
        ("imei", json!(s.imei)),
        ("language", json!("vi")),
    ]);
    let enc = s.encoded_params(&body)?;
    let host = if s.domains.friend.is_empty() {
        "https://tt-friend-wpa.chat.zalo.me"
    } else {
        &s.domains.friend
    };
    let url = format!(
        "{}/api/friend/sendreq?{}",
        host.trim_end_matches('/'),
        s.common_params()
    );
    let resp = s.post_form(&url, format!("params={enc}")).await?;
    let dec = s.maybe_decrypt(resp, s.session_key_bytes()).await;
    if let Some(code) = dec.get("error_code").and_then(Value::as_i64) {
        if code != 0 {
            let err_msg = dec
                .get("error_message")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            anyhow::bail!("sendreq error {code}: {err_msg}");
        }
    }
    Ok(dec)
}

/// Accept a pending incoming friend request (`/api/friend/accept`, cmd `11427`).
pub async fn accept_friend_request(s: &Session, fid: &str) -> Result<Value> {
    if s.login.is_none() {
        anyhow::bail!("chưa đăng nhập");
    }
    let body = obj(&[("fid", json!(fid)), ("language", json!("vi"))]);
    let enc = s.encoded_params(&body)?;
    let host = if s.domains.friend.is_empty() {
        "https://tt-friend-wpa.chat.zalo.me"
    } else {
        &s.domains.friend
    };
    let url = format!(
        "{}/api/friend/accept?{}",
        host.trim_end_matches('/'),
        s.common_params()
    );
    let resp = s.post_form(&url, format!("params={enc}")).await?;
    let dec = s.maybe_decrypt(resp, s.session_key_bytes()).await;
    if let Some(code) = dec.get("error_code").and_then(Value::as_i64) {
        if code != 0 {
            let err_msg = dec
                .get("error_message")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            anyhow::bail!("accept error {code}: {err_msg}");
        }
    }
    Ok(dec)
}

/// Cancel / undo a pending outgoing friend request (`/api/friend/undo`, cmd `11441`).
pub async fn undo_friend_request(s: &Session, fid: &str) -> Result<Value> {
    if s.login.is_none() {
        anyhow::bail!("chưa đăng nhập");
    }
    let body = obj(&[("fid", json!(fid)), ("imei", json!(s.imei))]);
    let enc = s.encoded_params(&body)?;
    let host = if s.domains.friend.is_empty() {
        "https://tt-friend-wpa.chat.zalo.me"
    } else {
        &s.domains.friend
    };
    let url = format!(
        "{}/api/friend/undo?{}",
        host.trim_end_matches('/'),
        s.common_params()
    );
    let resp = s.post_form(&url, format!("params={enc}")).await?;
    Ok(s.maybe_decrypt(resp, s.session_key_bytes()).await)
}

/// Reject a pending incoming friend request (`/api/friend/reject`, cmd `11428`).
pub async fn reject_friend_request(s: &Session, fid: &str) -> Result<Value> {
    if s.login.is_none() {
        anyhow::bail!("chưa đăng nhập");
    }
    let body = obj(&[("fid", json!(fid)), ("imei", json!(s.imei))]);
    let enc = s.encoded_params(&body)?;
    let host = if s.domains.friend.is_empty() {
        "https://tt-friend-wpa.chat.zalo.me"
    } else {
        &s.domains.friend
    };
    let url = format!(
        "{}/api/friend/reject?{}",
        host.trim_end_matches('/'),
        s.common_params()
    );
    let resp = s.post_form(&url, format!("params={enc}")).await?;
    Ok(s.maybe_decrypt(resp, s.session_key_bytes()).await)
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PhoneLookupResult {
    pub user_id: String,
    pub display_name: String,
    pub zalo_name: String,
    pub phone: String,
    pub avatar: String,
    pub status: String,
    #[serde(default)]
    pub friendship: FriendshipStatus,
}

/// Normalize a Vietnamese or international phone string for Zalo's `/api/friend/profile/get`.
///
/// Converts leading `0` to `84` (e.g. `0912345678` -> `84912345678`), strips `+`, spaces, dots, and dashes.
pub fn normalize_phone_for_lookup(raw: &str) -> String {
    let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
    if let Some(rest) = digits.strip_prefix('0') {
        format!("84{rest}")
    } else {
        digits
    }
}

/// Look up a Zalo user profile by phone number (`getUserByPhone` in `compact-app-pc.js:14306443`, cmd `11420`).
///
/// Endpoint: `GET {domains.friend}/api/friend/profile/get?{common}&params={encodeAES({ phone, avatar_size: 240, language: "vi", imei, reqSrc: 40 })}`
pub async fn find_user_by_phone(s: &Session, raw_phone: &str) -> Result<PhoneLookupResult> {
    if s.login.is_none() {
        anyhow::bail!("chưa đăng nhập");
    }
    let normalized = normalize_phone_for_lookup(raw_phone);
    if normalized.len() < 8 {
        anyhow::bail!("Số điện thoại không hợp lệ (cần ít nhất 9 chữ số)");
    }
    let body = obj(&[
        ("phone", json!(normalized)),
        ("avatar_size", json!(240)),
        ("language", json!("vi")),
        ("imei", json!(s.imei)),
        ("reqSrc", json!(40)),
    ]);
    let enc = s.encoded_params(&body)?;
    let host = if s.domains.friend.is_empty() {
        "https://tt-friend-wpa.chat.zalo.me"
    } else {
        s.domains.friend.trim_end_matches('/')
    };
    let url = format!(
        "{host}/api/friend/profile/get?{}&params={enc}",
        s.common_params()
    );
    let resp = s.get(&url).await?;
    let dec = s.maybe_decrypt(resp, s.session_key_bytes()).await;
    if let Some(code) = dec.get("error_code").and_then(Value::as_i64) {
        if code != 0 {
            if code == 216 {
                anyhow::bail!(
                    "Số điện thoại chưa đăng ký Zalo hoặc người dùng đã chặn tìm kiếm qua SĐT (mã 216)"
                );
            }
            let err_msg = dec
                .get("error_message")
                .and_then(Value::as_str)
                .unwrap_or("Không tìm thấy tài khoản");
            anyhow::bail!("{err_msg} (mã {code})");
        }
    }

    let data = dec.get("data").unwrap_or(&dec);
    let Some(uid) = pick_str(data, &["uid", "userId", "id", "globalId"]) else {
        anyhow::bail!("Không tìm thấy tài khoản Zalo ứng với số điện thoại này");
    };
    let display_name = pick_str(
        data,
        &["display_name", "displayName", "zalo_name", "zaloName"],
    )
    .unwrap_or_else(|| raw_phone.trim().to_string());
    let zalo_name = pick_str(
        data,
        &["zalo_name", "zaloName", "display_name", "displayName"],
    )
    .unwrap_or_else(|| display_name.clone());
    let avatar = pick_str(data, &["avatar", "avt"]).unwrap_or_default();
    let status = pick_str(data, &["status"]).unwrap_or_default();

    let friendship = check_friendship_status(s, &uid)
        .await
        .unwrap_or(FriendshipStatus::Unknown);

    Ok(PhoneLookupResult {
        user_id: uid,
        display_name,
        zalo_name,
        phone: raw_phone.trim().to_string(),
        avatar,
        status,
        friendship,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unread_sync_targets_metadata_only_threads_in_bounded_batches() {
        let mut threads: Vec<_> = (1..=201)
            .map(|id| Thread {
                presentation: Default::default(),
                id: id.to_string(),
                name: String::new(),
                preview: String::new(),
                time: String::new(),
                avatar: None,
                is_group: id % 2 == 0,
                messages: Vec::new(),
                unread: Default::default(),
            })
            .collect();
        for id in ["", "0"] {
            let mut invalid = threads[0].clone();
            invalid.id = id.into();
            threads.push(invalid);
        }
        let targets = unread_metadata_targets(&threads);
        assert_eq!(targets.len(), 201);
        assert_eq!(
            targets
                .chunks(100)
                .map(|batch| batch.len())
                .collect::<Vec<_>>(),
            vec![100, 100, 1]
        );
        assert!(targets.iter().all(|thread| thread.messages.is_empty()));
    }

    #[test]
    fn sync_errors_after_decryption_are_not_successful_empty_lists() {
        assert!(
            checked_sync_response(json!({"error_code": 0, "data": {"error_code": 600}})).is_err()
        );
        assert!(checked_sync_response(json!({"error_code": "600", "data": []})).is_err());
        assert!(
            checked_sync_response(json!({"error_code": 0, "data": "encrypted-fixture"})).is_err()
        );
        let good =
            checked_sync_response(json!({"error_code": 0, "data": "{\"friends\":[]}"})).unwrap();
        assert_eq!(good["data"]["friends"], json!([]));
    }

    #[test]
    fn sync_errors_preserve_typed_auth_and_transient_status() {
        for code in [600, -102] {
            let error =
                checked_sync_response(json!({"error_code": 0, "data": {"error_code": code}}))
                    .unwrap_err();
            assert_eq!(
                crate::session::SessionStatus::from_error(&error),
                crate::session::SessionStatus::Expired
            );
        }
        for code in [429, 500, 18022] {
            let error = checked_sync_response(json!({"error_code": code})).unwrap_err();
            assert_eq!(
                crate::session::SessionStatus::from_error(&error),
                crate::session::SessionStatus::Unreachable
            );
        }
    }

    #[tokio::test]
    async fn unauthenticated_calls_fail_cleanly() {
        let s = Session::new(false, false, false).unwrap();
        let err1 = get_friends(&s).await.unwrap_err();
        assert_eq!(err1.to_string(), "chưa đăng nhập");

        let err2 = get_last_msgs(&s, &["test_tid".into()]).await.unwrap_err();
        assert_eq!(err2.to_string(), "chưa đăng nhập");

        let err3 = send_message(&s, "test_to", "hello").await.unwrap_err();
        assert_eq!(err3.to_string(), "chưa đăng nhập");

        let err4 = get_pinned(&s).await.unwrap_err();
        assert_eq!(err4.to_string(), "chưa đăng nhập");

        let err5 = get_group_history(&s, "test_gid", 20).await.unwrap_err();
        assert_eq!(err5.to_string(), "chưa đăng nhập");
    }

    fn sample_thread(id: &str) -> Thread {
        Thread {
            presentation: Default::default(),
            unread: Default::default(),
            id: id.into(),
            name: format!("Name {id}"),
            preview: String::new(),
            time: String::new(),
            avatar: None,
            is_group: false,
            messages: Vec::new(),
        }
    }

    #[test]
    fn merge_thread_into_keeps_identity_rules_with_indexed_lookup() {
        let m = |id: &str, text: &str, ts: i64, out: bool| ChatMessage {
            id: id.into(),
            text: text.into(),
            ts,
            time: format!("{}", ts / 60_000),
            is_outgoing: out,
            ..Default::default()
        };
        let mut target = Thread {
            presentation: Default::default(),
            unread: Default::default(),
            id: "t".into(),
            name: "A".into(),
            preview: String::new(),
            time: String::new(),
            avatar: None,
            is_group: false,
            messages: vec![
                m("1", "chào", 1_000_000, false),
                ChatMessage {
                    client_message_id: "client-9".into(),
                    ..m("out_9", "ok", 2_000_000, true)
                },
            ],
        };
        let incoming = Thread {
            messages: vec![
                m("1", "chào", 1_000_000, false), // same id -> no duplicate
                ChatMessage {
                    client_message_id: "client-9".into(),
                    ..m("77", "ok", 2_002_000, true)
                }, // verified client echo
                m("78", "ok", 9_000_000_000, true), // same text, other day -> new message
                m("79", "mới", 3_000_000, false), // new
                m("80", "ok", 2_003_000, true),   // distinct ID and no verified client echo
            ],
            ..target.clone()
        };
        merge_thread_into(&mut target, incoming, false);
        let ids: Vec<&str> = target.messages.iter().map(|x| x.id.as_str()).collect();
        assert_eq!(ids, vec!["1", "77", "80", "79", "78"]);
    }

    #[test]
    fn test_merge_previews_single_message_object() {
        let mut threads = vec![sample_thread("u1")];
        let resp = json!({
            "error_code": 0,
            "data": [
                {
                    "idTo": "u1",
                    "message": {
                        "msgId": "m_single",
                        "content": "Single message text",
                        "ts": 1729501000000_i64,
                        "uidFrom": "u1"
                    }
                }
            ]
        });

        merge_previews(&mut threads, &resp);
        assert_eq!(threads[0].messages.len(), 1);
        assert_eq!(threads[0].messages[0].id, "m_single");
        assert_eq!(threads[0].messages[0].text, "Single message text");
        assert_eq!(threads[0].preview, "Single message text");
    }

    #[test]
    fn test_merge_previews_last_msg_object() {
        let mut threads = vec![sample_thread("u2")];
        let resp = json!({
            "error_code": 0,
            "data": [
                {
                    "idTo": "u2",
                    "lastMsg": {
                        "msgId": "m_last",
                        "content": "LastMsg text",
                        "ts": 1729502000000_i64,
                        "uidFrom": "u2"
                    }
                }
            ]
        });

        merge_previews(&mut threads, &resp);
        assert_eq!(threads[0].messages.len(), 1);
        assert_eq!(threads[0].messages[0].id, "m_last");
        assert_eq!(threads[0].preview, "LastMsg text");
    }

    #[test]
    fn test_merge_previews_messages_array_three_items() {
        let mut threads = vec![sample_thread("u3")];
        // Provide items out of order to verify timestamp ascending sort
        let resp = json!({
            "error_code": 0,
            "data": [
                {
                    "idTo": "u3",
                    "messages": [
                        {
                            "msgId": "m_item1",
                            "content": "First message",
                            "ts": 1000_i64,
                            "uidFrom": "u3"
                        },
                        {
                            "msgId": "m_item3",
                            "content": "Third message",
                            "ts": 3000_i64,
                            "uidFrom": "u3"
                        },
                        {
                            "msgId": "m_item2",
                            "content": "Second message",
                            "ts": 2000_i64,
                            "uidFrom": "0"
                        }
                    ]
                }
            ]
        });

        merge_previews(&mut threads, &resp);
        assert_eq!(threads[0].messages.len(), 3);
        assert_eq!(threads[0].messages[0].id, "m_item1");
        assert_eq!(threads[0].messages[0].text, "First message");
        assert_eq!(threads[0].messages[1].id, "m_item2");
        assert_eq!(threads[0].messages[1].text, "Second message");
        assert!(threads[0].messages[1].is_outgoing);
        assert_eq!(threads[0].messages[2].id, "m_item3");
        assert_eq!(threads[0].messages[2].text, "Third message");
        // Preview must be from the newest message (m_item3)
        assert_eq!(threads[0].preview, "Third message");
    }

    #[test]
    fn test_merge_previews_entry_with_no_message() {
        let mut threads = vec![sample_thread("u4")];
        threads[0].preview = "Existing preview".into();
        let resp = json!({
            "error_code": 0,
            "data": [
                {
                    "idTo": "u4"
                },
                {
                    "idTo": "u4",
                    "messages": []
                }
            ]
        });

        merge_previews(&mut threads, &resp);
        assert!(threads[0].messages.is_empty());
        assert_eq!(threads[0].preview, "Existing preview");
    }

    #[test]
    fn test_merge_group_history_fixture() {
        let mut thread = sample_thread("grp_123");
        thread.is_group = true;

        let resp = json!({
            "error_code": 0,
            "data": {
                "groupMsgs": [
                    {
                        "msgId": "gm_2",
                        "content": "Second group message",
                        "ts": 2000_i64,
                        "uidFrom": "member_2"
                    },
                    {
                        "msgId": "gm_1",
                        "content": "First group message",
                        "ts": 1000_i64,
                        "uidFrom": "member_1"
                    },
                    {
                        "msgId": "gm_2",
                        "content": "Duplicate second message",
                        "ts": 2000_i64,
                        "uidFrom": "member_2"
                    }
                ]
            }
        });

        merge_group_history(&mut thread, &resp);
        assert_eq!(thread.messages.len(), 2);
        assert_eq!(thread.messages[0].id, "gm_1");
        assert_eq!(thread.messages[1].id, "gm_2");
        assert_eq!(thread.preview, "Second group message");
    }

    #[test]
    fn test_merge_previews_incoming_1_1_and_group_and_cloud() {
        let mut cloud = sample_thread("9000000000000000001");
        cloud.name = "Cloud của tôi (Lưu trữ)".into();
        let friend = sample_thread("1000000000000000001");
        let mut group = sample_thread("888999000");
        group.is_group = true;
        let mut threads = vec![cloud, friend, group];

        let resp = json!({
            "error_code": 0,
            "data": {
                "msgs": [
                    {
                        "msgId": "in_1",
                        "content": "Tin nhan den tu ban be",
                        "idTo": "0",
                        "uidFrom": "1000000000000000001",
                        "ts": "1729505000000"
                    },
                    {
                        "msgId": "cloud_1",
                        "content": "Tin nhan tu gui vao Cloud",
                        "idTo": "0",
                        "uidFrom": "0",
                        "ts": "1729506000000"
                    }
                ],
                "groupMsgs": [
                    {
                        "msgId": "grp_msg_1",
                        "content": "Tin nhan trong nhom",
                        "idTo": "888999000",
                        "uidFrom": "111222333",
                        "dName": "Nguyen Van B",
                        "ts": "1729507000000"
                    }
                ]
            }
        });

        merge_previews(&mut threads, &resp);
        // Cloud message routed to `9000000000000000001`
        assert_eq!(threads[0].messages.len(), 1);
        assert_eq!(threads[0].messages[0].text, "Tin nhan tu gui vao Cloud");
        assert!(threads[0].messages[0].is_outgoing);

        // Incoming 1-1 routed to `1000000000000000001`
        assert_eq!(threads[1].messages.len(), 1);
        assert_eq!(threads[1].messages[0].text, "Tin nhan den tu ban be");
        assert!(!threads[1].messages[0].is_outgoing);

        // Group message routed to `888999000` (not `111222333`)
        assert_eq!(threads[2].messages.len(), 1);
        assert_eq!(threads[2].messages[0].text, "Tin nhan trong nhom");
        assert_eq!(threads[2].messages[0].sender, "Nguyen Van B");
        assert!(!threads[2].messages[0].is_outgoing);
    }

    #[test]
    fn test_poll_state_encode_and_update() {
        let mut ps = PollState::default();
        let encoded = ps.encode_params();
        assert!(encoded.starts_with("1,1,"));
        assert!(encoded.ends_with("%2C1"));

        let resp = json!({
            "data": {
                "lastActionId": "8399086511016",
                "lastActionIdOther": "8399036593265",
                "queueStatus": {
                    "1": {
                        "lastId": "8399086511016",
                        "ids": ["8399086511016"]
                    },
                    "15": {
                        "lastId": "8399036593265",
                        "ids": ["8399036593265"]
                    }
                }
            }
        });
        ps.update_from_response(&resp);
        assert_eq!(ps.last_action_id, "8399086511016");
        assert_eq!(ps.last_action_id_other, "8399036593265");
        assert_eq!(ps.queues.get("1").unwrap().last_id, "8399086511016");
        assert_eq!(
            ps.queues.get("1").unwrap().ids,
            vec!["8399086511016".to_string()]
        );
        // Socket-only queue "15" must NOT be added to HTTP polling queues
        assert!(!ps.queues.contains_key("15"));
    }

    #[test]
    fn test_parse_chat_message_sticker_json_string() {
        let msg_val = json!({
            "msgId": "stk_1",
            "msgType": "chat.sticker",
            "content": "{\"catId\":1002,\"id\":2005,\"type\":1}",
            "uidFrom": "user_a",
            "ts": 1729508000000_i64
        });
        let parsed = parse_chat_message(&msg_val).expect("must parse sticker");
        assert_eq!(parsed.text, "[Nhãn dán]");
        assert_eq!(parsed.id, "stk_1");
        assert_eq!(parsed.sticker_id, Some(2005));
        assert_eq!(parsed.sticker_cate_id, Some(1002));
        assert_eq!(parsed.sticker_type, Some(1));
        assert_eq!(
            parsed.sticker_image_url().as_deref(),
            Some("https://api.zaloapp.com/api/emoticon/sticker/webpc?eid=2005&size=130")
        );
    }

    #[test]
    fn test_parse_chat_message_integer_msg_types() {
        // Integer msgType = 5 (sticker) with empty content
        let stk = json!({
            "msgId": "stk_int",
            "msgType": 5,
            "content": "",
            "uidFrom": "user_b",
            "ts": 1000_i64
        });
        assert_eq!(parse_chat_message(&stk).unwrap().text, "[Nhãn dán]");

        // Integer msgType = 2 (photo)
        let photo = json!({
            "msgId": "photo_int",
            "msgType": 2,
            "content": "",
            "uidFrom": "user_b",
            "ts": 2000_i64
        });
        assert_eq!(parse_chat_message(&photo).unwrap().text, "[Hình ảnh]");

        // Integer msgType = 3 (voice)
        let voice = json!({
            "msgId": "voice_int",
            "msgType": 3,
            "content": "",
            "uidFrom": "user_b",
            "ts": 3000_i64
        });
        assert_eq!(parse_chat_message(&voice).unwrap().text, "[Tin nhắn thoại]");

        // Integer msgType = 19 (file)
        let file = json!({
            "msgId": "file_int",
            "msgType": 19,
            "content": "",
            "uidFrom": "user_b",
            "ts": 4000_i64
        });
        assert_eq!(parse_chat_message(&file).unwrap().text, "[Tập tin]");

        // Integer msgType = 31 (undo)
        let undo = json!({
            "msgId": "undo_int",
            "msgType": 31,
            "content": "",
            "uidFrom": "user_b",
            "ts": 5000_i64
        });
        assert_eq!(
            parse_chat_message(&undo).unwrap().text,
            "[Tin nhắn đã thu hồi]"
        );
    }

    #[test]
    fn test_parse_chat_message_with_plain_message_string() {
        let msg = json!({
            "msgId": "msg_str",
            "message": "Xin chao buoi sang",
            "uidFrom": "user_c",
            "dName": "Charlie",
            "ts": 1000_i64
        });
        let parsed = parse_chat_message(&msg).unwrap();
        assert_eq!(parsed.text, "Xin chao buoi sang");
        assert_eq!(parsed.sender, "Charlie");
    }

    #[test]
    fn test_parse_thread_group_metadata_does_not_use_contact_activity_time() {
        let val = json!({
            "userId": "grp_test",
            "displayName": "Nhom Chem Gio",
            "isGroup": true,
            "lastActionTime": "1729505000000"
        });
        let thread = parse_thread(&val).unwrap();
        assert!(thread.is_group);
        assert_eq!(thread.name, "Nhom Chem Gio");
        assert!(thread.time.is_empty());
        assert_eq!(thread.sidebar_activity_ts(), 0);
    }

    #[test]
    fn test_deduplicate_threads_preserves_distinct_ids() {
        let old_dtc = Thread { presentation: Default::default(), unread: Default::default(),
            id: "1000000000000000005".into(),
            name: "Đtc".into(),
            preview: "Hi".into(),
            time: "10:21".into(),
            avatar: Some("https://s120-26-ava-talk.zadn.vn/5/00000000000000000000000000000001.jpg?key=abc&time=1797763127".into()),
            is_group: false,
            messages: vec![ChatMessage {
                id: "m1".into(),
                sender: "Đtc".into(),
                text: "Hi".into(),
                time: "10:21".into(),
                is_outgoing: false,
                ts: 1792578088952,
                ..Default::default()
            }],
        };
        let new_dtc = Thread { presentation: Default::default(), unread: Default::default(),
            id: "1000000000000000006".into(),
            name: "Đtc".into(),
            preview: "".into(),
            time: "16:48".into(),
            avatar: Some("https://s160-26-ava-talk.zadn.vn/5/00000000000000000000000000000001.jpg?key=xyz&time=1797876325".into()),
            is_group: false,
            messages: Vec::new(),
        };
        let mut threads = vec![old_dtc, new_dtc];
        deduplicate_threads(&mut threads, None);
        assert_eq!(threads.len(), 2);
        assert_eq!(threads[0].id, "1000000000000000005");
        assert_eq!(threads[0].messages.len(), 1);
        assert_eq!(threads[0].preview, "Hi");
    }

    #[test]
    fn test_merge_group_history_parses_stringified_cloud_data() {
        let mut cloud_thread = Thread {
            presentation: Default::default(),
            unread: Default::default(),
            id: "9000000000000000002".into(),
            name: "Cloud của tôi (Lưu trữ)".into(),
            preview: String::new(),
            time: String::new(),
            avatar: None,
            is_group: false,
            messages: Vec::new(),
        };
        let raw_cloud_resp = json!({
            "error_code": 0,
            "data": r#"{"error":0,"groupMsgs":[{"msgId":"1000000000001","cliMsgId":"1700000000001","msgType":"chat.recommended","uidFrom":"0","idTo":"9000000000000000002","dName":"User Test","ts":"1700000000001","content":{"title":"","description":"Ghi chú đám mây","href":"https://zalo.me/test","action":"recommened.link"}}]}"#
        });
        merge_group_history(&mut cloud_thread, &raw_cloud_resp);
        assert_eq!(cloud_thread.messages.len(), 1);
        assert!(cloud_thread.messages[0].text.contains("Ghi chú đám mây"));
        assert_eq!(
            cloud_thread.messages[0].link_url.as_deref(),
            Some("https://zalo.me/test")
        );
    }

    #[test]
    fn test_strip_vn_for_sticker_kw() {
        assert_eq!(strip_vn_for_sticker_kw("Xin chào"), "xin chao");
        assert_eq!(strip_vn_for_sticker_kw("Cảm ơn nhiều"), "cam on nhieu");
        assert_eq!(strip_vn_for_sticker_kw("Ngủ ngon nhé"), "ngu ngon nhe");
        assert_eq!(strip_vn_for_sticker_kw("Nguyễn Văn An"), "nguyen van an");
        assert_eq!(
            normalize_sticker_keyword("  iu\u{a0}that\u{a0}  "),
            "iu that"
        );

        let q1 = canonical_vn_sticker_queries("Mệt quá");
        assert!(q1.contains(&"mệt quá".to_string()));
        assert!(q1.contains(&"met qua".to_string()));

        let q2 = canonical_vn_sticker_queries("khoc");
        assert!(q2.contains(&"khoc".to_string()));
        assert!(q2.contains(&"khóc".to_string()));
    }

    #[test]
    fn test_parse_friendship_status_response() {
        let r_friend = json!({"error_code": 0, "data": {"is_friend": 1, "is_requested": 0, "is_requesting": 0}});
        assert_eq!(
            parse_friendship_status_response(&r_friend),
            FriendshipStatus::Friend
        );

        let r_inc = json!({"error_code": 0, "data": {"is_friend": 0, "is_requested": 0, "is_requesting": 1}});
        assert_eq!(
            parse_friendship_status_response(&r_inc),
            FriendshipStatus::IncomingPending
        );

        let r_out = json!({"error_code": 0, "data": {"is_friend": 0, "is_requested": 1, "is_requesting": 0}});
        assert_eq!(
            parse_friendship_status_response(&r_out),
            FriendshipStatus::OutgoingPending
        );

        let r_stranger = json!({"error_code": 0, "data": {"is_friend": 0, "is_requested": 0, "is_requesting": 0}});
        assert_eq!(
            parse_friendship_status_response(&r_stranger),
            FriendshipStatus::Stranger
        );
    }

    #[test]
    fn test_prevent_cloud_message_pollution_across_threads() {
        let mut contact = Thread {
            presentation: Default::default(),
            unread: Default::default(),
            id: "1000000000000000009".into(),
            name: "Người dùng Test".into(),
            preview: String::new(),
            time: String::new(),
            avatar: None,
            is_group: false,
            messages: Vec::new(),
        };
        let raw_cloud_resp = json!({
            "error_code": 0,
            "data": {
                "groupMsgs": [{
                    "msgId": "1000000000002",
                    "msgType": "webchat",
                    "uidFrom": "0",
                    "idTo": "9000000000000000001",
                    "dName": "User Test",
                    "ts": "1700000000002",
                    "content": "Ghi chú Cloud của tôi"
                }]
            }
        });
        merge_group_history(&mut contact, &raw_cloud_resp);
        assert!(contact.messages.is_empty());

        let cloud = Thread {
            presentation: Default::default(),
            unread: Default::default(),
            id: "9000000000000000001".into(),
            name: "Cloud của tôi (Lưu trữ)".into(),
            preview: "Ghi chú Cloud của tôi".into(),
            time: "12:00".into(),
            avatar: None,
            is_group: false,
            messages: vec![ChatMessage {
                id: "1000000000002".into(),
                sender: "Tôi".into(),
                text: "Ghi chú Cloud của tôi".into(),
                time: "12:00".into(),
                is_outgoing: true,
                ts: 1700000000002,
                ..Default::default()
            }],
        };
        let polluted_contact = Thread {
            presentation: Default::default(),
            unread: Default::default(),
            id: "1000000000000000009".into(),
            name: "Người dùng Test".into(),
            preview: "Ghi chú Cloud của tôi".into(),
            time: "12:00".into(),
            avatar: None,
            is_group: false,
            messages: vec![ChatMessage {
                id: "1000000000002".into(),
                sender: "Tôi".into(),
                text: "Ghi chú Cloud của tôi".into(),
                time: "12:00".into(),
                is_outgoing: true,
                ts: 1700000000002,
                ..Default::default()
            }],
        };
        let mut list = vec![cloud, polluted_contact];
        deduplicate_threads(&mut list, None);
        assert_eq!(list[0].messages.len(), 1);
        assert_eq!(list[1].messages.len(), 1);
        assert_eq!(list[1].preview, "Ghi chú Cloud của tôi");
    }
}

#[cfg(test)]
mod heavy_merge_regression_tests {
    use super::*;

    fn fixture(count: usize) -> Thread {
        Thread {
            presentation: Default::default(),
            unread: Default::default(),
            id: "fixture-thread".into(),
            name: "Fixture".into(),
            preview: String::new(),
            time: String::new(),
            avatar: None,
            is_group: false,
            messages: (0..count)
                .map(|index| ChatMessage {
                    id: format!("fixture-{index}"),
                    text: format!("message-{index}"),
                    ts: index as i64 + 1,
                    history_source: Some(json!({"body": "fixture"})),
                    ..Default::default()
                })
                .collect(),
        }
    }

    #[test]
    fn repeated_11800_message_merge_is_unchanged_and_preserves_metadata_enrichment() {
        let incoming = fixture(11_800);
        let mut target = fixture(0);
        assert!(merge_thread_into(&mut target, incoming.clone(), true));
        for _ in 0..3 {
            assert!(!merge_thread_into(&mut target, incoming.clone(), true));
            assert_eq!(target.messages.len(), 11_800);
        }
        let mut enriched = incoming.clone();
        enriched.messages[6000].link_url = Some("https://example.invalid/fixture".into());
        enriched.messages[6000].link_title = Some("Fixture title".into());
        enriched.messages[6000].history_source = Some(json!({"body": "enriched"}));
        assert!(merge_thread_into(&mut target, enriched.clone(), true));
        assert_eq!(
            target.messages[6000].link_title.as_deref(),
            Some("Fixture title")
        );
        assert_eq!(
            target.messages[6000].history_source,
            enriched.messages[6000].history_source
        );
        assert!(!merge_thread_into(&mut target, enriched, true));
        assert_eq!(target.messages.len(), 11_800);
    }

    #[test]
    fn exact_id_precedes_unverified_pending_text_match() {
        let mut target = fixture(2);
        target.messages[0].id = "out_pending".into();
        target.messages[0].text = "same".into();
        target.messages[0].time = "12:00".into();
        target.messages[1].id = "server-id".into();
        target.messages[1].text = "different".into();
        let mut incoming = fixture(1);
        incoming.messages[0].id = "server-id".into();
        incoming.messages[0].text = "same".into();
        incoming.messages[0].time = "12:00".into();
        incoming.messages[0].link_title = Some("Enriched earlier pending".into());
        incoming.messages[0].link_url = Some("https://example.invalid/fixture".into());
        merge_thread_into(&mut target, incoming, true);
        assert_eq!(target.messages.len(), 2);
        assert_eq!(target.messages[0].id, "out_pending");
        assert_eq!(
            target.messages[1].link_title.as_deref(),
            Some("Enriched earlier pending")
        );
        assert_eq!(target.messages[1].text, "same");
    }
}

#[cfg(test)]
mod media_metadata_regressions {
    use super::*;

    #[test]
    fn explicit_photo_and_video_types_set_media_source_without_url_changes() {
        for (kind, source) in [
            (json!(2), "Hình ảnh Zalo"),
            (json!("2"), "Hình ảnh Zalo"),
            (json!("chat.photo"), "Hình ảnh Zalo"),
            (json!(4), "Video Zalo"),
        ] {
            let url = "https://example.invalid/media.jxl?token=synthetic%2Bvalue&v=1";
            let message = parse_chat_message(&json!({"msgId":"fixture", "msgType":kind,
                "content":{"href":url,"description":"Tiếng Việt 🧵","thumb":"https://example.invalid/thumb.jpg?q=synthetic%2Fvalue"}})).unwrap();
            assert_eq!(message.link_src.as_deref(), Some(source));
            assert_eq!(message.link_url.as_deref(), Some(url));
            assert_eq!(message.text, "Tiếng Việt 🧵");
            assert_eq!(
                message.link_thumb.as_deref(),
                Some("https://example.invalid/thumb.jpg?q=synthetic%2Fvalue")
            );
        }
    }

    #[test]
    fn cached_phone_types_repair_media_without_changing_source_or_caption() {
        for (kind, expected) in [
            (3, "Hình ảnh Zalo"),
            (4, "Hình ảnh Zalo"),
            (19, "Video Zalo"),
        ] {
            let source = json!({"msgType":kind,"meta":{"attachsList":[{"href":"https://example.invalid/media?x=1%2F2", "thumb":"https://example.invalid/thumb?x=2%2B3"}]}});
            let mut message = ChatMessage {
                text: "[Photo]".into(),
                link_src: Some("example.invalid".into()),
                history_source: Some(source.clone()),
                ..Default::default()
            };
            assert!(message.hydrate_media_metadata());
            assert_eq!(message.link_src.as_deref(), Some(expected));
            assert_eq!(
                message.link_url.as_deref(),
                Some("https://example.invalid/media?x=1%2F2")
            );
            assert_eq!(
                message.link_thumb.as_deref(),
                Some("https://example.invalid/thumb?x=2%2B3")
            );
            assert_eq!(message.text, "[Photo]");
            assert_eq!(message.history_source, Some(source));
            assert!(!message.hydrate_media_metadata());
        }
    }

    #[test]
    fn generic_link_is_not_media_and_empty_thumb_uses_valid_hd() {
        let message = parse_chat_message(&json!({"msgId":"fixture", "msgType":"chat.recommended",
            "content":{"href":"https://example.invalid/article", "thumb":"", "hd":"https://example.invalid/cover.jpg?q=1"}})).unwrap();
        assert_eq!(message.link_src.as_deref(), Some("example.invalid"));
        assert_eq!(
            message.link_thumb.as_deref(),
            Some("https://example.invalid/cover.jpg?q=1")
        );
    }

    #[test]
    fn duplicate_with_existing_url_receives_media_thumb_and_source() {
        let url = "https://example.invalid/media.mp4?token=synthetic";
        let mut thread = Thread {
            presentation: Default::default(),
            unread: Default::default(),
            id: "fixture-peer".into(),
            name: "Fixture".into(),
            preview: String::new(),
            time: String::new(),
            avatar: None,
            is_group: false,
            messages: vec![ChatMessage {
                id: "fixture-id".into(),
                text: "[Video]".into(),
                link_url: Some(url.into()),
                link_src: Some("example.invalid".into()),
                ..Default::default()
            }],
        };
        let mut incoming = thread.clone();
        incoming.messages[0].link_thumb = Some("https://example.invalid/thumb.jpg".into());
        incoming.messages[0].link_src = Some("Video Zalo".into());
        assert!(merge_thread_into(&mut thread, incoming, true));
        assert_eq!(thread.messages.len(), 1);
        assert_eq!(thread.messages[0].link_src.as_deref(), Some("Video Zalo"));
        assert_eq!(
            thread.messages[0].link_thumb.as_deref(),
            Some("https://example.invalid/thumb.jpg")
        );
    }

    #[test]
    fn recalled_message_does_not_regain_media_from_history() {
        let mut message = ChatMessage {
            text: "[Tin nhắn đã thu hồi]".into(),
            history_source: Some(
                json!({"msgType":3,"meta":{"attachsList":[{"href":"https://example.invalid/photo.jpg"}]}}),
            ),
            ..Default::default()
        };
        assert!(!message.hydrate_media_metadata());
        assert!(message.link_url.is_none());
    }
}

/// Result of a preview-only renewal request. Async control delivery stays explicit.
#[cfg(test)]
mod parity_regression_tests {
    use super::*;

    #[test]
    fn parity_message_hour_matches_local_timezone() {
        // Compare with the runtime local timezone, including UTC CI hosts.
        let ts = 1793425680000_i64;
        let msg = parse_chat_message(
            &json!({"msgId":"fixture-time","uidFrom":"peer","content":"Ok","ts":ts}),
        )
        .unwrap();
        let expected = chrono::DateTime::from_timestamp_millis(ts)
            .unwrap()
            .with_timezone(&chrono::Local)
            .format("%H:%M")
            .to_string();
        assert_eq!(
            msg.time, expected,
            "Message time must match the Mac's local time, not raw UTC"
        );
    }

    #[test]
    fn parity_distinct_groups_never_merge_by_name_or_avatar() {
        let mut threads: Vec<Thread> = ["group-a", "group-b"]
            .into_iter()
            .map(|id| Thread {
                presentation: Default::default(),
                unread: Default::default(),
                id: id.into(),
                name: "Same group name".into(),
                preview: String::new(),
                time: String::new(),
                avatar: Some("same-avatar".into()),
                is_group: true,
                messages: vec![ChatMessage {
                    id: format!("message-{id}"),
                    text: id.into(),
                    ts: 1000,
                    ..Default::default()
                }],
            })
            .collect();
        deduplicate_threads(&mut threads, None);
        assert_eq!(
            threads.len(),
            2,
            "Different group IDs are distinct even when names and avatars match"
        );
    }
}

#[cfg(test)]
mod reaction_order_tests {
    use super::*;
    fn event(id: &str, ts: i64, kind: i32, user: &str) -> crate::reactions::ReactionEvent {
        crate::reactions::ReactionEvent {
            event_id: id.into(),
            ts,
            r_type: kind,
            sender_id: user.into(),
            emoji: crate::reactions::emoji_for_type(kind).into(),
            ..Default::default()
        }
    }
    fn count(m: &ChatMessage, kind: i32) -> u64 {
        m.reaction_summaries
            .iter()
            .filter(|s| s.r_type == Some(kind))
            .map(|s| s.count)
            .sum()
    }
    #[test]
    fn newest_then_older_taps_count_and_replay_is_idempotent() {
        let mut m = ChatMessage::default();
        assert!(m.apply_reaction(&event("2", 2000, 5, "a")));
        assert!(m.apply_reaction(&event("1", 1000, 5, "a")));
        assert_eq!(count(&m, 5), 2);
        assert!(!m.apply_reaction(&event("1", 1000, 5, "a")));
        assert_eq!(count(&m, 5), 2);
    }
    #[test]
    fn late_clear_keeps_newer_taps_and_other_reactors() {
        for order in [[0, 1, 2, 3, 4], [4, 3, 2, 1, 0], [3, 0, 4, 1, 2]] {
            let events = [
                event("1", 1000, 5, "a"),
                event("2", 2000, 3, "a"),
                event("3", 3000, -1, "a"),
                event("4", 4000, 5, "a"),
                event("5", 1000, 5, "b"),
            ];
            let mut m = ChatMessage::default();
            for i in order {
                m.apply_reaction(&events[i]);
            }
            assert_eq!(count(&m, 5), 2);
            assert_eq!(count(&m, 3), 0);
            assert_eq!(m.reaction_summaries[0].reactor_counts.get("a"), Some(&1));
            assert_eq!(m.reaction_summaries[0].reactor_counts.get("b"), Some(&1));
        }
    }
    #[test]
    fn zero_ids_are_unknown_and_unknown_type_is_not_heart() {
        let mut m = ChatMessage::default();
        let mut a = event("0", 1000, 5, "a");
        a.client_event_id = "0".into();
        let mut b = a.clone();
        b.ts = 2000;
        assert!(m.apply_reaction(&a));
        assert!(m.apply_reaction(&b));
        assert!(!m.apply_reaction(&a));
        m.apply_reaction(&event("3", 3000, 999, "a"));
        assert_eq!(count(&m, 5), 2);
        assert_eq!(count(&m, 999), 1);
        assert_eq!(
            m.reaction_summaries
                .iter()
                .find(|s| s.r_type == Some(999))
                .unwrap()
                .emoji,
            "◦"
        );
    }
    #[test]
    fn zero_ids_without_timestamp_are_not_a_shared_identity() {
        let mut m = ChatMessage::default();
        let mut unknown = event("0", 0, 5, "a");
        unknown.client_event_id = "0".into();
        assert!(m.apply_reaction(&unknown));
        assert!(m.apply_reaction(&unknown));
        assert_eq!(count(&m, 5), 2);
    }
    #[test]
    fn ledger_merges_union_events_without_snapshot_rewind() {
        let mut a = ChatMessage::default();
        a.apply_reaction(&event("2", 2000, 5, "a"));
        let mut b = ChatMessage::default();
        b.apply_reaction(&event("1", 1000, 5, "a"));
        a.merge_reaction_metadata(&b);
        a.merge_reaction_metadata(&b);
        assert_eq!(count(&a, 5), 2);
        let snapshot = ChatMessage {
            reaction_snapshot: true,
            reaction_summaries: crate::reactions::parse_reactions(
                &json!({"reactions":{"a":{"5":1},"b":{"3":2}}}),
            ),
            ..Default::default()
        };
        a.merge_reaction_metadata(&snapshot);
        assert_eq!(count(&a, 5), 2);
        assert_eq!(count(&a, 3), 2);
    }
    #[test]
    fn absent_metadata_preserves_legacy_reactions() {
        let mut m = ChatMessage {
            reactions: vec!["❤️".into()],
            ..Default::default()
        };
        assert!(!m.merge_reaction_metadata(&ChatMessage::default()));
        assert_eq!(m.reactions, vec!["❤️"]);
        let mut empty = ChatMessage::default();
        assert!(empty.merge_reaction_metadata(&m));
        assert_eq!(empty.reactions, vec!["❤️"]);
        empty.merge_reaction_metadata(&ChatMessage {
            reaction_snapshot: true,
            ..Default::default()
        });
        assert!(empty.reactions.is_empty());
        assert!(!empty.merge_reaction_metadata(&m));
        assert!(empty.reactions.is_empty());
    }
    #[test]
    fn authoritative_snapshot_clears_omitted_untracked_user_only() {
        let mut m = ChatMessage {
            reaction_summaries: crate::reactions::parse_reactions(
                &json!({"reactions":{"b":{"3":2}}}),
            ),
            ..Default::default()
        };
        m.apply_reaction(&event("1", 1000, 5, "a"));
        assert_eq!(count(&m, 3), 2);
        // A partial ledger must not clear a different reactor's aggregate.
        let mut partial = ChatMessage::default();
        partial.apply_reaction(&event("2", 2000, 5, "a"));
        m.merge_reaction_metadata(&partial);
        assert_eq!(count(&m, 3), 2);
        assert_eq!(count(&m, 5), 2);
        // A full snapshot omitting b clears b but must not rewind tracked a.
        let snapshot = ChatMessage {
            reaction_snapshot: true,
            reaction_summaries: crate::reactions::parse_reactions(
                &json!({"reactions":{"a":{"5":1}}}),
            ),
            ..Default::default()
        };
        m.merge_reaction_metadata(&snapshot);
        assert_eq!(count(&m, 3), 0);
        assert_eq!(count(&m, 5), 2);
        m.merge_reaction_metadata(&ChatMessage::default());
        assert_eq!(count(&m, 5), 2);
    }
    #[test]
    fn null_snapshot_does_not_clear_existing_aggregate() {
        let mut m = ChatMessage {
            reaction_summaries: crate::reactions::parse_reactions(
                &json!({"reactions":{"a":{"5":2}}}),
            ),
            ..Default::default()
        };
        let incoming = parse_chat_message(&json!({"msgId":"fixture","uidFrom":"peer",
            "content":"Hello","reacts":null}))
        .unwrap();
        assert!(!incoming.reaction_snapshot);
        m.merge_reaction_metadata(&incoming);
        assert_eq!(count(&m, 5), 2);
    }
    fn actual_snapshot(count: u64) -> ChatMessage {
        parse_chat_message(
            &json!({"msgId":"fixture","uidFrom":"peer","content":"Hello",
            "reacts":{"reactions":if count == 0 { json!({}) } else { json!({"a":{"5":count}}) }}}),
        )
        .unwrap()
    }
    #[test]
    fn authoritative_aggregate_does_not_double_count_old_history() {
        let mut m = actual_snapshot(2);
        assert!(m.reaction_authoritative_snapshot);
        let old = crate::reactions::ReactionEvent {
            historical: true,
            ..event("1", 1000, 5, "a")
        };
        assert!(m.apply_reaction(&old));
        assert_eq!(count(&m, 5), 2);
        assert!(!m.apply_reaction(&old));
        m.apply_reaction(&event("3", 3000, 5, "a"));
        assert_eq!(count(&m, 5), 3);
        let cached = m.clone();
        m.merge_reaction_metadata(&cached);
        assert_eq!(count(&m, 5), 3);
        let mut historical_cache = ChatMessage::default();
        historical_cache.apply_reaction(&crate::reactions::ReactionEvent {
            historical: true,
            ..event("4", 500, 5, "a")
        });
        let mut fresh = actual_snapshot(2);
        fresh.merge_reaction_metadata(&historical_cache);
        assert_eq!(count(&fresh, 5), 2);
        let mut empty = actual_snapshot(0);
        empty.merge_reaction_metadata(&historical_cache);
        assert_eq!(count(&empty, 5), 0);
    }
    #[test]
    fn fresh_snapshot_covers_live_identity_without_replaying_it() {
        let first = event("1", 1000, 5, "a");
        let second = event("2", 2000, 5, "a");
        let mut m = ChatMessage::default();
        m.apply_reaction(&first);
        m.merge_reaction_metadata(&actual_snapshot(1));
        assert_eq!(count(&m, 5), 1);
        assert!(!m.apply_reaction(&first));
        assert_eq!(count(&m, 5), 1);
        m.apply_reaction(&second);
        assert_eq!(count(&m, 5), 2);
        let cached: ChatMessage =
            serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert!(!cached.reaction_snapshot_fresh);
        m.merge_reaction_metadata(&cached);
        assert_eq!(count(&m, 5), 2);
        assert!(!m.apply_reaction(&first));
        assert!(!m.apply_reaction(&second));
        let mut restored = ChatMessage::default();
        restored.merge_reaction_metadata(&cached);
        assert_eq!(count(&restored, 5), 2);
        assert!(!restored.apply_reaction(&first));
        assert!(!restored.apply_reaction(&second));
    }
    #[test]
    fn authoritative_empty_clears_older_history_and_prevents_resurrection() {
        let mut m = ChatMessage::default();
        let old = crate::reactions::ReactionEvent {
            historical: true,
            ..event("1", 1000, 5, "a")
        };
        m.apply_reaction(&old);
        assert_eq!(count(&m, 5), 1);
        m.merge_reaction_metadata(&actual_snapshot(0));
        assert_eq!(count(&m, 5), 0);
        m.apply_reaction(&crate::reactions::ReactionEvent {
            historical: true,
            ..event("2", 2000, 5, "a")
        });
        assert_eq!(count(&m, 5), 0);
        m.apply_reaction(&event("3", 3000, 5, "a"));
        assert_eq!(count(&m, 5), 1);
    }
    #[test]
    fn legacy_unattributed_counts_survive_sender_clear() {
        let mut m = ChatMessage {
            reactions: vec!["❤️".into()],
            ..Default::default()
        };
        m.apply_reaction(&event("1", 1000, 5, "a"));
        m.apply_reaction(&event("2", 2000, -1, "a"));
        assert_eq!(count(&m, 5), 1);
    }
}
