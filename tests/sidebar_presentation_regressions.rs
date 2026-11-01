use chrono::{FixedOffset, TimeZone};
use serde_json::json;
use zalo_rust::chat::{ChatMessage, SidebarFontStyle, Thread};
use zalo_rust::clock::sidebar_time_in;

fn thread() -> Thread {
    serde_json::from_value(json!({"id":"group-fixture","name":"Group","preview":"Hello","time":"","avatar":null,"is_group":true,"messages":[]})).unwrap()
}

#[test]
fn group_metadata_uses_roster_and_authoritative_total_not_recent_senders() {
    let mut t = thread();
    t.messages.push(ChatMessage {
        sender: "Not a member sample".into(),
        ..Default::default()
    });
    t.apply_group_sidebar_metadata(&json!({"totalMember":"19","memVerList":["a_1","b_2","a_3"],"topMember":[{"id":"a","displayName":"Alice","avatar":"fixture-avatar"}]}));
    assert_eq!(t.presentation.member_count, Some(19));
    assert_eq!(t.presentation.members.len(), 2);
    assert_eq!(
        t.presentation.members[0].avatar.as_deref(),
        Some("fixture-avatar")
    );
    let mut unknown = thread();
    unknown.apply_group_sidebar_metadata(&json!({"memberVerMap":{"a":1,"b":2}}));
    assert_eq!(unknown.presentation.member_count, None);
    assert_eq!(unknown.presentation.members.len(), 2);
}

#[test]
fn preview_prefix_and_metadata_only_roundtrip() {
    let mut t = thread();
    t.messages.push(ChatMessage {
        sender: "Alice".into(),
        ts: 1793425680000,
        ..Default::default()
    });
    assert_eq!(t.sidebar_preview(), "Alice: Hello");
    t.messages[0].is_outgoing = true;
    assert_eq!(t.sidebar_preview(), "Bạn: Hello");
    t.refresh_sidebar_presentation();
    t.messages.clear();
    let mut restored: Thread = serde_json::from_value(serde_json::to_value(t).unwrap()).unwrap();
    assert_eq!(restored.sidebar_preview(), "Bạn: Hello");
    restored.presentation.last_outgoing = false;
    restored.is_group = false;
    assert_eq!(restored.sidebar_preview(), "Hello");
    assert_eq!(restored.presentation.last_message_ts, 1793425680000);
}

#[test]
fn unread_font_includes_manual_and_muted_unread() {
    let mut t = thread();
    assert_eq!(t.sidebar_font_style(), SidebarFontStyle::Regular);
    t.unread.marked_unread = true;
    t.unread.muted = true;
    assert_eq!(t.sidebar_font_style(), SidebarFontStyle::Semibold);
    t.unread.marked_unread = false;
    t.unread.unread_ids.insert("42".into());
    assert_eq!(t.sidebar_font_style(), SidebarFontStyle::Semibold);
}

#[test]
fn timestamps_use_elapsed_24_hours_seconds_millis_and_year() {
    let tz = FixedOffset::east_opt(7 * 3600).unwrap();
    let ts = tz
        .with_ymd_and_hms(2026, 10, 31, 12, 0, 0)
        .unwrap()
        .timestamp_millis();
    assert_eq!(sidebar_time_in(ts, ts + 86_399_999, &tz), "12:00");
    assert_eq!(sidebar_time_in(ts, ts + 86_400_000, &tz), "31/10");
    assert_eq!(
        sidebar_time_in(ts / 1000, (ts + 86_400_000) / 1000, &tz),
        "31/10"
    );
    let next_year = tz
        .with_ymd_and_hms(2027, 1, 1, 0, 0, 0)
        .unwrap()
        .timestamp_millis();
    assert_eq!(sidebar_time_in(ts, next_year, &tz), "31/10/2026");
    assert_eq!(sidebar_time_in(0, next_year, &tz), "");
    assert_eq!(sidebar_time_in(i64::MAX, next_year, &tz), "");
}

