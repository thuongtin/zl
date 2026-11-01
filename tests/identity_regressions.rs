use serde_json::json;
use zalo_rust::chat::{
    deduplicate_threads, matching_message_index, merge_previews, merge_thread_into,
    parse_chat_message, ChatMessage, Thread,
};
use zalo_rust::message_model::{DeliveryState, MessageKind, UnreadPolicy};

fn thread(id: &str, messages: Vec<ChatMessage>) -> Thread {
    Thread {
        presentation: Default::default(),
        id: id.into(),
        name: "Same name".into(),
        preview: String::new(),
        time: String::new(),
        avatar: Some("https://example.invalid/identical-avatar.jpg".into()),
        is_group: false,
        messages,
        unread: Default::default(),
    }
}
fn message(id: &str, client: &str) -> ChatMessage {
    ChatMessage {
        id: id.into(),
        client_message_id: client.into(),
        text: "Repeated text".into(),
        ts: 1793425680000,
        time: "12:48".into(),
        is_outgoing: true,
        ..Default::default()
    }
}
#[test]
fn identical_names_and_avatars_do_not_establish_contact_identity() {
    let mut threads = vec![thread("a", vec![]), thread("b", vec![])];
    deduplicate_threads(&mut threads, None);
    assert_eq!(threads.len(), 2);
}
#[test]
fn distinct_server_ids_with_identical_text_and_time_survive_all_merge_paths() {
    let mut existing = thread("contact", vec![message("server-a", "a")]);
    merge_thread_into(
        &mut existing,
        thread("contact", vec![message("server-b", "b")]),
        true,
    );
    assert_eq!(existing.messages.len(), 2);
    let mut threads = vec![existing];
    merge_previews(
        &mut threads,
        &json!({"data":[{"messages":[{"msgId":"server-c","cliMsgId":"c","idTo":"contact","uidFrom":"0","ts":1793425680000_i64,"content":"Repeated text"}]}]}),
    );
    assert_eq!(threads[0].messages.len(), 3);
}
#[test]
fn only_unique_client_echo_reconciles_pending() {
    let mut existing = thread("a", vec![message("out_1", "client-1")]);
    merge_thread_into(
        &mut existing,
        thread("a", vec![message("server-1", "client-1")]),
        true,
    );
    assert_eq!(existing.messages.len(), 1);
    assert_eq!(existing.messages[0].id, "server-1");
    let ambiguous = vec![message("out_1", "client"), message("out_2", "client")];
    assert_eq!(
        matching_message_index(&ambiguous, &message("server", "client")),
        None
    );
    assert_eq!(
        matching_message_index(&[message("out_no_client", "")], &message("server", "")),
        None
    );
    assert_eq!(
        matching_message_index(
            &[message("server-a", "client")],
            &message("server-b", "client")
        ),
        None
    );
}
#[test]
fn parse_normalizes_epoch_and_retains_original_wire_values() {
    let raw = json!({"msgId":5,"cliMsgId":"client","uidFrom":0,"idTo":"a","msgType":"chat.photo","cmd":501,"st":0,"at":null,"ts":"1793425680","content":{"href":"signed+bytes","attachments":[{"future":true}]},"paramsExt":{"countUnread":0},"historySource":{"opaque":[1,2]}});
    let parsed = parse_chat_message(&raw).unwrap();
    assert_eq!(parsed.ts, 1793425680000);
    assert_eq!(parsed.kind, MessageKind::Photo);
    assert_eq!(parsed.wire_ref.fields.get("content"), raw.get("content"));
    assert_eq!(parsed.wire_ref.ts, Some(json!("1793425680")));
    assert_eq!(parsed.wire_ref.at, Some(json!(null)));
    assert_eq!(parsed.history_source, raw.get("historySource").cloned());
    assert!(!parsed.unread_policy.counts_unread());
    let encoded = serde_json::to_string(&parsed).unwrap();
    let restored: ChatMessage = serde_json::from_str(&encoded).unwrap();
    assert_eq!(restored.wire_ref, parsed.wire_ref);
    assert_eq!(restored.kind, parsed.kind);
    let mut legacy = serde_json::to_value(parsed).unwrap();
    for field in ["kind", "wire_ref", "unread_policy", "delivery"] {
        legacy.as_object_mut().unwrap().remove(field);
    }
    assert_eq!(
        serde_json::from_value::<ChatMessage>(legacy).unwrap().kind,
        MessageKind::Unknown
    );
    assert_eq!(
        parse_chat_message(&json!({"msgId":"m","content":"x","ts":1793425680000_i64}))
            .unwrap()
            .ts,
        1793425680000
    );
}
#[test]
fn unread_requires_explicit_numeric_one_when_params_ext_exists() {
    for (raw, expected) in [
        (json!({}), true),
        (json!({"paramsExt":{}}), false),
        (json!({"paramsExt":{"countUnread":0}}), false),
        (json!({"paramsExt":{"countUnread":1}}), true),
        (json!({"paramsExt":{"countUnread":"1"}}), false),
    ] {
        assert_eq!(UnreadPolicy::from_raw(&raw).counts_unread(), expected);
    }
}
#[test]
fn delivery_progress_is_monotonic_and_internal_mapping_is_conservative() {
    assert_eq!(
        DeliveryState::Read.merge(DeliveryState::Delivered),
        DeliveryState::Read
    );
    assert_eq!(
        DeliveryState::Delivered.merge(DeliveryState::Failed),
        DeliveryState::Delivered
    );
    assert_eq!(
        DeliveryState::Pending.merge(DeliveryState::ServerAccepted),
        DeliveryState::ServerAccepted
    );
    assert_eq!(
        MessageKind::from_original_internal_code(18),
        MessageKind::Video
    );
    assert_eq!(
        MessageKind::from_original_internal_code(987),
        MessageKind::Unknown
    );
}

#[test]
fn missing_wire_fields_are_not_synthesized() {
    let parsed = parse_chat_message(&json!({"content":"No envelope fields"})).unwrap();
    assert_eq!(parsed.ts, 0);
    assert!(parsed.wire_ref.server_id.is_none());
    assert!(parsed.wire_ref.client_id.is_none());
    assert!(parsed.wire_ref.sender.is_none());
    assert!(parsed.wire_ref.destination.is_none());
    assert!(parsed.wire_ref.original_type.is_none());
    assert!(parsed.wire_ref.cmd.is_none());
    assert!(parsed.wire_ref.st.is_none());
    assert!(parsed.wire_ref.at.is_none());
    assert!(parsed.wire_ref.ts.is_none());
    let encoded = serde_json::to_value(&parsed.wire_ref).unwrap();
    assert!(encoded.get("ts").is_none());
    assert_eq!(
        serde_json::from_value::<zalo_rust::message_model::WireMessageRef>(encoded).unwrap(),
        parsed.wire_ref
    );
}
