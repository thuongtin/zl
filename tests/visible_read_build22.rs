//! Disposable fixtures only; no network, native activation, or account state.
use serde_json::json;
use zalo_rust::{message_model::WireMessageRef, read_receipts::SeenBatch};

#[test]
fn incoming_wire_zero_destination_is_a_valid_seen_target() {
    let wire = WireMessageRef::from_raw(&json!({
        "msgId":"101", "cliMsgId":"202", "uidFrom":"42",
        "idTo":"0", "msgType":"chat.text"
    }));
    assert!(SeenBatch::from_wire("42", false, std::slice::from_ref(&wire), "fixture").is_ok());
    assert!(
        SeenBatch::from_wire("other-peer", false, std::slice::from_ref(&wire), "fixture").is_err()
    );
    assert!(SeenBatch::from_wire("g42", true, &[wire], "fixture").is_err());
}

use zalo_rust::{
    chat::ChatMessage,
    read_receipts::{
        read_local_visible, verified_foreground, verified_visible_receipts, ReadVisibilityGate,
    },
    unread::UnreadState,
};

fn message(id: &str) -> ChatMessage {
    ChatMessage {
        id: id.into(),
        ..Default::default()
    }
}

#[test]
fn local_archive_and_manual_unread_clear_without_network_and_preserve_newer_offscreen() {
    let mut state = UnreadState::default();
    state.advance_seen("100");
    state.observe(&[message("101"), message("102"), message("110")]);
    state.marked_unread = true;
    let archive = ChatMessage {
        history_source: Some(json!({"id":"102"})),
        ..message("102")
    };
    assert!(verified_visible_receipts("42", false, std::slice::from_ref(&archive)).is_empty());
    assert!(read_local_visible(&mut state, &[archive]));
    assert_eq!(state.last_seen_id.as_deref(), Some("102"));
    assert_eq!(
        state
            .unread_ids
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["110"]
    );
    assert!(!state.marked_unread);
    assert!(
        !state.fresh,
        "local archive view must not fabricate server acknowledgement"
    );
    state.marked_unread = true;
    assert!(read_local_visible(&mut state, &[message("opaque-archive")]));
    assert!(!state.marked_unread);
    assert_eq!(state.last_seen_id.as_deref(), Some("102"));
}

#[test]
fn metadata_only_and_latest_outgoing_visible_cursor_clear_earlier_incoming() {
    let mut state = UnreadState {
        marked_unread: true,
        ..Default::default()
    };
    let latest = ChatMessage {
        is_outgoing: true,
        ..message("120")
    };
    assert!(read_local_visible(
        &mut state,
        std::slice::from_ref(&latest)
    ));
    assert_eq!(state.last_seen_id.as_deref(), Some("120"));
    assert!(!state.has_badge());
    state.advance_seen("100");
    assert_eq!(state.last_seen_id.as_deref(), Some("120"));
    assert!(verified_visible_receipts("42", false, &[latest]).is_empty());
}

#[test]
fn inactive_minimized_hidden_or_short_view_never_changes_local_unread() {
    let mut state = UnreadState::default();
    state.advance_seen("100");
    state.observe(&[message("101")]);
    state.marked_unread = true;
    let original = state.clone();
    let mut gate = ReadVisibilityGate::default();
    for (logical, native, minimized, visible, time) in [
        (true, false, false, true, 0),
        (true, false, false, true, 1000),
        (false, true, false, true, 2000),
        (true, true, true, true, 3000),
        (true, true, false, false, 4000),
        (true, true, false, true, 5000),
        (true, true, false, true, 5499),
    ] {
        if gate.observe(
            "42",
            verified_foreground(logical, native),
            minimized,
            visible,
            time,
        ) {
            read_local_visible(&mut state, &[message("101")]);
        }
        assert_eq!(state, original);
    }
    assert!(gate.observe("42", verified_foreground(true, true), false, true, 5500));
    read_local_visible(&mut state, &[message("101")]);
    assert!(!state.has_badge());
}

#[test]
fn mixed_visible_rows_only_send_verified_conversation_wire_identities() {
    let good = ChatMessage {
        wire_ref: WireMessageRef::from_raw(
            &json!({"msgId":"101","cliMsgId":"201","uidFrom":"42","idTo":0,"msgType":"chat.text"}),
        ),
        ..message("101")
    };
    let archive = ChatMessage {
        history_source: Some(json!({"archive":true})),
        ..good.clone()
    };
    let mut partial = good.clone();
    partial.wire_ref.client_id = None;
    let mut mismatch = good.clone();
    mismatch.wire_ref.sender = Some(json!("other"));
    let selected =
        verified_visible_receipts("42", false, &[archive, partial, mismatch, good.clone()]);
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].id, good.id);
    let wires = selected
        .iter()
        .map(|m| m.wire_ref.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        SeenBatch::from_wire("42", false, &wires, "fixture")
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        verified_visible_receipts("42", false, &vec![good; 60]).len(),
        50
    );
}

#[test]
fn large_visible_batch_keeps_newest_fifty_and_new_arrival_changes_signature() {
    let rows:Vec<_>=(101..=161).map(|id|ChatMessage {
        wire_ref:WireMessageRef::from_raw(&json!({"msgId":id.to_string(),"cliMsgId":(id+1000).to_string(),"uidFrom":"42","idTo":"0","msgType":"chat.text"})),
        ..message(&id.to_string())
    }).collect();
    let first = verified_visible_receipts("42", false, &rows[..60]);
    assert_eq!(first.len(), 50);
    assert_eq!(first.first().unwrap().id, "111");
    assert_eq!(first.last().unwrap().id, "160");
    let next = verified_visible_receipts("42", false, &rows);
    assert_eq!(next.last().unwrap().id, "161");
    assert_ne!(
        first.iter().map(|m| &m.id).collect::<Vec<_>>(),
        next.iter().map(|m| &m.id).collect::<Vec<_>>()
    );
}
