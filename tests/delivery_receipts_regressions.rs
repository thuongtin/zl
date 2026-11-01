use serde_json::json;
use zalo_rust::{
    chat::{ChatMessage, Thread},
    delivery_receipts::{apply_delivery_events, parse_delivery_events, DeliveryEvent},
    message_model::DeliveryState,
};
fn thread(id: &str, messages: Vec<ChatMessage>) -> Thread {
    Thread {
        presentation: Default::default(),
        id: id.into(),
        name: String::new(),
        preview: String::new(),
        time: String::new(),
        avatar: None,
        is_group: id.starts_with('g'),
        messages,
        unread: Default::default(),
    }
}
fn message(id: &str, client: &str, outgoing: bool) -> ChatMessage {
    ChatMessage {
        id: id.into(),
        client_message_id: client.into(),
        is_outgoing: outgoing,
        delivery: DeliveryState::ServerAccepted,
        ..Default::default()
    }
}
#[test]
fn original_status_commands_and_payload_shapes() {
    for cmd in [501, 502, 521, 522, 534, 570] {
        let events = parse_delivery_events(
            cmd,
            1,
            &json!({"error_code":0,"data":{
                "delivereds":[{"msgId":"9007199254740993","cliMsgId":42,"threadId":"peer","seen":0,"deliveredUids":["peer"]}],
                "groupSeens":[{"groupId":"123","msgId":"8"}],
                "seens":[{"idTo":"peer","isGroup":0,"msgId":"9"},{"idTo":"456","isGroup":1,"msgId":"10"}]
            }}),
            "self",
        );
        assert_eq!(events.len(), 4);
        assert_eq!(events[0].message_id.as_deref(), Some("9007199254740993"));
        assert_eq!(events[0].client_id.as_deref(), Some("42"));
        assert_eq!(events[0].state, DeliveryState::Delivered);
        assert_eq!(events[1].thread_id, "g123");
        assert_eq!(events[1].state, DeliveryState::Read);
        assert_eq!(events[3].thread_id, "g456");
    }
}
#[test]
fn unknown_error_own_and_malformed_packets_never_imply_delivery() {
    let packet = json!({"delivereds":[{"msgId":"1","threadId":"peer","seen":1}]});
    assert!(parse_delivery_events(612, 1, &packet, "self").is_empty());
    assert!(parse_delivery_events(502, 1, &packet, "").is_empty());
    assert!(
        parse_delivery_events(502, 1, &json!({"error_code":-1,"data":packet}), "self").is_empty()
    );
    for record in [
        json!({"msgId":"1","threadId":"peer","deliveredUids":["self"]}),
        json!({"msgId":"1","threadId":"peer","seen":1,"seenUids":["self"]}),
        json!({"msgId":"1","threadId":"peer","seen":null}),
        json!({"msgId":"1","threadId":"peer","seen":2}),
        json!({"msgId":"1","threadId":"peer","seen":true}),
        json!({"threadId":"peer","seen":1}),
        json!({"msgId":"1","threadId":"peer","deliveredUids":["0"]}),
        json!({"msgId":"1","threadId":"peer","deliveredUids":null}),
    ] {
        assert!(parse_delivery_events(502, 1, &json!({"delivereds":[record]}), "self").is_empty());
    }
    assert!(parse_delivery_events(
        502,
        1,
        &json!({"seens":[{"idTo":"peer","msgId":"1"}]}),
        "self"
    )
    .is_empty());
}
#[test]
fn fallback_thread_and_raw_missing_null_are_preserved() {
    let events = parse_delivery_events(
        502,
        0,
        &json!({"delivereds":[
            {"msgId":99,"deliveredUids":["peer"],"version":null,"cliMsgId":null},
            {"msgId":"100","threadId":"peer"}
        ]}),
        "self",
    );
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].thread_id, "peer");
    assert_eq!(events[0].version, Some(json!(null)));
    assert_eq!(events[1].version, None);
    assert_eq!(events[0].raw.get("cliMsgId"), Some(&json!(null)));
    assert!(events[1].raw.get("cliMsgId").is_none());
    let roundtrip: Vec<DeliveryEvent> =
        serde_json::from_value(serde_json::to_value(&events).unwrap()).unwrap();
    assert_eq!(roundtrip, events);
}
#[test]
fn exact_outgoing_only_monotonic_and_idempotent() {
    let mut threads = vec![
        thread(
            "peer",
            vec![
                message("1", "a", true),
                message("2", "b", false),
                message("3", "c", true),
            ],
        ),
        thread("other", vec![message("1", "a", true)]),
    ];
    let events = parse_delivery_events(
        502,
        1,
        &json!({"delivereds":[
            {"threadId":"peer","msgId":"1","seen":1},
            {"threadId":"peer","msgId":"1","seen":0},
            {"threadId":"peer","msgId":"2","seen":1}
        ]}),
        "self",
    );
    assert_eq!(apply_delivery_events(&mut threads, &events).len(), 1);
    assert_eq!(threads[0].messages[0].delivery, DeliveryState::Read);
    assert_eq!(
        threads[0].messages[1].delivery,
        DeliveryState::ServerAccepted
    );
    assert_eq!(
        threads[0].messages[2].delivery,
        DeliveryState::ServerAccepted
    );
    assert_eq!(
        threads[1].messages[0].delivery,
        DeliveryState::ServerAccepted
    );
    assert!(apply_delivery_events(&mut threads, &events).is_empty());
}
#[test]
fn client_matching_is_unique_and_cannot_override_server_conflict() {
    let events = parse_delivery_events(
        522,
        1,
        &json!({"delivereds":[{"groupId":"7","msgId":"99","cliMsgId":"c","seen":0}]}),
        "self",
    );
    for messages in [
        vec![message("out_a", "c", true), message("out_b", "c", true)],
        vec![message("other-server", "c", true)],
    ] {
        let mut threads = vec![thread("g7", messages)];
        assert!(apply_delivery_events(&mut threads, &events).is_empty());
    }
    let mut threads = vec![thread("g7", vec![message("out_a", "c", true)])];
    assert_eq!(apply_delivery_events(&mut threads, &events).len(), 1);
    assert_eq!(threads[0].messages[0].id, "out_a");
    assert_eq!(threads[0].messages[0].delivery, DeliveryState::Delivered);
    let mut duplicate_threads = vec![threads[0].clone(), threads[0].clone()];
    assert!(apply_delivery_events(&mut duplicate_threads, &events).is_empty());
}
