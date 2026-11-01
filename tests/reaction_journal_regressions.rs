//! Disposable synthetic SQLite fixtures. No account/network/original-app access.
use std::path::PathBuf;
use zalo_rust::{
    chat::{ChatMessage, Thread},
    reaction_journal::{event_key, ReactionJournal},
    reactions::ReactionEvent,
};
static NEXT_FIXTURE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "zl-reaction-journal-fixture-{}-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn event(id: &str, target: &str, kind: i32, ts: i64) -> ReactionEvent {
    ReactionEvent {
        message_id: target.into(),
        client_message_id: "client-fixture".into(),
        thread_id: "peer-fixture".into(),
        sender_id: "reactor-fixture".into(),
        event_id: id.into(),
        r_type: kind,
        emoji: zalo_rust::reactions::emoji_for_type(kind).into(),
        ts,
        ..Default::default()
    }
}
fn thread(id: &str, messages: Vec<ChatMessage>) -> Thread {
    Thread {
        presentation: Default::default(),
        id: id.into(),
        name: "Fixture".into(),
        preview: String::new(),
        time: String::new(),
        avatar: None,
        is_group: false,
        messages,
        unread: Default::default(),
    }
}
fn message(id: &str, client: &str) -> ChatMessage {
    ChatMessage {
        id: id.into(),
        client_message_id: client.into(),
        ..Default::default()
    }
}
#[test]
fn journal_deduplicates_wire_ids_but_preserves_same_icon_distinct_versions_after_reopen() {
    let fixture = Fixture::new();
    let mut journal = ReactionJournal::open(&fixture.0, "account-a").unwrap();
    let first = event("event-1", "message", 5, 1700000000000);
    let second = event("event-2", "message", 5, 1700000000001);
    assert_eq!(journal.append(&[first.clone(), first, second]).unwrap(), 2);
    assert_eq!(journal.pending(1).unwrap().len(), 1);
    drop(journal);
    let mut journal = ReactionJournal::open(&fixture.0, "account-a").unwrap();
    assert_eq!(journal.pending_count().unwrap(), 2);
    let mut threads = vec![thread(
        "peer-fixture",
        vec![message("message", "client-fixture")],
    )];
    let result = journal.apply_loaded(&mut threads).unwrap();
    assert_eq!(result.applied, 2);
    assert_eq!(result.resolved, 2);
    assert_eq!(threads[0].messages[0].reaction_summaries[0].count, 2);
    assert_eq!(journal.apply_loaded(&mut threads).unwrap().applied, 0);
    assert_eq!(journal.pending_count().unwrap(), 0);
}
#[test]
fn queue_over_4096_is_persisted_and_late_loaded_target_is_not_starved() {
    let fixture = Fixture::new();
    let mut journal = ReactionJournal::open(&fixture.0, "account-a").unwrap();
    let events: Vec<_> = (0..4200)
        .map(|i| {
            let mut e = event(
                &format!("event-{i}"),
                &format!("message-{i}"),
                5,
                1700000000000 + i,
            );
            e.client_message_id = format!("client-{i}");
            e
        })
        .collect();
    assert_eq!(journal.append(&events).unwrap(), 4200);
    assert_eq!(journal.pending(9999).unwrap().len(), 4096);
    drop(journal);
    let mut journal = ReactionJournal::open(&fixture.0, "account-a").unwrap();
    assert_eq!(journal.pending_count().unwrap(), 4200);
    let mut threads = vec![thread(
        "peer-fixture",
        vec![message("message-4199", "client-4199")],
    )];
    let result = journal.apply_loaded(&mut threads).unwrap();
    assert_eq!(result.scanned, 4200);
    assert_eq!(result.applied, 1);
    assert_eq!(result.pending, 4199);
}
#[test]
fn account_isolation_and_persistent_partial_checkpoints() {
    let fixture = Fixture::new();
    let mut a = ReactionJournal::open(&fixture.0, "account-a").unwrap();
    let b = ReactionJournal::open(&fixture.0, "account-b").unwrap();
    assert_ne!(a.path(), b.path());
    a.append(&[event("event-1", "missing", 5, 1700000000000)])
        .unwrap();
    assert_eq!(b.pending_count().unwrap(), 0);
    a.record_checkpoint("direct", 21, true, None).unwrap();
    a.record_checkpoint("groups", 3, false, Some("timeout"))
        .unwrap();
    drop(a);
    let mut a = ReactionJournal::open(&fixture.0, "account-a").unwrap();
    let state = a.status().unwrap();
    assert!(state.partial);
    assert_eq!(state.checkpoints[0].reason.as_deref(), Some("page_limit"));
    assert_eq!(state.checkpoints[1].reason.as_deref(), Some("timeout"));
    a.resolve(&a.pending(1).unwrap()[0].key).unwrap();
    a.record_checkpoint("direct", 2, true, None).unwrap();
    a.record_checkpoint("groups", 3, true, None).unwrap();
    assert!(!a.status().unwrap().partial);
}
#[test]
fn only_unique_scoped_target_resolves_exact_global_wins_client() {
    let fixture = Fixture::new();
    let mut journal = ReactionJournal::open(&fixture.0, "account-a").unwrap();
    let e = event("event-1", "exact", 5, 1700000000000);
    journal.append(&[e]).unwrap();
    let mut threads = vec![
        thread("wrong-peer", vec![message("exact", "client-fixture")]),
        thread(
            "peer-fixture",
            vec![
                message("other", "client-fixture"),
                message("exact", "different"),
            ],
        ),
    ];
    assert_eq!(journal.apply_loaded(&mut threads).unwrap().resolved, 1);
    assert!(threads[0].messages[0].reaction_summaries.is_empty());
    assert!(threads[1].messages[0].reaction_summaries.is_empty());
    assert_eq!(threads[1].messages[1].reaction_summaries[0].count, 1);
    let ambiguous = event("event-2", "missing", 5, 1700000000001);
    journal.append(&[ambiguous]).unwrap();
    threads[1]
        .messages
        .push(message("other2", "client-fixture"));
    let result = journal.apply_loaded(&mut threads).unwrap();
    assert_eq!(result.ambiguous, 1);
    assert_eq!(result.pending, 1);
}
#[test]
fn replay_after_resolve_recovers_stale_cache_and_does_not_rewind_live_clear() {
    let fixture = Fixture::new();
    let mut journal = ReactionJournal::open(&fixture.0, "account-a").unwrap();
    let old = event("event-old", "message", 5, 1700000000000);
    journal.append(&[old]).unwrap();
    let mut loaded = vec![thread(
        "peer-fixture",
        vec![message("message", "client-fixture")],
    )];
    journal.apply_loaded(&mut loaded).unwrap();
    assert_eq!(journal.pending_count().unwrap(), 0);
    drop(journal);
    let mut journal = ReactionJournal::open(&fixture.0, "account-a").unwrap();
    let mut stale = vec![thread(
        "peer-fixture",
        vec![message("message", "client-fixture")],
    )];
    assert_eq!(journal.apply_loaded(&mut stale).unwrap().applied, 1);
    let clear = event("event-clear", "message", -1, 1700000000010);
    stale[0].messages[0].apply_reaction(&clear);
    journal.apply_loaded(&mut stale).unwrap();
    assert!(stale[0].messages[0].reaction_summaries.is_empty());
}
#[test]
fn no_id_hash_is_exact_and_append_batch_is_atomic() {
    let fixture = Fixture::new();
    let mut journal = ReactionJournal::open(&fixture.0, "account-a").unwrap();
    let mut first = event("", "message", 5, 1700000000000);
    let mut second = first.clone();
    second.ts += 1;
    assert_ne!(event_key(&first).unwrap(), event_key(&second).unwrap());
    journal
        .append(&[first.clone(), first.clone(), second])
        .unwrap();
    assert_eq!(journal.pending_count().unwrap(), 2);
    first.sender_id.clear();
    let result = journal.append(&[event("valid-new", "message", 5, 1700000000002), first]);
    assert!(result.is_err());
    assert_eq!(journal.pending_count().unwrap(), 2);
}
#[test]
fn authoritative_newer_baseline_covers_old_live_add_and_clear() {
    let fixture = Fixture::new();
    let mut journal = ReactionJournal::open(&fixture.0, "account-a").unwrap();
    journal
        .append(&[
            event("old-add", "message", 5, 1700000000000),
            event("old-clear", "message", -1, 1700000000001),
        ])
        .unwrap();
    let mut m = message("message", "client-fixture");
    m.reaction_summaries = zalo_rust::reactions::parse_reactions(
        &serde_json::json!({"reactions":{"reactor-fixture":{"5":2}}}),
    );
    m.reaction_authoritative_snapshot = true;
    m.reaction_snapshot_provenance = zalo_rust::reactions::ReactionSnapshotProvenance {
        source: "wire".into(),
        captured_at_ms: 1700000000010,
    };
    let mut threads = vec![thread("peer-fixture", vec![m])];
    let result = journal.apply_loaded(&mut threads).unwrap();
    assert_eq!(result.applied, 0);
    assert_eq!(result.resolved, 2);
    assert_eq!(threads[0].messages[0].reaction_summaries[0].count, 2);
}
#[test]
fn self_reactor_zero_survives_mixed_append_unloaded_reopen_and_replay() {
    let fixture = Fixture::new();
    let mut journal = ReactionJournal::open(&fixture.0, "account-a").unwrap();
    let mut own = event("own-event", "message", 5, 1700000000000);
    own.sender_id = "0".into();
    let other = event("other-event", "message", 3, 1700000000001);
    assert_eq!(journal.append(&[own.clone(), other]).unwrap(), 2);
    assert_eq!(journal.apply_loaded(&mut []).unwrap().pending, 2);
    drop(journal);
    let mut journal = ReactionJournal::open(&fixture.0, "account-a").unwrap();
    assert_eq!(journal.pending_count().unwrap(), 2);
    let mut loaded = vec![thread(
        "peer-fixture",
        vec![message("message", "client-fixture")],
    )];
    let result = journal.apply_loaded(&mut loaded).unwrap();
    assert_eq!(result.applied, 2);
    assert_eq!(result.resolved, 2);
    let summaries = &loaded[0].messages[0].reaction_summaries;
    assert_eq!(
        summaries
            .iter()
            .find(|s| s.r_type == Some(5))
            .unwrap()
            .reactor_counts["0"],
        1
    );
    assert_eq!(
        summaries
            .iter()
            .find(|s| s.r_type == Some(3))
            .unwrap()
            .reactor_counts["reactor-fixture"],
        1
    );
    assert_eq!(journal.append(&[own]).unwrap(), 0);
    assert_eq!(journal.pending_count().unwrap(), 0);
}
