//! Local message and thread store backed by SQLite (rusqlite bundled).
//!
//! Persists every message seen via polling or group history so chat history accumulates locally.

use anyhow::Result;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::chat::{ChatMessage, Thread};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StoredMessage {
    pub msg_id: String,
    pub thread_id: String,
    pub sender_id: String,
    pub sender_name: String,
    pub text: String,
    pub msg_type: String,
    pub ts: i64,
    pub is_outgoing: bool,
    pub raw_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StoredThread {
    pub thread_id: String,
    pub name: String,
    pub is_group: bool,
    pub last_ts: i64,
}

pub struct MessageStore {
    pub conn: Connection,
}

impl MessageStore {
    /// Messages per thread loaded at startup / on refresh. The list only needs the newest one
    /// for ordering and preview; opening a thread reads deeper (`LoadGroupHistory`).
    pub const PRELOAD_MSGS: usize = 20;

    /// Open SQLite database at `path` and initialize table schemas and indices if absent.
    pub fn open(path: &str) -> Result<Self> {
        let conn = if path == ":memory:" {
            Connection::open_in_memory()?
        } else {
            Connection::open(path)?
        };

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS messages (
                msg_id TEXT PRIMARY KEY,
                thread_id TEXT NOT NULL,
                sender_id TEXT,
                sender_name TEXT,
                text TEXT,
                msg_type TEXT,
                ts INTEGER,
                is_outgoing INTEGER,
                raw_json TEXT
            );
            CREATE TABLE IF NOT EXISTS thread_unread (thread_id TEXT PRIMARY KEY, state_json TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS idx_thread_ts ON messages(thread_id, ts);
            CREATE TABLE IF NOT EXISTS threads (
                thread_id TEXT PRIMARY KEY,
                name TEXT,
                is_group INTEGER,
                last_ts INTEGER
            );",
        )?;
        // Migration 1: one-time cleanup of placeholder rows. This DELETE used to run (a full
        // table scan) on every hydrate; new rows are filtered in `persist_threads` instead.
        if conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))? < 1 {
            conn.execute_batch(
                "BEGIN;
                 DELETE FROM messages WHERE text = 'sendBubbleMessage';
                 PRAGMA user_version = 1;
                 COMMIT;",
            )?;
        }

        Ok(Self { conn })
    }

    /// Insert a message. Returns `true` if a new row was inserted, `false` if already existed.
    pub fn upsert_message(&self, msg: &StoredMessage) -> Result<bool> {
        let rows = self.conn.execute(
            "INSERT OR IGNORE INTO messages (msg_id, thread_id, sender_id, sender_name, text, msg_type, ts, is_outgoing, raw_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                msg.msg_id,
                msg.thread_id,
                msg.sender_id,
                msg.sender_name,
                msg.text,
                msg.msg_type,
                msg.ts,
                if msg.is_outgoing { 1 } else { 0 },
                msg.raw_json,
            ],
        )?;
        if rows == 0 && !msg.raw_json.is_empty() {
            self.conn.execute(
                "UPDATE messages SET raw_json = ?1, msg_type = ?2, text = ?4 WHERE msg_id = ?3",
                params![msg.raw_json, msg.msg_type, msg.msg_id, msg.text],
            )?;
        }
        Ok(rows > 0)
    }

    /// Delete a message from local SQLite store when user selects "Xóa chỉ ở phía tôi".
    pub fn delete_message(&mut self, thread_id: &str, msg_id: &str, ts: i64) -> Result<()> {
        let _ = ts; // Timestamp is not a message identity.
        if !thread_id.is_empty() && !msg_id.is_empty() {
            self.conn.execute(
                "DELETE FROM messages WHERE thread_id = ?1 AND msg_id = ?2",
                params![thread_id, msg_id],
            )?;
        }
        Ok(())
    }

    /// Insert or update thread metadata.
    pub fn upsert_thread(&self, t: &StoredThread) -> Result<()> {
        self.conn.execute(
            "INSERT INTO threads (thread_id, name, is_group, last_ts)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(thread_id) DO UPDATE SET
                 name = excluded.name,
                 is_group = excluded.is_group,
                 last_ts = excluded.last_ts",
            params![
                t.thread_id,
                t.name,
                if t.is_group { 1 } else { 0 },
                t.last_ts,
            ],
        )?;
        Ok(())
    }

    /// Retrieve the newest `limit` messages for a thread, ordered by timestamp ascending.
    pub fn messages_for(&self, thread_id: &str, limit: usize) -> Result<Vec<StoredMessage>> {
        let mut stmt = self.conn.prepare(
            // Newest `limit` rows, returned oldest-first. `ORDER BY ts ASC LIMIT` handed back the
            // *oldest* rows, so a thread longer than the limit reopened on stale messages.
            "SELECT msg_id, thread_id, sender_id, sender_name, text, msg_type, ts, is_outgoing, raw_json
             FROM messages
             WHERE thread_id = ?1
             ORDER BY ts DESC, msg_id DESC
             LIMIT ?2",
        )?;

        let rows = stmt.query_map(params![thread_id, limit as i64], |row| {
            let is_outgoing_num: i64 = row.get(7)?;
            Ok(StoredMessage {
                msg_id: row.get(0)?,
                thread_id: row.get(1)?,
                sender_id: row.get(2).unwrap_or_default(),
                sender_name: row.get(3).unwrap_or_default(),
                text: row.get(4).unwrap_or_default(),
                msg_type: row.get(5).unwrap_or_default(),
                ts: row.get(6)?,
                is_outgoing: is_outgoing_num != 0,
                raw_json: row.get(8).unwrap_or_default(),
            })
        })?;

        let mut list = Vec::new();
        for r in rows {
            list.push(r?);
        }
        list.reverse();
        Ok(list)
    }

    /// Retrieve all persisted thread metadata ordered by `last_ts` descending.
    pub fn all_threads(&self) -> Result<Vec<StoredThread>> {
        let mut stmt = self.conn.prepare(
            "SELECT thread_id, name, is_group, last_ts
             FROM threads
             ORDER BY last_ts DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            let is_group_num: i64 = row.get(2).unwrap_or(0);
            Ok(StoredThread {
                thread_id: row.get(0)?,
                name: row.get(1).unwrap_or_default(),
                is_group: is_group_num != 0,
                last_ts: row.get(3).unwrap_or(0),
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Hydrate `threads` with persisted threads and message history from SQLite.
    /// Returns the total number of messages loaded into `threads`.
    pub fn hydrate_threads(
        &self,
        threads: &mut Vec<Thread>,
        limit_per_thread: usize,
    ) -> Result<usize> {
        crate::chat::deduplicate_threads(threads, None);

        let stored_threads = self.all_threads().unwrap_or_default();
        for st in stored_threads {
            if st.thread_id.is_empty() || st.thread_id == "0" {
                continue;
            }
            if let Some(existing) = threads.iter_mut().find(|t| t.id == st.thread_id) {
                if crate::chat::is_generic_thread_name(&existing.name) && !st.name.is_empty() {
                    existing.name = st.name;
                }
                existing.is_group = existing.is_group || st.is_group;
            } else {
                threads.push(Thread {
                    presentation: Default::default(),
                    unread: Default::default(),
                    id: st.thread_id.clone(),
                    name: if st.name.is_empty() {
                        format!("Người dùng {}", st.thread_id)
                    } else {
                        st.name
                    },
                    preview: String::new(),
                    time: String::new(),
                    avatar: None,
                    is_group: st.is_group,
                    messages: Vec::new(),
                });
            }
        }

        // Primary-key lookups for the messages actually in memory. This used to load every
        // (msg_id, thread_id) row in the database into a HashMap on each call. One transaction
        // around them: in autocommit every SELECT takes and drops the file lock, ~50 µs on
        // Windows, which made 62k lookups cost 3 s.
        let tx = self.conn.unchecked_transaction()?;
        let mut owner_of = self
            .conn
            .prepare("SELECT thread_id FROM messages WHERE msg_id = ?1")?;

        let mut loaded_count = 0;
        for t in threads.iter_mut() {
            let before = t.messages.len();
            let tid = t.id.clone();
            t.messages.retain(|m| {
                owner_of
                    .query_row(params![m.id], |r| r.get::<_, String>(0))
                    .map_or(true, |owner| owner == tid)
            });
            if t.messages.len() != before && t.messages.is_empty() {
                t.preview.clear();
                t.time.clear();
                let _ = self.conn.execute(
                    "UPDATE threads SET last_ts = 0 WHERE thread_id = ?1",
                    params![t.id],
                );
            }
            if t.unread.last_seen_id.is_none() {
                if let Ok(raw) = self.conn.query_row(
                    "SELECT state_json FROM thread_unread WHERE thread_id=?1",
                    params![t.id],
                    |row| row.get::<_, String>(0),
                ) {
                    if let Ok(state) = serde_json::from_str(&raw) {
                        t.unread = state;
                    }
                }
            }
            let saved_msgs = self.messages_for(&t.id, limit_per_thread)?;
            for sm in saved_msgs {
                let time = crate::clock::message_time(sm.ts);
                let mut sticker_id = None;
                let mut sticker_cate_id = None;
                let mut sticker_type = None;
                let mut reply_to_id = None;
                let mut reply_to_sender = None;
                let mut reply_to_text = None;
                let mut reactions = Vec::new();
                let mut reaction_summaries = Vec::new();
                let mut reaction_snapshot = false;
                let mut reaction_authoritative_snapshot = false;
                let mut reaction_versions = Default::default();
                let mut client_message_id = String::new();
                let mut is_pinned = false;
                let mut is_starred = false;
                let mut link_url = None;
                let mut link_title = None;
                let mut link_desc = None;
                let mut link_thumb = None;
                let mut link_src = None;
                let mut history_source = None;
                let mut kind = Default::default();
                let mut wire_ref = Default::default();
                let mut unread_policy = Default::default();
                let mut delivery = Default::default();
                let mut reaction_snapshot_provenance = Default::default();
                if !sm.raw_json.is_empty() {
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(&sm.raw_json) {
                        history_source = val.get("history_source").cloned();
                        kind = val
                            .get("kind")
                            .and_then(|v| serde_json::from_value(v.clone()).ok())
                            .unwrap_or_default();
                        wire_ref = val
                            .get("wire_ref")
                            .and_then(|v| serde_json::from_value(v.clone()).ok())
                            .unwrap_or_default();
                        unread_policy = val
                            .get("unread_policy")
                            .and_then(|v| serde_json::from_value(v.clone()).ok())
                            .unwrap_or_default();
                        delivery = val
                            .get("delivery")
                            .and_then(|v| serde_json::from_value(v.clone()).ok())
                            .unwrap_or_default();
                        reaction_snapshot_provenance = val
                            .get("reaction_snapshot_provenance")
                            .and_then(|v| serde_json::from_value(v.clone()).ok())
                            .unwrap_or_default();
                        sticker_id = val
                            .get("stickerId")
                            .or_else(|| val.get("id"))
                            .and_then(|v| v.as_u64());
                        sticker_cate_id = val
                            .get("cateId")
                            .or_else(|| val.get("catId"))
                            .and_then(|v| v.as_u64());
                        sticker_type = val.get("type").and_then(|v| v.as_u64()).map(|n| n as u32);
                        reply_to_id = val
                            .get("reply_to_id")
                            .and_then(|v| v.as_str())
                            .map(str::to_string);
                        reply_to_sender = val
                            .get("reply_to_sender")
                            .and_then(|v| v.as_str())
                            .map(str::to_string);
                        reply_to_text = val
                            .get("reply_to_text")
                            .and_then(|v| v.as_str())
                            .map(str::to_string);
                        reaction_summaries = val
                            .get("reaction_summaries")
                            .and_then(|v| serde_json::from_value(v.clone()).ok())
                            .unwrap_or_default();
                        reaction_snapshot = val
                            .get("reaction_snapshot")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false);
                        reaction_authoritative_snapshot = val
                            .get("reaction_authoritative_snapshot")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false);
                        reaction_versions = val
                            .get("reaction_versions")
                            .and_then(|v| serde_json::from_value(v.clone()).ok())
                            .unwrap_or_default();
                        client_message_id = val
                            .get("client_message_id")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        if let Some(arr) = val.get("reactions").and_then(|v| v.as_array()) {
                            reactions = arr
                                .iter()
                                .filter_map(|v| v.as_str().map(str::to_string))
                                .collect();
                        }
                        is_pinned = val
                            .get("is_pinned")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        is_starred = val
                            .get("is_starred")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        link_url = val
                            .get("link_url")
                            .and_then(|v| v.as_str())
                            .map(str::to_string);
                        link_title = val
                            .get("link_title")
                            .and_then(|v| v.as_str())
                            .map(str::to_string);
                        link_desc = val
                            .get("link_desc")
                            .and_then(|v| v.as_str())
                            .map(str::to_string);
                        link_thumb = val
                            .get("link_thumb")
                            .and_then(|v| v.as_str())
                            .map(str::to_string);
                        link_src = val
                            .get("link_src")
                            .and_then(|v| v.as_str())
                            .map(str::to_string);
                    }
                }
                let cm = ChatMessage {
                    id: sm.msg_id,
                    sender: if !sm.sender_name.is_empty() {
                        sm.sender_name
                    } else {
                        sm.sender_id
                    },
                    text: sm.text,
                    time,
                    is_outgoing: sm.is_outgoing,
                    ts: sm.ts,
                    sticker_id,
                    sticker_cate_id,
                    sticker_type,
                    reply_to_id,
                    reply_to_sender,
                    reply_to_text,
                    reactions,
                    reaction_summaries,
                    reaction_snapshot,
                    reaction_authoritative_snapshot,
                    reaction_snapshot_fresh: false,
                    reaction_versions,
                    client_message_id,
                    is_pinned,
                    is_starred,
                    link_url,
                    link_title,
                    link_desc,
                    link_thumb,
                    link_src,
                    history_source,
                    kind,
                    wire_ref,
                    unread_policy,
                    delivery,
                    reaction_snapshot_provenance,
                    ..Default::default()
                };
                if let Some(existing) = t
                    .messages
                    .iter_mut()
                    .find(|existing| !existing.id.is_empty() && existing.id == cm.id)
                {
                    existing.merge_reaction_metadata(&cm);
                    if cm.history_source.is_some() {
                        existing.history_source = cm.history_source.clone();
                    }
                    if existing.sticker_id.is_none() && cm.sticker_id.is_some() {
                        existing.sticker_id = cm.sticker_id;
                        existing.sticker_cate_id = cm.sticker_cate_id;
                        existing.sticker_type = cm.sticker_type;
                    }
                    if existing.reply_to_text.is_none() && cm.reply_to_text.is_some() {
                        existing.reply_to_id = cm.reply_to_id;
                        existing.reply_to_sender = cm.reply_to_sender;
                        existing.reply_to_text = cm.reply_to_text;
                    }
                    if existing.link_url.is_none() && cm.link_url.is_some() {
                        existing.link_url = cm.link_url;
                        existing.link_title = cm.link_title;
                        existing.link_desc = cm.link_desc;
                        existing.link_thumb = cm.link_thumb;
                        existing.link_src = cm.link_src;
                        if !cm.text.is_empty() {
                            existing.text = cm.text;
                        }
                    }
                    if existing.reactions.is_empty() && !cm.reactions.is_empty() {
                        existing.reactions = cm.reactions;
                    }
                    existing.is_pinned = existing.is_pinned || cm.is_pinned;
                    existing.is_starred = existing.is_starred || cm.is_starred;
                } else {
                    t.messages.push(cm);
                    loaded_count += 1;
                }
            }
            if !t.messages.is_empty() {
                t.messages.sort_by_key(|m| m.ts);
                if let Some(newest) = t.messages.last() {
                    if !newest.text.is_empty() {
                        t.preview = newest.text.replace('\n', " ");
                        t.time = newest.time.clone();
                    }
                }
            }
        }
        drop(owner_of);
        tx.commit()?;
        crate::chat::deduplicate_threads(threads, None);
        Ok(loaded_count)
    }
}

impl From<&ChatMessage> for StoredMessage {
    fn from(m: &ChatMessage) -> Self {
        let mut meta = serde_json::Map::new();
        if m.kind != Default::default() {
            meta.insert("kind".into(), serde_json::json!(m.kind));
        }
        if m.wire_ref != Default::default() {
            meta.insert("wire_ref".into(), serde_json::json!(m.wire_ref));
        }
        if m.unread_policy != Default::default() {
            meta.insert("unread_policy".into(), serde_json::json!(m.unread_policy));
        }
        if m.delivery != Default::default() {
            meta.insert("delivery".into(), serde_json::json!(m.delivery));
        }
        if m.reaction_snapshot_provenance != Default::default() {
            meta.insert(
                "reaction_snapshot_provenance".into(),
                serde_json::json!(m.reaction_snapshot_provenance),
            );
        }

        if let Some(source) = &m.history_source {
            meta.insert("history_source".into(), source.clone());
        }
        let msg_type = if let Some(sid) = m.sticker_id {
            let cid = m.sticker_cate_id.unwrap_or(0);
            let st = m.sticker_type.unwrap_or(7);
            meta.insert("stickerId".into(), serde_json::json!(sid));
            meta.insert("cateId".into(), serde_json::json!(cid));
            meta.insert("type".into(), serde_json::json!(st));
            "chat.sticker".to_string()
        } else {
            "chat.text".to_string()
        };
        if let Some(ref rid) = m.reply_to_id {
            meta.insert("reply_to_id".into(), serde_json::json!(rid));
        }
        if let Some(ref rsender) = m.reply_to_sender {
            meta.insert("reply_to_sender".into(), serde_json::json!(rsender));
        }
        if let Some(ref rtext) = m.reply_to_text {
            meta.insert("reply_to_text".into(), serde_json::json!(rtext));
        }
        if !m.reaction_summaries.is_empty() || m.reaction_snapshot {
            meta.insert(
                "reaction_summaries".into(),
                serde_json::json!(m.reaction_summaries),
            );
            meta.insert(
                "reaction_snapshot".into(),
                serde_json::json!(m.reaction_snapshot),
            );
        }
        if m.reaction_authoritative_snapshot {
            meta.insert(
                "reaction_authoritative_snapshot".into(),
                serde_json::json!(true),
            );
        }
        if !m.reaction_versions.is_empty() {
            meta.insert(
                "reaction_versions".into(),
                serde_json::json!(m.reaction_versions),
            );
        }
        if !m.client_message_id.is_empty() {
            meta.insert(
                "client_message_id".into(),
                serde_json::json!(m.client_message_id),
            );
        }
        if !m.reactions.is_empty() {
            meta.insert("reactions".into(), serde_json::json!(m.reactions));
        }
        if m.is_pinned {
            meta.insert("is_pinned".into(), serde_json::json!(true));
        }
        if m.is_starred {
            meta.insert("is_starred".into(), serde_json::json!(true));
        }
        if let Some(lurl) = &m.link_url {
            meta.insert("link_url".into(), serde_json::json!(lurl));
        }
        if let Some(ltitle) = &m.link_title {
            meta.insert("link_title".into(), serde_json::json!(ltitle));
        }
        if let Some(ldesc) = &m.link_desc {
            meta.insert("link_desc".into(), serde_json::json!(ldesc));
        }
        if let Some(lthumb) = &m.link_thumb {
            meta.insert("link_thumb".into(), serde_json::json!(lthumb));
        }
        if let Some(lsrc) = &m.link_src {
            meta.insert("link_src".into(), serde_json::json!(lsrc));
        }
        let raw_json = if meta.is_empty() {
            String::new()
        } else {
            serde_json::Value::Object(meta).to_string()
        };
        Self {
            msg_id: if !m.id.is_empty() {
                m.id.clone()
            } else {
                format!("msg_{}", m.ts)
            },
            thread_id: String::new(),
            sender_id: if m.is_outgoing {
                "0".into()
            } else {
                m.sender.clone()
            },
            sender_name: m.sender.clone(),
            text: m.text.clone(),
            msg_type,
            ts: m.ts,
            is_outgoing: m.is_outgoing,
            raw_json,
        }
    }
}

/// Persist threads and their messages to SQLite store, returning the count of newly inserted messages.
pub fn persist_threads(store: &mut MessageStore, threads: &[Thread]) -> Result<usize> {
    // One atomic snapshot avoids a filesystem sync per message.
    let transaction = store.conn.unchecked_transaction()?;
    let count = persist_threads_snapshot(store, threads)?;
    transaction.commit()?;
    Ok(count)
}

fn persist_threads_snapshot(store: &MessageStore, threads: &[Thread]) -> Result<usize> {
    let mut new_count = 0;
    for t in threads {
        let last_ts = t.messages.iter().map(|m| m.ts).max().unwrap_or(0);
        let st = StoredThread {
            thread_id: t.id.clone(),
            name: t.name.clone(),
            is_group: t.is_group,
            last_ts,
        };
        store.upsert_thread(&st)?;
        store.conn.execute("INSERT INTO thread_unread(thread_id,state_json) VALUES(?1,?2) ON CONFLICT(thread_id) DO UPDATE SET state_json=excluded.state_json",
            params![t.id,serde_json::to_string(&t.unread)?])?;

        for m in t.messages.iter().filter(|m| m.text != "sendBubbleMessage") {
            let mut sm = StoredMessage::from(m);
            sm.thread_id = t.id.clone();
            if sm.msg_id.is_empty() {
                sm.msg_id = format!("{}_{}", t.id, sm.ts);
            }
            if store.upsert_message(&sm)? {
                new_count += 1;
            }
        }
    }
    Ok(new_count)
}

/// Convenience helper: open `zalo_history.db` (or `path`) and hydrate `threads` with saved history.
pub fn load_history_into_threads(path: &str, threads: &mut Vec<Thread>, limit_per_thread: usize) {
    if let Ok(store) = MessageStore::open(path) {
        let _ = store.hydrate_threads(threads, limit_per_thread);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disposable_path(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "zl-{label}-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn newest_bounded_window_is_latest_500_ascending() {
        let mut store = MessageStore::open(":memory:").unwrap();
        persist_threads(&mut store, &[snapshot_fixture(601)]).unwrap();
        let messages = store.messages_for("snapshot-thread", 500).unwrap();
        assert_eq!(messages.len(), 500);
        assert_eq!(messages[0].msg_id, "snapshot-101");
        assert_eq!(messages[499].msg_id, "snapshot-600");
        assert!(messages.windows(2).all(|pair| pair[0].ts < pair[1].ts));
    }

    #[test]
    fn identical_group_names_and_distinct_message_ids_survive_reopen() {
        let path = disposable_path("group-identity");
        let mut store = MessageStore::open(path.to_str().unwrap()).unwrap();
        let mut a = snapshot_fixture(2);
        a.id = "group-a".into();
        a.is_group = true;
        a.name = "Same group".into();
        for m in &mut a.messages {
            m.text = "Identical".into();
            m.ts = 1000;
        }
        let mut b = a.clone();
        b.id = "group-b".into();
        for m in &mut b.messages {
            m.id = format!("other-{}", m.id);
        }
        persist_threads(&mut store, &[a, b]).unwrap();
        drop(store);
        let store = MessageStore::open(path.to_str().unwrap()).unwrap();
        let mut threads = Vec::new();
        store.hydrate_threads(&mut threads, 500).unwrap();
        assert_eq!(threads.len(), 2);
        for t in &threads {
            assert!(t.is_group);
            assert_eq!(t.name, "Same group");
            assert_eq!(t.messages.len(), 2);
            assert_ne!(t.messages[0].id, t.messages[1].id);
        }
        drop(store);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn authoritative_reaction_baseline_and_empty_tombstone_survive_reopen() {
        let path = disposable_path("reaction-authoritative");
        let mut store = MessageStore::open(path.to_str().unwrap()).unwrap();
        let mut thread = snapshot_fixture(1);
        thread.messages[0].reaction_authoritative_snapshot = true;
        thread.messages[0].reaction_snapshot = true;
        thread.messages[0].reaction_summaries =
            crate::reactions::parse_reactions(&serde_json::json!({"reactions":{"a":{"5":2}}}));
        let old = crate::reactions::ReactionEvent {
            sender_id: "a".into(),
            event_id: "1".into(),
            historical: true,
            r_type: 5,
            emoji: "❤️".into(),
            ts: 1000,
            ..Default::default()
        };
        thread.messages[0].apply_reaction(&old);
        persist_threads(&mut store, &[thread]).unwrap();
        drop(store);
        let mut store = MessageStore::open(path.to_str().unwrap()).unwrap();
        let mut threads = Vec::new();
        store.hydrate_threads(&mut threads, 500).unwrap();
        let m = &mut threads[0].messages[0];
        assert!(m.reaction_authoritative_snapshot);
        assert_eq!(m.reaction_summaries[0].count, 2);
        assert!(!m.apply_reaction(&old));
        m.merge_reaction_metadata(&ChatMessage {
            reaction_snapshot: true,
            reaction_authoritative_snapshot: true,
            reaction_snapshot_fresh: true,
            ..Default::default()
        });
        persist_threads(&mut store, &threads).unwrap();
        drop(store);
        let store = MessageStore::open(path.to_str().unwrap()).unwrap();
        let mut threads = Vec::new();
        store.hydrate_threads(&mut threads, 500).unwrap();
        let m = &mut threads[0].messages[0];
        assert!(m.reaction_authoritative_snapshot);
        m.apply_reaction(&crate::reactions::ReactionEvent {
            event_id: "2".into(),
            ts: 2000,
            ..old
        });
        assert!(m.reaction_summaries.is_empty());
        assert!(m.reactions.is_empty());
        drop(store);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn reaction_ledger_replay_after_database_reopen_is_idempotent() {
        let path = disposable_path("reaction-ledger");
        let mut store = MessageStore::open(path.to_str().unwrap()).unwrap();
        let mut thread = snapshot_fixture(1);
        let newest = crate::reactions::ReactionEvent {
            sender_id: "a".into(),
            event_id: "2".into(),
            r_type: 5,
            emoji: "❤️".into(),
            ts: 2000,
            ..Default::default()
        };
        let older = crate::reactions::ReactionEvent {
            event_id: "1".into(),
            ts: 1000,
            ..newest.clone()
        };
        thread.messages[0].apply_reaction(&newest);
        thread.messages[0].apply_reaction(&older);
        persist_threads(&mut store, &[thread]).unwrap();
        drop(store);
        let mut store = MessageStore::open(path.to_str().unwrap()).unwrap();
        let mut threads = Vec::new();
        store.hydrate_threads(&mut threads, 500).unwrap();
        let m = &mut threads[0].messages[0];
        assert_eq!(m.reaction_summaries[0].count, 2);
        assert!(!m.apply_reaction(&newest));
        assert!(!m.apply_reaction(&older));
        assert_eq!(m.reaction_summaries[0].reactor_counts["a"], 2);
        assert!(m.apply_reaction(&crate::reactions::ReactionEvent {
            event_id: "3".into(),
            ts: 1500,
            r_type: -1,
            ..older.clone()
        }));
        assert_eq!(m.reaction_summaries[0].count, 1);
        persist_threads(&mut store, &threads).unwrap();
        drop(store);
        let store = MessageStore::open(path.to_str().unwrap()).unwrap();
        let mut threads = Vec::new();
        store.hydrate_threads(&mut threads, 500).unwrap();
        assert!(!threads[0].messages[0].apply_reaction(&older));
        assert_eq!(threads[0].messages[0].reaction_summaries[0].count, 1);
        drop(store);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn test_insert_twice_returns_false() {
        let store = MessageStore::open(":memory:").unwrap();
        let msg = StoredMessage {
            msg_id: "m_1001".into(),
            thread_id: "thread_1".into(),
            sender_id: "u_1".into(),
            sender_name: "Alice".into(),
            text: "Xin chao".into(),
            msg_type: "chat.text".into(),
            ts: 1729500000000,
            is_outgoing: false,
            raw_json: "".into(),
        };

        assert!(store.upsert_message(&msg).unwrap());
        assert!(!store.upsert_message(&msg).unwrap());
    }

    #[test]
    fn test_messages_for_returns_ordered_rows() {
        let store = MessageStore::open(":memory:").unwrap();
        let thread_id = "thread_order_test";

        let m1 = StoredMessage {
            msg_id: "m1".into(),
            thread_id: thread_id.into(),
            sender_id: "u1".into(),
            sender_name: "User".into(),
            text: "Msg 200".into(),
            msg_type: "chat.text".into(),
            ts: 200,
            is_outgoing: false,
            raw_json: "".into(),
        };
        let m2 = StoredMessage {
            msg_id: "m2".into(),
            thread_id: thread_id.into(),
            sender_id: "u1".into(),
            sender_name: "User".into(),
            text: "Msg 100".into(),
            msg_type: "chat.text".into(),
            ts: 100,
            is_outgoing: false,
            raw_json: "".into(),
        };
        let m3 = StoredMessage {
            msg_id: "m3".into(),
            thread_id: thread_id.into(),
            sender_id: "u1".into(),
            sender_name: "User".into(),
            text: "Msg 300".into(),
            msg_type: "chat.text".into(),
            ts: 300,
            is_outgoing: false,
            raw_json: "".into(),
        };

        store.upsert_message(&m1).unwrap();
        store.upsert_message(&m2).unwrap();
        store.upsert_message(&m3).unwrap();

        let msgs = store.messages_for(thread_id, 10).unwrap();
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0].ts, 100);
        assert_eq!(msgs[0].msg_id, "m2");
        assert_eq!(msgs[1].ts, 200);
        assert_eq!(msgs[1].msg_id, "m1");
        assert_eq!(msgs[2].ts, 300);
        assert_eq!(msgs[2].msg_id, "m3");

        // A limit keeps the *newest* rows, still returned oldest-first.
        let newest = store.messages_for(thread_id, 2).unwrap();
        assert_eq!(
            newest.iter().map(|m| m.ts).collect::<Vec<_>>(),
            vec![200, 300]
        );
    }

    fn snapshot_fixture(count: usize) -> Thread {
        Thread {
            presentation: Default::default(),
            unread: Default::default(),
            id: "snapshot-thread".into(),
            name: "Fixture".into(),
            messages: (0..count)
                .map(|i| ChatMessage {
                    id: format!("snapshot-{i}"),
                    text: format!("Fixture {i}"),
                    ts: i as i64 + 1,
                    ..Default::default()
                })
                .collect(),
            preview: String::new(),
            time: String::new(),
            avatar: None,
            is_group: false,
        }
    }

    #[test]
    fn snapshot_rolls_back_on_insert_and_update_failure() {
        let mut store = MessageStore::open(":memory:").unwrap();
        store.conn.execute_batch("CREATE TRIGGER reject_insert BEFORE INSERT ON messages WHEN NEW.msg_id = 'snapshot-1' BEGIN SELECT RAISE(ABORT, 'fixture failure'); END;").unwrap();
        assert!(persist_threads(&mut store, &[snapshot_fixture(2)]).is_err());
        assert!(store.all_threads().unwrap().is_empty());
        assert!(store
            .messages_for("snapshot-thread", 10)
            .unwrap()
            .is_empty());
        store
            .conn
            .execute_batch("DROP TRIGGER reject_insert;")
            .unwrap();
        assert_eq!(
            persist_threads(&mut store, &[snapshot_fixture(2)]).unwrap(),
            2
        );
        store.conn.execute_batch("CREATE TRIGGER reject_update BEFORE UPDATE ON messages BEGIN SELECT RAISE(ABORT, 'fixture update failure'); END;").unwrap();
        let mut changed = snapshot_fixture(2);
        changed.name = "Changed".into();
        changed.messages[0].text = "Changed".into();
        changed.messages[0].history_source = Some(serde_json::json!({"fixture":true}));
        assert!(persist_threads(&mut store, &[changed]).is_err());
        assert_eq!(store.all_threads().unwrap()[0].name, "Fixture");
        assert_eq!(
            store.messages_for("snapshot-thread", 10).unwrap()[0].text,
            "Fixture 0"
        );
        assert!(store.conn.is_autocommit());
    }

    #[test]
    fn bulk_snapshot_survives_reopen() {
        let path = std::env::temp_dir().join(format!(
            "zl-bulk-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut store = MessageStore::open(path.to_str().unwrap()).unwrap();
        assert_eq!(
            persist_threads(&mut store, &[snapshot_fixture(5000)]).unwrap(),
            5000
        );
        drop(store);
        let store = MessageStore::open(path.to_str().unwrap()).unwrap();
        let messages = store.messages_for("snapshot-thread", 6000).unwrap();
        assert_eq!(messages.len(), 5000);
        assert_eq!(messages[0].msg_id, "snapshot-0");
        assert_eq!(messages[4999].msg_id, "snapshot-4999");
        drop(store);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn test_persist_threads_and_upsert_thread() {
        let mut store = MessageStore::open(":memory:").unwrap();
        let thread = Thread {
            presentation: Default::default(),
            unread: Default::default(),
            id: "t_test".into(),
            name: "Test Thread".into(),
            preview: "Preview".into(),
            time: "10:00".into(),
            avatar: None,
            is_group: false,
            messages: vec![
                ChatMessage {
                    id: "pt_m1".into(),
                    sender: "Alice".into(),
                    text: "Hello".into(),
                    time: "10:00".into(),
                    is_outgoing: false,
                    ts: 1000,
                    ..Default::default()
                },
                ChatMessage {
                    id: "pt_m2".into(),
                    sender: "Tôi".into(),
                    text: "Hi Alice".into(),
                    time: "10:01".into(),
                    is_outgoing: true,
                    ts: 2000,
                    ..Default::default()
                },
            ],
        };

        let count1 = persist_threads(&mut store, std::slice::from_ref(&thread)).unwrap();
        assert_eq!(count1, 2);

        // Second persist returns 0 newly inserted
        let count2 = persist_threads(&mut store, &[thread]).unwrap();
        assert_eq!(count2, 0);

        let msgs = store.messages_for("t_test", 10).unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].text, "Hello");
        assert_eq!(msgs[1].text, "Hi Alice");

        // Hydrate into an empty thread list
        let mut hydrated: Vec<Thread> = Vec::new();
        let loaded = store.hydrate_threads(&mut hydrated, 50).unwrap();
        assert_eq!(loaded, 2);
        assert_eq!(hydrated.len(), 1);
        assert_eq!(hydrated[0].id, "t_test");
        assert_eq!(hydrated[0].name, "Test Thread");
        assert_eq!(hydrated[0].messages.len(), 2);
        assert_eq!(hydrated[0].preview, "Hi Alice");
    }
}
