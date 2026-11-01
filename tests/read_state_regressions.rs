//! Synthetic fixtures only. These tests perform no network or original-app operations.
use serde_json::json;
use std::collections::BTreeMap;
use zalo_rust::{
    chat::{ChatMessage, Thread},
    message_model::{UnreadPolicy, WireMessageRef},
    presence::{configured_active_threshold, Presence, VersionedPresence},
    reactions::{ReactionEvent, ReactionSnapshotProvenance, ReactionVersion},
    read_receipts::{parse_seen_response, ReadVisibilityGate, SeenBatch},
    unread::{self, UnreadState},
};
fn thread() -> Thread {
    Thread {
        presentation: Default::default(),
        id: "42".into(),
        name: "Fixture".into(),
        preview: String::new(),
        time: String::new(),
        avatar: None,
        is_group: false,
        messages: vec![],
        unread: UnreadState::default(),
    }
}
#[test]
fn read_points_never_create_unknown_conversations() {
    let mut threads = vec![thread()];
    unread::merge_read_points(
        &mut threads,
        &json!({"clearUnreads":[{"idTo":"unknown","lastMsgId":"10"},{"idTo":"unknown-group","lastMsgId":"10","isGroup":1}]}),
    );
    assert_eq!(threads.len(), 1);
    assert_eq!(threads[0].unread.last_seen_id, None);
}
#[test]
fn unread_policy_and_visible_cursor_preserve_offscreen_badges() {
    let mut state = UnreadState::default();
    state.advance_seen("10");
    let m = |id: &str, raw| ChatMessage {
        id: id.into(),
        unread_policy: UnreadPolicy::from_raw(&raw),
        ..Default::default()
    };
    state.observe(&[
        m("11", json!({})),
        m("12", json!({"paramsExt":{}})),
        m("13", json!({"paramsExt":{"countUnread":1}})),
        m("14", json!({"paramsExt":{"countUnread":"1"}})),
    ]);
    assert_eq!(state.count(), 2);
    state.read_visible(&[m("11", json!({}))]);
    assert_eq!(state.count(), 1);
    assert!(state.unread_ids.contains("13"));
}
#[test]
fn visibility_requires_continuous_active_nonminimized_bottom() {
    let mut gate = ReadVisibilityGate::default();
    assert!(!gate.observe("42", true, false, true, 100));
    assert!(!gate.observe("42", true, false, true, 599));
    assert!(gate.observe("42", true, false, true, 600));
    assert!(!gate.observe("42", false, false, true, 700));
    assert!(!gate.observe("42", true, false, true, 1000));
    assert!(!gate.observe("42", true, true, true, 1500));
    assert!(!gate.observe("42", true, false, false, 1600));
    assert!(!gate.observe("43", true, false, true, 2000));
    assert!(gate.observe("43", true, false, true, 2500));
}
#[test]
fn manual_marks_decode_and_only_merge_known_rows() {
    let marks=unread::parse_manual_unread_marks(&json!({"error_code":0,"data":json!({"convsGroup":[{"id":"77","cliMsgId":123}],"convsUser":[{"id":"42","cliMsgId":"124"}]}).to_string()})).unwrap();
    assert_eq!(marks[0].thread_id, "g77");
    let mut threads = vec![thread()];
    unread::merge_manual_unread_marks(&mut threads, &marks);
    assert!(threads[0].unread.marked_unread);
    assert_eq!(threads.len(), 1);
    assert!(unread::parse_manual_unread_marks(&json!({"data":"{\"error_code\":9}"})).is_err());
    assert!(unread::parse_manual_unread_marks(&json!({"data":{}})).is_err());
}
#[test]
fn seen_v2_requires_wire_mapping_and_bounded_batches() {
    let wire = WireMessageRef::from_raw(
        &json!({"msgId":"9007199254740993001","cliMsgId":"9007199254740993002","uidFrom":"42","idTo":"self","msgType":"chat.text","st":0,"at":0,"cmd":501,"ts":1700000000000_i64}),
    );
    let batch = SeenBatch::from_wire("42", false, std::slice::from_ref(&wire), "fixture").unwrap();
    assert_eq!(batch.command, 12118);
    assert_eq!(batch.path, "/api/message/seenv2");
    assert_eq!(batch.len(), 1);
    assert!(
        SeenBatch::from_wire("other-peer", false, std::slice::from_ref(&wire), "fixture").is_err()
    );
    let mut group_wire = wire.clone();
    group_wire.destination = Some(json!("g77"));
    group_wire.fields.insert("idTo".into(), json!("g77"));
    assert_eq!(
        SeenBatch::from_wire("g77", true, &[group_wire], "fixture")
            .unwrap()
            .command,
        12124
    );

    assert!(SeenBatch::from_wire("42", false, &vec![wire.clone(); 51], "fixture").is_err());
    let archive = WireMessageRef::from_raw(
        &json!({"id":"local-1","clientId":123,"fromId":"42","toId":"self","type":1}),
    );
    assert!(SeenBatch::from_wire("42", false, &[archive], "fixture").is_err());
    assert!(parse_seen_response(&json!({"error_code":0,"data":"{\"error_code\":7}"})).is_err());
    assert!(parse_seen_response(&json!({"error_code":0,"data":{"error_code":0}})).is_ok());
}
#[test]
fn cached_reaction_baseline_cannot_rewind_live_clear() {
    let event = ReactionEvent {
        sender_id: "fixture".into(),
        event_id: "clear-1".into(),
        r_type: -1,
        ts: 1700000001000,
        ..Default::default()
    };
    let mut versions = BTreeMap::new();
    let mut version = ReactionVersion::default();
    version.events.push(event);
    versions.insert("fixture".into(), version);
    let state = ReactionSnapshotProvenance {
        source: "wire".into(),
        captured_at_ms: 1700000000000,
    };
    let old = ReactionSnapshotProvenance {
        source: "cache".into(),
        captured_at_ms: 1700000000001,
    };
    assert!(!state.accepts_snapshot(&old, &versions, false));
    assert!(!state.accepts_snapshot(&old, &versions, true));
    assert!(!serde_json::from_str::<ReactionSnapshotProvenance>(
        &serde_json::to_string(&state).unwrap()
    )
    .unwrap()
    .accepts_snapshot(&old, &versions, false));
}
#[test]
fn presence_error_and_stale_reply_never_invent_online() {
    let now = 1793425680000_i64;
    let mut state = VersionedPresence::default();
    state.merge(
        2,
        Presence::parse(&json!({"lastOnline":now-3_600_000}), now),
    );
    state.merge(1, Presence::parse(&json!({"lastOnline":-1}), now));
    assert!(!state.value.is_recent(now));
    state.merge(
        3,
        Presence::parse(&json!({"error_code":9,"lastOnline":-1}), now),
    );
    assert!(!state.value.is_recent(now));
    assert_eq!(
        configured_active_threshold(&json!({"online_configs":{"user_active_time":1000}})),
        1000
    );
}

