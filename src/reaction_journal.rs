//! Durable, account-isolated reaction inbox and history coverage checkpoints.
//! Resolved rows remain as a replay journal: a crash before message-cache persistence
//! must not lose the event. Message ledgers make replay into loaded pages idempotent.
use crate::{chat::Thread, reactions::ReactionEvent};
use anyhow::{ensure, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub const JOURNAL_PAGE_SIZE: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingReaction {
    pub key: String,
    pub event: ReactionEvent,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverageCheckpoint {
    pub scope: String,
    pub pages: u64,
    pub complete: bool,
    pub reason: Option<String>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JournalStatus {
    pub pending: u64,
    pub total: u64,
    pub partial: bool,
    pub checkpoints: Vec<CoverageCheckpoint>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApplySummary {
    pub scanned: u64,
    pub applied: u64,
    pub resolved: u64,
    pub ambiguous: u64,
    pub pending: u64,
}

pub struct ReactionJournal {
    conn: Connection,
    path: PathBuf,
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn known(id: &str) -> bool {
    !id.is_empty() && id != "0"
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
/// Wire event IDs carry their version. Scope includes target and reactor because
/// one wire envelope may contain several targets. No-ID events use the exact payload.
pub fn event_key(event: &ReactionEvent) -> Result<String> {
    let target = if known(&event.message_id) {
        ("global", &event.message_id)
    } else {
        ("client", &event.client_message_id)
    };
    let identity = if known(&event.event_id) {
        serde_json::to_vec(&(
            "server",
            &event.thread_id,
            target,
            &event.sender_id,
            &event.event_id,
        ))?
    } else if known(&event.client_event_id) {
        serde_json::to_vec(&(
            "client",
            &event.thread_id,
            target,
            &event.sender_id,
            &event.client_event_id,
        ))?
    } else {
        serde_json::to_vec(event)?
    };
    Ok(hash(&identity))
}
impl ReactionJournal {
    pub fn open(root: &Path, account: &str) -> Result<Self> {
        ensure!(!account.is_empty(), "Account identity required");
        let owner = hash(account.as_bytes());
        let directory = root.join("accounts").join(&owner);
        std::fs::create_dir_all(&directory)?;
        private(&root.join("accounts"), true)?;
        private(&directory, true)?;
        let path = directory.join("reactions-v2.sqlite");
        if !path.exists() {
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&path) {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error.into()),
            }
        }
        private(&path, false)?;
        let mut conn = Connection::open(&path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL;
            CREATE TABLE IF NOT EXISTS owner(owner_hash TEXT PRIMARY KEY NOT NULL);
            CREATE TABLE IF NOT EXISTS events(seq INTEGER PRIMARY KEY AUTOINCREMENT,event_key TEXT UNIQUE NOT NULL,payload TEXT NOT NULL,resolved INTEGER NOT NULL DEFAULT 0);
            CREATE INDEX IF NOT EXISTS pending_events ON events(resolved,seq);
            CREATE TABLE IF NOT EXISTS checkpoints(scope TEXT PRIMARY KEY NOT NULL,pages INTEGER NOT NULL,complete INTEGER NOT NULL,reason TEXT);")?;
        let tx = conn.transaction()?;
        let stored: Option<String> = tx
            .query_row("SELECT owner_hash FROM owner LIMIT 1", [], |r| r.get(0))
            .optional()?;
        match stored {
            Some(stored) => ensure!(stored == owner, "Reaction journal account mismatch"),
            None => {
                tx.execute("INSERT INTO owner VALUES(?1)", [owner])?;
            }
        }
        tx.commit()?;
        Ok(Self { conn, path })
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    /// Atomic batch append. Existing rows are never dropped to enforce a memory limit.
    pub fn append(&mut self, events: &[ReactionEvent]) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let mut inserted = 0;
        for event in events {
            ensure!(
                known(&event.thread_id)
                    && !event.sender_id.is_empty()
                    && (known(&event.message_id) || known(&event.client_message_id))
                    && event.r_type >= -1,
                "Invalid reaction journal event"
            );
            inserted += tx.execute(
                "INSERT OR IGNORE INTO events(event_key,payload) VALUES(?1,?2)",
                params![event_key(event)?, serde_json::to_string(event)?],
            )?;
        }
        tx.commit()?;
        Ok(inserted)
    }
    /// Bounded in-memory page. The on-disk queue has no 4096-event eviction policy.
    pub fn pending(&self, limit: usize) -> Result<Vec<PendingReaction>> {
        let mut statement = self.conn.prepare(
            "SELECT event_key,payload FROM events WHERE resolved=0 ORDER BY seq LIMIT ?1",
        )?;
        let rows = statement.query_map([limit.min(4096) as i64], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut result = Vec::new();
        for row in rows {
            let (key, payload) = row?;
            result.push(PendingReaction {
                key,
                event: serde_json::from_str(&payload)?,
            });
        }
        Ok(result)
    }
    /// Remove from the pending inbox, retaining the replay payload for crash recovery.
    pub fn resolve(&mut self, key: &str) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE events SET resolved=1 WHERE event_key=?1 AND resolved=0",
            [key],
        )? > 0)
    }
    /// Scan bounded SQLite pages, including previously resolved rows for stale-cache recovery.
    /// A missing/ambiguous target stays pending. Existing message ledgers own chronology.
    pub fn apply_loaded(&mut self, threads: &mut [Thread]) -> Result<ApplySummary> {
        let mut summary = ApplySummary::default();
        let mut after = 0_i64;
        loop {
            let rows = {
                let mut statement = self.conn.prepare("SELECT seq,event_key,payload,resolved FROM events WHERE seq>?1 ORDER BY seq LIMIT ?2")?;
                let rows = statement.query_map(params![after, JOURNAL_PAGE_SIZE as i64], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, bool>(3)?,
                    ))
                })?;
                rows.collect::<rusqlite::Result<Vec<_>>>()?
            };
            if rows.is_empty() {
                break;
            }
            for (seq, key, payload, was_resolved) in rows {
                after = seq;
                summary.scanned += 1;
                let event: ReactionEvent = serde_json::from_str(&payload)?;
                let scopes: Vec<_> = threads
                    .iter()
                    .enumerate()
                    .filter(|(_, thread)| {
                        thread.id == event.thread_id
                            || (thread.is_group
                                && thread.id.strip_prefix('g').unwrap_or(&thread.id)
                                    == event
                                        .thread_id
                                        .strip_prefix('g')
                                        .unwrap_or(&event.thread_id))
                    })
                    .map(|(index, _)| index)
                    .collect();
                let [thread_index] = scopes.as_slice() else {
                    if scopes.len() > 1 {
                        summary.ambiguous += 1;
                    }
                    continue;
                };
                let thread = &mut threads[*thread_index];
                let exact: Vec<_> = thread
                    .messages
                    .iter()
                    .enumerate()
                    .filter(|(_, m)| known(&event.message_id) && m.id == event.message_id)
                    .map(|(i, _)| i)
                    .collect();
                let matches = if exact.is_empty() {
                    thread
                        .messages
                        .iter()
                        .enumerate()
                        .filter(|(_, m)| {
                            known(&event.client_message_id)
                                && (m.client_message_id == event.client_message_id
                                    || m.history_source
                                        .as_ref()
                                        .and_then(|s| s.get("clientId"))
                                        .is_some_and(|id| {
                                            id.as_str()
                                                .map(str::to_owned)
                                                .or_else(|| id.as_u64().map(|id| id.to_string()))
                                                .as_deref()
                                                == Some(event.client_message_id.as_str())
                                        }))
                        })
                        .map(|(i, _)| i)
                        .collect()
                } else {
                    exact
                };
                let [message_index] = matches.as_slice() else {
                    if matches.len() > 1 {
                        summary.ambiguous += 1;
                    }
                    continue;
                };
                let message = &mut thread.messages[*message_index];
                let watermark = &message.reaction_snapshot_provenance;
                let covered_by_snapshot = message.reaction_authoritative_snapshot
                    && !watermark.source.is_empty()
                    && watermark.captured_at_ms > 0
                    && event.ts > 0
                    && crate::clock::normalize_ts_ms(event.ts) <= watermark.captured_at_ms;
                if !covered_by_snapshot && message.apply_reaction(&event) {
                    summary.applied += 1;
                }
                if !was_resolved && self.resolve(&key)? {
                    summary.resolved += 1;
                }
            }
        }
        summary.pending = self.pending_count()?;
        Ok(summary)
    }
    pub fn pending_count(&self) -> Result<u64> {
        Ok(self
            .conn
            .query_row("SELECT count(*) FROM events WHERE resolved=0", [], |r| {
                r.get(0)
            })?)
    }
    /// Only an explicitly completed full scan clears partial coverage for this scope.
    pub fn record_checkpoint(
        &mut self,
        scope: &str,
        pages: u64,
        complete: bool,
        reason: Option<&str>,
    ) -> Result<()> {
        ensure!(
            !scope.is_empty() && pages <= i64::MAX as u64,
            "Invalid reaction checkpoint"
        );
        let complete = complete && pages <= 20 && reason.is_none();
        let reason = if pages > 20 {
            Some("page_limit")
        } else if !complete && reason.is_none() {
            Some("incomplete")
        } else {
            reason
        };
        self.conn.execute("INSERT INTO checkpoints(scope,pages,complete,reason) VALUES(?1,?2,?3,?4) ON CONFLICT(scope) DO UPDATE SET pages=excluded.pages,complete=excluded.complete,reason=excluded.reason",params![scope,pages as i64,complete,reason])?;
        Ok(())
    }
    pub fn status(&self) -> Result<JournalStatus> {
        let mut statement = self
            .conn
            .prepare("SELECT scope,pages,complete,reason FROM checkpoints ORDER BY scope")?;
        let checkpoints = statement
            .query_map([], |r| {
                Ok(CoverageCheckpoint {
                    scope: r.get(0)?,
                    pages: r.get(1)?,
                    complete: r.get(2)?,
                    reason: r.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let pending = self.pending_count()?;
        Ok(JournalStatus {
            pending,
            total: self
                .conn
                .query_row("SELECT count(*) FROM events", [], |r| r.get(0))?,
            partial: pending > 0
                || checkpoints.is_empty()
                || checkpoints.iter().any(|c| !c.complete),
            checkpoints,
        })
    }
}
