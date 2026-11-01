//! Account-scoped asynchronous work and durable per-conversation drafts/outbox.
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionScope {
    pub account: String,
    pub generation: u64,
}
#[derive(Default)]
pub struct ScopeRegistry {
    state: Mutex<SessionScope>,
    changed: tokio::sync::Notify,
}
impl ScopeRegistry {
    pub fn current(&self) -> SessionScope {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    pub fn bind(&self, account: &str) -> SessionScope {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if s.account != account {
            s.generation = s.generation.wrapping_add(1);
            s.account = account.into();
            self.changed.notify_waiters();
        }
        s.clone()
    }
    pub fn invalidate(&self) -> SessionScope {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        s.generation = s.generation.wrapping_add(1);
        s.account.clear();
        self.changed.notify_waiters();
        s.clone()
    }
    pub fn accepts(&self, s: &SessionScope) -> bool {
        self.current() == *s
    }
    pub async fn cancelled(&self, s: &SessionScope) {
        loop {
            let n = self.changed.notified();
            if !self.accepts(s) {
                return;
            }
            n.await;
        }
    }
}
pub fn registry() -> &'static ScopeRegistry {
    static R: OnceLock<ScopeRegistry> = OnceLock::new();
    R.get_or_init(Default::default)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Draft {
    pub text: String,
    pub reply: Option<crate::chat::ChatMessage>,
    #[serde(default)]
    pub attachments: Vec<PathBuf>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboxEntry {
    #[serde(default)]
    pub attachment_client_ids: Vec<String>,
    pub id: String,
    pub thread_id: String,
    pub draft: Draft,
    pub state: crate::message_model::DeliveryState,
}
#[derive(Clone, Default, Serialize, Deserialize)]
struct ComposerData {
    account: String,
    drafts: BTreeMap<String, Draft>,
    outbox: BTreeMap<String, OutboxEntry>,
    next_id: u64,
}
pub struct ComposerStore {
    path: PathBuf,
    data: ComposerData,
}
fn terminal_delivery(state: crate::message_model::DeliveryState) -> bool {
    matches!(
        state,
        crate::message_model::DeliveryState::ServerAccepted
            | crate::message_model::DeliveryState::Delivered
            | crate::message_model::DeliveryState::Read
    )
}
fn composer_mutations() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}
impl ComposerStore {
    pub fn open(root: &Path, account: &str) -> Result<Self> {
        ensure!(!account.is_empty(), "Account identity required");
        let dir = root
            .join("accounts")
            .join(format!("{:x}", Sha256::digest(account.as_bytes())));
        std::fs::create_dir_all(&dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        }
        let path = dir.join("composer.json");
        let data = if path.is_file() {
            serde_json::from_slice::<ComposerData>(&std::fs::read(&path)?)?
        } else {
            ComposerData {
                account: account.into(),
                ..Default::default()
            }
        };
        ensure!(data.account == account, "Composer owner mismatch");
        Ok(Self { path, data })
    }
    fn snapshot(&self) -> Result<ComposerData> {
        if !self.path.exists() {
            return Ok(self.data.clone());
        }
        let data: ComposerData = serde_json::from_slice(&std::fs::read(&self.path)?)?;
        ensure!(data.account == self.data.account, "Composer owner mismatch");
        Ok(data)
    }
    fn reload(&mut self) -> Result<()> {
        let mut data = self.snapshot()?;
        data.outbox
            .retain(|_, entry| !terminal_delivery(entry.state));
        self.data = data;
        Ok(())
    }
    pub fn draft(&self, thread: &str) -> Draft {
        self.snapshot()
            .unwrap_or_else(|_| self.data.clone())
            .drafts
            .get(thread)
            .cloned()
            .unwrap_or_default()
    }
    pub fn set_draft(&mut self, thread: &str, draft: Draft) -> Result<()> {
        let _guard = composer_mutations()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.reload()?;
        let before = self.data.drafts.insert(thread.into(), draft);
        if let Err(error) = self.save() {
            if let Some(previous) = before {
                self.data.drafts.insert(thread.into(), previous);
            } else {
                self.data.drafts.remove(thread);
            }
            return Err(error);
        }
        Ok(())
    }
    pub fn stage(&mut self, thread: &str, draft: Draft) -> Result<String> {
        let _guard = composer_mutations()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.reload()?;
        ensure!(!thread.is_empty(), "Conversation identity required");
        ensure!(self.data.outbox.len() < 128, "Outbox pending limit reached");
        ensure!(
            draft.attachments.len() <= 32,
            "Attachment batch limit reached"
        );
        let now = u64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_millis(),
        )?;
        let persisted_max = self
            .data
            .outbox
            .keys()
            .filter_map(|id| id.parse::<u64>().ok())
            .max()
            .unwrap_or(0);
        let previous = self.data.next_id.max(persisted_max);
        let next = previous
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("Client ID space exhausted"))?
            .max(now);
        let before = self.data.clone();
        let attachment_client_ids = (0..draft.attachments.len())
            .map(|index| {
                next.checked_add(index as u64 + 1)
                    .map(|id| id.to_string())
                    .ok_or_else(|| anyhow::anyhow!("Client ID space exhausted"))
            })
            .collect::<Result<Vec<_>>>()?;
        self.data.next_id = next
            .checked_add(draft.attachments.len() as u64)
            .ok_or_else(|| anyhow::anyhow!("Client ID space exhausted"))?;
        let id = next.to_string();
        self.data.outbox.insert(
            id.clone(),
            OutboxEntry {
                attachment_client_ids,
                id: id.clone(),
                thread_id: thread.into(),
                draft,
                state: crate::message_model::DeliveryState::Pending,
            },
        );
        self.data.drafts.remove(thread);
        if let Err(error) = self.save() {
            self.data = before;
            return Err(error);
        }
        Ok(id)
    }
    pub fn complete(&mut self, id: &str, state: crate::message_model::DeliveryState) -> Result<()> {
        let _guard = composer_mutations()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.reload()?;
        let before = self.data.clone();
        let terminal = if let Some(entry) = self.data.outbox.get_mut(id) {
            entry.state = entry.state.merge(state);
            terminal_delivery(entry.state)
        } else {
            false
        };
        if terminal {
            self.data.outbox.remove(id);
        }
        if let Err(error) = self.save() {
            self.data = before;
            return Err(error);
        }
        Ok(())
    }
    pub fn recover_interrupted(&mut self) -> Result<usize> {
        let _guard = composer_mutations()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.reload()?;
        let before = self.data.clone();
        let mut count = 0;
        for entry in self.data.outbox.values_mut() {
            if entry.state == crate::message_model::DeliveryState::Pending {
                entry.state = crate::message_model::DeliveryState::DeliveryUnknown;
                count += 1;
            }
        }
        if count > 0 {
            if let Err(error) = self.save() {
                self.data = before;
                return Err(error);
            }
        }
        Ok(count)
    }
    pub fn complete_attachment(&mut self, id: &str, path: &Path) -> Result<()> {
        let _guard = composer_mutations()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.reload()?;
        let before = self.data.clone();
        let entry = self
            .data
            .outbox
            .get_mut(id)
            .ok_or_else(|| anyhow::anyhow!("Outbox entry not found"))?;
        if let Some(index) = entry.draft.attachments.iter().position(|item| item == path) {
            entry.draft.attachments.remove(index);
            if index < entry.attachment_client_ids.len() {
                entry.attachment_client_ids.remove(index);
            }
        }
        if let Err(error) = self.save() {
            self.data = before;
            return Err(error);
        }
        Ok(())
    }
    pub fn failed_for(&self, thread: &str) -> Vec<OutboxEntry> {
        self.snapshot()
            .unwrap_or_else(|_| self.data.clone())
            .outbox
            .values()
            .filter(|e| {
                e.thread_id == thread
                    && matches!(
                        e.state,
                        crate::message_model::DeliveryState::Failed
                            | crate::message_model::DeliveryState::DeliveryUnknown
                    )
            })
            .cloned()
            .collect()
    }
    pub fn pending(&self) -> Vec<OutboxEntry> {
        self.snapshot()
            .unwrap_or_else(|_| self.data.clone())
            .outbox
            .values()
            .filter(|e| {
                matches!(
                    e.state,
                    crate::message_model::DeliveryState::Pending
                        | crate::message_model::DeliveryState::DeliveryUnknown
                        | crate::message_model::DeliveryState::Failed
                )
            })
            .cloned()
            .collect()
    }
    pub fn entry(&self, id: &str) -> Option<OutboxEntry> {
        self.snapshot()
            .unwrap_or_else(|_| self.data.clone())
            .outbox
            .get(id)
            .cloned()
    }
    fn save(&self) -> Result<()> {
        let tmp = self
            .path
            .with_extension(format!("tmp-{}", std::process::id()));
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        use std::io::Write;
        let mut f = opts.open(&tmp)?;
        f.write_all(&serde_json::to_vec(&self.data)?)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(tmp, &self.path)?;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scope_rejects_late_results_after_account_switch() {
        let r = ScopeRegistry::default();
        let a = r.bind("A");
        r.bind("B");
        assert!(!r.accepts(&a));
        let b = r.current();
        r.invalidate();
        assert!(!r.accepts(&b));
    }
    #[test]
    fn composer_failure_keeps_exact_owner_and_thread() {
        let p = std::env::temp_dir().join(format!("composer-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        let mut s = ComposerStore::open(&p, "A").unwrap();
        let id = s
            .stage(
                "chat-A",
                Draft {
                    text: "original".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        s.set_draft(
            "chat-B",
            Draft {
                text: "new".into(),
                ..Default::default()
            },
        )
        .unwrap();
        s.complete(&id, crate::message_model::DeliveryState::Failed)
            .unwrap();
        assert_eq!(s.draft("chat-B").text, "new");
        assert_eq!(s.failed_for("chat-A")[0].draft.text, "original");
        drop(s);
        let s = ComposerStore::open(&p, "A").unwrap();
        assert_eq!(s.failed_for("chat-A").len(), 1);
        assert!(ComposerStore::open(&p, "B").unwrap().pending().is_empty());
        let _ = std::fs::remove_dir_all(p);
    }
}

#[cfg(test)]
mod durable_id_tests {
    use super::*;
    #[test]
    fn staged_client_ids_are_decimal_monotonic_and_persisted() {
        let p = std::env::temp_dir().join(format!("composer-id-test-{}", std::process::id()));
        let mut store = ComposerStore::open(&p, "fixture-account").unwrap();
        let first = store
            .stage(
                "A",
                Draft {
                    text: "first".into(),
                    ..Default::default()
                },
            )
            .unwrap()
            .parse::<u64>()
            .unwrap();
        store.data.next_id = first + 10000;
        store.save().unwrap();
        let second = store
            .stage(
                "B",
                Draft {
                    text: "second".into(),
                    ..Default::default()
                },
            )
            .unwrap()
            .parse::<u64>()
            .unwrap();
        assert!(second > first + 10000);
        drop(store);
        let mut store = ComposerStore::open(&p, "fixture-account").unwrap();
        let third = store
            .stage(
                "A",
                Draft {
                    text: "third".into(),
                    ..Default::default()
                },
            )
            .unwrap()
            .parse::<u64>()
            .unwrap();
        assert!(third > second);
        assert_eq!(store.pending().len(), 3);
        std::fs::remove_dir_all(p).unwrap();
    }
    #[test]
    fn stage_failure_preserves_draft_and_does_not_enqueue() {
        let p = std::env::temp_dir().join(format!("composer-rollback-test-{}", std::process::id()));
        let mut store = ComposerStore::open(&p, "fixture-account").unwrap();
        store
            .set_draft(
                "A",
                Draft {
                    text: "keep".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        store.path = p.join("missing-parent/composer.json");
        assert!(store
            .stage(
                "A",
                Draft {
                    text: "keep".into(),
                    ..Default::default()
                }
            )
            .is_err());
        assert_eq!(store.draft("A").text, "keep");
        assert!(store.pending().is_empty());
        std::fs::remove_dir_all(p).unwrap();
    }
}

#[cfg(test)]
mod concurrent_store_tests {
    use super::*;
    #[test]
    fn alternating_store_instances_preserve_drafts_and_attachment_ack() {
        let root =
            std::env::temp_dir().join(format!("composer-multiple-test-{}", std::process::id()));
        let mut ui = ComposerStore::open(&root, "fixture").unwrap();
        let mut worker = ComposerStore::open(&root, "fixture").unwrap();
        ui.set_draft(
            "A",
            Draft {
                text: "draft A".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let path = PathBuf::from("fixture-photo.png");
        let id = ui
            .stage(
                "A",
                Draft {
                    text: "caption".into(),
                    attachments: vec![path.clone()],
                    ..Default::default()
                },
            )
            .unwrap();
        worker
            .set_draft(
                "B",
                Draft {
                    text: "draft B".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        ui.set_draft(
            "C",
            Draft {
                text: "draft C".into(),
                ..Default::default()
            },
        )
        .unwrap();
        worker.complete_attachment(&id, &path).unwrap();
        assert!(ui.entry(&id).unwrap().draft.attachments.is_empty());
        ui.set_draft(
            "B",
            Draft {
                text: "draft B edited".into(),
                ..Default::default()
            },
        )
        .unwrap();
        worker
            .complete(&id, crate::message_model::DeliveryState::ServerAccepted)
            .unwrap();
        assert_eq!(ui.draft("B").text, "draft B edited");
        assert_eq!(worker.draft("C").text, "draft C");
        assert!(ui.entry(&id).is_none());
        let second = worker
            .stage(
                "B",
                Draft {
                    text: "next".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(second.parse::<u64>().unwrap() > id.parse::<u64>().unwrap());
        assert_eq!(ui.pending().len(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod terminal_outbox_tests {
    use super::*;
    #[test]
    fn terminal_completions_keep_the_composer_file_bounded() {
        let root =
            std::env::temp_dir().join(format!("composer-bounded-test-{}", std::process::id()));
        let mut store = ComposerStore::open(&root, "fixture").unwrap();
        let mut last = 0u64;
        for index in 0..128 {
            let id = store
                .stage(
                    "thread",
                    Draft {
                        text: format!("message {index}"),
                        ..Default::default()
                    },
                )
                .unwrap();
            assert!(id.parse::<u64>().unwrap() > last);
            last = id.parse().unwrap();
            store
                .complete(&id, crate::message_model::DeliveryState::ServerAccepted)
                .unwrap();
            assert!(store.pending().is_empty());
            assert!(store.entry(&id).is_none());
        }
        assert!(std::fs::metadata(&store.path).unwrap().len() < 1024);
        assert_eq!(store.data.next_id, last);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod auth_recovery_tests {
    use super::*;
    #[tokio::test]
    async fn same_account_auth_refresh_cancels_old_epoch() {
        let registry = ScopeRegistry::default();
        let old = registry.bind("A");
        registry.invalidate();
        let new = registry.bind("A");
        assert_ne!(old, new);
        assert!(!registry.accepts(&old));
        assert!(registry.accepts(&new));
        tokio::time::timeout(
            std::time::Duration::from_millis(50),
            registry.cancelled(&old),
        )
        .await
        .unwrap();
    }
    #[test]
    fn interrupted_send_recovers_as_unknown_without_resending() {
        let p = std::env::temp_dir().join(format!("composer-recovery-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        let mut store = ComposerStore::open(&p, "A").unwrap();
        let id = store
            .stage(
                "chat",
                Draft {
                    text: "keep".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        drop(store);
        let mut reopened = ComposerStore::open(&p, "A").unwrap();
        assert_eq!(reopened.recover_interrupted().unwrap(), 1);
        assert_eq!(
            reopened.entry(&id).unwrap().state,
            crate::message_model::DeliveryState::DeliveryUnknown
        );
        assert_eq!(reopened.entry(&id).unwrap().draft.text, "keep");
        assert_eq!(reopened.recover_interrupted().unwrap(), 0);
        let _ = std::fs::remove_dir_all(p);
    }
}
