//! Account-bound, read-only import of original macOS reaction-cache aggregates.
use std::{collections::HashMap, fs::File, io::Read, path::Path};

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::Value;

use crate::{chat::Thread, reactions::ReactionSummary};

const MAX_BYTES: u64 = 16 * 1024 * 1024;
const MAX_SNAPSHOTS: usize = 20_000;
const SOURCE: &str = "zalo-macos-local-cache";

#[derive(Deserialize)]
struct Envelope {
    schema: u32,
    account_id: String,
    captured_at_ms: u64,
    source: String,
    snapshots: Vec<RawSnapshot>,
}

#[derive(Deserialize)]
struct RawSnapshot {
    #[serde(rename = "idTo")]
    thread_id: String,
    #[serde(rename = "rClientMsgId")]
    client_id: Value,
    #[serde(rename = "rMsgId")]
    message_id: Value,
    reactions: Value,
}

#[derive(Debug)]
struct Snapshot {
    thread_id: String,
    client_id: String,
    message_id: String,
    summaries: Vec<ReactionSummary>,
}

#[derive(Debug)]
pub struct OriginalReactionCache {
    snapshots: Vec<Snapshot>,
    captured_at_ms: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportSummary {
    pub snapshots_loaded: usize,
    pub messages_matched: usize,
    pub messages_updated: usize,
    pub messages_protected: usize,
    pub ambiguous_matches: usize,
    pub empty_snapshots_skipped: usize,
}

fn valid_id(value: &str) -> bool {
    !value.is_empty() && value != "0" && value.len() <= 512 && !value.chars().any(char::is_control)
}

fn id(value: &Value) -> Result<String> {
    match value {
        Value::String(value) if valid_id(value) => Ok(value.clone()),
        Value::String(value) if value.is_empty() || value == "0" => Ok(String::new()),
        Value::Number(value) if value.as_u64().is_some() => Ok(value
            .as_u64()
            .filter(|n| *n != 0)
            .map(|n| n.to_string())
            .unwrap_or_default()),
        _ => bail!("Invalid original reaction identifier"),
    }
}

fn thread_key(value: &str) -> &str {
    value.strip_prefix('g').unwrap_or(value)
}

fn validate(bytes: &[u8], expected_account: &str) -> Result<OriginalReactionCache> {
    if !valid_id(expected_account) || bytes.len() as u64 > MAX_BYTES {
        bail!("Invalid original reaction cache bounds");
    }
    // Sanitized errors never include serde's offending string/value.
    let envelope: Envelope = serde_json::from_slice(bytes)
        .map_err(|_| anyhow::anyhow!("Invalid original reaction cache envelope"))?;
    if envelope.schema != 1
        || envelope.source != SOURCE
        || envelope.captured_at_ms == 0
        || envelope.account_id != expected_account
        || envelope.snapshots.len() > MAX_SNAPSHOTS
    {
        bail!("Original reaction cache provenance mismatch");
    }
    let mut snapshots = Vec::with_capacity(envelope.snapshots.len());
    for raw in envelope.snapshots {
        if !valid_id(&raw.thread_id) || !valid_id(thread_key(&raw.thread_id)) {
            bail!("Invalid original reaction thread");
        }
        let client_id = id(&raw.client_id)?;
        let message_id = id(&raw.message_id)?;
        if client_id.is_empty() && message_id.is_empty() {
            bail!("Original reaction snapshot lacks message identity");
        }
        let users = raw
            .reactions
            .as_object()
            .context("Invalid original reaction aggregate")?;
        let mut total = 0_u64;
        for (user, types) in users {
            if !valid_id(user) && user != "0" {
                bail!("Invalid original reaction reactor");
            }
            let types = types
                .as_object()
                .context("Invalid original reaction type map")?;
            for (kind, count) in types {
                let kind = kind
                    .parse::<i32>()
                    .ok()
                    .filter(|kind| *kind >= 0)
                    .context("Invalid original reaction type")?;
                let _ = kind;
                let count = count
                    .as_u64()
                    .filter(|n| *n > 0)
                    .context("Invalid original reaction count")?;
                total = total
                    .checked_add(count)
                    .context("Original reaction count overflow")?;
            }
        }
        let summaries =
            crate::reactions::parse_reactions(&serde_json::json!({"reactions":raw.reactions}));
        snapshots.push(Snapshot {
            thread_id: raw.thread_id,
            client_id,
            message_id,
            summaries,
        });
    }
    Ok(OriginalReactionCache {
        snapshots,
        captured_at_ms: i64::try_from(envelope.captured_at_ms)
            .map_err(|_| anyhow::anyhow!("Invalid cache capture time"))?,
    })
}

/// Missing files are optional. Every present row must validate before any import is possible.
/// This function opens only read handles and never logs cache contents or identifiers.
pub fn load_validated_private_cache(
    path: &Path,
    expected_account: &str,
) -> Result<Option<OriginalReactionCache>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => bail!("Cannot open original reaction cache"),
    };
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| anyhow::anyhow!("Cannot read original reaction cache"))?;
    validate(&bytes, expected_account).map(Some)
}