#[test]
fn older_history_never_replaces_metadata_only_latest_preview() {
    let mut t = thread();
    t.preview = "Latest".into();
    t.presentation.last_message_ts = 1793425680000;
    t.presentation.last_sender = "Alice".into();
    t.presentation.member_count = Some(25);
    let mut older = thread();
    older.preview = "Old page".into();
    older.messages.push(ChatMessage {
        id: "old".into(),
        sender: "Bob".into(),
        text: "Old page".into(),
        ts: 1793425580000,
        ..Default::default()
    });
    zalo_rust::chat::merge_thread_into(&mut t, older, false);
    assert_eq!(t.sidebar_preview(), "Alice: Latest");
    assert_eq!(t.presentation.last_message_ts, 1793425680000);
    assert_eq!(t.presentation.member_count, Some(25));
    let mut next = thread();
    next.preview = "New metadata".into();
    next.presentation.last_message_ts = 1793425780000;
    next.presentation.last_sender = "Carol".into();
    zalo_rust::chat::merge_thread_into(&mut t, next, false);
    assert_eq!(t.sidebar_preview(), "Carol: New metadata");
}

#[test]
fn prefixes_do_not_duplicate_or_mislabel_numeric_sender_ids() {
    let mut t = thread();
    t.presentation.last_sender = "123456".into();
    assert_eq!(t.sidebar_preview(), "Hello");
    t.presentation.last_outgoing = true;
    t.preview = "Bạn: Hello".into();
    assert_eq!(t.sidebar_preview(), "Bạn: Hello");
    t.presentation.last_outgoing = false;
    t.presentation.last_sender = "Alice".into();
    t.preview = "Alice: Hello".into();
    assert_eq!(t.sidebar_preview(), "Alice: Hello");
}

#[test]
fn known_roster_resolves_numeric_sender_without_inventing_a_name() {
    let mut t = thread();
    t.presentation.last_sender = "123456".into();
    t.apply_group_sidebar_metadata(&json!({"topMember":[{"id":"123456","displayName":"Alice"}]}));
    assert_eq!(t.sidebar_preview(), "Alice: Hello");
    let mut legacy = thread();
    legacy.messages.push(ChatMessage {
        id: "m".into(),
        sender: "Alice".into(),
        text: "Hello".into(),
        ts: 1793425680000,
        ..Default::default()
    });
    assert!(zalo_rust::chat::merge_thread_into(
        &mut legacy,
        thread(),
        false
    ));
    assert_eq!(legacy.presentation.last_sender, "Alice");
}

#[test]
fn metadata_activity_keeps_order_when_loaded_page_is_older() {
    let mut a = thread();
    a.id = "a".into();
    a.presentation.last_message_ts = 300;
    a.messages.push(ChatMessage {
        ts: 100,
        ..Default::default()
    });
    let mut b = thread();
    b.id = "b".into();
    b.presentation.last_message_ts = 200;
    let mut threads = [b, a];
    threads.sort_by_key(|t| std::cmp::Reverse(t.sidebar_activity_ts()));
    assert_eq!(threads[0].id, "a");
    assert_eq!(threads[0].sidebar_activity_ts(), 300000);
}

#[test]
fn exact_sender_profile_enrichment_survives_numeric_message_refresh() {
    let mut t = thread();
    t.messages = vec![ChatMessage {
        id: "latest".into(),
        sender: "123".into(),
        text: "Hello".into(),
        ts: 1793425680000,
        wire_ref: zalo_rust::message_model::WireMessageRef::from_raw(&json!({"uidFrom":"123"})),
        ..Default::default()
    }];
    t.refresh_sidebar_presentation();
    assert_eq!(t.presentation.last_sender_id, "123");
    let mut enriched = t.clone();
    enriched.messages.clear();
    enriched.presentation.last_sender = "PHƯƠNG".into();
    zalo_rust::chat::merge_thread_into(&mut t, enriched, false);
    t.refresh_sidebar_presentation();
    assert_eq!(t.sidebar_preview(), "PHƯƠNG: Hello");
    assert!(zalo_rust::chat::sidebar_sender_id(&ChatMessage {
        sender: "123".into(),
        ..Default::default()
    })
    .is_none());
    assert!(zalo_rust::chat::sidebar_sender_id(&ChatMessage {
        history_source: Some(json!({"senderId":"opaque"})),
        ..Default::default()
    })
    .is_none());
}
