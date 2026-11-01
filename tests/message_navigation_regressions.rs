use serde_json::json;
use zalo_rust::{
    chat::ChatMessage,
    message_model::WireMessageRef,
    message_navigation::{message_mentions, unique_reference_index, utf16_byte_range},
};

fn message(id: &str, client: &str) -> ChatMessage {
    ChatMessage {
        id: id.into(),
        client_message_id: client.into(),
        ..Default::default()
    }
}
fn mentioned(text: &str, mentions: serde_json::Value) -> ChatMessage {
    ChatMessage {
        text: text.into(),
        wire_ref: WireMessageRef::from_raw(&json!({"mentions": mentions})),
        ..Default::default()
    }
}
#[test]
fn exact_references_require_one_message_across_both_namespaces() {
    let messages = vec![
        message("server-1", "client-1"),
        message("server-2", "client-2"),
    ];
    assert_eq!(unique_reference_index(&messages, "client-2"), Some(1));
    assert_eq!(unique_reference_index(&messages, "server-1"), Some(0));
    assert_eq!(unique_reference_index(&messages, ""), None);
    assert_eq!(
        unique_reference_index(&[message("a", "same"), message("same", "b")], "same"),
        None
    );
    assert_eq!(
        unique_reference_index(&[message("a", "same"), message("b", "same")], "same"),
        None
    );
    assert_eq!(unique_reference_index(&[message("001", "")], "1"), None);
}
#[test]
fn exact_wire_references_are_retained_without_float_coercion() {
    let mut msg = message("display", "");
    msg.wire_ref =
        WireMessageRef::from_raw(&json!({"msgId": 18446744073709551615_u64, "cliMsgId":"0002"}));
    assert_eq!(
        unique_reference_index(&[msg.clone()], "18446744073709551615"),
        Some(0)
    );
    assert_eq!(unique_reference_index(&[msg.clone()], "0002"), Some(0));
    assert_eq!(unique_reference_index(&[msg], "2"), None);
}
#[test]
fn unicode_ranges_use_utf16_and_reject_partial_surrogates_or_graphemes() {
    let text = "😀 @Hà a\u{301}";
    assert_eq!(&text[utf16_byte_range(text, 3, 3).unwrap()], "@Hà");
    assert_eq!(utf16_byte_range(text, 1, 1), None);
    assert_eq!(utf16_byte_range(text, 7, 1), None);
    assert_eq!(utf16_byte_range(text, usize::MAX, 1), None);
    assert_eq!(utf16_byte_range(text, 3, 0), None);
}
#[test]
fn duplicate_aliases_keep_their_exact_uid_per_range() {
    let msg = mentioned(
        "@An @An",
        json!([{"uid":"11","pos":0,"len":3},{"uid":"22","pos":4,"len":3}]),
    );
    let spans = message_mentions(&msg);
    assert_eq!(spans.len(), 2);
    assert_eq!(spans[0].user_id, "11");
    assert_eq!(spans[1].user_id, "22");
}
#[test]
fn names_or_global_ids_never_fabricate_a_profile_uid() {
    let msg = mentioned(
        "@An @All",
        json!([{"name":"An","pos":0,"len":3},{"gUid":"global","pos":0,"len":3},{"uid":"-1","pos":4,"len":4}]),
    );
    assert!(message_mentions(&msg).is_empty());
    assert!(message_mentions(&message("@An", "")).is_empty());
}
#[test]
fn conflicting_claims_and_invalid_ranges_are_not_clickable() {
    let msg = mentioned(
        "@An",
        json!([{"uid":"11","pos":0,"len":3},{"uid":"22","pos":0,"len":3},{"uid":"33","pos":0,"len":99}]),
    );
    assert!(message_mentions(&msg).is_empty());
}
#[test]
fn archive_mentions_require_decoded_identity_and_live_metadata_wins() {
    let mut msg = ChatMessage {
        text: "@An".into(),
        history_source: Some(
            json!({"meta":{"mentionsList":[{"uid":"opaque/noised","pos":0,"len":3}]}}),
        ),
        ..Default::default()
    };
    assert!(message_mentions(&msg).is_empty());
    msg.history_source.as_mut().unwrap()["meta"]["mentionsList"][0]["uid"] = json!(12);
    assert_eq!(message_mentions(&msg)[0].user_id, "12");
    msg.wire_ref.fields.insert("mentions".into(), json!([]));
    assert!(message_mentions(&msg).is_empty());
}

