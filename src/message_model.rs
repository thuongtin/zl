//! Lossless wire metadata and conservative message semantics.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    Text,
    Photo,
    Video,
    Voice,
    Sticker,
    File,
    Link,
    Location,
    Contact,
    Gif,
    Recall,
    System,
    Action,
    WebContent,
    Delete,
    #[default]
    Unknown,
}
impl MessageKind {
    /// Original shared-worker-original.js, module 1pet, UTF-8 bytes 1505800..1505954.
    /// Internal UI codes are not archive or live protocol numeric codes.
    pub fn from_original_internal_code(code: i64) -> Self {
        match code {
            1 => Self::Text,
            2 | 11 => Self::Photo,
            3 => Self::Voice,
            4 => Self::Sticker,
            6 => Self::Contact,
            7 => Self::Gif,
            17 => Self::Location,
            18 => Self::Video,
            19 => Self::File,
            20 => Self::Recall,
            _ => Self::Unknown,
        }
    }

    /// Original pc-dist/shared-worker.06caa1ffa528545b8f43.js:
    /// Ai at CP209938..210486 and ji at CP210841..211325. These are archive codes.
    pub fn from_archive_code(code: i64) -> Self {
        match code {
            0 => Self::Text,
            2 | 3 | 4 | 31 => Self::Photo,
            6 => Self::Voice,
            10 => Self::Sticker,
            12 => Self::Contact,
            15 => Self::Action,
            18 => Self::Location,
            19 | 25 => Self::Video,
            22 => Self::File,
            23 => Self::Gif,
            24 | 53 => Self::WebContent,
            33 => Self::Delete,
            // Named Ai variants absent from ji have no verified archive decoder.
            20 | 21 | 26 | 36 => Self::Unknown,
            _ => Self::Unknown,
        }
    }
    pub fn archive_code_name(code: i64) -> Option<&'static str> {
        Some(match code {
            0 => "Text",
            2 => "Doodle",
            3 => "Photo",
            4 => "Photo_V2",
            6 => "Voice",
            10 => "Sticker",
            12 => "Contact",
            15 => "OA",
            18 => "Location",
            19 => "Video",
            20 => "ChatInfo",
            21 => "ECard",
            22 => "File",
            23 => "Gift",
            24 => "Webcontent",
            25 => "VideoLive",
            26 => "Poll",
            31 => "PhotoV2",
            33 => "ChatDelete",
            36 => "Undo",
            53 => "WebcontentV2",
            _ => return None,
        })
    }
    /// Exact ji mapping. Missing decoder names remain opaque, not fabricated wire types.
    pub fn archive_wire_name(code: i64) -> Option<&'static str> {
        Some(match code {
            0 => "webchat",
            2 => "chat.doodle",
            3 | 4 | 31 => "chat.photo",
            6 => "chat.voice",
            10 => "chat.sticker",
            12 => "chat.recommended",
            15 => "chat.list.action",
            18 => "chat.location.new",
            19 => "chat.video.msg",
            22 => "share.file",
            23 => "chat.gif",
            24 => "chat.webcontent",
            25 => "chat.video.live.msg",
            33 => "chat.delete",
            53 => "chat.webcontent.v2",
            _ => return None,
        })
    }

    /// Numeric archive types require their own verified adapter, not wire guesses.
    pub fn from_wire_name(name: &str) -> Self {
        match name {
            "chat.text" | "webchat" | "text" => Self::Text,
            "chat.photo" | "photo" => Self::Photo,
            "chat.video" | "chat.video.msg" | "chat.video.live.msg" | "video" => Self::Video,
            "chat.voice" | "voice" => Self::Voice,
            "chat.sticker" | "sticker" => Self::Sticker,
            "chat.file" | "share.file" | "file" => Self::File,
            "chat.recommended" | "link" => Self::Link,
            "chat.location" | "chat.location.new" | "location" => Self::Location,
            "chat.contact" | "contact" => Self::Contact,
            "chat.gif" | "gif" => Self::Gif,
            "chat.undo" | "undo" => Self::Recall,
            "chat.system" | "system" => Self::System,
            "chat.list.action" => Self::Action,
            "chat.webcontent" | "chat.webcontent.v2" => Self::WebContent,
            "chat.delete" => Self::Delete,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WireMessageRef {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_value"
    )]
    pub server_id: Option<Value>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_value"
    )]
    pub client_id: Option<Value>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_value"
    )]
    pub sender: Option<Value>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_value"
    )]
    pub destination: Option<Value>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_value"
    )]
    pub original_type: Option<Value>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_value"
    )]
    pub cmd: Option<Value>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_value"
    )]
    pub st: Option<Value>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_value"
    )]
    pub at: Option<Value>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_value"
    )]
    pub ts: Option<Value>,
    /// Original keys and values, including attachments, without synthesized defaults.
    pub fields: BTreeMap<String, Value>,
}
impl WireMessageRef {
    pub fn from_raw(raw: &Value) -> Self {
        let pick = |keys: &[&str]| keys.iter().find_map(|k| raw.get(*k).cloned());
        Self {
            server_id: pick(&["msgId", "globalMsgId", "lastMsgId", "id"]),
            client_id: pick(&["cliMsgId", "clientId", "client_message_id"]),
            sender: pick(&["uidFrom", "fromUid", "fromId", "senderId", "sender"]),
            destination: pick(&["idTo", "toId", "toUid", "grid", "groupId"]),
            original_type: pick(&["msgType", "messageType", "type"]),
            cmd: pick(&["cmd"]),
            st: pick(&["st"]),
            at: pick(&["at"]),
            ts: pick(&["ts"]),
            fields: raw
                .as_object()
                .map(|o| o.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                .unwrap_or_default(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UnreadPolicy {
    pub params_ext_present: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_value"
    )]
    pub count_unread: Option<Value>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_value"
    )]
    pub params_ext: Option<Value>,
}
impl UnreadPolicy {
    pub fn from_raw(raw: &Value) -> Self {
        Self {
            params_ext_present: raw.get("paramsExt").is_some(),
            count_unread: raw
                .get("paramsExt")
                .and_then(|p| p.get("countUnread"))
                .cloned(),
            params_ext: raw.get("paramsExt").cloned(),
        }
    }
    /// Shipped JS uses strict numeric equality: paramsExt && countUnread !== 1.
    pub fn counts_unread(&self) -> bool {
        let params_truthy = self.params_ext.as_ref().is_some_and(|v| match v {
            Value::Null => false,
            Value::Bool(b) => *b,
            Value::Number(n) => n.as_f64().is_some_and(|n| n != 0.0),
            Value::String(s) => !s.is_empty(),
            _ => true,
        });
        !self.params_ext_present
            || !params_truthy
            || self.count_unread.as_ref().and_then(Value::as_f64) == Some(1.0)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryState {
    #[default]
    Unknown,
    Pending,
    ServerAccepted,
    Delivered,
    Read,
    Failed,
    DeliveryUnknown,
}
impl DeliveryState {
    /// Acknowledged progress never regresses; uncertainty and errors do not imply delivery.
    pub fn merge(self, incoming: Self) -> Self {
        use DeliveryState::*;
        let rank = |s| match s {
            Unknown => 0,
            Pending => 1,
            DeliveryUnknown => 2,
            Failed => 3,
            ServerAccepted => 4,
            Delivered => 5,
            Read => 6,
        };
        if rank(incoming) > rank(self) {
            incoming
        } else {
            self
        }
    }
}

fn deserialize_present_value<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

#[cfg(test)]
mod archive_code_tests {
    use super::*;
    #[test]
    fn archive_codes_follow_source_enum_and_decoder_without_internal_code_confusion() {
        for (code, kind, wire) in [
            (15, MessageKind::Action, "chat.list.action"),
            (24, MessageKind::WebContent, "chat.webcontent"),
            (31, MessageKind::Photo, "chat.photo"),
            (33, MessageKind::Delete, "chat.delete"),
            (53, MessageKind::WebContent, "chat.webcontent.v2"),
            (12, MessageKind::Contact, "chat.recommended"),
        ] {
            assert_eq!(MessageKind::from_archive_code(code), kind);
            assert_eq!(MessageKind::archive_wire_name(code), Some(wire));
        }
        for (code, name) in [(20, "ChatInfo"), (21, "ECard"), (26, "Poll"), (36, "Undo")] {
            assert_eq!(MessageKind::from_archive_code(code), MessageKind::Unknown);
            assert_eq!(MessageKind::archive_code_name(code), Some(name));
            assert_eq!(MessageKind::archive_wire_name(code), None);
        }
        for code in [29, 32, 35] {
            assert_eq!(MessageKind::from_archive_code(code), MessageKind::Unknown);
            assert_eq!(MessageKind::archive_code_name(code), None);
            assert_eq!(MessageKind::archive_wire_name(code), None);
        }
        assert_ne!(MessageKind::from_archive_code(33), MessageKind::Recall);
    }
}