#[test]
fn mentions_use_exact_raw_uids_and_source_verified_all_not_names_or_destinations() {
    use zalo_rust::unread::{
        classify_message_mention, detect_message_mention, MentionDetection as D,
    };
    let wire = |raw| ChatMessage {
        wire_ref: WireMessageRef::from_raw(&raw),
        ..Default::default()
    };
    assert!(detect_message_mention(
        &wire(json!({"mentions":[{"uid":"123"}]})),
        "123"
    ));
    assert!(detect_message_mention(
        &wire(json!({"mentions":[{"uid":123}]})),
        "123"
    ));
    assert!(detect_message_mention(
        &wire(json!({"mentions":[{"uid":"-1"}]})),
        "123"
    ));
    assert_eq!(
        classify_message_mention(
            &wire(json!({"mentions":[{"uid":"456","name":"123"}],"uidTo":"123"})),
            "123"
        ),
        D::NoMention
    );
    assert_eq!(
        classify_message_mention(&wire(json!({"mentions":[{"uidTo":"123"}]})), "123"),
        D::Unknown
    );
    assert_eq!(
        classify_message_mention(&wire(json!({"uidTo":"123","text":"@123 @all"})), "123"),
        D::Unknown
    );
    assert_eq!(
        classify_message_mention(&wire(json!({"mentions":[{"uid":-1}]})), "123"),
        D::Unknown
    );
    assert_eq!(
        classify_message_mention(&wire(json!({"mentions":[]})), "123"),
        D::NoMention
    );
    let archive = ChatMessage {
        history_source: Some(json!({"meta":{"mentionsList":[{"uid":"-1"}]}})),
        ..Default::default()
    };
    assert!(detect_message_mention(&archive, "123"));
    let opaque = ChatMessage {
        history_source: Some(json!({"meta":{"mentionsList":[{"uid":"opaque-noised-uid"}]}})),
        ..Default::default()
    };
    assert_eq!(classify_message_mention(&opaque, "123"), D::Unknown);
    let outgoing = ChatMessage {
        is_outgoing: true,
        wire_ref: WireMessageRef::from_raw(&json!({"mentions":[{"uid":"-1"}]})),
        ..Default::default()
    };
    assert!(!detect_message_mention(&outgoing, "123"));
}
#[test]
fn unread_mentions_survive_serde_merge_and_offscreen_until_numeric_cursor_passes_them() {
    let mentioned = |id: &str| ChatMessage {
        id: id.into(),
        wire_ref: WireMessageRef::from_raw(&json!({"mentions":[{"uid":"123"}]})),
        ..Default::default()
    };
    let mut state = UnreadState::default();
    state.advance_seen("10");
    state.observe_for_account(&[mentioned("11"), mentioned("99")], "123");
    assert_eq!(state.mention_ids.len(), 2);
    assert!(state.has_unread_mentions());
    state = serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
    state.merge_metadata(&UnreadState::default());
    state.read_visible(&[mentioned("11")]);
    assert_eq!(
        state.mention_ids.iter().cloned().collect::<Vec<_>>(),
        vec!["99"]
    );
    let incoming = UnreadState {
        fresh: true,
        last_seen_id: Some("50".into()),
        unread_ids: std::collections::BTreeSet::from(["70".into()]),
        mention_ids: std::collections::BTreeSet::from(["70".into(), "9".into()]),
        ..Default::default()
    };
    state.merge_metadata(&incoming);
    assert_eq!(
        state.mention_ids,
        std::collections::BTreeSet::from(["70".into(), "99".into()])
    );
    state.advance_seen("100");
    assert!(!state.has_unread_mentions());
    let mut legacy = serde_json::to_value(&state).unwrap();
    legacy.as_object_mut().unwrap().remove("mention_ids");
    assert!(serde_json::from_value::<UnreadState>(legacy)
        .unwrap()
        .mention_ids
        .is_empty());
}
#[test]
fn unread_mentions_do_not_count_disabled_policy_or_erase_unknown_metadata() {
    let mut state = UnreadState::default();
    state.advance_seen("10");
    let mention = ChatMessage {
        id: "11".into(),
        wire_ref: WireMessageRef::from_raw(&json!({"mentions":[{"uid":"-1"}]})),
        ..Default::default()
    };
    state.observe_for_account(std::slice::from_ref(&mention), "123");
    state.observe_mentions(
        &[ChatMessage {
            id: "11".into(),
            ..Default::default()
        }],
        "123",
    );
    assert!(state.mention_ids.contains("11"));
    let disabled = ChatMessage {
        id: "12".into(),
        unread_policy: UnreadPolicy::from_raw(&json!({"paramsExt":{"countUnread":0}})),
        ..mention
    };
    state.observe_for_account(&[disabled], "123");
    assert!(!state.mention_ids.contains("12"));
}
