//! Disposable SQLite fixtures only. No session, network, or real account state.
use serde_json::json;
use zalo_rust::{
    chat::{ChatMessage, Thread},
    read_receipts::read_local_visible,
    store_v2::AccountStore,
};

fn persist_local_view(store: &mut AccountStore, thread: &Thread, messages: &[ChatMessage]) {
    store.read_local_visible(&thread.id, messages).unwrap();
    store.persist_threads(std::slice::from_ref(thread)).unwrap();
}

fn roundtrip_case(visible_id: &str) {
    let root =
        std::env::temp_dir().join(format!("zl-read-delta-{}-{visible_id}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let mut thread: Thread = serde_json::from_value(json!({"id":"fixture","name":"Fixture","preview":"","time":"","avatar":null,"is_group":true,"messages":[],
        "unread":{"last_seen_id":"100","unread_ids":["101","102"],"mention_ids":["102"],"muted":true,"muted_until":4102444800_i64,"marked_unread":true}})).unwrap();
    let mut store = AccountStore::open(&root, "synthetic-account").unwrap();
    store.persist_threads(&[thread.clone()]).unwrap();
    let messages = [ChatMessage {
        id: visible_id.into(),
        history_source: Some(json!({"archive":true})),
        ..Default::default()
    }];
    assert!(read_local_visible(&mut thread.unread, &messages));
    persist_local_view(&mut store, &thread, &messages);
    drop(store);
    let store = AccountStore::open(&root, "synthetic-account").unwrap();
    let restored = store.unread("fixture").unwrap();
    assert!(
        !restored.marked_unread,
        "local manual clear must survive SQLite reopen even without cursor advancement"
    );
    assert_eq!(restored.last_seen_id.as_deref(), Some("100"));
    assert_eq!(restored.unread_ids, thread.unread.unread_ids);
    assert_eq!(restored.mention_ids, thread.unread.mention_ids);
    assert!(restored.muted);
    assert_eq!(restored.muted_until, Some(4102444800));
    assert!(!restored.fresh);
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn same_cursor_manual_read_is_durable() {
    roundtrip_case("100");
}
#[test]
fn opaque_archive_manual_read_is_durable() {
    roundtrip_case("opaque-archive");
}

#[test]
fn local_read_delta_preserves_latest_disk_state_and_does_not_create_unknown_thread() {
    let root = std::env::temp_dir().join(format!("zl-read-delta-latest-{}", std::process::id()));
    let mut store = AccountStore::open(&root, "synthetic-account").unwrap();
    let thread: Thread = serde_json::from_value(json!({"id":"fixture","name":"Fixture","preview":"","time":"","avatar":null,"is_group":true,"messages":[],
        "unread":{"last_seen_id":"100","unread_ids":["101","150"],"mention_ids":["150"],"muted":true,"muted_until":4102444800_i64,"marked_unread":true}})).unwrap();
    store.persist_threads(&[thread]).unwrap();
    let visible = [ChatMessage {
        id: "101".into(),
        ..Default::default()
    }];
    assert!(store.read_local_visible("fixture", &visible).unwrap());
    assert!(!store.read_local_visible("unknown", &visible).unwrap());
    assert!(!store.read_local_visible("", &visible).unwrap());
    assert!(!store.read_local_visible("fixture", &[]).unwrap());
    drop(store);
    let store = AccountStore::open(&root, "synthetic-account").unwrap();
    let state = store.unread("fixture").unwrap();
    assert_eq!(state.last_seen_id.as_deref(), Some("101"));
    assert_eq!(
        state
            .unread_ids
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["150"]
    );
    assert_eq!(
        state
            .mention_ids
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["150"]
    );
    assert!(state.muted);
    assert_eq!(state.muted_until, Some(4102444800));
    assert!(!state.marked_unread);
    assert!(!state.fresh);
    assert_eq!(store.all_thread_metadata().unwrap().len(), 1);
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}
