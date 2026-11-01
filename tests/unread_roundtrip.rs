//! Cross-version regression: old Thread ignored unread metadata during cache round trips.
use serde_json::json;
use zalo_rust::chat::Thread;
#[test]
fn unread_cache_roundtrip_preserves_read_cursor_and_badges() {
    let fixture = json!({"id":"fixture","name":"Fixture","preview":"","time":"","avatar":null,"is_group":true,"messages":[],
        "unread":{"last_seen_id":"100","unread_ids":["101","102"],"muted":true,"marked_unread":false}});
    let thread: Thread = serde_json::from_value(fixture.clone()).unwrap();
    let saved = serde_json::to_value(thread).unwrap();
    assert_eq!(
        saved.get("unread"),
        fixture.get("unread"),
        "unread metadata was dropped during cache serialization"
    );
}

#[test]
fn unread_sqlite_restart_preserves_state_without_counting_history() {
    use zalo_rust::store::{persist_threads, MessageStore};
    let mut store = MessageStore::open(":memory:").unwrap();
    let thread: Thread=serde_json::from_value(json!({"id":"fixture","name":"Fixture","preview":"","time":"","avatar":null,"is_group":true,"messages":[],
        "unread":{"last_seen_id":"100","unread_ids":["101","102"],"muted":true,"marked_unread":false}})).unwrap();
    persist_threads(&mut store, std::slice::from_ref(&thread)).unwrap();
    let mut restored = Vec::new();
    store.hydrate_threads(&mut restored, 200).unwrap();
    assert_eq!(restored[0].unread, thread.unread);
    assert_eq!(restored[0].unread.count(), 2);
    let incoming:Thread=serde_json::from_value(json!({"id":"fixture","name":"Fixture","preview":"","time":"","avatar":null,"is_group":true,"messages":[]})).unwrap();
    zalo_rust::chat::merge_thread_into(&mut restored[0], incoming, true);
    assert_eq!(restored[0].unread, thread.unread);
}

#[test]
fn unread_metadata_only_read_cursor_clears_without_message_batch() {
    let mut threads:Vec<Thread>=serde_json::from_value(json!([{"id":"g123","name":"Fixture","preview":"","time":"","avatar":null,"is_group":true,"messages":[],
        "unread":{"last_seen_id":"100","unread_ids":["101","102"],"muted":true,"marked_unread":false}}])).unwrap();
    zalo_rust::chat::merge_previews(
        &mut threads,
        &json!({"data":{"clearUnreads":[{"idTo":"123","lastMsgId":"102","isGroup":1}]}}),
    );
    assert_eq!(threads.len(), 1);
    assert_eq!(threads[0].unread.count(), 0);
    assert!(threads[0].unread.muted);
}
