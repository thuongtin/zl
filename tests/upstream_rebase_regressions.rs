use serde_json::json;
use zalo_rust::{
    chat::{self, ChatMessage, Thread},
    store::{persist_threads, MessageStore},
};

fn thread(messages: Vec<ChatMessage>) -> Thread {
    Thread {
        id: "fixture-group".into(),
        name: "Fixture".into(),
        preview: String::new(),
        time: String::new(),
        avatar: None,
        is_group: true,
        presentation: Default::default(),
        unread: Default::default(),
        messages,
    }
}

#[test]
fn indexed_group_merge_preserves_identity_and_updates_batch_indexes() {
    let mut value = thread(vec![ChatMessage {
        id: "out_pending".into(),
        client_message_id: "verified-client".into(),
        text: "Same".into(),
        is_outgoing: true,
        ..Default::default()
    }]);
    chat::merge_group_history(
        &mut value,
        &json!({"data":{"msgs":[
            {"msgId":"server-1","cliMsgId":"verified-client","content":"Same","isOutgoing":true,"ts":1000},
            {"msgId":"server-1","cliMsgId":"verified-client","content":"Same","isOutgoing":true,"ts":1000},
            {"msgId":"server-2","content":"Same","isOutgoing":true,"ts":1000}
        ]}}),
    );
    assert_eq!(value.messages.len(), 2);
    assert!(value.messages.iter().any(|m| m.id == "server-1"));
    assert!(value.messages.iter().any(|m| m.id == "server-2"));

    let mut enriched = thread(vec![ChatMessage {
        id: "out_enriched".into(),
        text: "Same".into(),
        is_outgoing: true,
        ..Default::default()
    }]);
    chat::merge_group_history(
        &mut enriched,
        &json!({"data":{"msgs":[
            {"msgId":"out_enriched","cliMsgId":"enriched-client","content":"Same","isOutgoing":true,"ts":1000},
            {"msgId":"server-enriched","cliMsgId":"enriched-client","content":"Same","isOutgoing":true,"ts":1000}
        ]}}),
    );
    assert_eq!(enriched.messages.len(), 1);
    assert_eq!(enriched.messages[0].id, "server-enriched");

    let mut ambiguous = thread(vec![
        ChatMessage {
            id: "out_a".into(),
            client_message_id: "ambiguous".into(),
            is_outgoing: true,
            ..Default::default()
        },
        ChatMessage {
            id: "out_b".into(),
            client_message_id: "ambiguous".into(),
            is_outgoing: true,
            ..Default::default()
        },
    ]);
    chat::merge_group_history(
        &mut ambiguous,
        &json!({"data":{"msgs":[
            {"msgId":"server-3","cliMsgId":"ambiguous","content":"Same","isOutgoing":true,"ts":1000}
        ]}}),
    );
    assert_eq!(ambiguous.messages.len(), 3);
}

#[test]
fn legacy_store_returns_newest_window_with_stable_tie_order() {
    let mut store = MessageStore::open(":memory:").unwrap();
    let source = thread(
        (0..600)
            .map(|i| ChatMessage {
                id: format!("message-{i:04}"),
                ts: i / 2,
                text: "Fixture".into(),
                ..Default::default()
            })
            .collect(),
    );
    persist_threads(&mut store, &[source]).unwrap();
    let messages = store.messages_for("fixture-group", 500).unwrap();
    assert_eq!(messages.len(), 500);
    assert_eq!(messages.first().unwrap().msg_id, "message-0100");
    assert_eq!(messages.last().unwrap().msg_id, "message-0599");
    assert!(messages
        .windows(2)
        .all(|p| (p[0].ts, &p[0].msg_id) < (p[1].ts, &p[1].msg_id)));
    assert_eq!(
        store
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
}

// This test binary has one working-directory fixture. Other tests use in-memory stores only.
#[test]
fn legacy_cache_is_compact_bounded_and_preserves_sidebar_metadata() {
    let original = std::env::current_dir().unwrap();
    let root = std::env::temp_dir().join(format!("zl-rebase-cache-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    struct Cleanup(std::path::PathBuf, std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            std::env::set_current_dir(&self.0).unwrap();
            let _ = std::fs::remove_dir_all(&self.1);
        }
    }
    let _cleanup = Cleanup(original, root.clone());
    std::env::set_current_dir(&root).unwrap();
    let mut source = thread(
        (0..600)
            .map(|i| ChatMessage {
                id: format!("message-{i:04}"),
                ts: i,
                text: "Fixture".into(),
                ..Default::default()
            })
            .collect(),
    );
    source.presentation.member_count = Some(54);
    chat::try_save_threads_cache(&[source]).unwrap();
    let bytes = std::fs::read("threads_cache.json").unwrap();
    let cached: Vec<Thread> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(cached[0].messages.len(), MessageStore::PRELOAD_MSGS);
    assert_eq!(cached[0].messages[0].id, "message-0580");
    assert_eq!(cached[0].presentation.member_count, Some(54));
    assert!(!bytes.contains(&b'\n'));
}