impl OriginalReactionCache {
    pub fn provenance(&self) -> crate::reactions::ReactionSnapshotProvenance {
        crate::reactions::ReactionSnapshotProvenance {
            source: SOURCE.into(),
            captured_at_ms: self.captured_at_ms,
        }
    }

    /// Import only pristine messages. A local cache cannot rewind live state or clear tombstones.
    pub fn apply_to_threads(&self, threads: &mut [Thread]) -> ImportSummary {
        let mut result = ImportSummary {
            snapshots_loaded: self.snapshots.len(),
            ..Default::default()
        };
        let mut scopes: HashMap<&str, Vec<usize>> = HashMap::new();
        for (index, thread) in threads.iter().enumerate() {
            if valid_id(&thread.id) {
                scopes
                    .entry(if thread.is_group {
                        thread_key(&thread.id)
                    } else {
                        &thread.id
                    })
                    .or_default()
                    .push(index);
            }
        }
        let mut assigned: HashMap<usize, Vec<&Snapshot>> = HashMap::new();
        for snapshot in &self.snapshots {
            if snapshot.summaries.is_empty() {
                result.empty_snapshots_skipped += 1;
                continue;
            }
            let candidates: Vec<_> = scopes
                .get(thread_key(&snapshot.thread_id))
                .into_iter()
                .flatten()
                .copied()
                .filter(|index| {
                    let thread = &threads[*index];
                    if thread.is_group {
                        thread_key(&thread.id) == thread_key(&snapshot.thread_id)
                    } else {
                        thread.id == snapshot.thread_id && !snapshot.thread_id.starts_with('g')
                    }
                })
                .collect();
            match candidates.as_slice() {
                [index] => assigned.entry(*index).or_default().push(snapshot),
                [] => {}
                _ => result.ambiguous_matches += 1,
            }
        }
        // Release immutable thread-ID borrows before updating message metadata.
        drop(scopes);
        for (index, snapshots) in assigned {
            let thread = &mut threads[index];
            let mut global: HashMap<&str, Vec<&Snapshot>> = HashMap::new();
            let mut client: HashMap<&str, Vec<&Snapshot>> = HashMap::new();
            for snapshot in snapshots {
                if !snapshot.message_id.is_empty() {
                    global
                        .entry(&snapshot.message_id)
                        .or_default()
                        .push(snapshot);
                }
                if !snapshot.client_id.is_empty() {
                    client
                        .entry(&snapshot.client_id)
                        .or_default()
                        .push(snapshot);
                }
            }
            let client_ids: Vec<_> = thread.messages.iter().map(message_client_id).collect();
            let mut global_counts = HashMap::new();
            let mut client_counts = HashMap::new();
            for (message, client_id) in thread.messages.iter().zip(&client_ids) {
                if valid_id(&message.id) {
                    *global_counts.entry(message.id.clone()).or_insert(0) += 1;
                }
                if !client_id.is_empty() {
                    *client_counts.entry(client_id.clone()).or_insert(0) += 1;
                }
            }
            // A source row with a local global match must never also attach by client ID.
            client.retain(|_, candidates| {
                candidates.retain(|snapshot| !global_counts.contains_key(&snapshot.message_id));
                !candidates.is_empty()
            });
            for (message, client_id) in thread.messages.iter_mut().zip(client_ids) {
                let exact = global
                    .get(message.id.as_str())
                    .filter(|_| valid_id(&message.id));
                let (matches, unique_message) = if let Some(exact) = exact {
                    (Some(exact), global_counts.get(&message.id) == Some(&1))
                } else {
                    (
                        client.get(client_id.as_str()),
                        client_counts.get(&client_id) == Some(&1),
                    )
                };
                let Some(matches) = matches else { continue };
                if matches.len() != 1 || !unique_message {
                    result.ambiguous_matches += 1;
                    continue;
                }
                result.messages_matched += 1;
                if message.reaction_authoritative_snapshot
                    || message.reaction_snapshot
                    || !message.reaction_versions.is_empty()
                    || !message.reaction_summaries.is_empty()
                    || !message.reactions.is_empty()
                {
                    result.messages_protected += 1;
                    continue;
                }
                message.reaction_snapshot_provenance = self.provenance();
                message.reaction_summaries = matches[0].summaries.clone();
                message.reactions = crate::reactions::legacy_emojis(&message.reaction_summaries);
                message.reaction_snapshot = true;
                message.reaction_authoritative_snapshot = true;
                message.reaction_snapshot_fresh = false;
                result.messages_updated += 1;
            }
        }
        result
    }
}

