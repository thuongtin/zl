//! Account-owned incremental SQLite history. No network or implicit identity inference.
use crate::{
    chat::{ChatMessage, Thread},
    store::{StoredMessage, StoredThread},
    unread::UnreadState,
};
use anyhow::{bail, ensure, Context, Result};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Latest forward-only account history schema supported by this build.
pub const CURRENT_SCHEMA_VERSION: u32 = 2;
pub const HISTORY_PAGE_SIZE: usize = 200;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryCursor {
    pub ts: i64,
    pub msg_id: String,
}
#[derive(Debug, Clone, Copy)]
pub enum PageDirection {
    Before,
    After,
}
#[derive(Debug)]
pub struct HistoryPage {
    pub messages: Vec<StoredMessage>,
    pub has_more: bool,
}
#[derive(Debug)]
pub struct SearchHit {
    pub message: StoredMessage,
    pub rank: f64,
}
#[derive(Debug, Clone)]
pub struct MessageKey {
    pub thread_id: String,
    pub msg_id: String,
}
#[derive(Default, Debug, Clone, Copy, Serialize, Deserialize)]
pub struct WriteStats {
    pub inserted: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub deleted: usize,
}
#[derive(Default)]
pub struct StoreDelta<'a> {
    pub threads: &'a [StoredThread],
    pub messages: &'a [StoredMessage],
    pub unread: &'a [(&'a str, &'a UnreadState)],
    pub deleted_messages: &'a [MessageKey],
}
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct MigrationReport {
    /// Source records processed. SQLite and JSON may contain overlapping identities.
    pub threads: usize,
    pub messages: usize,
    pub unread: usize,
    pub already_imported: bool,
}
/// Only construct after the protocol has provided both stable identities.
#[derive(Debug)]
pub struct ValidatedAlias {
    old_id: String,
    canonical_id: String,
}
impl ValidatedAlias {
    pub fn from_protocol_identity(
        old_id: &str,
        canonical_id: &str,
        confirmed_same_identity: bool,
    ) -> Result<Self> {
        ensure!(
            confirmed_same_identity
                && !old_id.is_empty()
                && !canonical_id.is_empty()
                && old_id != canonical_id,
            "Invalid identity alias"
        );
        Ok(Self {
            old_id: old_id.into(),
            canonical_id: canonical_id.into(),
        })
    }
}
pub struct AccountStore {
    conn: Connection,
    account: String,
    pub path: Option<PathBuf>,
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn fold(text: &str) -> String {
    text.replace('Đ', "D").replace('đ', "d")
}
fn private(path: &Path, directory: bool) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            path,
            std::fs::Permissions::from_mode(if directory { 0o700 } else { 0o600 }),
        )?;
    }
    Ok(())
}
impl AccountStore {
    pub fn open(root: &Path, account_id: &str) -> Result<Self> {
        ensure!(!account_id.is_empty(), "Account identity required");
        let dir = root.join("accounts").join(digest(account_id.as_bytes()));
        let path = dir.join("history-v2.sqlite3");
        if path.exists() {
            // Reject incompatible stores before changing even their filesystem metadata.
            let conn = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            Self::check_schema_version(&conn)?;
            Self::check_owner(&conn, account_id)?;
        }
        std::fs::create_dir_all(&dir)?;
        private(&root.join("accounts"), true)?;
        private(&dir, true)?;
        // Create with private permissions before SQLite writes any content.
        if !path.exists() {
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            options.open(&path)?;
        }
        private(&path, false)?;
        let conn = Connection::open(&path)?;
        Self::initialize(conn, account_id, Some(path))
    }
    pub fn in_memory(account_id: &str) -> Result<Self> {
        Self::initialize(Connection::open_in_memory()?, account_id, None)
    }
    fn initialize(mut conn: Connection, account: &str, path: Option<PathBuf>) -> Result<Self> {
        ensure!(!account.is_empty(), "Account identity required");
        // Inspect compatibility before any pragma that could change the database file.
        Self::check_schema_version(&conn)?;
        Self::check_owner(&conn, account)?;
        conn.busy_timeout(std::time::Duration::from_millis(5000))?;
        conn.pragma_update(None, "foreign_keys", true)?;
        Self::migrate_schema(&mut conn, account)
            .context("Không thể nâng cấp hoặc xác minh dữ liệu lịch sử")?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        Ok(Self {
            conn,
            account: account.to_owned(),
            path,
        })
    }

