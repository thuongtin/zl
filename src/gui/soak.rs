//! Opt-in instrumentation over actual GUI update, selection, and local-page events.
//! No input injection, focus changes, session substitution, or network commands.
use super::ZaloGuiApp;
use eframe::egui;
use serde_json::json;
use std::{
    path::PathBuf,
    process::Command,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

struct PendingPage {
    thread: String,
    started: Instant,
}
struct SoakState {
    output: PathBuf,
    started: Instant,
    seconds: u64,
    next_switch: Instant,
    next_sample: Instant,
    random: u64,
    switches: usize,
    frames_ms: Vec<f64>,
    pages_ms: Vec<f64>,
    rss: Vec<(f64, u64)>,
    pending: Option<PendingPage>,
    superseded_pages: usize,
    max_messages: usize,
    max_threads: usize,
    account: String,
    account_changed: bool,
    focused_frames: usize,
    heavy_threads: Vec<String>,
    heavy_min_messages: u64,
    heavy_max_messages: u64,
    renderer_cpu_ms: Vec<f64>,
    finished: bool,
}
static STATE: OnceLock<Mutex<Option<SoakState>>> = OnceLock::new();
fn state() -> &'static Mutex<Option<SoakState>> {
    STATE.get_or_init(|| {
        Mutex::new(std::env::var_os("DAILY_GUI_SOAK_OUTPUT").map(|output| {
            let now = Instant::now();
            let seconds = std::env::var("DAILY_GUI_SOAK_SECONDS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(3600);
            SoakState {
                output: PathBuf::from(output),
                started: now,
                seconds,
                next_switch: now,
                next_sample: now,
                random: 0x73c1_92b4,
                switches: 0,
                frames_ms: Vec::new(),
                pages_ms: Vec::new(),
                rss: Vec::new(),
                pending: None,
                superseded_pages: 0,
                max_messages: 0,
                max_threads: 0,
                account: String::new(),
                account_changed: false,
                focused_frames: 0,
                heavy_threads: Vec::new(),
                heavy_min_messages: 0,
                heavy_max_messages: 0,
                renderer_cpu_ms: Vec::new(),
                finished: false,
            }
        }))
    })
}
fn percentile(values: &[f64], fraction: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut values = values.to_vec();
    values.sort_by(f64::total_cmp);
    Some(
        values[((values.len() as f64 * fraction).ceil() as usize)
            .saturating_sub(1)
            .min(values.len() - 1)],
    )
}
fn rss_kib() -> Option<u64> {
    let out = Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()?.trim().parse().ok()
}
/// Baseline uses minutes 10..20, after the initial ten-minute warmup.
/// End state uses the final ten minutes; the full-hour gate is reported separately.
fn memory_drift(samples: &[(f64, u64)], elapsed: f64) -> Option<(f64, f64, f64, bool)> {
    if elapsed < 1200.0 {
        return None;
    }
    let baseline: Vec<_> = samples
        .iter()
        .filter(|(t, _)| *t >= 600.0 && *t < 1200.0)
        .map(|(_, v)| *v as f64)
        .collect();
    let ending: Vec<_> = samples
        .iter()
        .filter(|(t, _)| *t >= (elapsed - 600.0) && *t <= elapsed)
        .map(|(_, v)| *v as f64)
        .collect();
    let baseline = percentile(&baseline, 0.5)?;
    let ending = percentile(&ending, 0.5)?;
    let allowed = (baseline * 0.10).max(64.0 * 1024.0);
    Some((
        baseline,
        ending,
        allowed,
        (ending - baseline).max(0.0) <= allowed,
    ))
}
impl SoakState {
    fn write_report(&self) -> std::io::Result<()> {
        let elapsed = self.started.elapsed().as_secs_f64();
        let memory = memory_drift(&self.rss, elapsed);
        let page_p95 = percentile(&self.pages_ms, 0.95);
        let full_hour = elapsed >= 3600.0;
        let report = json!({
            "schema_version":1,"surface":"actual GUI App::update instrumentation; no synthetic frame or conversation fixture",
            "pid":std::process::id(),"requested_seconds":self.seconds,"actual_elapsed_seconds":elapsed,"finished":self.finished,
            "sixty_minute_gate":full_hour,"actual_conversation_switches":self.switches,"minimum_200_switches_gate":self.switches>=200,
            "actual_local_pages_loaded":self.pages_ms.len(),"minimum_200_loaded_pages_gate":self.pages_ms.len()>=200,"local_page_p95_ms":page_p95,"local_page_p95_under_250_ms_gate":page_p95.map(|p|p<=250.0),
            "page_latency_definition":"select_conversation dispatch until matching LocalPage event is applied on GUI thread; includes queue and hydration; superseded switches excluded",
            "superseded_pending_pages":self.superseded_pages,"pending_page_at_checkpoint":self.pending.is_some(),
            "update_cpu_wall_frame_count":self.frames_ms.len(),"update_cpu_wall_p50_ms":percentile(&self.frames_ms,0.5),"update_cpu_wall_p95_ms":percentile(&self.frames_ms,0.95),"update_cpu_wall_max_ms":self.frames_ms.iter().copied().reduce(f64::max),
            "frame_coverage_limit":"Elapsed wall time from begin_frame to end_frame entry inside eframe App::update; excludes end_frame RSS/report IO; not CPU cycles, complete frame pacing, GPU, compositor, vsync or visible pixel verification",
            "heavy_conversation_pool_count":self.heavy_threads.len(),"heavy_pool_min_messages":self.heavy_min_messages,"heavy_pool_max_messages":self.heavy_max_messages,
            "eframe_cpu_frame_p95_ms":percentile(&self.renderer_cpu_ms,0.95),"eframe_cpu_frame_under_32ms_gate":percentile(&self.renderer_cpu_ms,0.95).map(|p|p<=32.0),
            "eframe_cpu_definition":"eframe previous-frame cpu_usage; excludes waiting for vsync/GPU/compositor presentation",
            "max_resident_messages":self.max_messages,"max_threads":self.max_threads,"resident_messages_at_most_2000_gate":self.max_messages<=2000,
            "rss_samples_kib":self.rss,"rss_baseline_median_kib":memory.map(|v|v.0),"rss_final_10min_median_kib":memory.map(|v|v.1),"rss_allowed_increase_kib":memory.map(|v|v.2),"rss_drift_gate":memory.map(|v|v.3),
            "rss_definition":"ps RSS KiB, sampled every ten seconds; baseline minutes 10..20; final ten-minute median; allowed increase max(10 percent baseline, 64 MiB)",
            "account_changed":self.account_changed,"focused_update_frames":self.focused_frames,
            "acceptance_pass":self.finished&&full_hour&&self.switches>=200&&self.pages_ms.len()>=200&&page_p95.is_some_and(|p|p<=250.0)&&memory.is_some_and(|v|v.3)&&self.max_messages<=2000&&!self.account_changed&&percentile(&self.renderer_cpu_ms,0.95).is_some_and(|p|p<=32.0),
            "network_coverage_limit":"Instrumentation sends no network commands. Normal application networking remains active. Background selection must be guarded against sending seen by the caller. External network and focus evidence are separate."
        });
        if let Some(parent) = self.output.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temp = self
            .output
            .with_extension(format!("tmp-{}", std::process::id()));
        std::fs::write(&temp, serde_json::to_vec_pretty(&report)?)?;
        std::fs::rename(&temp, &self.output)
    }
}
/// Call at the start of the real App::update. Returns None when not opted in.
pub(super) fn begin_frame(app: &mut ZaloGuiApp, ctx: &egui::Context) -> Option<Instant> {
    let mut guard = state().lock().unwrap_or_else(|p| p.into_inner());
    let s = guard.as_mut()?;
    if s.finished {
        return None;
    }
    let frame_start = Instant::now();
    if s.account.is_empty() && !app.user_uin.is_empty() {
        s.account = app.user_uin.clone();
        if let Ok(counts) = super::storage::open_account_store(&s.account)
            .and_then(|store| store.thread_message_counts())
        {
            let threshold = if counts.values().any(|n| *n >= 500) {
                500
            } else {
                200
            };
            let pool: Vec<_> = counts
                .into_iter()
                .filter(|(_, n)| *n >= threshold)
                .collect();
            s.heavy_min_messages = pool.iter().map(|(_, n)| *n).min().unwrap_or(0);
            s.heavy_max_messages = pool.iter().map(|(_, n)| *n).max().unwrap_or(0);
            s.heavy_threads = pool.into_iter().map(|(id, _)| id).collect();
        }
    }
    if !s.account.is_empty() && s.account != app.user_uin {
        s.account_changed = true;
    }
    if ctx.input(|i| i.focused) {
        s.focused_frames += 1;
    }
    if frame_start.duration_since(s.started) < Duration::from_secs(s.seconds)
        && frame_start >= s.next_switch
        && app.is_logged_in
        && app.threads.len() > 1
    {
        s.random = s.random.wrapping_mul(6364136223846793005).wrapping_add(1);
        let candidates: Vec<usize> = app
            .threads
            .iter()
            .enumerate()
            .filter(|(index, thread)| {
                *index != app.selected_thread_index
                    && (s.heavy_threads.is_empty() || s.heavy_threads.contains(&thread.id))
            })
            .map(|(index, _)| index)
            .collect();
        let index = if candidates.is_empty() {
            (app.selected_thread_index + 1) % app.threads.len()
        } else {
            candidates[(s.random >> 32) as usize % candidates.len()]
        };
        let thread = app.threads[index].id.clone();
        if s.pending.is_some() {
            s.superseded_pages += 1;
        }
        s.pending = Some(PendingPage {
            thread,
            started: frame_start,
        });
        app.select_conversation(index);
        s.switches += 1;
        s.next_switch = frame_start + Duration::from_secs(10);
    }
    ctx.request_repaint_after(Duration::from_millis(500));
    Some(frame_start)
}
pub(super) fn renderer_cpu_frame(seconds: Option<f32>) {
    let Some(seconds) = seconds else {
        return;
    };
    let Some(lock) = STATE.get() else {
        return;
    };
    let mut guard = lock.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(s) = guard.as_mut() {
        if !s.finished {
            s.renderer_cpu_ms.push(seconds as f64 * 1000.0);
        }
    }
}
/// Call only after the matching account/thread LocalPage event has been applied.
pub(super) fn page_loaded(thread_id: &str) {
    let Some(lock) = STATE.get() else {
        return;
    };
    let mut guard = lock.lock().unwrap_or_else(|p| p.into_inner());
    let Some(s) = guard.as_mut() else {
        return;
    };
    if s.finished {
        return;
    }
    if s.pending.as_ref().is_some_and(|p| p.thread == thread_id) {
        if let Some(pending) = s.pending.take() {
            s.pages_ms
                .push(pending.started.elapsed().as_secs_f64() * 1000.0);
        }
    }
}
/// Call at the end of the real App::update. This does not measure GPU presentation.
pub(super) fn end_frame(app: &ZaloGuiApp, frame_start: Option<Instant>) {
    let Some(frame_start) = frame_start else {
        return;
    };
    let frame_ms = frame_start.elapsed().as_secs_f64() * 1000.0;
    let mut guard = state().lock().unwrap_or_else(|p| p.into_inner());
    let Some(s) = guard.as_mut() else {
        return;
    };
    if s.finished {
        return;
    }
    s.frames_ms.push(frame_ms);
    s.max_messages = s
        .max_messages
        .max(app.threads.iter().map(|t| t.messages.len()).sum());
    s.max_threads = s.max_threads.max(app.threads.len());
    let now = Instant::now();
    let finished = now.duration_since(s.started) >= Duration::from_secs(s.seconds);
    if now >= s.next_sample || finished {
        if let Some(rss) = rss_kib() {
            s.rss.push((s.started.elapsed().as_secs_f64(), rss));
        }
        s.finished = finished;
        s.next_sample = now + Duration::from_secs(10);
        if let Err(error) = s.write_report() {
            eprintln!("GUI soak report write failed: {error}");
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn metrics_use_nearest_rank_and_require_samples() {
        assert_eq!(percentile(&[], 0.95), None);
        assert_eq!(percentile(&[4.0, 1.0, 3.0, 2.0], 0.95), Some(4.0));
    }
    #[test]
    fn memory_gate_excludes_warmup_and_uses_64mib_floor() {
        let pass = [
            (1.0, 999999),
            (600.0, 100000),
            (900.0, 100000),
            (3000.0, 165536),
            (3590.0, 165536),
        ];
        assert_eq!(
            memory_drift(&pass, 3600.0),
            Some((100000.0, 165536.0, 65536.0, true))
        );
        let fail = [
            (600.0, 100000),
            (900.0, 100000),
            (3000.0, 165537),
            (3590.0, 165537),
        ];
        assert!(!memory_drift(&fail, 3600.0).unwrap().3);
        assert_eq!(memory_drift(&pass, 1199.0), None);
    }
    #[test]
    fn memory_gate_uses_ten_percent_for_large_process() {
        let samples = [
            (600.0, 1000000),
            (900.0, 1000000),
            (3000.0, 1100000),
            (3590.0, 1100000),
        ];
        assert_eq!(
            memory_drift(&samples, 3600.0),
            Some((1000000.0, 1100000.0, 100000.0, true))
        );
    }
}
