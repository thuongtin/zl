//! Disposable account database fixtures for forward-only schema upgrades.
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};
use zalo_rust::{
    chat::{ChatMessage, Thread},
    store_v2::{AccountStore, ValidatedAlias, CURRENT_SCHEMA_VERSION},
};

fn thread(id: &str) -> Thread {
    Thread {
        id: id.into(),
        name: "Fixture".into(),
        preview: String::new(),
        time: String::new(),
        avatar: None,
        is_group: false,
        messages: Vec::new(),
        unread: Default::default(),
        presentation: Default::default(),
    }
}

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "zl-schema-{}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn seeded(&self) -> PathBuf {
        let mut store = AccountStore::open(&self.0, "account-a").unwrap();
        let mut thread = Thread {
            id: "thread-1".into(),
            name: "Fixture".into(),
            messages: vec![ChatMessage {
                id: "101".into(),
                text: "migrationneedle".into(),
                ts: 1,
                history_source: Some(
                    serde_json::json!({"meta":{"mentionsList":[{"uid":"account-a"}]}}),
                ),
                ..Default::default()
            }],
            ..thread("thread-1")
        };
        thread.unread.last_seen_id = Some("100".into());
        thread.unread.unread_ids.insert("101".into());
        store.persist_threads(&[thread]).unwrap();
        store
            .register_alias(
                ValidatedAlias::from_protocol_identity("old", "thread-1", true).unwrap(),
            )
            .unwrap();
        let path = store.path.clone().unwrap();
        drop(store);
        let conn = Connection::open(&path).unwrap();
        conn.execute(
            "INSERT INTO migration_manifest VALUES('fixture','{\"messages\":1}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message_tombstones VALUES('thread-1','deleted')",
            [],
        )
        .unwrap();
        path
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn version(path: &Path) -> u32 {
    Connection::open(path)
        .unwrap()
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap()
}
fn snapshot(path: &Path) -> Vec<(String, Vec<String>)> {
    let conn = Connection::open(path).unwrap();
    [
        "owner",
        "threads",
        "thread_unread",
        "messages",
        "aliases",
        "migration_manifest",
        "message_tombstones",
        "message_search",
    ]
    .into_iter()
    .map(|table| {
        let mut stmt = conn
            .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
            .unwrap();
        let width = stmt.column_count();
        let rows = stmt
            .query_map([], |row| {
                Ok((0..width)
                    .map(|i| format!("{:?}", row.get_ref(i).unwrap()))
                    .collect::<Vec<_>>()
                    .join("|"))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        (table.to_owned(), rows)
    })
    .collect()
}

#[test]
fn fresh_schema_is_versioned_and_reopens_in_wal_mode() {
    let fixture = Fixture::new();
    let path = fixture.seeded();
    assert_eq!(version(&path), CURRENT_SCHEMA_VERSION);
    let before = snapshot(&path);
    for _ in 0..2 {
        let store = AccountStore::open(&fixture.0, "account-a").unwrap();
        assert_eq!(
            store.search("migrationneedle", None, 0, 10).unwrap().len(),
            1
        );
        let conn = Connection::open(&path).unwrap();
        let journal: String = conn
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .unwrap();
        assert_eq!(journal, "wal");
    }
    assert_eq!(snapshot(&path), before);
}

#[test]
fn unversioned_current_schema_preserves_every_table_and_owner() {
    let fixture = Fixture::new();
    let path = fixture.seeded();
    Connection::open(&path)
        .unwrap()
        .pragma_update(None, "user_version", 0)
        .unwrap();
    let before = snapshot(&path);
    drop(AccountStore::open(&fixture.0, "account-a").unwrap());
    assert_eq!(version(&path), CURRENT_SCHEMA_VERSION);
    assert_eq!(snapshot(&path), before);
}

#[test]
fn old_mention_schema_backfills_once_for_version_zero_and_one() {
    for old_version in [0, 1] {
        let fixture = Fixture::new();
        let path = fixture.seeded();
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "DROP INDEX mention_cursor; ALTER TABLE messages DROP COLUMN mention_eligible;",
        )
        .unwrap();
        conn.pragma_update(None, "user_version", old_version)
            .unwrap();
        drop(conn);
        let store = AccountStore::open(&fixture.0, "account-a").unwrap();
        let mut unread = store.unread("thread-1").unwrap();
        store.derive_unread("thread-1", &mut unread).unwrap();
        assert!(unread.mention_ids.contains("101"));
        assert_eq!(store.resolve_thread("old").unwrap(), "thread-1");
        assert_eq!(
            store.get_message("thread-1", "101").unwrap().unwrap().text,
            "migrationneedle"
        );
        drop(store);
        assert_eq!(version(&path), CURRENT_SCHEMA_VERSION);
        let before = snapshot(&path);
        drop(AccountStore::open(&fixture.0, "account-a").unwrap());
        assert_eq!(snapshot(&path), before);
    }
}

#[test]
fn newer_version_is_rejected_before_any_database_mutation() {
    let fixture = Fixture::new();
    let path = fixture.seeded();
    let conn = Connection::open(&path).unwrap();
    conn.pragma_update(None, "journal_mode", "DELETE").unwrap();
    conn.pragma_update(None, "user_version", CURRENT_SCHEMA_VERSION + 1)
        .unwrap();
    drop(conn);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
    }
    let before = std::fs::read(&path).unwrap();
    let error = AccountStore::open(&fixture.0, "account-a")
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("nâng cấp"));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(!path.with_extension("sqlite3-wal").exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
    }
}

