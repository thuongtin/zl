//! Disposable storage benchmark. Never opens the application's configured data directory.
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    path::Path,
    process::Command,
    time::{Duration, Instant},
};
use zalo_rust::{
    chat::ChatMessage,
    store::{MessageStore, StoredMessage, StoredThread},
    store_v2::{AccountStore, HistoryCursor, PageDirection, StoreDelta},
};

fn sample(i: usize) -> StoredMessage {
    StoredMessage {
        msg_id: format!("{:012}", i + 1),
        thread_id: format!(
            "thread-{}",
            if i.is_multiple_of(2) { 0 } else { 1 + i % 23 }
        ),
        sender_id: "synthetic-sender".into(),
        sender_name: "Synthetic Sender".into(),
        text: format!(
            "Synthetic record {} {}",
            i,
            if i.is_multiple_of(97) {
                "needle"
            } else {
                "ordinary"
            }
        ),
        msg_type: "webchat".into(),
        ts: 1_700_000_000_000 + i as i64,
        is_outgoing: false,
        raw_json: "{}".into(),
    }
}
fn elapsed(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}
fn percentile(values: &[f64], fraction: f64) -> f64 {
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    v[((v.len() as f64 * fraction).ceil() as usize)
        .saturating_sub(1)
        .min(v.len() - 1)]
}
fn summary(v: &[f64]) -> Value {
    json!({"count":v.len(),"p50_ms":percentile(v,0.5),"p95_ms":percentile(v,0.95),"max_ms":v.iter().copied().fold(0.0,f64::max)})
}
fn rss() -> Option<u64> {
    let out = Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()?;
    String::from_utf8(out.stdout).ok()?.trim().parse().ok()
}
fn run(root: &Path, count: usize, switches: usize, seconds: u64) -> Result<Value> {
    let root = root.join(count.to_string());
    std::fs::create_dir_all(&root)?;
    let mut store = AccountStore::open(&root, "synthetic-account")?;
    let threads: Vec<_> = (0..24)
        .map(|i| StoredThread {
            thread_id: format!("thread-{i}"),
            name: format!("Synthetic {i}"),
            is_group: false,
            last_ts: 0,
        })
        .collect();
    store.apply_delta(StoreDelta {
        threads: &threads,
        ..Default::default()
    })?;
    let seed = Instant::now();
    for start in (0..count).step_by(1000) {
        let batch: Vec<_> = (start..(start + 1000).min(count)).map(sample).collect();
        let stats = store.apply_delta(StoreDelta {
            messages: &batch,
            ..Default::default()
        })?;
        ensure!(stats.inserted == batch.len(), "Seed count mismatch");
    }
    let seed_ms = elapsed(seed);
    // Legacy preservation is exercised against a separate disposable source database.
    let legacy = root.join("legacy.sqlite3");
    {
        let old = MessageStore::open(legacy.to_str().context("Temporary path encoding")?)?;
        old.upsert_message(&StoredMessage {
            msg_id: "legacy-only".into(),
            thread_id: "legacy".into(),
            ..sample(0)
        })?;
    }
    let legacy_before = std::fs::read(&legacy)?;
    let migration = store.migrate_legacy(Some(&legacy), None)?;
    ensure!(
        migration.messages == 1 && !migration.already_imported,
        "Initial import mismatch"
    );
    let mut pages = Vec::new();
    let mut searches = Vec::new();
    let mut deltas = Vec::new();
    let mut switch_times = Vec::new();
    let mut random = 0x1234_5678_u64;
    let mut iterations = 0;
    let mut window = VecDeque::<ChatMessage>::new();
    let mut max_window = 0;
    let mut total_loaded = 0;
    let mut transitions = 0;
    let mut last_thread = None;
    let mut rss_samples = vec![rss()];
    let started = Instant::now();
    while iterations < switches || started.elapsed() < Duration::from_secs(seconds) {
        random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
        let selected = ((random >> 32) % 24) as usize;
        let thread = format!("thread-{selected}");
        if last_thread != Some(selected) {
            transitions += 1;
        }
        last_thread = Some(selected);
        let switch = Instant::now();
        window.clear();
        let mut cursor = None;
        // Read older pages past the local window capacity for the dense thread.
        for _ in 0..if selected == 0 { 12 } else { 1 } {
            let t = Instant::now();
            let page = store.history_page(&thread, cursor.as_ref(), PageDirection::Before)?;
            pages.push(elapsed(t));
            ensure!(page.messages.len() <= 200, "Page exceeded 200");
            cursor = page.messages.first().map(|m| HistoryCursor {
                ts: m.ts,
                msg_id: m.msg_id.clone(),
            });
            for message in &page.messages {
                let normalized = store.chat_message(message)?;
                ensure!(
                    normalized.id == message.msg_id
                        && normalized.text == message.text
                        && normalized.ts == message.ts,
                    "Normalized message mismatch"
                );
                window.push_front(normalized);
                if window.len() > 2000 {
                    window.pop_back();
                }
            }
            total_loaded += page.messages.len();
            max_window = max_window.max(window.len());
            ensure!(window.len() <= 2000, "Window exceeded 2000");
            if !page.has_more {
                break;
            }
        }
        switch_times.push(elapsed(switch));
        let t = Instant::now();
        let hits = store.search("needle", None, 0, 200)?;
        searches.push(elapsed(t));
        ensure!(!hits.is_empty() && hits.len() <= 200, "Search mismatch");
        // Exactly one new record is passed to apply_delta, never the full source list.
        let mut message = sample(count + iterations);
        message.text = format!("Incremental delta {}", iterations);
        let t = Instant::now();
        let stats = store.apply_delta(StoreDelta {
            messages: std::slice::from_ref(&message),
            ..Default::default()
        })?;
        deltas.push(elapsed(t));
        ensure!(
            stats.inserted == 1 && stats.updated == 0,
            "Delta was not one insertion"
        );
        iterations += 1;
        if iterations % 100 == 0 {
            rss_samples.push(rss());
        }
    }
    let soak_elapsed = started.elapsed().as_secs_f64();
    let expected = sample(count + iterations - 1);
    drop(store);
    let restart = Instant::now();
    let mut reopened = AccountStore::open(&root, "synthetic-account")?;
    let imported = reopened.migrate_legacy(Some(&legacy), None)?;
    ensure!(
        imported.already_imported,
        "Repeated migration not idempotent"
    );
    ensure!(
        reopened
            .get_message(&expected.thread_id, &expected.msg_id)?
            .is_some(),
        "Last delta lost after restart"
    );
    ensure!(
        reopened.get_message("legacy", "legacy-only")?.is_some(),
        "Legacy row lost after restart"
    );
    let stats = reopened.apply_delta(StoreDelta {
        messages: std::slice::from_ref(&StoredMessage {
            text: format!("Incremental delta {}", iterations - 1),
            ..expected
        }),
        ..Default::default()
    })?;
    ensure!(
        stats.unchanged == 1 && stats.inserted == 0 && stats.updated == 0,
        "Replay changed row"
    );
    ensure!(
        legacy_before == std::fs::read(&legacy)?,
        "Legacy source changed"
    );
    let restart_ms = elapsed(restart);
    rss_samples.push(rss());
    let db = reopened.path.clone().context("Missing database path")?;
    // Independent aggregate readback avoids accepting a last-row-only persistence check.
    let conn =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let rows: usize = conn.query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))?;
    ensure!(
        rows == count + iterations + 1,
        "Persisted row count mismatch"
    );
    Ok(
        json!({"messages_seeded":count,"seed_ms":seed_ms,"iterations":iterations,"thread_transitions":transitions,"soak_seconds_requested":seconds,"soak_seconds_observed":soak_elapsed,"page":summary(&pages),"search":summary(&searches),"delta_one_record":summary(&deltas),"synthetic_switch":summary(&switch_times),"max_window_messages":max_window,"total_loaded":total_loaded,"rss_kib_samples":rss_samples,"restart_ms":restart_ms,"row_count":rows,"restart_persistence":true,"migration_idempotent":true,"legacy_bytes_preserved":true,"replay_unchanged":true}),
    )
}
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    let value = |key: &str, default: &str| {
        args.windows(2)
            .find(|pair| pair[0] == key)
            .map(|pair| pair[1].clone())
            .unwrap_or_else(|| default.into())
    };
    let counts: Vec<usize> = value("--messages", "5000,50000,200000")
        .split(',')
        .map(str::parse)
        .collect::<std::result::Result<_, _>>()?;
    let switches: usize = value("--switches", "240").parse()?;
    let seconds: u64 = value("--soak-seconds", "20").parse()?;
    ensure!(switches >= 200, "At least 200 switches required");
    ensure!(
        counts.iter().all(|n| *n >= 5000),
        "At least 5000 messages per dataset required"
    );
    let output = value("--output", "dist/daily-parity-evidence/bench/metrics.json");
    let root = std::env::temp_dir().join(format!(
        "daily-parity-bench-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    std::fs::create_dir_all(&root)?;
    let result = (|| -> Result<Value> {
        let mut reports = Vec::new();
        for count in counts {
            reports.push(run(&root, count, switches, seconds)?);
        }
        Ok(
            json!({"schema_version":1,"surface":"synthetic AccountStore storage; no GUI, network, live account or device","coverage_limit":"Window and switches are benchmark-local state, not actual GUI; no frame time or 60-minute UI claim","profile":if cfg!(debug_assertions){"debug"}else{"release"},"datasets":reports}),
        )
    })();
    let cleanup = std::fs::remove_dir_all(&root);
    let report = result?;
    cleanup?;
    let encoded = serde_json::to_vec_pretty(&report)?;
    std::fs::write(&output, &encoded)?;
    let bytes = std::fs::read(&output)?;
    ensure!(bytes == encoded, "Evidence byte readback mismatch");
    let reread: Value = serde_json::from_slice(&bytes)?;
    ensure!(
        reread["schema_version"] == 1
            && reread["datasets"].as_array().map(Vec::len)
                == report["datasets"].as_array().map(Vec::len),
        "Evidence schema mismatch"
    );
    println!("{}", serde_json::to_string(&report)?);
    Ok(())
}