#[test]
fn asynchronous_pages_and_jumps_reject_stale_away_back_requests() {
    use zalo_rust::message_navigation::accepts_page_result;
    assert!(accepts_page_result(Some("a"), 1, "a", 1));
    assert!(!accepts_page_result(Some("b"), 2, "a", 1));
    assert!(!accepts_page_result(Some("a"), 3, "a", 1));
    assert!(!accepts_page_result(Some("a"), 4, "a", 3));
    assert!(!accepts_page_result(None, 1, "a", 1));
}

#[test]
fn zero_client_quote_uses_real_global_id_in_live_and_cached_messages() {
    use zalo_rust::message_navigation::{message_reply_reference, reply_reference};
    assert_eq!(
        reply_reference(&json!({"cliMsgId":"0","globalMsgId":"0002"})),
        Some("0002".into())
    );
    assert_eq!(
        reply_reference(&json!({"cliMsgId":0,"globalMsgId":"server"})),
        Some("server".into())
    );
    assert_eq!(reply_reference(&json!({"cliMsgId":0})), None);
    let mut m = message("reply", "");
    m.reply_to_id = Some("0".into());
    m.wire_ref
        .fields
        .insert("quote".into(), json!({"cliMsgId":0,"globalMsgId":"target"}));
    assert_eq!(message_reply_reference(&m), Some("target".into()));
}

#[test]
fn opaque_archive_mentions_use_only_exact_positive_uid_sidecar_keys() {
    let mut msg = ChatMessage {
        text: "@An".into(),
        history_source: Some(json!({"meta":{"mentionsList":[{"uid":"opaque-a","pos":0,"len":3}]}})),
        ..Default::default()
    };
    assert!(message_mentions(&msg).is_empty());
    msg.resolved_mention_uids
        .insert("different-key".into(), "11".into());
    assert!(message_mentions(&msg).is_empty());
    for invalid in ["0", "00", "-1", "not-numeric", "", " 11"] {
        msg.resolved_mention_uids
            .insert("opaque-a".into(), invalid.into());
        assert!(message_mentions(&msg).is_empty());
    }
    msg.resolved_mention_uids
        .insert("opaque-a".into(), "18446744073709551615999".into());
    let before = serde_json::to_value(&msg).unwrap();
    assert_eq!(message_mentions(&msg)[0].user_id, "18446744073709551615999");
    assert_eq!(serde_json::to_value(&msg).unwrap(), before);
}

#[test]
fn opaque_archive_duplicate_names_keep_distinct_mapping_and_grapheme_safe_ranges() {
    let mut msg = ChatMessage {
        text: "😀 @An @An a\u{301}".into(),
        history_source: Some(json!({"meta":{"mentionsList":[
            {"uid":"opaque-a","pos":3,"len":3},
            {"uid":"opaque-b","pos":7,"len":3},
            {"uid":"opaque-c","pos":11,"len":1}
        ]}})),
        ..Default::default()
    };
    msg.resolved_mention_uids.extend([
        ("opaque-a".into(), "11".into()),
        ("opaque-b".into(), "22".into()),
        ("opaque-c".into(), "33".into()),
    ]);
    let before = serde_json::to_value(&msg).unwrap();
    let spans = message_mentions(&msg);
    assert_eq!(spans.len(), 2);
    assert_eq!(spans[0].user_id, "11");
    assert_eq!(spans[1].user_id, "22");
    assert_eq!(&msg.text[spans[0].byte_range.clone()], "@An");
    assert_eq!(&msg.text[spans[1].byte_range.clone()], "@An");
    assert_eq!(serde_json::to_value(&msg).unwrap(), before);
}

#[test]
fn live_mentions_do_not_consult_archive_sidecar_or_names() {
    let mut msg = mentioned("@An", json!([{"uid":"opaque-live","pos":0,"len":3}]));
    msg.resolved_mention_uids
        .insert("opaque-live".into(), "11".into());
    assert_eq!(message_mentions(&msg)[0].user_id, "opaque-live");
    msg.wire_ref.fields.insert(
        "mentions".into(),
        json!([{"name":"An","gUid":"opaque-live","pos":0,"len":3}]),
    );
    assert!(message_mentions(&msg).is_empty());
}