fn message_client_id(message: &crate::chat::ChatMessage) -> String {
    if valid_id(&message.client_message_id) {
        return message.client_message_id.clone();
    }
    message
        .history_source
        .as_ref()
        .and_then(|source| source.get("clientId"))
        .and_then(|value| id(value).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::ChatMessage;
    use serde_json::json;

    fn row(thread: &str, global: &str, client: u64, count: u64) -> Value {
        json!({"idTo":thread,"rClientMsgId":client,"rMsgId":global,
            "reactions":{"reactor":{"5":count}},"lastUpdate":1700000000000_u64,"ts":"fixture"})
    }
    fn envelope(rows: Vec<Value>) -> Value {
        json!({"schema":1,"account_id":"account-fixture","captured_at_ms":1700000000000_u64,
            "source":SOURCE,"snapshots":rows})
    }
    fn cache(rows: Vec<Value>) -> OriginalReactionCache {
        validate(
            &serde_json::to_vec(&envelope(rows)).unwrap(),
            "account-fixture",
        )
        .unwrap()
    }
    fn thread(id: &str, group: bool, messages: Vec<ChatMessage>) -> Thread {
        Thread {
            presentation: Default::default(),
            id: id.into(),
            name: "Fixture".into(),
            is_group: group,
            unread: Default::default(),
            messages,
            preview: String::new(),
            time: String::new(),
            avatar: None,
        }
    }
    fn message(global: &str, client: &str) -> ChatMessage {
        ChatMessage {
            id: global.into(),
            client_message_id: client.into(),
            text: "Same fixture text".into(),
            ..Default::default()
        }
    }
    fn fixture_path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "zl-original-reaction-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn raw_database_shape_imports_counts_using_unique_client_not_text() {
        let cache = cache(vec![row("peer", "original-global", 123, 2)]);
        let mut threads = vec![thread(
            "peer",
            false,
            vec![message("phone-global", "123"), message("other", "999")],
        )];
        let result = cache.apply_to_threads(&mut threads);
        assert_eq!(result.messages_updated, 1);
        let m = &threads[0].messages[0];
        assert_eq!(m.reaction_summaries[0].count, 2);
        assert_eq!(m.reaction_summaries[0].reactor_counts["reactor"], 2);
        assert!(m.reaction_authoritative_snapshot);
        assert!(!m.reaction_snapshot_fresh);
        assert!(threads[0].messages[1].reaction_summaries.is_empty());
        assert_eq!(cache.apply_to_threads(&mut threads).messages_updated, 0);
    }

    #[test]
    fn original_self_reactor_zero_is_preserved() {
        let mut raw = row("peer", "global", 123, 2);
        raw["reactions"] = json!({"0":{"5":2}});
        let mut threads = vec![thread("peer", false, vec![message("global", "123")])];
        assert_eq!(
            cache(vec![raw])
                .apply_to_threads(&mut threads)
                .messages_updated,
            1
        );
        assert_eq!(
            threads[0].messages[0].reaction_summaries[0].reactor_counts["0"],
            2
        );
    }

    #[test]
    fn exact_global_id_wins_but_never_crosses_thread_scope() {
        let cache = cache(vec![row("peer", "global", 123, 2)]);
        let mut threads = vec![
            thread("other", false, vec![message("global", "123")]),
            thread(
                "peer",
                false,
                vec![message("global", "999"), message("different", "123")],
            ),
        ];
        cache.apply_to_threads(&mut threads);
        assert!(threads[0].messages[0].reaction_summaries.is_empty());
        assert_eq!(threads[1].messages[0].reaction_summaries[0].count, 2);
        assert!(threads[1].messages[1].reaction_summaries.is_empty());
    }

    #[test]
    fn duplicate_client_message_or_source_matches_are_skipped() {
        let cache = cache(vec![row("peer", "original", 123, 2)]);
        let mut threads = vec![thread(
            "peer",
            false,
            vec![message("a", "123"), message("b", "123")],
        )];
        let result = cache.apply_to_threads(&mut threads);
        assert_eq!(result.messages_updated, 0);
        assert_eq!(result.ambiguous_matches, 2);
        let duplicate =
            super::tests::cache(vec![row("peer", "a", 123, 2), row("peer", "b", 123, 3)]);
        let mut threads = vec![thread("peer", false, vec![message("phone", "123")])];
        assert_eq!(duplicate.apply_to_threads(&mut threads).messages_updated, 0);
    }

    #[test]
    fn group_prefix_normalizes_and_ambiguous_thread_scope_is_skipped() {
        for (source, target) in [("g77", "77"), ("77", "g77"), ("g77", "g77")] {
            let cache = cache(vec![row(source, "original", 123, 2)]);
            let mut threads = vec![thread(target, true, vec![message("phone", "123")])];
            assert_eq!(cache.apply_to_threads(&mut threads).messages_updated, 1);
        }
        let cache = cache(vec![row("77", "original", 123, 2)]);
        let mut threads = vec![
            thread("77", false, vec![message("phone", "123")]),
            thread("g77", true, vec![message("phone", "123")]),
        ];
        assert_eq!(cache.apply_to_threads(&mut threads).messages_updated, 0);
    }

    #[test]
    fn live_state_legacy_aggregates_and_empty_tombstones_are_protected() {
        let cache = cache(vec![row("peer", "global", 123, 2)]);
        let live = crate::reactions::ReactionEvent {
            sender_id: "reactor".into(),
            event_id: "event".into(),
            r_type: 5,
            emoji: "❤️".into(),
            ts: 1700000000000,
            ..Default::default()
        };
        let mut variants = vec![message("global", "123"); 5];
        variants[0].apply_reaction(&live);
        variants[1].apply_reaction(&live);
        variants[1].apply_reaction(&crate::reactions::ReactionEvent {
            event_id: "clear".into(),
            r_type: -1,
            ts: 1700000000001,
            ..live
        });
        variants[2].reaction_authoritative_snapshot = true;
        variants[3].reactions = vec!["❤️".into()];
        variants[4].reaction_summaries =
            crate::reactions::parse_reactions(&json!({"reactions":{"reactor":{"3":1}}}));
        for variant in variants {
            let expected = serde_json::to_value(&variant).unwrap();
            let mut threads = vec![thread("peer", false, vec![variant])];
            assert_eq!(cache.apply_to_threads(&mut threads).messages_protected, 1);
            assert_eq!(
                serde_json::to_value(&threads[0].messages[0]).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn empty_original_maps_are_not_imported_as_stale_tombstones() {
        let mut raw = row("peer", "global", 123, 2);
        raw["reactions"] = json!({});
        let mut threads = vec![thread("peer", false, vec![message("global", "123")])];
        let result = cache(vec![raw]).apply_to_threads(&mut threads);
        assert_eq!(result.empty_snapshots_skipped, 1);
        assert_eq!(result.messages_updated, 0);
        assert!(!threads[0].messages[0].reaction_snapshot);
    }

    #[test]
    fn serde_roundtrip_preserves_import_and_prevents_rewind() {
        let cache = cache(vec![row("peer", "global", 123, 2)]);
        let mut threads = vec![thread("peer", false, vec![message("global", "123")])];
        cache.apply_to_threads(&mut threads);
        let serialized = serde_json::to_string(&threads[0].messages[0]).unwrap();
        threads[0].messages[0] = serde_json::from_str(&serialized).unwrap();
        assert_eq!(cache.apply_to_threads(&mut threads).messages_updated, 0);
        assert_eq!(threads[0].messages[0].reaction_summaries[0].count, 2);
        assert_eq!(
            threads[0].messages[0].reaction_snapshot_provenance,
            cache.provenance()
        );
    }

    #[test]
    fn invalid_row_rejects_entire_envelope_and_errors_are_sanitized() {
        let good = row("peer", "global", 123, 2);
        for bad in [
            json!({"reactions":[]}),
            {
                let mut bad = good.clone();
                bad["reactions"] = json!({"reactor":{"5":0}});
                bad
            },
            {
                let mut bad = good.clone();
                bad["reactions"] = json!({"reactor":{"-1":2}});
                bad
            },
            {
                let mut bad = good.clone();
                bad["rMsgId"] = json!(1.5);
                bad
            },
        ] {
            let result = validate(
                &serde_json::to_vec(&envelope(vec![good.clone(), bad])).unwrap(),
                "account-fixture",
            );
            assert!(result.is_err());
            assert!(!format!("{:?}", result.unwrap_err()).contains("reactor"));
        }
        let bytes = serde_json::to_vec(&envelope(vec![good])).unwrap();
        assert!(validate(&bytes, "other-account").is_err());
        assert!(validate(&vec![b' '; MAX_BYTES as usize + 1], "account-fixture").is_err());
        let mut too_many = envelope(vec![]);
        too_many["snapshots"] = json!(vec![row("peer", "global", 123, 2); MAX_SNAPSHOTS + 1]);
        assert!(validate(&serde_json::to_vec(&too_many).unwrap(), "account-fixture").is_err());
    }

    #[test]
    fn optional_read_only_file_load_does_not_modify_input() {
        let path = fixture_path();
        assert!(load_validated_private_cache(&path, "account-fixture")
            .unwrap()
            .is_none());
        let bytes = serde_json::to_vec(&envelope(vec![row("peer", "global", 123, 2)])).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        assert!(load_validated_private_cache(&path, "account-fixture")
            .unwrap()
            .is_some());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn historical_client_id_fallback_and_unknown_reaction_type_are_lossless() {
        let mut raw = row("peer", "original", 123, 2);
        raw["reactions"] = json!({"reactor":{"999":3}});
        let mut m = message("phone", "");
        m.history_source = Some(json!({"clientId":123}));
        let mut threads = vec![thread("peer", false, vec![m])];
        assert_eq!(
            cache(vec![raw])
                .apply_to_threads(&mut threads)
                .messages_updated,
            1
        );
        assert_eq!(
            threads[0].messages[0].reaction_summaries[0].r_type,
            Some(999)
        );
        assert_ne!(threads[0].messages[0].reaction_summaries[0].emoji, "❤️");
    }
}
