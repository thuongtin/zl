//! Copy-on-write legacy import, account-owned storage and bounded UI hydration.
use super::*;
use std::path::Path;
pub(super) fn initialize_legacy_owner(account: Option<&str>) -> anyhow::Result<()> {
    let path = Path::new("legacy-owner.json");
    if !path.exists() {
        crate::data_dir::write_private_atomic(
            path,
            &serde_json::to_vec(&serde_json::json!({"account":account}))?,
        )?;
    }
    Ok(())
}
pub(super) fn mutation_fence() -> &'static std::sync::Mutex<()> {
    static FENCE: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    FENCE.get_or_init(Default::default)
}
fn account_directory(root: &Path, account: &str) -> std::path::PathBuf {
    use sha2::{Digest, Sha256};
    root.join("accounts")
        .join(format!("{:x}", Sha256::digest(account.as_bytes())))
}
pub(super) fn legacy_import_enabled(root: &Path, account: &str) -> bool {
    !account_directory(root, account)
        .join("local-wipe.json")
        .exists()
}
pub(super) fn open_account_store(account: &str) -> anyhow::Result<crate::store_v2::AccountStore> {
    open_account_store_at(Path::new("."), account)
}
fn open_account_store_at(
    root: &Path,
    account: &str,
) -> anyhow::Result<crate::store_v2::AccountStore> {
    let mut store = crate::store_v2::AccountStore::open(root, account)?;
    let owner = std::fs::read(root.join("legacy-owner.json"))
        .ok()
        .and_then(|v| serde_json::from_slice::<serde_json::Value>(&v).ok());
    if legacy_import_enabled(root, account)
        && owner.as_ref().and_then(|v| v["account"].as_str()) == Some(account)
    {
        let db = root.join("zalo_history.db");
        let json = root.join("threads_cache.json");
        if db.is_file() || json.is_file() {
            store.migrate_legacy(
                db.is_file().then_some(db.as_path()),
                json.is_file().then_some(json.as_path()),
            )?;
        }
    }
    Ok(store)
}
struct StagedWipe {
    entries: Vec<(std::path::PathBuf, std::path::PathBuf)>,
    marker_created: Option<std::path::PathBuf>,
}
impl StagedWipe {
    fn stage(&mut self, source: &Path) -> anyhow::Result<()> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let parent = source
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Missing cache parent"))?;
        let quarantine = loop {
            let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = parent.join(format!(".wipe-quarantine-{}-{id}", std::process::id()));
            match std::fs::create_dir(&path) {
                Ok(()) => break path,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        };
        let destination = quarantine.join("payload");
        self.entries.push((source.to_owned(), destination.clone()));
        std::fs::rename(source, destination)?;
        Ok(())
    }
    fn rollback(&mut self) -> anyhow::Result<()> {
        let mut failures = Vec::new();
        if let Some(marker) = self.marker_created.take() {
            if let Err(error) = std::fs::remove_file(marker) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    failures.push(error);
                }
            }
        }
        for (original, staged) in self.entries.drain(..).rev() {
            if staged.exists() {
                if let Err(error) = std::fs::rename(&staged, original) {
                    failures.push(error);
                    continue;
                }
            }
            if let Some(parent) = staged.parent() {
                if let Err(error) = std::fs::remove_dir(parent) {
                    failures.push(error);
                }
            }
        }
        anyhow::ensure!(
            failures.is_empty(),
            "Wipe rollback encountered {} filesystem errors",
            failures.len()
        );
        Ok(())
    }
    fn discard(self) {
        // A committed clear must always acknowledge success; quarantine cleanup is retryable housekeeping.
        for (_, staged) in self.entries {
            if let Some(parent) = staged.parent() {
                if let Err(error) = std::fs::remove_dir_all(parent) {
                    crate::logger::log_error("gui::wipe_quarantine_cleanup", &error.to_string());
                }
            }
        }
    }
}
/// Stage cache cleanup reversibly, then atomically clear account history as the final fallible step.
/// Session credentials, existing import fences and other accounts remain unchanged on failure.
pub(super) fn wipe_account(root: &Path, account: &str) -> anyhow::Result<()> {
    anyhow::ensure!(!account.is_empty(), "Account identity required");
    let _fence = mutation_fence().lock().unwrap_or_else(|e| e.into_inner());
    let mut store = crate::store_v2::AccountStore::open(root, account)?;
    let directory = account_directory(root, account);
    let marker = directory.join("local-wipe.json");
    let mut sources = Vec::new();
    for name in [
        "reactions-v2.sqlite",
        "reactions-v2.sqlite-wal",
        "reactions-v2.sqlite-shm",
        "composer.json",
    ] {
        let path = directory.join(name);
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) => {
                anyhow::ensure!(metadata.is_file(), "Account cache must be a regular file");
                sources.push(path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    match std::fs::symlink_metadata(&marker) {
        Ok(metadata) => anyhow::ensure!(metadata.is_file(), "Import fence must be a regular file"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    use sha2::{Digest, Sha256};
    let media = root
        .join("media_cache")
        .join(format!("{:x}", Sha256::digest(account.as_bytes())));
    match std::fs::symlink_metadata(&media) {
        Ok(metadata) => {
            anyhow::ensure!(metadata.is_dir(), "Account media cache must be a directory");
            sources.push(media);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let mut staged = StagedWipe {
        entries: Vec::new(),
        marker_created: None,
    };
    let result = (|| -> anyhow::Result<()> {
        for source in sources {
            staged.stage(&source)?;
        }
        if !marker.exists() {
            staged.marker_created = Some(marker.clone());
            crate::data_dir::write_private_atomic(&marker, b"{\"legacy_import_disabled\":true}")?;
        }
        store.clear_local_history()?;
        Ok(())
    })();
    if let Err(error) = result {
        staged
            .rollback()
            .map_err(|rollback| anyhow::anyhow!("{error}; {rollback}"))?;
        return Err(error);
    }
    drop(store);
    staged.discard();
    Ok(())
}
pub(super) fn flush_shutdown(
    root: &Path,
    account: &str,
    threads: &[Thread],
    read_deltas: &[(String, Vec<ChatMessage>)],
    draft: Option<(String, crate::runtime_state::Draft)>,
) -> anyhow::Result<()> {
    if account.is_empty() {
        return Ok(());
    }
    let _fence = mutation_fence().lock().unwrap_or_else(|e| e.into_inner());
    let mut store = open_account_store_at(root, account)?;
    store.persist_threads(threads)?;
    for (thread, messages) in read_deltas {
        store.read_local_visible(thread, messages)?;
    }
    if let Some((thread, draft)) = draft {
        crate::runtime_state::ComposerStore::open(root, account)?.set_draft(&thread, draft)?;
    }
    Ok(())
}
pub(super) fn hydrate_model(message: &mut ChatMessage) {
    use crate::message_model::MessageKind as K;
    if let Some(source) = message.history_source.as_ref() {
        message.kind = source["msgType"]
            .as_i64()
            .map(K::from_archive_code)
            .unwrap_or_default();
        // Archive IDs are not interchangeable with server receipt IDs.
        message.wire_ref = Default::default();
    } else if message.kind == K::Unknown && message.wire_ref.original_type.is_none() {
        message.kind = if message.sticker_id.is_some() {
            K::Sticker
        } else if message.link_src.as_deref() == Some("photo") {
            K::Photo
        } else if message.link_src.as_deref() == Some("video") {
            K::Video
        } else {
            K::Text
        };
    }
    message.ts = crate::clock::normalize_ts_ms(message.ts);
    if message.ts > 0 {
        message.time = crate::clock::message_time(message.ts);
    }
    if message.history_source.is_none() || message.kind != K::Unknown {
        message.hydrate_media_metadata();
    }
}
pub(super) fn load_page(
    account: &str,
    thread: &str,
    cursor: Option<&crate::store_v2::HistoryCursor>,
    direction: crate::store_v2::PageDirection,
) -> anyhow::Result<(Vec<ChatMessage>, bool)> {
    let store = open_account_store(account)?;
    let page = store.history_page(thread, cursor, direction)?;
    let mut messages = page
        .messages
        .iter()
        .map(|m| store.chat_message(m))
        .collect::<anyhow::Result<Vec<_>>>()?;
    for message in &mut messages {
        hydrate_model(message);
    }
    let mut threads = vec![Thread {
        presentation: Default::default(),
        id: thread.into(),
        name: String::new(),
        preview: String::new(),
        time: String::new(),
        avatar: None,
        is_group: thread.starts_with('g'),
        messages,
        unread: Default::default(),
    }];
    if let Ok(mut journal) = crate::reaction_journal::ReactionJournal::open(Path::new("."), account)
    {
        journal.apply_loaded(&mut threads)?;
    }
    let messages = threads.pop().unwrap().messages;
    Ok((messages, page.has_more))
}
impl ZaloGuiApp {
    pub(super) fn retain_message_window(&mut self) {
        const MAX_UI_MESSAGES: usize = 2000;
        let selected = self.selected_thread_index;
        let mut remaining = MAX_UI_MESSAGES;
        if let Some(t) = self.threads.get_mut(selected) {
            if t.messages.len() > 200 {
                t.messages.drain(..t.messages.len() - 200);
            }
            remaining = remaining.saturating_sub(t.messages.len());
        }
        for (index, t) in self.threads.iter_mut().enumerate() {
            if index == selected {
                continue;
            }
            let keep = t.messages.len().min(remaining).min(20);
            if t.messages.len() > keep {
                t.messages.drain(..t.messages.len() - keep);
            }
            remaining = remaining.saturating_sub(keep);
        }
    }
    pub(super) fn save_composer(&mut self) {
        let Some(t) = self.threads.get(self.selected_thread_index) else {
            return;
        };
        if let Some(s) = self.composer_store.as_mut() {
            let _ = s.set_draft(
                &t.id,
                crate::runtime_state::Draft {
                    text: self.input_message.clone(),
                    reply: self.replying_to.clone(),
                    attachments: self.composer_attachments.clone(),
                },
            );
        }
    }
    pub(super) fn select_conversation(&mut self, index: usize) {
        if self.selected_thread_index == index
            && self
                .threads
                .get(index)
                .is_some_and(|t| !t.messages.is_empty())
        {
            return;
        }
        self.save_composer();
        self.navigation_request = self.navigation_request.wrapping_add(1);
        self.pending_jump = None;
        self.mention_profile = None;
        self.selected_thread_index = index;
        if let Some(t) = self.threads.get(index) {
            let draft = self
                .composer_store
                .as_ref()
                .map(|s| s.draft(&t.id))
                .unwrap_or_default();
            self.input_message = draft.text;
            self.replying_to = draft.reply;
            self.composer_attachments = draft.attachments;
            if let Some(tx) = WORKER_TX.get() {
                let _ = tx.send(WorkerCmd::LoadLocalPage {
                    request_id: {
                        self.navigation_request = self.navigation_request.wrapping_add(1);
                        self.pending_jump = None;
                        self.navigation_request
                    },
                    thread_id: t.id.clone(),
                    cursor: None,
                    direction: crate::store_v2::PageDirection::Before,
                    replace: true,
                });
            }
        }
    }
}

#[cfg(test)]
mod lifecycle_wipe_tests {
    use super::*;
    #[test]
    fn wipe_is_account_scoped_and_restart_never_reimports_legacy() {
        let root = std::env::temp_dir().join(format!("wipe-lifecycle-{}", std::process::id()));
        if root.exists() {
            std::fs::remove_dir_all(&root).unwrap();
        }
        std::fs::create_dir_all(&root).unwrap();
        let thread = Thread {
            id: "fixture-thread".into(),
            name: "Fixture".into(),
            preview: String::new(),
            time: String::new(),
            avatar: None,
            is_group: false,
            presentation: Default::default(),
            unread: Default::default(),
            messages: vec![ChatMessage {
                id: "42".into(),
                text: "legacy".into(),
                ..Default::default()
            }],
        };
        std::fs::write(root.join("session.json"), b"fixture-session").unwrap();
        std::fs::write(
            root.join("legacy-owner.json"),
            br#"{"account":"fixture-A"}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("threads_cache.json"),
            serde_json::to_vec(&vec![thread.clone()]).unwrap(),
        )
        .unwrap();
        open_account_store_at(&root, "fixture-A").unwrap();
        open_account_store_at(&root, "fixture-B")
            .unwrap()
            .persist_threads(&[thread])
            .unwrap();
        let directory = account_directory(&root, "fixture-A");
        for file in ["composer.json", "reactions-v2.sqlite"] {
            std::fs::write(directory.join(file), b"fixture-cache").unwrap();
        }
        use sha2::{Digest, Sha256};
        let media_a = root
            .join("media_cache")
            .join(format!("{:x}", Sha256::digest(b"fixture-A")));
        let media_b = root
            .join("media_cache")
            .join(format!("{:x}", Sha256::digest(b"fixture-B")));
        std::fs::create_dir_all(&media_a).unwrap();
        std::fs::create_dir_all(&media_b).unwrap();
        std::fs::write(media_a.join("fixture.media"), b"a").unwrap();
        std::fs::write(media_b.join("fixture.media"), b"b").unwrap();
        wipe_account(&root, "fixture-A").unwrap();
        assert!(!legacy_import_enabled(&root, "fixture-A"));
        assert!(open_account_store_at(&root, "fixture-A")
            .unwrap()
            .all_thread_metadata()
            .unwrap()
            .is_empty());
        assert!(open_account_store_at(&root, "fixture-B")
            .unwrap()
            .get_message("fixture-thread", "42")
            .unwrap()
            .is_some());
        assert_eq!(
            std::fs::read(root.join("session.json")).unwrap(),
            b"fixture-session"
        );
        assert!(root.join("threads_cache.json").exists());
        assert!(!directory.join("composer.json").exists());
        assert!(!directory.join("reactions-v2.sqlite").exists());
        assert!(!media_a.exists());
        assert_eq!(std::fs::read(media_b.join("fixture.media")).unwrap(), b"b");
        // Even a modified shared legacy source is not imported again after the explicit wipe.
        std::fs::write(
            root.join("threads_cache.json"),
            b"invalid changed legacy fixture",
        )
        .unwrap();
        assert!(open_account_store_at(&root, "fixture-A")
            .unwrap()
            .all_thread_metadata()
            .unwrap()
            .is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }
    fn failure_fixture(label: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("wipe-{label}-{}", std::process::id()));
        if root.exists() {
            std::fs::remove_dir_all(&root).unwrap();
        }
        std::fs::create_dir_all(&root).unwrap();
        let thread = Thread {
            id: "fixture-thread".into(),
            name: "Fixture".into(),
            preview: String::new(),
            time: String::new(),
            avatar: None,
            is_group: false,
            presentation: Default::default(),
            unread: Default::default(),
            messages: vec![ChatMessage {
                id: "42".into(),
                text: "keep history".into(),
                ..Default::default()
            }],
        };
        for account in ["fixture-A", "fixture-B"] {
            crate::store_v2::AccountStore::open(&root, account)
                .unwrap()
                .persist_threads(std::slice::from_ref(&thread))
                .unwrap();
            std::fs::write(
                account_directory(&root, account).join("composer.json"),
                b"keep composer",
            )
            .unwrap();
        }
        std::fs::write(root.join("session.json"), b"keep session").unwrap();
        root
    }
    fn assert_failure_preserved(root: &Path) {
        for account in ["fixture-A", "fixture-B"] {
            let store = crate::store_v2::AccountStore::open(root, account).unwrap();
            assert_eq!(
                store
                    .get_message("fixture-thread", "42")
                    .unwrap()
                    .unwrap()
                    .text,
                "keep history"
            );
            assert_eq!(
                std::fs::read(account_directory(root, account).join("composer.json")).unwrap(),
                b"keep composer"
            );
        }
        assert_eq!(
            std::fs::read(root.join("session.json")).unwrap(),
            b"keep session"
        );
    }
    #[test]
    fn invalid_reaction_cache_directory_fails_before_changing_history_or_composer() {
        let root = failure_fixture("invalid-reaction-directory");
        let directory = account_directory(&root, "fixture-A");
        std::fs::create_dir(directory.join("reactions-v2.sqlite")).unwrap();
        std::fs::write(
            directory.join("reactions-v2.sqlite/keep.txt"),
            b"keep directory",
        )
        .unwrap();
        std::fs::write(directory.join("local-wipe.json"), b"existing marker bytes").unwrap();
        assert!(wipe_account(&root, "fixture-A").is_err());
        assert_failure_preserved(&root);
        assert_eq!(
            std::fs::read(directory.join("local-wipe.json")).unwrap(),
            b"existing marker bytes"
        );
        assert_eq!(
            std::fs::read(directory.join("reactions-v2.sqlite/keep.txt")).unwrap(),
            b"keep directory"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn sql_clear_failure_rolls_back_staged_caches_and_preserves_existing_marker() {
        for existing_marker in [false, true] {
            let root = failure_fixture(if existing_marker {
                "sql-marker-existing"
            } else {
                "sql-marker-new"
            });
            let directory = account_directory(&root, "fixture-A");
            let marker = directory.join("local-wipe.json");
            if existing_marker {
                std::fs::write(&marker, b"existing marker bytes").unwrap();
            }
            std::fs::write(directory.join("reactions-v2.sqlite"), b"keep reactions").unwrap();
            use sha2::{Digest, Sha256};
            let media = root
                .join("media_cache")
                .join(format!("{:x}", Sha256::digest(b"fixture-A")));
            std::fs::create_dir_all(&media).unwrap();
            std::fs::write(media.join("keep.media"), b"keep media").unwrap();
            let connection =
                rusqlite::Connection::open(directory.join("history-v2.sqlite3")).unwrap();
            connection.execute_batch("CREATE TRIGGER fail_wipe BEFORE DELETE ON messages BEGIN SELECT RAISE(ABORT, 'fixture failure'); END;").unwrap();
            drop(connection);
            assert!(wipe_account(&root, "fixture-A").is_err());
            assert_failure_preserved(&root);
            assert_eq!(
                std::fs::read(directory.join("reactions-v2.sqlite")).unwrap(),
                b"keep reactions"
            );
            assert_eq!(
                std::fs::read(media.join("keep.media")).unwrap(),
                b"keep media"
            );
            if existing_marker {
                assert_eq!(std::fs::read(&marker).unwrap(), b"existing marker bytes");
            } else {
                assert!(!marker.exists());
            }
            assert!(!std::fs::read_dir(&directory).unwrap().any(|entry| entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".wipe-quarantine-")));
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}