#[test]
fn owner_mismatch_is_rejected_before_schema_upgrade() {
    let fixture = Fixture::new();
    let path = fixture.seeded();
    let conn = Connection::open(&path).unwrap();
    conn.execute("UPDATE owner SET account='account-b'", [])
        .unwrap();
    conn.pragma_update(None, "user_version", 0).unwrap();
    drop(conn);
    let before = std::fs::read(&path).unwrap();
    assert!(AccountStore::open(&fixture.0, "account-a").is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(version(&path), 0);
}

#[test]
fn failed_mention_backfill_rolls_back_column_and_version_then_retries() {
    let fixture = Fixture::new();
    let path = fixture.seeded();
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "DROP INDEX mention_cursor; ALTER TABLE messages DROP COLUMN mention_eligible;",
    )
    .unwrap();
    conn.execute("UPDATE messages SET chat_json='invalid-json'", [])
        .unwrap();
    conn.pragma_update(None, "user_version", 1).unwrap();
    drop(conn);
    let before = snapshot(&path);
    assert!(AccountStore::open(&fixture.0, "account-a").is_err());
    assert_eq!(version(&path), 1);
    assert_eq!(snapshot(&path), before);
    let conn = Connection::open(&path).unwrap();
    let has_column: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('messages') WHERE name='mention_eligible')", [], |row| row.get(0)).unwrap();
    assert!(!has_column);
    conn.execute("UPDATE messages SET chat_json=NULL", [])
        .unwrap();
    drop(conn);
    drop(AccountStore::open(&fixture.0, "account-a").unwrap());
    assert_eq!(version(&path), CURRENT_SCHEMA_VERSION);
}

#[test]
fn failed_bootstrap_rolls_back_created_tables_and_owner() {
    let fixture = Fixture::new();
    let store = AccountStore::open(&fixture.0, "account-a").unwrap();
    let path = store.path.clone().unwrap();
    drop(store);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("DROP TABLE messages; DROP TABLE owner; DROP TABLE threads; CREATE TABLE messages(bad TEXT); PRAGMA user_version=0;").unwrap();
    drop(conn);
    assert!(AccountStore::open(&fixture.0, "account-a").is_err());
    assert_eq!(version(&path), 0);
    let conn = Connection::open(&path).unwrap();
    let created: u32 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name IN ('owner','threads')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(created, 0);
}

#[test]
fn local_clear_keeps_owner_manifest_and_other_accounts() {
    let fixture = Fixture::new();
    let path = fixture.seeded();
    let mut other = AccountStore::open(&fixture.0, "account-b").unwrap();
    other
        .persist_threads(&[Thread {
            id: "other".into(),
            name: "Other".into(),
            ..thread("other")
        }])
        .unwrap();
    let mut store = AccountStore::open(&fixture.0, "account-a").unwrap();
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TRIGGER reject_clear BEFORE DELETE ON threads BEGIN SELECT RAISE(ABORT,'fixture failure'); END;").unwrap();
    let before = snapshot(&path);
    assert!(store.clear_local_history().is_err());
    assert_eq!(snapshot(&path), before);
    conn.execute_batch("DROP TRIGGER reject_clear;").unwrap();
    store.clear_local_history().unwrap();
    assert!(store.all_threads().unwrap().is_empty());
    assert!(store
        .search("migrationneedle", None, 0, 10)
        .unwrap()
        .is_empty());
    for table in [
        "messages",
        "thread_unread",
        "aliases",
        "message_tombstones",
        "message_search",
    ] {
        let count: u32 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0, "{table}");
    }
    let owner: String = conn
        .query_row("SELECT account FROM owner", [], |row| row.get(0))
        .unwrap();
    assert_eq!(owner, "account-a");
    let manifest: String = conn
        .query_row(
            "SELECT report_json FROM migration_manifest WHERE source=?1",
            params!["fixture"],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(manifest, "{\"messages\":1}");
    assert_eq!(other.all_threads().unwrap().len(), 1);
}

#[test]
fn current_schema_open_does_not_decode_historical_message_bodies() {
    let fixture = Fixture::new();
    let path = fixture.seeded();
    let conn = Connection::open(&path).unwrap();
    conn.execute("UPDATE messages SET chat_json='opaque-invalid-json'", [])
        .unwrap();
    drop(conn);
    let before = snapshot(&path);
    let store = AccountStore::open(&fixture.0, "account-a").unwrap();
    let mut unread = store.unread("thread-1").unwrap();
    store.derive_unread("thread-1", &mut unread).unwrap();
    assert!(unread.mention_ids.contains("101"));
    drop(store);
    assert_eq!(snapshot(&path), before);
}

#[test]
fn supported_version_with_missing_schema_is_rejected_without_repair_writes() {
    let fixture = Fixture::new();
    let path = fixture.seeded();
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("DROP TABLE aliases;").unwrap();
    drop(conn);
    let before = std::fs::read(&path).unwrap();
    assert!(AccountStore::open(&fixture.0, "account-a").is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(version(&path), CURRENT_SCHEMA_VERSION);
}
