use std::path::{Path, PathBuf};
use zalo_rust::{
    chat::{ChatMessage, Thread},
    store::{persist_threads, MessageStore, StoredMessage, StoredThread},
    store_v2::{
        AccountStore, HistoryCursor, MessageKey, PageDirection, StoreDelta, ValidatedAlias,
    },
};
fn temp(label: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "zl-v2-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&p).unwrap();
    p
}
fn message(t: &str, id: &str, ts: i64, text: &str) -> StoredMessage {
    StoredMessage {
        msg_id: id.into(),
        thread_id: t.into(),
        sender_id: "sender".into(),
        sender_name: "Sender".into(),
        text: text.into(),
        msg_type: "chat.text".into(),
        ts,
        is_outgoing: false,
        raw_json: "{\"opaque\":true,\"reactions\":[\"❤️\"]}".into(),
    }
}
fn thread(id: &str) -> Thread {
    Thread {
        presentation: Default::default(),
        id: id.into(),
        name: "Same name".into(),
        preview: "Preview".into(),
        time: "12:00".into(),
        avatar: Some("https://fixture.invalid/avatar".into()),
        is_group: false,
        messages: Vec::new(),
        unread: Default::default(),
    }
}
#[test]
fn composite_identity_cursor_search_accent_and_restart() {
    let root = temp("restart");
    let mut store = AccountStore::open(&root, "account-a").unwrap();
    let records: Vec<_> = (0..605)
        .map(|i| {
            message(
                "a",
                &format!("m{i:04}"),
                i / 3,
                "Đặng đã đến thành phố Hồ Chí Minh",
            )
        })
        .chain([message("b", "m0000", 0, "Other thread")])
        .collect();
    assert_eq!(
        store
            .apply_delta(StoreDelta {
                messages: &records,
                ..Default::default()
            })
            .unwrap()
            .inserted,
        606
    );
    assert_eq!(
        store
            .apply_delta(StoreDelta {
                messages: &records,
                ..Default::default()
            })
            .unwrap()
            .unchanged,
        606
    );
    drop(store);
    let store = AccountStore::open(&root, "account-a").unwrap();
    let page = store
        .history_page("a", None, PageDirection::Before)
        .unwrap();
    assert_eq!(page.messages.len(), 200);
    assert!(page.has_more);
    assert_eq!(page.messages.last().unwrap().msg_id, "m0604");
    let oldest = page.messages.first().unwrap();
    let cursor = HistoryCursor {
        ts: oldest.ts,
        msg_id: oldest.msg_id.clone(),
    };
    let before = store
        .history_page("a", Some(&cursor), PageDirection::Before)
        .unwrap();
    assert_eq!(before.messages.len(), 200);
    assert!(before
        .messages
        .iter()
        .all(|m| (m.ts, &m.msg_id) < (cursor.ts, &cursor.msg_id)));
    let after = store
        .history_page("a", Some(&cursor), PageDirection::After)
        .unwrap();
    assert_eq!(after.messages.len(), 199);
    assert!(after
        .messages
        .iter()
        .all(|m| (m.ts, &m.msg_id) > (cursor.ts, &cursor.msg_id)));
    assert_eq!(
        store
            .history_page("b", None, PageDirection::Before)
            .unwrap()
            .messages
            .len(),
        1
    );
    let first = store
        .search("dang da den thanh pho ho chi minh", None, 0, 200)
        .unwrap();
    let second = store
        .search("dang da den thanh pho ho chi minh", None, 200, 200)
        .unwrap();
    assert_eq!(first.len(), 200);
    assert_eq!(second.len(), 200);
    assert_ne!(first[0].message.msg_id, second[0].message.msg_id);
    for query in ["\" OR *", "' ; DROP TABLE messages; --", "NEAR(", "*"] {
        assert!(store.search(query, None, 0, 200).is_ok());
    }
    assert_eq!(
        store
            .search("Other thread", Some("a"), 0, 200)
            .unwrap()
            .len(),
        0
    );
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn partial_snapshot_preserves_unloaded_history_and_metadata() {
    let mut store = AccountStore::in_memory("a").unwrap();
    let mut t = thread("a");
    t.unread.last_seen_id = Some("seen".into());
    t.unread.muted = true;
    t.messages = (0..405)
        .map(|i| ChatMessage {
            id: format!("{i:04}"),
            text: "hello".into(),
            ts: i,
            ..Default::default()
        })
        .collect();
    assert_eq!(store.persist_threads(&[t.clone()]).unwrap().inserted, 405);
    t.messages = t.messages[400..].to_vec();
    t.messages[0].text = "edited".into();
    let stats = store.persist_threads(&[t.clone()]).unwrap();
    assert_eq!(stats.updated, 1);
    assert_eq!(stats.unchanged, 4);
    assert_eq!(store.search("hello", None, 200, 200).unwrap().len(), 200);
    let meta = store.all_thread_metadata().unwrap();
    assert_eq!(meta[0].avatar, t.avatar);
    assert_eq!(meta[0].preview, t.preview);
    assert!(meta[0].messages.is_empty());
    assert_eq!(meta[0].unread, t.unread);
}
#[test]
fn delta_transaction_rolls_back_and_delete_is_exact() {
    let mut store = AccountStore::in_memory("a").unwrap();
    let records = [
        message("a", "one", 10, "first"),
        message("a", "two", 10, "second"),
        message("b", "one", 10, "third"),
    ];
    store
        .apply_delta(StoreDelta {
            messages: &records,
            ..Default::default()
        })
        .unwrap();
    let bad = [
        message("a", "three", 11, "rollback"),
        message("a", "", 12, "invalid"),
    ];
    assert!(store
        .apply_delta(StoreDelta {
            messages: &bad,
            ..Default::default()
        })
        .is_err());
    assert!(store.search("rollback", None, 0, 200).unwrap().is_empty());
    store
        .apply_delta(StoreDelta {
            deleted_messages: &[MessageKey {
                thread_id: "a".into(),
                msg_id: "one".into(),
            }],
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        store
            .history_page("a", None, PageDirection::Before)
            .unwrap()
            .messages[0]
            .msg_id,
        "two"
    );
    assert_eq!(
        store
            .history_page("b", None, PageDirection::Before)
            .unwrap()
            .messages
            .len(),
        1
    );
    assert!(store.search("first", None, 0, 200).unwrap().is_empty());
}
#[test]
fn account_isolation_permissions_alias_validation_and_same_name() {
    let root = temp("accounts");
    let mut a = AccountStore::open(&root, "a/../x").unwrap();
    let b = AccountStore::open(&root, "b").unwrap();
    a.persist_threads(&[thread("a"), thread("b")]).unwrap();
    assert_eq!(a.all_threads().unwrap().len(), 2);
    assert!(b.all_threads().unwrap().is_empty());
    assert!(ValidatedAlias::from_protocol_identity("a", "b", false).is_err());
    assert_eq!(a.resolve_thread("a").unwrap(), "a");
    a.register_alias(ValidatedAlias::from_protocol_identity("old", "a", true).unwrap())
        .unwrap();
    assert_eq!(a.resolve_thread("old").unwrap(), "a");
    assert!(a
        .register_alias(ValidatedAlias::from_protocol_identity("a", "old", true).unwrap())
        .is_err());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let p = a.path.as_ref().unwrap();
        assert_eq!(
            std::fs::metadata(p).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(p.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
    drop(a);
    drop(b);
    std::fs::remove_dir_all(root).unwrap();
}
fn bytes(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap()
}
#[test]
fn copy_on_write_migration_preserves_raw_unread_reactions_and_idempotence() {
    let root = temp("migration");
    let db = root.join("legacy.db");
    let json = root.join("legacy.json");
    let mut legacy = MessageStore::open(db.to_str().unwrap()).unwrap();
    let mut t = thread("old");
    t.unread.last_seen_id = Some("seen".into());
    t.unread.muted = true;
    t.messages.push(ChatMessage {
        id: "one".into(),
        text: "Đã lưu".into(),
        ts: 1,
        reactions: vec!["❤️".into()],
        ..Default::default()
    });
    persist_threads(&mut legacy, &[t.clone()]).unwrap();
    legacy
        .conn
        .execute(
            "UPDATE messages SET raw_json=?1",
            ["{\"opaque_wire\":17,\"reactions\":[\"❤️\"]}"],
        )
        .unwrap();
    drop(legacy);
    let mut stale_cache = t.clone();
    stale_cache.unread = Default::default();
    stale_cache.messages[0].reactions.clear();
    std::fs::write(&json, serde_json::to_vec(&vec![stale_cache]).unwrap()).unwrap();
    let before_db = bytes(&db);
    let before_json = bytes(&json);
    let mut store = AccountStore::open(&root, "account").unwrap();
    let report = store.migrate_legacy(Some(&db), Some(&json)).unwrap();
    assert!(!report.already_imported);
    drop(store);
    let mut store = AccountStore::open(&root, "account").unwrap();
    assert!(
        store
            .migrate_legacy(Some(&db), Some(&json))
            .unwrap()
            .already_imported
    );
    assert_eq!(before_db, bytes(&db));
    assert_eq!(before_json, bytes(&json));
    let saved = &store
        .history_page("old", None, PageDirection::Before)
        .unwrap()
        .messages[0];
    assert!(saved.raw_json.contains("opaque_wire"));
    assert_eq!(
        store.chat_message(saved).unwrap().reactions,
        t.messages[0].reactions
    );
    assert_eq!(store.unread("old").unwrap(), t.unread);
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn invalid_migration_rolls_back_without_touching_legacy() {
    let root = temp("bad-migration");
    let db = root.join("legacy.db");
    let json = root.join("bad.json");
    let legacy = MessageStore::open(db.to_str().unwrap()).unwrap();
    legacy
        .upsert_thread(&StoredThread {
            thread_id: "x".into(),
            name: "x".into(),
            is_group: false,
            last_ts: 1,
        })
        .unwrap();
    legacy
        .upsert_message(&message("x", "one", 1, "hello"))
        .unwrap();
    drop(legacy);
    std::fs::write(&json, b"{bad json").unwrap();
    let original = bytes(&db);
    let mut store = AccountStore::open(&root, "a").unwrap();
    assert!(store.migrate_legacy(Some(&db), Some(&json)).is_err());
    assert!(store.all_threads().unwrap().is_empty());
    assert!(store.search("hello", None, 0, 200).unwrap().is_empty());
    assert_eq!(bytes(&db), original);
    std::fs::remove_file(&json).unwrap();
    assert_eq!(store.migrate_legacy(Some(&db), None).unwrap().messages, 1);
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn legacy_same_name_hydration_and_timestamp_delete_keep_distinct_identity() {
    let mut store = MessageStore::open(":memory:").unwrap();
    let mut old = thread("old");
    old.messages = vec![
        ChatMessage {
            id: "one".into(),
            text: "same".into(),
            ts: 1,
            ..Default::default()
        },
        ChatMessage {
            id: "two".into(),
            text: "same".into(),
            ts: 1,
            ..Default::default()
        },
    ];
    persist_threads(&mut store, &[old]).unwrap();
    let mut live = vec![thread("live")];
    store.hydrate_threads(&mut live, 200).unwrap();
    assert_eq!(live.len(), 2);
    assert_eq!(
        live.iter().find(|t| t.id == "old").unwrap().messages.len(),
        2
    );
    store.delete_message("old", "one", 1).unwrap();
    let rows = store.messages_for("old", 200).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].msg_id, "two");
}

#[test]
fn ten_thousand_rows_repeated_snapshot_only_updates_changed_row() {
    let mut store = AccountStore::in_memory("perf-fixture").unwrap();
    let mut t = thread("large");
    t.messages = (0..10_000)
        .map(|i| ChatMessage {
            id: format!("large-{i:05}"),
            text: format!("Tin nhắn đã lưu {i}"),
            ts: i,
            ..Default::default()
        })
        .collect();
    let start = std::time::Instant::now();
    assert_eq!(
        store
            .persist_threads(std::slice::from_ref(&t))
            .unwrap()
            .inserted,
        10_000
    );
    let imported = start.elapsed();
    t.messages[3].text = "Only changed row".into();
    let start = std::time::Instant::now();
    let result = store.persist_threads(std::slice::from_ref(&t)).unwrap();
    let delta = start.elapsed();
    assert_eq!(result.updated, 1);
    assert_eq!(result.unchanged, 9999);
    assert_eq!(result.inserted, 0);
    let start = std::time::Instant::now();
    assert_eq!(
        store
            .history_page("large", None, PageDirection::Before)
            .unwrap()
            .messages
            .len(),
        200
    );
    let page = start.elapsed();
    let start = std::time::Instant::now();
    assert_eq!(
        store
            .search("tin nhan da luu", None, 200, 200)
            .unwrap()
            .len(),
        200
    );
    println!("fixture_rows=10000 initial_ms={} repeat_ms={} page_ms={} search_ms={} changed=1 unchanged=9999",imported.as_millis(),delta.as_millis(),page.as_millis(),start.elapsed().as_millis());
}

#[test]
fn partial_network_batch_preserves_reactions_delivery_cursor_and_avatar() {
    use zalo_rust::message_model::DeliveryState;
    let mut store = AccountStore::in_memory("partial").unwrap();
    let mut initial = thread("one");
    initial.unread.last_seen_id = Some("100".into());
    initial.unread.unread_ids.insert("101".into());
    initial.unread.muted = true;
    let mut m = ChatMessage {
        id: "101".into(),
        text: "hello".into(),
        ts: 1,
        delivery: DeliveryState::Read,
        reaction_authoritative_snapshot: true,
        reaction_snapshot: true,
        ..Default::default()
    };
    m.reaction_summaries =
        zalo_rust::reactions::parse_reactions(&serde_json::json!({"reactions":{"a":{"5":2}}}));
    initial.messages = vec![m.clone()];
    store.persist_threads(&[initial.clone()]).unwrap();
    let mut partial = thread("one");
    partial.name.clear();
    partial.preview.clear();
    partial.time.clear();
    partial.avatar = None;
    partial.messages = vec![ChatMessage {
        id: "101".into(),
        text: "hello".into(),
        ts: 1,
        ..Default::default()
    }];
    store.persist_threads(&[partial.clone()]).unwrap();
    let saved = store.get_message("one", "101").unwrap().unwrap();
    assert_eq!(saved.reaction_summaries[0].count, 2);
    assert_eq!(saved.delivery, DeliveryState::Read);
    let metadata = store.all_thread_metadata().unwrap();
    assert_eq!(metadata[0].avatar, initial.avatar);
    assert_eq!(metadata[0].preview, initial.preview);
    assert_eq!(metadata[0].unread, initial.unread);
    let mut tombstone = m;
    tombstone.reaction_summaries.clear();
    tombstone.reactions.clear();
    tombstone.reaction_snapshot_fresh = true;
    partial.messages = vec![tombstone];
    store.persist_threads(&[partial.clone()]).unwrap();
    partial.messages = vec![ChatMessage {
        id: "101".into(),
        text: "hello".into(),
        ts: 1,
        ..Default::default()
    }];
    store.persist_threads(&[partial.clone()]).unwrap();
    let saved = store.get_message("one", "101").unwrap().unwrap();
    assert!(saved.reaction_summaries.is_empty());
    assert!(saved.reaction_authoritative_snapshot);
    partial.unread = initial.unread;
    partial.unread.read_visible(&partial.messages);
    store.persist_threads(&[partial]).unwrap();
    let unread = store.unread("one").unwrap();
    assert_eq!(unread.last_seen_id.as_deref(), Some("101"));
    assert_eq!(unread.count(), 0);
    assert!(unread.muted);
    assert!(store.get_message("one", "absent").unwrap().is_none());
}

#[test]
fn unread_derivation_uses_all_persisted_eligible_ids_not_the_visible_page() {
    use zalo_rust::message_model::{MessageKind, UnreadPolicy};
    let mut store = AccountStore::in_memory("unread").unwrap();
    let mut t = thread("large");
    t.messages = (1..=605)
        .map(|i| ChatMessage {
            id: i.to_string(),
            text: "incoming".into(),
            ts: i,
            ..Default::default()
        })
        .collect();
    t.messages.push(ChatMessage {
        id: "606".into(),
        is_outgoing: true,
        ..Default::default()
    });
    t.messages.push(ChatMessage {
        id: "607".into(),
        unread_policy: UnreadPolicy::from_raw(&serde_json::json!({"paramsExt":{"countUnread":0}})),
        ..Default::default()
    });
    t.messages.push(ChatMessage {
        id: "608".into(),
        kind: MessageKind::System,
        ..Default::default()
    });
    t.messages.push(ChatMessage {
        id: "unknown-wire".into(),
        ..Default::default()
    });
    t.messages.push(ChatMessage {
        id: "000609".into(),
        text: "numeric leading zeros".into(),
        ..Default::default()
    });
    store.persist_threads(&[t]).unwrap();
    let mut state = zalo_rust::unread::UnreadState {
        last_seen_id: Some("5".into()),
        muted: true,
        marked_unread: true,
        ..Default::default()
    };
    store.derive_unread("large", &mut state).unwrap();
    assert_eq!(state.count(), 601);
    assert!(state.unread_ids.contains("6"));
    assert!(state.unread_ids.contains("000609"));
    assert!(!state.unread_ids.contains("607"));
    assert!(state.muted);
    assert!(state.marked_unread);
    state.last_seen_id = Some("605".into());
    store.derive_unread("large", &mut state).unwrap();
    assert_eq!(
        state.unread_ids.iter().cloned().collect::<Vec<_>>(),
        vec!["000609"]
    );
    store
        .apply_delta(StoreDelta {
            unread: &[("large", &state)],
            ..Default::default()
        })
        .unwrap();
    let mut visible = thread("large");
    visible.messages = vec![ChatMessage {
        id: "605".into(),
        ..Default::default()
    }];
    store.persist_threads(&[visible]).unwrap();
    assert_eq!(store.unread("large").unwrap().unread_ids, state.unread_ids);
}

#[test]
fn legacy_normalized_metadata_and_reaction_provenance_survive_restart() {
    use zalo_rust::message_model::{DeliveryState, MessageKind, UnreadPolicy, WireMessageRef};
    let root = temp("legacy-metadata");
    let path = root.join("legacy.db");
    let mut legacy = MessageStore::open(path.to_str().unwrap()).unwrap();
    let mut t = thread("typed");
    let m = ChatMessage {
        id: "42".into(),
        text: "Attachment".into(),
        kind: MessageKind::Photo,
        wire_ref: WireMessageRef::from_raw(&serde_json::json!({"msgId":"42","unknownWire":null})),
        unread_policy: UnreadPolicy::from_raw(&serde_json::json!({"paramsExt":{"countUnread":0}})),
        delivery: DeliveryState::Read,
        reaction_snapshot_provenance: zalo_rust::reactions::ReactionSnapshotProvenance {
            source: "fixture".into(),
            captured_at_ms: 12345,
        },
        ..Default::default()
    };
    t.messages = vec![m.clone()];
    persist_threads(&mut legacy, &[t]).unwrap();
    drop(legacy);
    let legacy = MessageStore::open(path.to_str().unwrap()).unwrap();
    let mut restored = Vec::new();
    legacy.hydrate_threads(&mut restored, 200).unwrap();
    let saved = &restored[0].messages[0];
    assert_eq!(saved.kind, m.kind);
    assert_eq!(saved.wire_ref, m.wire_ref);
    assert_eq!(saved.unread_policy, m.unread_policy);
    assert_eq!(saved.delivery, m.delivery);
    assert_eq!(
        saved.reaction_snapshot_provenance,
        m.reaction_snapshot_provenance
    );
    drop(legacy);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn persisted_mentions_survive_offscreen_pages_restart_and_account_isolation() {
    use zalo_rust::message_model::{UnreadPolicy, WireMessageRef};
    let root = temp("mentions");
    let mut a = AccountStore::open(&root, "account-a").unwrap();
    let mut b = AccountStore::open(&root, "account-b").unwrap();
    let mut t = thread("mentions");
    t.is_group = true;
    t.messages = (1..=605)
        .map(|i| ChatMessage {
            id: i.to_string(),
            text: "history".into(),
            ts: i,
            ..Default::default()
        })
        .collect();
    let personal = ChatMessage {
        id: "2".into(),
        text: "personal mention".into(),
        ts: 2,
        wire_ref: WireMessageRef::from_raw(&serde_json::json!({"mentions":[{"uid":"account-a"}]})),
        ..Default::default()
    };
    t.messages[1] = personal;
    t.messages[2] = ChatMessage {
        id: "3".into(),
        text: "all mention".into(),
        ts: 3,
        history_source: Some(serde_json::json!({"meta":{"mentionsList":[{"uid":"-1"}]}})),
        ..Default::default()
    };
    t.messages[3] = ChatMessage {
        id: "4".into(),
        text: "suppressed mention".into(),
        ts: 4,
        wire_ref: WireMessageRef::from_raw(&serde_json::json!({"mentions":[{"uid":"account-a"}]})),
        unread_policy: UnreadPolicy::from_raw(&serde_json::json!({"paramsExt":{"countUnread":0}})),
        ..Default::default()
    };
    t.messages[4] = ChatMessage {
        id: "5".into(),
        text: "unverified uidTo".into(),
        ts: 5,
        wire_ref: WireMessageRef::from_raw(
            &serde_json::json!({"mentions":[{"uidTo":"account-a"}]}),
        ),
        ..Default::default()
    };
    a.persist_threads(&[t.clone()]).unwrap();
    b.persist_threads(&[t]).unwrap();
    assert!(a
        .history_page("mentions", None, PageDirection::Before)
        .unwrap()
        .messages
        .iter()
        .all(|m| m.ts > 5));
    let mut state = zalo_rust::unread::UnreadState {
        last_seen_id: Some("1".into()),
        muted: true,
        marked_unread: true,
        ..Default::default()
    };
    a.derive_unread("mentions", &mut state).unwrap();
    assert_eq!(
        state.mention_ids.iter().cloned().collect::<Vec<_>>(),
        vec!["2", "3"]
    );
    assert!(!state.unread_ids.contains("4"));
    assert!(state.muted && state.marked_unread);
    a.apply_delta(StoreDelta {
        unread: &[("mentions", &state)],
        ..Default::default()
    })
    .unwrap();
    let mut visible = thread("mentions");
    visible.messages = vec![ChatMessage {
        id: "605".into(),
        text: "history".into(),
        ts: 605,
        ..Default::default()
    }];
    a.persist_threads(&[visible]).unwrap();
    assert_eq!(a.unread("mentions").unwrap().mention_ids, state.mention_ids);
    let mut other = state.clone();
    b.derive_unread("mentions", &mut other).unwrap();
    assert_eq!(
        other.mention_ids.iter().cloned().collect::<Vec<_>>(),
        vec!["3"]
    );
    drop(a);
    let mut a = AccountStore::open(&root, "account-a").unwrap();
    let mut state = a.unread("mentions").unwrap();
    assert_eq!(state.mention_ids.len(), 2);
    state.advance_seen("2");
    a.derive_unread("mentions", &mut state).unwrap();
    assert_eq!(
        state.mention_ids.iter().cloned().collect::<Vec<_>>(),
        vec!["3"]
    );
    a.apply_delta(StoreDelta {
        unread: &[("mentions", &state)],
        ..Default::default()
    })
    .unwrap();
    state.read_visible(&[ChatMessage {
        id: "605".into(),
        ..Default::default()
    }]);
    a.derive_unread("mentions", &mut state).unwrap();
    assert!(state.mention_ids.is_empty());
    assert!(state.unread_ids.is_empty());
    assert!(state.muted);
    let mut local_read = thread("mentions");
    local_read.unread = state;
    a.persist_threads(&[local_read]).unwrap();
    drop(a);
    drop(b);
    let a = AccountStore::open(&root, "account-a").unwrap();
    assert!(a.unread("mentions").unwrap().mention_ids.is_empty());
    drop(a);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn old_v2_schema_backfills_mention_index_once_without_rewriting_messages() {
    use zalo_rust::message_model::WireMessageRef;
    let root = temp("mention-upgrade");
    let mut store = AccountStore::open(&root, "owner").unwrap();
    let mut t = thread("thread");
    t.messages = vec![ChatMessage {
        id: "20".into(),
        text: "mention".into(),
        wire_ref: WireMessageRef::from_raw(&serde_json::json!({"mentions":[{"uid":"owner"}]})),
        ..Default::default()
    }];
    store.persist_threads(&[t]).unwrap();
    let path = store.path.clone().unwrap();
    drop(store);
    let conn = rusqlite::Connection::open(&path).unwrap();
    let before: String = conn
        .query_row("SELECT fingerprint FROM messages", [], |r| r.get(0))
        .unwrap();
    conn.execute_batch(
        "DROP INDEX mention_cursor; ALTER TABLE messages DROP COLUMN mention_eligible; PRAGMA user_version=0;",
    )
    .unwrap();
    drop(conn);
    let store = AccountStore::open(&root, "owner").unwrap();
    let mut state = zalo_rust::unread::UnreadState {
        last_seen_id: Some("1".into()),
        ..Default::default()
    };
    store.derive_unread("thread", &mut state).unwrap();
    assert!(state.mention_ids.contains("20"));
    drop(store);
    let conn = rusqlite::Connection::open(&path).unwrap();
    let after: String = conn
        .query_row("SELECT fingerprint FROM messages", [], |r| r.get(0))
        .unwrap();
    assert_eq!(before, after);
    let plan:String=conn.query_row("EXPLAIN QUERY PLAN SELECT msg_id FROM messages WHERE thread_id='thread' AND unread_eligible=1 AND mention_eligible=1 AND (numeric_len,numeric_id)>(1,'1') ORDER BY numeric_len,numeric_id,msg_id",[],|r|r.get(3)).unwrap();
    assert!(plan.contains("mention_cursor"), "{plan}");
    drop(conn);
    let store = AccountStore::open(&root, "owner").unwrap();
    store.derive_unread("thread", &mut state).unwrap();
    assert_eq!(state.mention_ids.len(), 1);
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn thread_message_counts_aggregates_only_persisted_rows_across_threads() {
    let mut store = AccountStore::in_memory("counts").unwrap();
    assert!(store.thread_message_counts().unwrap().is_empty());
    let rows = [
        message("heavy", "same-wire", 1, "one"),
        message("heavy", "another-wire", 2, "two"),
        message("other", "same-wire", 1, "three"),
    ];
    store
        .apply_delta(StoreDelta {
            messages: &rows,
            ..Default::default()
        })
        .unwrap();
    let counts = store.thread_message_counts().unwrap();
    assert_eq!(counts.get("heavy"), Some(&2));
    assert_eq!(counts.get("other"), Some(&1));
    assert_eq!(counts.len(), 2);
    store
        .apply_delta(StoreDelta {
            deleted_messages: &[MessageKey {
                thread_id: "heavy".into(),
                msg_id: "same-wire".into(),
            }],
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        store.thread_message_counts().unwrap().get("heavy"),
        Some(&1)
    );
}

#[test]
fn exact_local_delete_tombstones_block_stale_replay_after_restart() {
    let root = temp("delete-tombstones");
    let mut store = AccountStore::open(&root, "owner").unwrap();
    let rows = [
        message("a", "1", 10, "deletedneedle"),
        message("a", "2", 10, "retainedneedle"),
        message("b", "1", 10, "otherneedle"),
    ];
    let mut unread = zalo_rust::unread::UnreadState {
        last_seen_id: Some("0".into()),
        ..Default::default()
    };
    unread.unread_ids.extend(["1".into(), "2".into()]);
    unread.mention_ids.insert("1".into());
    store
        .apply_delta(StoreDelta {
            messages: &rows,
            unread: &[("a", &unread)],
            ..Default::default()
        })
        .unwrap();
    store
        .apply_delta(StoreDelta {
            deleted_messages: &[
                MessageKey {
                    thread_id: "a".into(),
                    msg_id: "1".into(),
                },
                MessageKey {
                    thread_id: "a".into(),
                    msg_id: "3".into(),
                },
            ],
            ..Default::default()
        })
        .unwrap();
    assert!(store
        .search("deletedneedle", None, 0, 200)
        .unwrap()
        .is_empty());
    let mut stale = thread("a");
    stale.unread = unread;
    stale.messages = vec![
        ChatMessage {
            id: "1".into(),
            text: "deletedneedle".into(),
            ts: 10,
            ..Default::default()
        },
        ChatMessage {
            id: "3".into(),
            text: "neverloadedneedle".into(),
            ts: 12,
            ..Default::default()
        },
    ];
    let result = store.persist_threads(&[stale.clone()]).unwrap();
    assert_eq!(
        result.inserted, 0,
        "stale cache replay resurrected an exact local deletion"
    );
    let state = store.unread("a").unwrap();
    assert!(!state.unread_ids.contains("1"));
    assert!(!state.mention_ids.contains("1"));
    assert!(state.unread_ids.contains("2"));
    drop(store);
    let mut store = AccountStore::open(&root, "owner").unwrap();
    store.persist_threads(&[stale]).unwrap();
    store
        .apply_delta(StoreDelta {
            messages: &[rows[0].clone(), message("a", "3", 12, "neverloadedneedle")],
            ..Default::default()
        })
        .unwrap();
    assert!(store.get_message("a", "1").unwrap().is_none());
    assert!(store.get_message("a", "3").unwrap().is_none());
    assert_eq!(
        store.get_message("a", "2").unwrap().unwrap().text,
        "retainedneedle"
    );
    assert_eq!(
        store.get_message("b", "1").unwrap().unwrap().text,
        "otherneedle"
    );
    assert!(store
        .search("deletedneedle", None, 0, 200)
        .unwrap()
        .is_empty());
    assert!(store
        .search("neverloadedneedle", None, 0, 200)
        .unwrap()
        .is_empty());
    assert_eq!(store.search("otherneedle", None, 0, 200).unwrap().len(), 1);
    let counts = store.thread_message_counts().unwrap();
    assert_eq!(counts.get("a"), Some(&1));
    assert_eq!(counts.get("b"), Some(&1));
    let mut other = AccountStore::open(&root, "other-owner").unwrap();
    other
        .apply_delta(StoreDelta {
            messages: &[rows[0].clone()],
            ..Default::default()
        })
        .unwrap();
    assert!(other.get_message("a", "1").unwrap().is_some());
    drop(other);
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn invalid_delete_batch_rolls_back_tombstone_message_fts_and_badges() {
    let mut store = AccountStore::in_memory("rollback-delete").unwrap();
    let row = message("thread", "1", 1, "preservedneedle");
    let mut state = zalo_rust::unread::UnreadState {
        last_seen_id: Some("0".into()),
        ..Default::default()
    };
    state.unread_ids.insert("1".into());
    state.mention_ids.insert("1".into());
    store
        .apply_delta(StoreDelta {
            messages: std::slice::from_ref(&row),
            unread: &[("thread", &state)],
            ..Default::default()
        })
        .unwrap();
    assert!(store
        .apply_delta(StoreDelta {
            deleted_messages: &[
                MessageKey {
                    thread_id: "thread".into(),
                    msg_id: "1".into()
                },
                MessageKey {
                    thread_id: "".into(),
                    msg_id: "invalid".into()
                }
            ],
            ..Default::default()
        })
        .is_err());
    assert!(store.get_message("thread", "1").unwrap().is_some());
    assert_eq!(
        store.search("preservedneedle", None, 0, 200).unwrap().len(),
        1
    );
    assert_eq!(store.unread("thread").unwrap(), state);
    let mut updated = row;
    updated.text = "updatedneedle".into();
    assert_eq!(
        store
            .apply_delta(StoreDelta {
                messages: &[updated],
                ..Default::default()
            })
            .unwrap()
            .updated,
        1
    );
    assert_eq!(
        store.search("updatedneedle", None, 0, 200).unwrap().len(),
        1
    );
}

#[test]
fn quote_navigation_resolves_offscreen_exact_client_reference_and_is_bounded() {
    let mut store = AccountStore::in_memory("fixture-account").unwrap();
    let mut t = thread("group");
    t.is_group = true;
    t.messages = (0..700)
        .map(|i| ChatMessage {
            id: format!("s{i:04}"),
            client_message_id: format!("c{i:04}"),
            ts: 1_790_000_000_000 + i,
            sender: "Rin".into(),
            text: format!("body {i}"),
            ..Default::default()
        })
        .collect();
    store.persist_threads(&[t]).unwrap();
    let (id, page) = store.navigation_page("group", "c0300").unwrap().unwrap();
    assert_eq!(id, "s0300");
    assert_eq!(page.len(), 200);
    assert_eq!(page[80].id, id);
    assert!(store.navigation_page("other", "c0300").unwrap().is_none());
    assert!(store
        .navigation_page("group", "body 300")
        .unwrap()
        .is_none());
}

#[test]
fn quote_navigation_rejects_server_client_namespace_collision() {
    let mut store = AccountStore::in_memory("fixture-account").unwrap();
    let mut t = thread("group");
    t.messages = vec![
        ChatMessage {
            id: "collision".into(),
            ts: 1,
            ..Default::default()
        },
        ChatMessage {
            id: "other".into(),
            client_message_id: "collision".into(),
            ts: 2,
            ..Default::default()
        },
    ];
    store.persist_threads(&[t]).unwrap();
    assert!(store
        .navigation_page("group", "collision")
        .unwrap()
        .is_none());
}

#[test]
fn sidebar_metadata_survives_unloaded_restart_and_old_page_update() {
    let mut store = AccountStore::in_memory("fixture-account").unwrap();
    let mut t = thread("group");
    t.is_group = true;
    t.preview = "latest".into();
    t.presentation.member_count = Some(54);
    t.presentation.members = vec![zalo_rust::chat::SidebarMember {
        id: "member".into(),
        name: "Rin".into(),
        avatar: Some("https://fixture.invalid/member.jpg".into()),
    }];
    t.messages = vec![ChatMessage {
        id: "latest".into(),
        ts: 1_790_000_000_000,
        sender: "Rin".into(),
        text: "latest".into(),
        ..Default::default()
    }];
    store.persist_threads(&[t]).unwrap();
    let mut old = thread("group");
    old.preview = "old".into();
    old.messages = vec![ChatMessage {
        id: "old".into(),
        ts: 1_780_000_000_000,
        sender: "Other".into(),
        text: "old".into(),
        ..Default::default()
    }];
    store.persist_threads(&[old]).unwrap();
    let restored = store.all_thread_metadata().unwrap().pop().unwrap();
    assert!(restored.messages.is_empty());
    assert_eq!(restored.sidebar_preview(), "Rin: latest");
    assert_eq!(restored.presentation.member_count, Some(54));
    assert_eq!(restored.presentation.members.len(), 1);
}

#[test]
fn resolved_sidebar_sender_survives_numeric_duplicate_and_restart() {
    let mut store = AccountStore::in_memory("fixture-account").unwrap();
    let mut t = thread("group");
    t.is_group = true;
    t.preview = "Hello".into();
    t.presentation = zalo_rust::chat::SidebarPresentation {
        last_message_ts: 1793425680000,
        last_message_id: "latest".into(),
        last_sender: "PHƯƠNG".into(),
        last_sender_id: "123".into(),
        ..Default::default()
    };
    store.persist_threads(&[t.clone()]).unwrap();
    t.presentation = Default::default();
    t.messages = vec![ChatMessage {
        id: "latest".into(),
        sender: "123".into(),
        text: "Hello".into(),
        ts: 1793425680000,
        wire_ref: zalo_rust::message_model::WireMessageRef::from_raw(
            &serde_json::json!({"uidFrom":"123"}),
        ),
        ..Default::default()
    }];
    store.persist_threads(&[t]).unwrap();
    let restored = store.all_thread_metadata().unwrap().pop().unwrap();
    assert_eq!(restored.sidebar_preview(), "PHƯƠNG: Hello");
    assert!(restored.messages.is_empty());
}

#[test]
fn legacy_contact_activity_timestamp_is_repaired_from_actual_latest_message() {
    let mut store = AccountStore::in_memory("fixture-account").unwrap();
    let mut t = thread("peer");
    t.presentation.last_message_ts = 1799999999000;
    t.preview = "Actual body".into();
    t.messages = vec![ChatMessage {
        id: "actual".into(),
        ts: 1793425680000,
        text: "Actual body".into(),
        ..Default::default()
    }];
    store.persist_threads(&[t]).unwrap();
    let metadata = store.all_thread_metadata().unwrap().pop().unwrap();
    assert_eq!(metadata.sidebar_activity_ts(), 1793425680000);
    assert_eq!(metadata.presentation.last_message_id, "actual");
    assert!(metadata.messages.is_empty());
    store.persist_threads(&[metadata]).unwrap();
    assert_eq!(
        store.all_thread_metadata().unwrap()[0].sidebar_activity_ts(),
        1793425680000
    );
}

#[test]
fn archive_mention_sidecar_write_never_changes_sidebar_or_unread_or_raw() {
    let mut store = AccountStore::in_memory("fixture-account").unwrap();
    let mut t = thread("group");
    t.is_group = true;
    let source = serde_json::json!({"senderId":"opaque-sender","meta":{"mentionsList":[{"uid":"opaque-target","pos":0,"len":3}]}});
    t.messages = vec![ChatMessage {
        id: "100".into(),
        sender: "123".into(),
        ts: 1793425680000,
        text: "@An".into(),
        history_source: Some(source.clone()),
        ..Default::default()
    }];
    store.persist_threads(&[t]).unwrap();
    let mut metadata = store.all_thread_metadata().unwrap().pop().unwrap();
    metadata.presentation.last_sender = "PHƯƠNG".into();
    metadata.presentation.last_sender_id = "123".into();
    metadata.unread.unread_ids.insert("100".into());
    metadata.unread.marked_unread = true;
    store.persist_threads(&[metadata]).unwrap();
    let before = store.all_thread_metadata().unwrap().pop().unwrap();
    let unread = store.unread("group").unwrap();
    let mappings = vec![(
        "100".into(),
        std::collections::BTreeMap::from([
            ("opaque-target".into(), "456".into()),
            ("not-present".into(), "789".into()),
        ]),
    )];
    assert_eq!(
        store.persist_mention_mappings("group", &mappings).unwrap(),
        1
    );
    assert_eq!(
        store.persist_mention_mappings("other", &mappings).unwrap(),
        0
    );
    let after = store.all_thread_metadata().unwrap().pop().unwrap();
    assert_eq!(before.presentation, after.presentation);
    assert_eq!(unread, store.unread("group").unwrap());
    let saved = store.get_message("group", "100").unwrap().unwrap();
    assert_eq!(saved.history_source, Some(source));
    assert_eq!(saved.text, "@An");
    assert_eq!(saved.resolved_mention_uids.len(), 1);
    assert_eq!(saved.resolved_mention_uids["opaque-target"], "456");
    assert_eq!(
        store.persist_mention_mappings("group", &mappings).unwrap(),
        0
    );
}

#[test]
fn mention_sidecar_is_durable_account_isolated_and_never_resurrects_deleted_message() {
    let root = temp("markup");
    let mut store = AccountStore::open(&root, "account-a").unwrap();
    let mut t = thread("direct");
    t.messages = vec![ChatMessage {
        id: "100".into(),
        text: "@An".into(),
        ts: 1793425680000,
        history_source: Some(
            serde_json::json!({"meta":{"mentionsList":[{"uid":"opaque","pos":0,"len":3}]}}),
        ),
        ..Default::default()
    }];
    store.persist_threads(&[t]).unwrap();
    let map = vec![(
        "100".into(),
        std::collections::BTreeMap::from([("opaque".into(), "456".into())]),
    )];
    assert_eq!(store.persist_mention_mappings("direct", &map).unwrap(), 1);
    assert_eq!(store.thread_is_group("direct").unwrap(), Some(false));
    drop(store);
    let mut store = AccountStore::open(&root, "account-a").unwrap();
    assert_eq!(
        store
            .get_message("direct", "100")
            .unwrap()
            .unwrap()
            .resolved_mention_uids["opaque"],
        "456"
    );
    let mut other = AccountStore::open(&root, "account-b").unwrap();
    assert_eq!(other.persist_mention_mappings("direct", &map).unwrap(), 0);
    store
        .apply_delta(StoreDelta {
            deleted_messages: &[MessageKey {
                thread_id: "direct".into(),
                msg_id: "100".into(),
            }],
            ..Default::default()
        })
        .unwrap();
    assert_eq!(store.persist_mention_mappings("direct", &map).unwrap(), 0);
    assert!(store.get_message("direct", "100").unwrap().is_none());
}