    fn check_schema_version(conn: &Connection) -> Result<u32> {
        let version = conn.pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))?;
        ensure!(
            version <= CURRENT_SCHEMA_VERSION,
            "Lịch sử dùng phiên bản dữ liệu mới hơn ({version}); hãy nâng cấp ứng dụng"
        );
        Ok(version)
    }

    fn check_owner(conn: &Connection, account: &str) -> Result<()> {
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='owner')",
            [],
            |row| row.get(0),
        )?;
        if exists {
            let mut owners = conn.prepare("SELECT account FROM owner")?;
            for owner in owners.query_map([], |row| row.get::<_, String>(0))? {
                ensure!(owner? == account, "Account owner mismatch");
            }
        }
        Ok(())
    }

    fn migrate_schema(conn: &mut Connection, account: &str) -> Result<()> {
        loop {
            // Recheck under the write lock so concurrent opens cannot repeat a step.
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let version = Self::check_schema_version(&tx)?;
            Self::check_owner(&tx, account)?;
            if version == CURRENT_SCHEMA_VERSION {
                Self::validate_schema(&tx)?;
                tx.commit()?;
                return Ok(());
            }
            match version {
                0 => {
                    tx.execute_batch("            CREATE TABLE IF NOT EXISTS owner(account TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS threads(thread_id TEXT PRIMARY KEY,name TEXT NOT NULL,is_group INTEGER NOT NULL,last_ts INTEGER NOT NULL,metadata_json TEXT);
            CREATE TABLE IF NOT EXISTS thread_unread(thread_id TEXT PRIMARY KEY,state_json TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS messages(msg_id TEXT NOT NULL,thread_id TEXT NOT NULL,sender_id TEXT NOT NULL,sender_name TEXT NOT NULL,text TEXT NOT NULL,msg_type TEXT NOT NULL,ts INTEGER NOT NULL,is_outgoing INTEGER NOT NULL,raw_json TEXT NOT NULL,fingerprint TEXT NOT NULL,chat_json TEXT,numeric_id TEXT,numeric_len INTEGER,unread_eligible INTEGER NOT NULL DEFAULT 0,PRIMARY KEY(thread_id,msg_id));
            CREATE INDEX IF NOT EXISTS history_cursor ON messages(thread_id,ts,msg_id);
            CREATE INDEX IF NOT EXISTS unread_cursor ON messages(thread_id,numeric_len,numeric_id,msg_id) WHERE unread_eligible=1;
            CREATE VIRTUAL TABLE IF NOT EXISTS message_search USING fts5(text,tokenize='unicode61 remove_diacritics 2');
            CREATE TABLE IF NOT EXISTS aliases(old_id TEXT PRIMARY KEY,canonical_id TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS migration_manifest(source TEXT PRIMARY KEY,report_json TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS message_tombstones(thread_id TEXT NOT NULL,msg_id TEXT NOT NULL,PRIMARY KEY(thread_id,msg_id));")?;
                    tx.execute("INSERT INTO owner(account) SELECT ?1 WHERE NOT EXISTS(SELECT 1 FROM owner)", [account])?;
                }
                1 => Self::migrate_mentions(&tx, account)?,
                _ => unreachable!("schema version was checked above"),
            }
            if version + 1 == CURRENT_SCHEMA_VERSION {
                Self::validate_schema(&tx)?;
            }
            tx.pragma_update(None, "user_version", version + 1)?;
            tx.commit()?;
        }
    }

    fn validate_schema(conn: &Connection) -> Result<()> {
        // Version numbers alone cannot establish that a supported schema is intact.
        for query in [
            "SELECT account FROM owner LIMIT 0",
            "SELECT thread_id,name,is_group,last_ts,metadata_json FROM threads LIMIT 0",
            "SELECT thread_id,state_json FROM thread_unread LIMIT 0",
            "SELECT msg_id,thread_id,sender_id,sender_name,text,msg_type,ts,is_outgoing,raw_json,fingerprint,chat_json,numeric_id,numeric_len,unread_eligible,mention_eligible FROM messages LIMIT 0",
            "SELECT old_id,canonical_id FROM aliases LIMIT 0",
            "SELECT source,report_json FROM migration_manifest LIMIT 0",
            "SELECT thread_id,msg_id FROM message_tombstones LIMIT 0",
            "SELECT rowid,text FROM message_search LIMIT 0",
        ] {
            conn.prepare(query)?;
        }
        let owners: u32 = conn.query_row("SELECT COUNT(*) FROM owner", [], |row| row.get(0))?;
        ensure!(owners == 1, "Account owner record missing or ambiguous");
        Ok(())
    }

    fn migrate_mentions(conn: &Connection, account: &str) -> Result<()> {
        let has_column: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('messages') WHERE name='mention_eligible')",
            [], |row| row.get(0))?;
        if !has_column {
            conn.execute_batch(
                "ALTER TABLE messages ADD COLUMN mention_eligible INTEGER NOT NULL DEFAULT 0;",
            )?;
            let mut stmt = conn.prepare("SELECT msg_id,thread_id,sender_id,sender_name,text,msg_type,ts,is_outgoing,raw_json,chat_json FROM messages WHERE unread_eligible=1")?;
            let rows = stmt.query_map([], |row| {
                Ok((read_message(row)?, row.get::<_, Option<String>>(9)?))
            })?;
            for row in rows {
                let (message, chat) = row?;
                let eligible = crate::unread::detect_message_mention(
                    &decode_message(&message, chat.as_deref())?,
                    account,
                );
                conn.execute(
                    "UPDATE messages SET mention_eligible=?3 WHERE thread_id=?1 AND msg_id=?2",
                    params![message.thread_id, message.msg_id, eligible],
                )?;
            }
        }
        // Supports unread badge derivation without decoding historical bodies on open.
        conn.execute_batch("CREATE INDEX IF NOT EXISTS mention_cursor ON messages(thread_id,numeric_len,numeric_id,msg_id) WHERE unread_eligible=1 AND mention_eligible=1;")?;
        Ok(())
    }

    /// Erase this account's cached history atomically without changing its owner.
    /// The import manifest remains so a restart cannot silently restore deleted history.
    ///
    /// # Errors
    /// Returns an error if any deletion fails; the entire clear is then rolled back.
    pub fn clear_local_history(&mut self) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute_batch(
            "DELETE FROM message_search;
            DELETE FROM message_tombstones;
            DELETE FROM thread_unread;
            DELETE FROM messages;
            DELETE FROM threads;
            DELETE FROM aliases;",
        )?;
        tx.commit()?;
        Ok(())
    }
    pub fn register_alias(&mut self, alias: ValidatedAlias) -> Result<()> {
        let target = self.resolve_thread(&alias.canonical_id)?;
        ensure!(target != alias.old_id, "Identity alias cycle");
        self.conn.execute("INSERT INTO aliases VALUES(?1,?2) ON CONFLICT(old_id) DO UPDATE SET canonical_id=excluded.canonical_id", params![alias.old_id,target])?;
        Ok(())
    }
    pub fn resolve_thread(&self, id: &str) -> Result<String> {
        let mut id = id.to_owned();
        let mut seen = std::collections::HashSet::new();
        loop {
            ensure!(seen.insert(id.clone()), "Identity alias cycle");
            match self
                .conn
                .query_row(
                    "SELECT canonical_id FROM aliases WHERE old_id=?1",
                    [&id],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
            {
                Some(next) => id = next,
                None => return Ok(id),
            }
        }
    }
    /// Lightweight aggregate for selecting bounded history workloads without loading bodies.
    pub fn thread_message_counts(&self) -> Result<std::collections::BTreeMap<String, u64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT thread_id,COUNT(*) FROM messages GROUP BY thread_id")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u64>(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
    pub fn all_threads(&self) -> Result<Vec<StoredThread>> {
        let mut stmt=self.conn.prepare("SELECT thread_id,name,is_group,last_ts FROM threads ORDER BY last_ts DESC,thread_id ASC")?;
        let rows = stmt.query_map([], |r| {
            Ok(StoredThread {
                thread_id: r.get(0)?,
                name: r.get(1)?,
                is_group: r.get(2)?,
                last_ts: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
    /// Metadata only: no message vectors are read from SQLite.
    pub fn all_thread_metadata(&self) -> Result<Vec<Thread>> {
        let mut out = Vec::new();
        for st in self.all_threads()? {
            let raw: Option<String> = self.conn.query_row(
                "SELECT metadata_json FROM threads WHERE thread_id=?1",
                [&st.thread_id],
                |r| r.get(0),
            )?;
            let mut t = match raw {
                Some(raw) => serde_json::from_str::<Thread>(&raw)?,
                None => Thread {
                    presentation: Default::default(),
                    id: st.thread_id.clone(),
                    name: st.name.clone(),
                    preview: String::new(),
                    time: String::new(),
                    avatar: None,
                    is_group: st.is_group,
                    messages: Vec::new(),
                    unread: Default::default(),
                },
            };
            t.id = st.thread_id;
            t.name = st.name;
            t.is_group = st.is_group;
            self.repair_sidebar_metadata(&mut t)?;
            t.messages.clear();
            t.unread = self.unread(&t.id)?;
            out.push(t);
        }
        Ok(out)
    }
    /// Sidecar-only update: never changes thread metadata, raw payload, unread or FTS.
    pub fn persist_mention_mappings(
        &mut self,
        thread_id: &str,
        mappings: &[(String, std::collections::BTreeMap<String, String>)],
    ) -> Result<usize> {
        ensure!(mappings.len() <= 200, "Mention page exceeds bound");
        let tx = self.conn.unchecked_transaction()?;
        let mut changed = 0;
        for (id, map) in mappings {
            ensure!(map.len() <= 1024, "Mention map exceeds bound");
            let Some(stored)=self.conn.query_row("SELECT msg_id,thread_id,sender_id,sender_name,text,msg_type,ts,is_outgoing,raw_json FROM messages WHERE thread_id=?1 AND msg_id=?2",params![thread_id,id],read_message).optional()? else{continue;};
            let mut message = self.chat_message(&stored)?;
            let before = message.resolved_mention_uids.clone();
            let raw = message
                .history_source
                .as_ref()
                .and_then(|v| v.pointer("/meta/mentionsList"))
                .and_then(serde_json::Value::as_array);
            for (source, uid) in map {
                let claimed = raw.is_some_and(|list| {
                    list.iter().any(|mention| match mention.get("uid") {
                        Some(serde_json::Value::String(v)) => v == source,
                        Some(serde_json::Value::Number(v)) => v.to_string() == *source,
                        _ => false,
                    })
                });
                if claimed
                    && !uid.is_empty()
                    && uid.bytes().all(|b| b.is_ascii_digit())
                    && uid.parse::<u64>().is_ok_and(|v| v > 0)
                {
                    message
                        .resolved_mention_uids
                        .insert(source.clone(), uid.clone());
                }
            }
            if before != message.resolved_mention_uids {
                let chat = serde_json::to_string(&message)?;
                let fingerprint = digest(&serde_json::to_vec(&(&stored, Some(chat.as_str())))?);
                changed+=self.conn.execute("UPDATE messages SET chat_json=?3,fingerprint=?4 WHERE thread_id=?1 AND msg_id=?2",params![thread_id,id,chat,fingerprint])?;
            }
        }
        tx.commit()?;
        Ok(changed)
    }
    pub fn thread_is_group(&self, thread_id: &str) -> Result<Option<bool>> {
        Ok(self
            .conn
            .query_row(
                "SELECT is_group FROM threads WHERE thread_id=?1",
                [thread_id],
                |r| r.get::<_, bool>(0),
            )
            .optional()?)
    }
    /// One indexed row, not a 200-body page, for durable sidebar presentation.
    pub fn latest_message(&self, thread_id: &str) -> Result<Option<ChatMessage>> {
        let id:Option<String>=self.conn.query_row("SELECT msg_id FROM messages WHERE thread_id=?1 ORDER BY ts DESC,msg_id DESC LIMIT 1",[thread_id],|r|r.get(0)).optional()?;
        id.map(|id| self.get_message(thread_id, &id))
            .transpose()
            .map(Option::flatten)
    }
    fn repair_sidebar_metadata(&self, t: &mut Thread) -> Result<()> {
        if let Some(message) = self.latest_message(&t.id)? {
            let previous_ts = crate::clock::normalize_ts_ms(t.presentation.last_message_ts);
            let actual_ts = crate::clock::normalize_ts_ms(message.ts);
            if previous_ts > actual_ts {
                // Repair pre-fix contact activity accidentally stored as message time.
                t.presentation.last_message_ts = 0;
                t.presentation.last_message_id.clear();
                t.presentation.last_sender.clear();
                t.presentation.last_sender_id.clear();
                if !message.text.is_empty() {
                    t.preview = message.text.replace('\n', " ");
                }
            }
            t.messages = vec![message];
            t.refresh_sidebar_presentation();
            t.messages.clear();
        } else if t.presentation.last_message_id.is_empty() {
            t.presentation.last_message_ts = 0;
            t.time.clear();
        }
        Ok(())
    }
    fn write_thread_metadata(&self, t: &Thread) -> Result<()> {
        let metadata = serde_json::json!({"id":t.id,"name":t.name,"preview":t.preview,"time":t.time,"avatar":t.avatar,"is_group":t.is_group,"presentation":t.presentation,"messages":[]});
        self.conn.execute("UPDATE threads SET metadata_json=?2 WHERE thread_id=?1 AND (metadata_json IS NULL OR metadata_json!=?2)",params![t.id,serde_json::to_string(&metadata)?])?;
        Ok(())
    }
    pub fn unread(&self, id: &str) -> Result<UnreadState> {
        let raw = self
            .conn
            .query_row(
                "SELECT state_json FROM thread_unread WHERE thread_id=?1",
                [id],
                |r| r.get::<_, String>(0),
            )
            .optional()?;
        Ok(match raw {
            Some(raw) => serde_json::from_str(&raw)?,
            None => UnreadState::default(),
        })
    }
    /// Apply an explicit local viewport read to the latest durable state, not a GUI snapshot.
    pub fn read_local_visible(
        &mut self,
        thread_id: &str,
        messages: &[ChatMessage],
    ) -> Result<bool> {
        if thread_id.is_empty() || messages.is_empty() {
            return Ok(false);
        }
        let tx = self.conn.unchecked_transaction()?;
        let exists: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM threads WHERE thread_id=?1)",
            [thread_id],
            |row| row.get(0),
        )?;
        if !exists {
            return Ok(false);
        }
        let mut state = self.unread(thread_id)?;
        let changed = crate::read_receipts::read_local_visible(&mut state, messages);
        if changed {
            self.write_unread(thread_id, &state)?;
        }
        tx.commit()?;
        Ok(changed)
    }
    pub fn apply_delta(&mut self, delta: StoreDelta<'_>) -> Result<WriteStats> {
        let tx = self.conn.unchecked_transaction()?;
        let mut stats = WriteStats::default();
        for t in delta.threads {
            self.write_thread(t)?;
        }
        for (id, state) in delta.unread {
            self.write_unread(id, state)?;
        }
        for m in delta.messages {
            self.write_message(m, None, &mut stats)?;
        }
        for key in delta.deleted_messages {
            ensure!(
                !key.thread_id.is_empty() && !key.msg_id.is_empty(),
                "Composite deletion identity required"
            );
            // A local deletion is durable even when history has not loaded this row yet.
            // No cache/network/history replay may clear this exact account-owned identity.
            self.conn.execute(
                "INSERT OR IGNORE INTO message_tombstones(thread_id,msg_id) VALUES(?1,?2)",
                params![key.thread_id, key.msg_id],
            )?;
            self.conn.execute("DELETE FROM message_search WHERE rowid IN (SELECT rowid FROM messages WHERE thread_id=?1 AND msg_id=?2)",params![key.thread_id,key.msg_id])?;
            stats.deleted += self.conn.execute(
                "DELETE FROM messages WHERE thread_id=?1 AND msg_id=?2",
                params![key.thread_id, key.msg_id],
            )?;
            let mut state = self.unread(&key.thread_id)?;
            let unread_changed = state.unread_ids.remove(&key.msg_id);
            let mention_changed = state.mention_ids.remove(&key.msg_id);
            if unread_changed || mention_changed {
                self.write_unread(&key.thread_id, &state)?;
            }
        }
        tx.commit()?;
        Ok(stats)
    }
    fn write_thread(&self, t: &StoredThread) -> Result<()> {
        ensure!(!t.thread_id.is_empty(), "Thread identity required");
        self.conn.execute("INSERT INTO threads(thread_id,name,is_group,last_ts) VALUES(?1,?2,?3,?4) ON CONFLICT(thread_id) DO UPDATE SET name=excluded.name,is_group=excluded.is_group,last_ts=MAX(threads.last_ts,excluded.last_ts) WHERE name!=excluded.name OR is_group!=excluded.is_group OR last_ts<excluded.last_ts",params![t.thread_id,t.name,t.is_group,t.last_ts])?;
        Ok(())
    }
    fn write_unread(&self, id: &str, state: &UnreadState) -> Result<()> {
        let mut state = state.clone();
        // Stale partial snapshots must not resurrect badges for locally deleted rows.
        let mut stmt = self
            .conn
            .prepare("SELECT msg_id FROM message_tombstones WHERE thread_id=?1")?;
        for row in stmt.query_map([id], |r| r.get::<_, String>(0))? {
            let deleted = row?;
            state.unread_ids.remove(&deleted);
            state.mention_ids.remove(&deleted);
        }
        let raw = serde_json::to_string(&state)?;
        self.conn.execute("INSERT INTO thread_unread VALUES(?1,?2) ON CONFLICT(thread_id) DO UPDATE SET state_json=excluded.state_json WHERE state_json!=excluded.state_json",params![id,raw])?;
        Ok(())
    }
    fn is_deleted(&self, thread_id: &str, msg_id: &str) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM message_tombstones WHERE thread_id=?1 AND msg_id=?2)",
            params![thread_id, msg_id],
            |r| r.get(0),
        )?)
    }
    fn write_message(
        &self,
        m: &StoredMessage,
        chat: Option<&str>,
        stats: &mut WriteStats,
    ) -> Result<()> {
        ensure!(
            !m.thread_id.is_empty() && !m.msg_id.is_empty(),
            "Composite message identity required"
        );
        if self.is_deleted(&m.thread_id, &m.msg_id)? {
            stats.unchanged += 1;
            return Ok(());
        }
        let fingerprint = digest(&serde_json::to_vec(&(m, chat))?);
        let prior: Option<(i64, String)> = self
            .conn
            .query_row(
                "SELECT rowid,fingerprint FROM messages WHERE thread_id=?1 AND msg_id=?2",
                params![m.thread_id, m.msg_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if prior.as_ref().is_some_and(|(_, f)| f == &fingerprint) {
            stats.unchanged += 1;
            return Ok(());
        }
        if let Some((rowid, _)) = &prior {
            self.conn
                .execute("DELETE FROM message_search WHERE rowid=?1", [rowid])?;
        }
        let normalized = numeric_id(&m.msg_id);
        let eligible = normalized.is_some() && unread_eligible(m, chat)?;
        let mention = eligible
            && crate::unread::detect_message_mention(&decode_message(m, chat)?, &self.account);
        self.conn.execute("INSERT INTO messages(msg_id,thread_id,sender_id,sender_name,text,msg_type,ts,is_outgoing,raw_json,fingerprint,chat_json,numeric_id,numeric_len,unread_eligible,mention_eligible) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15) ON CONFLICT(thread_id,msg_id) DO UPDATE SET sender_id=excluded.sender_id,sender_name=excluded.sender_name,text=excluded.text,msg_type=excluded.msg_type,ts=excluded.ts,is_outgoing=excluded.is_outgoing,raw_json=excluded.raw_json,fingerprint=excluded.fingerprint,chat_json=excluded.chat_json,numeric_id=excluded.numeric_id,numeric_len=excluded.numeric_len,unread_eligible=excluded.unread_eligible,mention_eligible=excluded.mention_eligible",params![m.msg_id,m.thread_id,m.sender_id,m.sender_name,m.text,m.msg_type,m.ts,m.is_outgoing,m.raw_json,fingerprint,chat,normalized,normalized.map(|id|id.len() as i64),eligible,mention])?;
        self.conn.execute("INSERT INTO message_search(rowid,text) SELECT rowid,?3 FROM messages WHERE thread_id=?1 AND msg_id=?2",params![m.thread_id,m.msg_id,fold(&m.text)])?;
        if prior.is_some() {
            stats.updated += 1;
        } else {
            stats.inserted += 1;
        }
        Ok(())
    }
    /// Rebuild only IDs newer than the numeric server cursor using a covering partial
    /// index. Historical bodies are never decoded and mute/manual marks are untouched.
    pub fn derive_unread(&self, thread_id: &str, state: &mut UnreadState) -> Result<()> {
        let Some(point) = state.last_seen_id.as_deref().and_then(numeric_id) else {
            return Ok(());
        };
        let mut stmt=self.conn.prepare("SELECT msg_id FROM messages WHERE thread_id=?1 AND unread_eligible=1 AND (numeric_len,numeric_id)>(?2,?3) ORDER BY numeric_len,numeric_id,msg_id")?;
        let ids = stmt
            .query_map(params![thread_id, point.len() as i64, point], |r| {
                r.get::<_, String>(0)
            })?
            .collect::<rusqlite::Result<std::collections::BTreeSet<_>>>()?;
        state.unread_ids = ids;
        let mut stmt=self.conn.prepare("SELECT msg_id FROM messages WHERE thread_id=?1 AND unread_eligible=1 AND mention_eligible=1 AND (numeric_len,numeric_id)>(?2,?3) ORDER BY numeric_len,numeric_id,msg_id")?;
        state.mention_ids = stmt
            .query_map(params![thread_id, point.len() as i64, point], |r| {
                r.get::<_, String>(0)
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(())
    }
    /// Retrieve one exact account/thread/wire identity without loading a history window.
    pub fn get_message(&self, thread_id: &str, msg_id: &str) -> Result<Option<ChatMessage>> {
        let stored = self.conn.query_row("SELECT msg_id,thread_id,sender_id,sender_name,text,msg_type,ts,is_outgoing,raw_json FROM messages WHERE thread_id=?1 AND msg_id=?2",params![thread_id,msg_id],read_message).optional()?;
        stored.as_ref().map(|m| self.chat_message(m)).transpose()
    }
    fn merge_unread(&self, id: &str, incoming: &UnreadState) -> Result<UnreadState> {
        let mut state = self.unread(id)?;
        if *incoming == UnreadState::default() && !incoming.fresh {
            return Ok(state);
        }
        if state == UnreadState::default() {
            return Ok(incoming.clone());
        }
        if incoming.fresh {
            state.merge_metadata(incoming);
        }
        // A local visible-page read advances the cursor without requiring a network receipt.
        if let Some(point) = &incoming.last_seen_id {
            let before = state.last_seen_id.clone();
            state.advance_seen(point);
            if state.last_seen_id != before {
                state.marked_unread = false;
            }
        }
        for id in &incoming.unread_ids {
            if state.last_seen_id.as_deref().is_none_or(|point| {
                crate::unread::compare_ids(id, point).is_some_and(|v| v.is_gt())
            }) {
                state.unread_ids.insert(id.clone());
            }
        }
        for id in &incoming.mention_ids {
            if state.unread_ids.contains(id) {
                state.mention_ids.insert(id.clone());
            }
        }
        state.mention_ids.retain(|id| state.unread_ids.contains(id));
        if incoming.settings_fresh {
            state.muted = incoming.muted;
            state.muted_until = incoming.muted_until;
            state.marked_unread = incoming.marked_unread;
        } else {
            if incoming.muted {
                state.muted = true;
                state.muted_until = incoming.muted_until.or(state.muted_until);
            }
            if incoming.marked_unread {
                state.marked_unread = true;
            }
        }
        Ok(state)
    }
    fn merge_message(
        &self,
        thread_id: &str,
        is_group: bool,
        message: &ChatMessage,
    ) -> Result<ChatMessage> {
        let merged = if let Some(existing) = self.get_message(thread_id, &message.id)? {
            // Both vectors contain exactly the same nonempty identity, so the pure
            // reducer cannot infer identity from names, text, timestamps or avatars.
            let preserve_recall = existing.text == "[Tin nhắn đã thu hồi]";
            let mut holder = Thread {
                presentation: Default::default(),
                id: thread_id.to_owned(),
                name: String::new(),
                preview: String::new(),
                time: String::new(),
                avatar: None,
                is_group,
                messages: vec![existing],
                unread: Default::default(),
            };
            let mut delta = holder.clone();
            delta.messages = vec![message.clone()];
            crate::chat::merge_thread_into(&mut holder, delta, false);
            let mut merged = holder.messages.pop().unwrap_or_else(|| message.clone());
            if !message.text.is_empty() && !preserve_recall {
                merged.text = message.text.clone();
            }
            merged
        } else {
            message.clone()
        };
        Ok(merged)
    }
    /// Borrows each row; serializes/fingerprints individual records, never an entire history vector.
    /// Partial network snapshots enrich durable rows instead of erasing reaction/read metadata.
    pub fn persist_threads(&mut self, threads: &[Thread]) -> Result<WriteStats> {
        let tx = self.conn.unchecked_transaction()?;
        let mut stats = WriteStats::default();
        for t in threads {
            let prior: Option<String> = self
                .conn
                .query_row(
                    "SELECT metadata_json FROM threads WHERE thread_id=?1",
                    [&t.id],
                    |r| r.get(0),
                )
                .optional()?
                .flatten();
            let mut metadata = match prior {
                Some(raw) => serde_json::from_str::<Thread>(&raw)?,
                None => Thread {
                    presentation: Default::default(),
                    id: t.id.clone(),
                    name: String::new(),
                    preview: String::new(),
                    time: String::new(),
                    avatar: None,
                    is_group: false,
                    messages: Vec::new(),
                    unread: Default::default(),
                },
            };
            self.repair_sidebar_metadata(&mut metadata)?;
            if !t.name.is_empty()
                && (!crate::chat::is_generic_thread_name(&t.name) || metadata.name.is_empty())
            {
                metadata.name = t.name.clone();
            }
            if t.avatar.is_some() {
                metadata.avatar = t.avatar.clone();
            }
            if !t.presentation.members.is_empty() {
                metadata.presentation.members = t.presentation.members.clone();
            }
            if t.presentation.member_count.is_some() {
                metadata.presentation.member_count = t.presentation.member_count;
            }
            let mut incoming = t.clone();
            incoming.refresh_sidebar_presentation();
            if crate::clock::normalize_ts_ms(incoming.presentation.last_message_ts)
                >= crate::clock::normalize_ts_ms(metadata.presentation.last_message_ts)
                && incoming.presentation.last_message_ts > 0
            {
                let same_sender_and_time =
                    crate::clock::normalize_ts_ms(incoming.presentation.last_message_ts)
                        == crate::clock::normalize_ts_ms(metadata.presentation.last_message_ts)
                        && metadata.presentation.last_message_id
                            == incoming.presentation.last_message_id
                        && !metadata.presentation.last_sender_id.is_empty()
                        && metadata.presentation.last_sender_id
                            == incoming.presentation.last_sender_id;
                let keep_resolved = same_sender_and_time
                    && !metadata.presentation.last_sender.is_empty()
                    && !metadata
                        .presentation
                        .last_sender
                        .bytes()
                        .all(|b| b.is_ascii_digit())
                    && (incoming.presentation.last_sender.is_empty()
                        || incoming
                            .presentation
                            .last_sender
                            .bytes()
                            .all(|b| b.is_ascii_digit()));
                metadata.presentation.last_message_ts = incoming.presentation.last_message_ts;
                metadata.presentation.last_message_id = incoming.presentation.last_message_id;
                if !keep_resolved {
                    metadata.presentation.last_sender = incoming.presentation.last_sender;
                }
                metadata.presentation.last_sender_id = incoming.presentation.last_sender_id;
                metadata.presentation.last_outgoing = incoming.presentation.last_outgoing;
                if !t.preview.is_empty() {
                    metadata.preview = t.preview.clone();
                }
            }
            if metadata.presentation.last_message_ts == 0 {
                if !t.preview.is_empty() {
                    metadata.preview = t.preview.clone();
                }
                if !t.time.is_empty() {
                    metadata.time = t.time.clone();
                }
            }
            metadata.is_group |= t.is_group;
            self.write_thread(&StoredThread {
                thread_id: t.id.clone(),
                name: metadata.name.clone(),
                is_group: metadata.is_group,
                last_ts: t.messages.iter().map(|m| m.ts).max().unwrap_or(0),
            })?;
            self.write_thread_metadata(&metadata)?;
            self.write_unread(&t.id, &self.merge_unread(&t.id, &t.unread)?)?;
            for message in &t.messages {
                if self.is_deleted(&t.id, &message.id)? {
                    stats.unchanged += 1;
                    continue;
                }
                let merged = self.merge_message(&t.id, t.is_group, message)?;
                let mut stored = StoredMessage::from(&merged);
                stored.thread_id = t.id.clone();
                // Preserve unknown legacy wire fields while replacing normalized metadata,
                // including absent reaction keys that represent an authoritative tombstone.
                let old: Option<String> = self
                    .conn
                    .query_row(
                        "SELECT raw_json FROM messages WHERE thread_id=?1 AND msg_id=?2",
                        params![t.id, stored.msg_id],
                        |r| r.get(0),
                    )
                    .optional()?;
                if let Some(raw) = old {
                    if let Ok(serde_json::Value::Object(mut raw)) =
                        serde_json::from_str::<serde_json::Value>(&raw)
                    {
                        for key in [
                            "kind",
                            "wire_ref",
                            "unread_policy",
                            "delivery",
                            "reaction_snapshot_provenance",
                            "history_source",
                            "stickerId",
                            "cateId",
                            "type",
                            "reply_to_id",
                            "reply_to_sender",
                            "reply_to_text",
                            "reaction_summaries",
                            "reaction_snapshot",
                            "reaction_authoritative_snapshot",
                            "reaction_versions",
                            "client_message_id",
                            "reactions",
                            "is_pinned",
                            "is_starred",
                            "link_url",
                            "link_title",
                            "link_desc",
                            "link_thumb",
                            "link_src",
                        ] {
                            raw.remove(key);
                        }
                        if let Ok(serde_json::Value::Object(current)) =
                            serde_json::from_str::<serde_json::Value>(&stored.raw_json)
                        {
                            raw.extend(current);
                        }
                        if !raw.is_empty() {
                            stored.raw_json = serde_json::to_string(&raw)?;
                        }
                    }
                }
                self.write_message(&stored, Some(&serde_json::to_string(&merged)?), &mut stats)?;
            }
        }
        tx.commit()?;
        Ok(stats)
    }
    pub fn history_page(
        &self,
        thread_id: &str,
        cursor: Option<&HistoryCursor>,
        direction: PageDirection,
    ) -> Result<HistoryPage> {
        let after = matches!(direction, PageDirection::After);
        let cmp = if after { ">" } else { "<" };
        let order = if after { "ASC" } else { "DESC" };
        let sql=format!("SELECT msg_id,thread_id,sender_id,sender_name,text,msg_type,ts,is_outgoing,raw_json FROM messages WHERE thread_id=?1 AND (?2 IS NULL OR (ts,msg_id) {cmp} (?2,?3)) ORDER BY ts {order},msg_id {order} LIMIT 201");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(
            params![
                thread_id,
                cursor.map(|c| c.ts),
                cursor.map(|c| c.msg_id.as_str())
            ],
            read_message,
        )?;
        let mut messages = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        let has_more = messages.len() > HISTORY_PAGE_SIZE;
        messages.truncate(HISTORY_PAGE_SIZE);
        if !after {
            messages.reverse();
        }
        Ok(HistoryPage { messages, has_more })
    }
    /// Resolve one exact server/client reference inside this thread, never by text.
    /// Ambiguous aliases stay unresolved. At most 200 bodies reach the UI.
    pub fn navigation_page(
        &self,
        thread_id: &str,
        reference: &str,
    ) -> Result<Option<(String, Vec<ChatMessage>)>> {
        if reference.is_empty() {
            return Ok(None);
        }
        let mut stmt = self.conn.prepare("SELECT msg_id,ts FROM messages WHERE thread_id=?1 AND (msg_id=?2 OR json_extract(chat_json,'$.client_message_id')=?2 OR CAST(json_extract(chat_json,'$.wire_ref.fields.cliMsgId') AS TEXT)=?2 OR CAST(json_extract(chat_json,'$.wire_ref.client_id') AS TEXT)=?2 OR CAST(json_extract(chat_json,'$.wire_ref.server_id') AS TEXT)=?2) LIMIT 2")?;
        let rows = stmt
            .query_map(params![thread_id, reference], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if rows.len() != 1 {
            return Ok(None);
        }
        let (id, ts) = &rows[0];
        let cursor = HistoryCursor {
            ts: *ts,
            msg_id: id.clone(),
        };
        let before = self.history_page(thread_id, Some(&cursor), PageDirection::Before)?;
        let after = self.history_page(thread_id, Some(&cursor), PageDirection::After)?;
        let mut messages = before
            .messages
            .iter()
            .rev()
            .take(80)
            .map(|m| self.chat_message(m))
            .collect::<Result<Vec<_>>>()?;
        messages.reverse();
        if let Some(target) = self.get_message(thread_id, id)? {
            messages.push(target);
        }
        messages.extend(
            after
                .messages
                .iter()
                .take(119)
                .map(|m| self.chat_message(m))
                .collect::<Result<Vec<_>>>()?,
        );
        Ok(Some((id.clone(), messages)))
    }

    pub fn search(
        &self,
        query: &str,
        thread_id: Option<&str>,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<SearchHit>> {
        // User text is always a quoted literal phrase, never FTS syntax or SQL.
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }
        let query = format!("\"{}\"", fold(query.trim()).replace('"', "\"\""));
        let mut stmt=self.conn.prepare("SELECT m.msg_id,m.thread_id,m.sender_id,m.sender_name,m.text,m.msg_type,m.ts,m.is_outgoing,m.raw_json,bm25(message_search) AS rank FROM message_search JOIN messages m ON m.rowid=message_search.rowid WHERE message_search MATCH ?1 AND (?2 IS NULL OR m.thread_id=?2) ORDER BY rank ASC,m.ts DESC,m.thread_id ASC,m.msg_id ASC LIMIT ?3 OFFSET ?4")?;
        let rows = stmt.query_map(
            params![
                query,
                thread_id,
                limit.min(200) as i64,
                offset.min(i64::MAX as usize) as i64
            ],
            |r| {
                Ok(SearchHit {
                    message: read_message(r)?,
                    rank: r.get(9)?,
                })
            },
        )?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
    pub fn chat_message(&self, m: &StoredMessage) -> Result<ChatMessage> {
        let chat: Option<String> = self.conn.query_row(
            "SELECT chat_json FROM messages WHERE thread_id=?1 AND msg_id=?2",
            params![m.thread_id, m.msg_id],
            |r| r.get(0),
        )?;
        decode_message(m, chat.as_deref())
    }
    pub fn hydrate_recent(&self, threads: &mut Vec<Thread>) -> Result<usize> {
        let mut loaded = 0;
        for stored in self.all_threads()? {
            if !threads.iter().any(|t| t.id == stored.thread_id) {
                threads.push(Thread {
                    presentation: Default::default(),
                    id: stored.thread_id.clone(),
                    name: stored.name,
                    preview: String::new(),
                    time: String::new(),
                    avatar: None,
                    is_group: stored.is_group,
                    messages: Vec::new(),
                    unread: self.unread(&stored.thread_id)?,
                });
            }
            let t = threads
                .iter_mut()
                .find(|t| t.id == stored.thread_id)
                .unwrap();
            t.unread = self.unread(&t.id)?;
            for m in self
                .history_page(&t.id, None, PageDirection::Before)?
                .messages
            {
                if !t.messages.iter().any(|cm| cm.id == m.msg_id) {
                    t.messages.push(self.chat_message(&m)?);
                    loaded += 1;
                }
            }
            t.messages.sort_by(|a, b| (a.ts, &a.id).cmp(&(b.ts, &b.id)));
            if let Some(last) = t.messages.last() {
                t.preview = last.text.replace('\n', " ");
                t.time = last.time.clone();
            }
        }
        Ok(loaded)
    }
    /// Copy sources into this account's DB atomically. Source files are only opened read-only.
    pub fn migrate_legacy(
        &mut self,
        db: Option<&Path>,
        json: Option<&Path>,
    ) -> Result<MigrationReport> {
        // An import is a one-time snapshot per explicit source set. Restarts consult
        // the manifest without reading or hashing a potentially huge legacy cache.
        let mut identity = Vec::new();
        for (role, path) in [(b"db".as_slice(), db), (b"json".as_slice(), json)] {
            let Some(path) = path else { continue };
            identity.extend_from_slice(role);
            identity.push(0);
            identity.extend_from_slice(std::fs::canonicalize(path)?.to_string_lossy().as_bytes());
            identity.push(0);
        }
        ensure!(!identity.is_empty(), "Migration source required");
        let source = digest(&identity);
        if let Some(raw) = self
            .conn
            .query_row(
                "SELECT report_json FROM migration_manifest WHERE source=?1",
                [&source],
                |r| r.get::<_, String>(0),
            )
            .optional()?
        {
            let mut report: MigrationReport = serde_json::from_str(&raw)?;
            report.already_imported = true;
            return Ok(report);
        }
        let tx = self.conn.unchecked_transaction()?;
        let mut report = MigrationReport::default();
        let mut stats = WriteStats::default();
        if let Some(path) = db {
            let legacy = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            let _legacy_snapshot = legacy.unchecked_transaction()?;
            let mut stmt = legacy.prepare("SELECT thread_id,name,is_group,last_ts FROM threads")?;
            for t in stmt.query_map([], |r| {
                Ok(StoredThread {
                    thread_id: r.get(0)?,
                    name: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    is_group: r.get(2)?,
                    last_ts: r.get(3)?,
                })
            })? {
                self.write_thread(&t?)?;
                report.threads += 1;
            }
            let mut stmt=legacy.prepare("SELECT msg_id,thread_id,COALESCE(sender_id,''),COALESCE(sender_name,''),COALESCE(text,''),COALESCE(msg_type,''),COALESCE(ts,0),COALESCE(is_outgoing,0),COALESCE(raw_json,'') FROM messages")?;
            for m in stmt.query_map([], read_message)? {
                self.write_message(&m?, None, &mut stats)?;
                report.messages += 1;
            }
            let exists: bool = legacy.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='thread_unread')",
                [],
                |r| r.get(0),
            )?;
            if exists {
                let mut stmt = legacy.prepare("SELECT thread_id,state_json FROM thread_unread")?;
                for pair in
                    stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                {
                    let (id, raw) = pair?;
                    self.write_unread(&id, &serde_json::from_str(&raw)?)?;
                    report.unread += 1;
                }
            }
        }
        if let Some(path) = json {
            let value: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)?;
            let list = if value.is_array() {
                value
            } else if let Some(threads) = value.get("threads") {
                threads.clone()
            } else {
                bail!("Unsupported legacy cache schema")
            };
            let threads: Vec<Thread> = serde_json::from_value(list)?;
            for t in &threads {
                self.write_thread(&StoredThread {
                    thread_id: t.id.clone(),
                    name: t.name.clone(),
                    is_group: t.is_group,
                    last_ts: t.messages.iter().map(|m| m.ts).max().unwrap_or(0),
                })?;
                self.write_thread_metadata(t)?;
                self.write_unread(&t.id, &self.merge_unread(&t.id, &t.unread)?)?;
                report.threads += 1;
                report.unread += 1;
                for m in &t.messages {
                    let merged = self.merge_message(&t.id, t.is_group, m)?;
                    let mut stored = StoredMessage::from(&merged);
                    stored.thread_id = t.id.clone();
                    // The database holds opaque raw JSON that cache serialization cannot reproduce.
                    let raw: Option<String> = self
                        .conn
                        .query_row(
                            "SELECT raw_json FROM messages WHERE thread_id=?1 AND msg_id=?2",
                            params![t.id, stored.msg_id],
                            |r| r.get(0),
                        )
                        .optional()?;
                    if let Some(raw) = raw {
                        stored.raw_json = raw;
                    }
                    self.write_message(
                        &stored,
                        Some(&serde_json::to_string(&merged)?),
                        &mut stats,
                    )?;
                    report.messages += 1;
                }
            }
        }
        self.conn.execute(
            "INSERT INTO migration_manifest VALUES(?1,?2)",
            params![source, serde_json::to_string(&report)?],
        )?;
        tx.commit()?;
        Ok(report)
    }
}
fn read_message(r: &rusqlite::Row<'_>) -> rusqlite::Result<StoredMessage> {
    Ok(StoredMessage {
        msg_id: r.get(0)?,
        thread_id: r.get(1)?,
        sender_id: r.get(2)?,
        sender_name: r.get(3)?,
        text: r.get(4)?,
        msg_type: r.get(5)?,
        ts: r.get(6)?,
        is_outgoing: r.get(7)?,
        raw_json: r.get(8)?,
    })
}

fn numeric_id(id: &str) -> Option<&str> {
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let normalized = id.trim_start_matches('0');
    Some(if normalized.is_empty() {
        "0"
    } else {
        normalized
    })
}
fn unread_eligible(m: &StoredMessage, chat: Option<&str>) -> Result<bool> {
    if m.is_outgoing
        || matches!(
            m.msg_type.as_str(),
            "chat.system" | "chat.control" | "chat.reaction" | "chat.receipt" | "control"
        )
        || m.text == "sendBubbleMessage"
    {
        return Ok(false);
    }
    if let Some(raw) = chat {
        let message: ChatMessage = serde_json::from_str(raw)?;
        return Ok(message.kind != crate::message_model::MessageKind::System
            && message.unread_policy.counts_unread());
    }
    let raw =
        serde_json::from_str::<serde_json::Value>(&m.raw_json).unwrap_or(serde_json::Value::Null);
    let policy = raw
        .get("unread_policy")
        .map(|value| serde_json::from_value::<crate::message_model::UnreadPolicy>(value.clone()))
        .transpose()?
        .unwrap_or_else(|| {
            crate::message_model::UnreadPolicy::from_raw(raw.get("history_source").unwrap_or(&raw))
        });
    Ok(policy.counts_unread())
}

fn decode_message(m: &StoredMessage, chat: Option<&str>) -> Result<ChatMessage> {
    if let Some(chat) = chat {
        return Ok(serde_json::from_str(chat)?);
    }
    let mut value = serde_json::to_value(ChatMessage {
        id: m.msg_id.clone(),
        sender: if m.sender_name.is_empty() {
            m.sender_id.clone()
        } else {
            m.sender_name.clone()
        },
        text: m.text.clone(),
        time: crate::clock::message_time(m.ts),
        ts: m.ts,
        is_outgoing: m.is_outgoing,
        ..Default::default()
    })?;
    if let Ok(serde_json::Value::Object(raw)) =
        serde_json::from_str::<serde_json::Value>(&m.raw_json)
    {
        for (key, val) in raw {
            let key = match key.as_str() {
                "stickerId" => "sticker_id",
                "cateId" => "sticker_cate_id",
                "type" => "sticker_type",
                _ => &key,
            };
            if !matches!(key, "id" | "sender" | "text" | "ts" | "is_outgoing") {
                value[key] = val;
            }
        }
    }
    Ok(serde_json::from_value(value)?)
}

#[cfg(test)]
mod schema_connection_tests {
    use super::*;

    #[test]
    fn connection_uses_required_pragmas() {
        let store = AccountStore::in_memory("fixture").unwrap();
        for (pragma, expected) in [
            ("synchronous", 1),
            ("busy_timeout", 5000),
            ("foreign_keys", 1),
        ] {
            let actual: u32 = store
                .conn
                .pragma_query_value(None, pragma, |row| row.get(0))
                .unwrap();
            assert_eq!(actual, expected, "{pragma}");
        }
    }
}
