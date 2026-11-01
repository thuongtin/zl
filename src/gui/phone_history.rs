//! Phone-backed transfer-sync-v2. The shared socket remains owned by worker.rs.
//! Private keys and session records live only in the native codec subprocess.
use super::{GuiEvent, Session, Thread};
use crate::socket::{WsCommand, WsEvent};
use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout},
    sync::mpsc::{UnboundedReceiver, UnboundedSender},
};

const MAX_BLOB: usize = 64 * 1024 * 1024;
// Bound control bookkeeping and final-index scans independently of peer input.
const MAX_ROUND_BATCHES: usize = 4096;
const WAIT: Duration = Duration::from_secs(180);

struct Codec {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}
impl Codec {
    async fn start() -> Result<Self> {
        let exe = std::env::current_exe()?;
        let contents = exe
            .parent()
            .and_then(|p| p.parent())
            .context("Thiếu thư mục app")?;
        let (helper, assets) = if contents.join("Helpers/HistoryCodec").is_file() {
            (
                contents.join("Helpers/HistoryCodec"),
                contents.join("Resources/HistoryCodecAssets"),
            )
        } else {
            let build = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("native/macos/build");
            (build.join("HistoryCodec"), build.join("HistoryCodecAssets"))
        };
        let assets = codec_assets_override(
            std::env::var_os("ZL_HISTORY_CODEC_ASSETS").map(PathBuf::from),
            assets,
        )?;
        let mut child = tokio::process::Command::new(helper)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .context("Chưa có bộ giải mã lịch sử trong app")?;
        let input = child.stdin.take().context("Thiếu đầu vào bộ giải mã")?;
        let output = BufReader::new(child.stdout.take().context("Thiếu đầu ra bộ giải mã")?);
        let mut codec = Self {
            child,
            input,
            output,
        };
        codec.call(json!({"op":"init","assetsDir":assets})).await?;
        Ok(codec)
    }
    async fn call(&mut self, value: Value) -> Result<Value> {
        let mut data = serde_json::to_vec(&value)?;
        data.push(b'\n');
        self.input.write_all(&data).await?;
        self.input.flush().await?;
        let mut line = Vec::new();
        let n = tokio::time::timeout(
            Duration::from_secs(45),
            self.output.read_until(b'\n', &mut line),
        )
        .await
        .context("Bộ giải mã chưa phản hồi")??;
        if n == 0 || n > MAX_BLOB * 2 {
            bail!("Bộ giải mã trả dữ liệu sai kích thước");
        }
        let response: Value =
            serde_json::from_slice(&line).context("Bộ giải mã trả phản hồi không hợp lệ")?;
        if response.get("ok").and_then(Value::as_bool) != Some(true) {
            if response.get("code").and_then(Value::as_str) == Some("history_codec_assets_missing")
            {
                bail!("Thiếu tài nguyên đồng bộ lịch sử. Trong mã nguồn, chạy python3 native/macos/prepare-history-assets.py --local-use --output /path/to/HistoryCodecAssets; đặt ZL_HISTORY_CODEC_ASSETS đến thư mục tuyệt đối đó rồi mở lại ứng dụng.");
            }
            let phase = response
                .get("error")
                .and_then(Value::as_str)
                .and_then(|e| e.strip_prefix("History codec operation failed at "));
            let phase = phase
                .filter(|p| {
                    [
                        "initialization",
                        "request",
                        "framing",
                        "decryptMetadata",
                        "decryptMessage",
                        "decompression",
                        "protobuf",
                    ]
                    .contains(p)
                })
                .unwrap_or("request");
            bail!("Giải mã lịch sử thất bại tại {phase}");
        }
        response
            .get("result")
            .cloned()
            .context("Bộ giải mã thiếu kết quả")
    }
}
impl Drop for Codec {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

struct ActiveSessions {
    commands: UnboundedSender<WsCommand>,
    ids: HashSet<String>,
    next_index: u32,
}
impl ActiveSessions {
    fn dispose(&mut self, id: &str) {
        if self.ids.remove(id) {
            let _ = self
                .commands
                .send(WsCommand::DisposeSyncV2 { sync_id: id.into() });
        }
    }
}
impl Drop for ActiveSessions {
    fn drop(&mut self) {
        for id in self.ids.drain() {
            let _ = self.commands.send(WsCommand::DisposeSyncV2 { sync_id: id });
        }
    }
}

fn status(
    tx: &super::worker::ScopedSender,
    ctx: &eframe::egui::Context,
    text: impl Into<String>,
    active: bool,
    generation: u64,
    account: &str,
) {
    let _ = tx.send(GuiEvent::PhoneHistoryStatus {
        generation,
        account: account.into(),
        message: text.into(),
        active,
    });
    ctx.request_repaint();
}

fn text_id(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.is_empty() && s != "0" => Some(s.clone()),
        Value::Number(n) if n.as_u64().is_some_and(|x| x > 0) => Some(n.to_string()),
        _ => None,
    }
}
fn numeric_uid(v: &str) -> bool {
    !v.is_empty() && v != "0" && v.bytes().all(|x| x.is_ascii_digit())
}
fn code(v: Option<&Value>) -> Option<i64> {
    v.and_then(|v| {
        v.as_i64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
    })
}

#[derive(Default)]
struct Round {
    batches: Vec<Value>,
    received_kinds: HashMap<usize, i64>,
    final_index: Option<usize>,
}
impl Round {
    fn done(&self, _wanted: &HashSet<String>) -> bool {
        // Match the owning Web worker's idx+1 final count and unique batch IDs.
        // Partition names alone do not prove that every archive batch arrived.
        let Some(last) = self.final_index else {
            return false;
        };
        self.received_kinds.values().any(|kind| *kind == 2)
            && self.received_kinds.len() == last + 1
            && (0..=last).all(|idx| self.received_kinds.contains_key(&idx))
    }
    fn accept(&mut self, act: &str, data: Value) -> Result<Option<&'static str>> {
        if code(data.get("err")).is_some_and(|n| n != 0) || act.contains("error") {
            bail!("Điện thoại báo lỗi đồng bộ");
        }
        if act == "transfer_status" {
            return match code(data.get("status")) {
                Some(3) => Ok(Some(
                    "Mở Zalo trên điện thoại và chọn Đồng bộ ngay để gửi lịch sử về Mac",
                )),
                Some(4 | 5) => Ok(Some("Điện thoại đã xác nhận, đang chuẩn bị lịch sử")),
                Some(6) => anyhow::bail!("Điện thoại đã từ chối yêu cầu đồng bộ"),
                Some(7) => anyhow::bail!("Yêu cầu đồng bộ đã bị hủy"),
                Some(8) => anyhow::bail!("Điện thoại đang bận đồng bộ, hãy thử lại sau"),
                Some(9) => anyhow::bail!("Điện thoại thiếu dung lượng để đồng bộ"),
                Some(10) => anyhow::bail!("Điện thoại đã đăng xuất"),
                _ => Ok(None),
            };
        }
        if act == "upload_batch" {
            if !data.get("msgUrl").is_some_and(Value::is_string) {
                bail!("Gói lịch sử thiếu địa chỉ tải");
            }
            if !matches!(code(data.get("batchType")), Some(1 | 2)) {
                bail!("Loại gói lịch sử chưa được hỗ trợ");
            }
            let index = code(data.get("idx"))
                .filter(|idx| *idx >= 0 && *idx < MAX_ROUND_BATCHES as i64)
                .context(
                    "Chỉ số gói lịch sử thiếu, sai định dạng hoặc vượt giới hạn 4096 gói mỗi phiên",
                )? as usize;
            let kind = code(data.get("batchType")).context("Loại gói lịch sử sai định dạng")?;
            let duplicate = if let Some(previous_kind) = self.received_kinds.get(&index) {
                anyhow::ensure!(
                    *previous_kind == kind,
                    "Gói lịch sử trùng chỉ số nhưng khác loại"
                );
                true
            } else {
                false
            };
            let is_last = data.get("isLast").and_then(Value::as_bool).unwrap_or(false)
                || code(data.get("isLast")) == Some(1);
            if let Some(last) = self.final_index {
                anyhow::ensure!(index <= last, "Gói lịch sử nằm sau chỉ số kết thúc");
                anyhow::ensure!(
                    !is_last || index == last,
                    "Gói lịch sử có chỉ số kết thúc mâu thuẫn"
                );
            }
            if is_last {
                anyhow::ensure!(
                    self.received_kinds.keys().all(|seen| *seen <= index),
                    "Chỉ số kết thúc lịch sử nằm trước gói đã nhận"
                );
                self.final_index = Some(index);
            }
            // Retries may add the final marker, but must not append a second body.
            if duplicate {
                return Ok(None);
            }
            self.received_kinds.insert(index, kind);
            self.batches.push(data);
        }
        Ok(None)
    }
}

fn controls(event: &WsEvent) -> Vec<(String, String, Value)> {
    let data = event.payload.get("data").unwrap_or(&event.payload);
    data.get("controls")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|control| {
            let content = control.get("content")?;
            if content.get("act_type")?.as_str()? != "transfer_sync2" {
                return None;
            }
            let data = content.get("data")?;
            let data = if let Some(s) = data.as_str() {
                serde_json::from_str(s).ok()?
            } else {
                data.clone()
            };
            Some((
                data.get("syncId")?.as_str()?.into(),
                content.get("act")?.as_str()?.into(),
                data,
            ))
        })
        .collect()
}

fn diagnostic(stage: &str, command: u16, state: Option<i64>, from_device: Option<i64>) {
    // Only allowlisted numeric metadata is retained, never payloads, IDs, keys or URLs.
    #[cfg(not(test))]
    let path = PathBuf::from("sync_diagnostics.json");
    #[cfg(test)]
    let path = std::env::temp_dir().join(format!(
        "zl-phone-fixture-diagnostics-{}.json",
        std::process::id()
    ));
    let mut entries = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice::<Vec<Value>>(&b).ok())
        .unwrap_or_default();
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    entries.push(json!({"time_ms":timestamp,"stage":stage,"command":command,"state":state,"from_device":from_device}));
    if entries.len() > 40 {
        entries.drain(..entries.len() - 40);
    }
    if let Ok(bytes) = serde_json::to_vec_pretty(&entries) {
        let _ = crate::data_dir::write_private_atomic(&path, &bytes);
    }
}

fn response_code(payload: &Value) -> Option<i64> {
    let mut node = payload;
    let mut success = None;
    for _ in 0..4 {
        if let Some(value) = code(node.get("error_code")) {
            if value != 0 {
                return Some(value);
            }
            success = Some(0);
        }
        if let Some(data) = node.get("data").filter(|v| v.is_object()) {
            node = data;
        } else {
            break;
        }
    }
    success
}

fn ack_matches(payload: &Value, command: u16, id: &str) -> bool {
    let expected = if command == 592 {
        format!("req_wake_{id}")
    } else {
        format!("req_{id}")
    };
    let request_id = payload
        .get("reqId")
        .or_else(|| payload.get("data").and_then(|d| d.get("reqId")))
        .and_then(Value::as_str);
    request_id == Some(expected.as_str())
}

// All channels and account-generation fences belong to one protocol round.

#[allow(clippy::too_many_arguments)]
async fn wait_round(
    events: &mut UnboundedReceiver<WsEvent>,
    wakeups: &mut UnboundedReceiver<()>,
    commands: &UnboundedSender<WsCommand>,
    id: &str,
    wanted: HashSet<String>,
    tx: &super::worker::ScopedSender,
    ctx: &eframe::egui::Context,
    generation: u64,
    account: &str,
) -> Result<Round> {
    let mut deadline = tokio::time::Instant::now() + WAIT;
    let mut last_wakeup: Option<tokio::time::Instant> = None;
    let mut round = Round::default();
    while !round.done(&wanted) {
        let event = tokio::select! {
            _ = tokio::time::sleep_until(deadline) => {
                diagnostic("timeout",590,None,None);
                bail!("Chưa nhận đủ lịch sử từ điện thoại; hãy mở điện thoại và thử lại");
            }
            wake = wakeups.recv() => {
                if wake.is_some() && last_wakeup.is_none_or(|last| last.elapsed() >= Duration::from_secs(15)) {
                    commands.send(WsCommand::WakeupSyncV2 {data:json!({"syncId":id,"toDevice":0}),request_id:format!("req_wake_{id}")}).context("Kết nối đồng bộ đã đóng")?;
                    diagnostic("wakeup_requested",592,None,None);
                    status(tx,ctx,"Đã gửi lệnh thông báo; giữ Zalo mở trên điện thoại và chờ phản hồi",true,generation,account);
                    last_wakeup = Some(tokio::time::Instant::now());
                    deadline = tokio::time::Instant::now() + WAIT;
                } else if wake.is_none() { bail!("Tác vụ đồng bộ đã đóng"); }
                continue;
            }
            event = events.recv() => event.context("Kết nối đồng bộ đã đóng")?,
        };
        if event.cmd == crate::socket::CMD_TRANSPORT_STATUS {
            bail!("Kết nối bị ngắt; lịch sử đã nhận được giữ lại, hãy thử đồng bộ lại");
        }
        if event.cmd == 590 || event.cmd == 592 {
            let result = response_code(&event.payload);
            if !ack_matches(&event.payload, event.cmd, id) {
                diagnostic("unmatched_ack", event.cmd, result, None);
                continue;
            }
            diagnostic("request_ack", event.cmd, result, None);
            if result.is_some_and(|c| c != 0) {
                bail!(
                    "Máy chủ từ chối lệnh đồng bộ {} (mã {})",
                    event.cmd,
                    result.unwrap()
                );
            }
            if event.cmd == 592 && result == Some(0) {
                status(
                    tx,
                    ctx,
                    "Máy chủ đã nhận lệnh thông báo; kiểm tra yêu cầu đồng bộ trên Zalo điện thoại",
                    true,
                    generation,
                    account,
                );
            }
        }
        for (sync_id, act, data) in controls(&event) {
            if sync_id == id {
                let stage = match act.as_str() {
                    "transfer_status" => "transfer_status",
                    "upload_batch" => "upload_batch",
                    _ => "transfer_control",
                };
                diagnostic(
                    stage,
                    event.cmd,
                    code(data.get("status")).or_else(|| code(data.get("err"))),
                    code(data.get("fromDevice")),
                );
                if act == "upload_batch" {
                    let stage = if code(data.get("batchType")) == Some(2) {
                        "upload_metadata"
                    } else {
                        "upload_message"
                    };
                    diagnostic(
                        stage,
                        event.cmd,
                        code(data.get("idx")),
                        code(data.get("fromDevice")),
                    );
                }
                if let Some(message) = round.accept(&act, data)? {
                    status(tx, ctx, message, true, generation, account);
                }
            }
        }
    }
    Ok(round)
}

fn generate_sync_id() -> Result<String> {
    use std::io::Read;
    let mut random = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut random)?;
    Ok(random.iter().map(|b| format!("{b:02x}")).collect())
}

async fn request_round(
    codec: &mut Codec,
    active: &mut ActiveSessions,
    kind: &str,
    queries: Vec<Value>,
) -> Result<String> {
    let id = generate_sync_id()?;
    let keys = codec.call(json!({"op":"newSession","id":id})).await?;
    let ek = keys
        .get("ek")
        .and_then(Value::as_str)
        .context("Thiếu khóa phiên lịch sử")?
        .into();
    let ik = keys
        .get("ik")
        .and_then(Value::as_str)
        .context("Thiếu khóa định danh lịch sử")?
        .into();
    let session_index = active.next_index;
    active.next_index = active
        .next_index
        .checked_add(1)
        .context("Quá nhiều phiên đồng bộ")?;
    active.ids.insert(id.clone());
    diagnostic("sync_requested", 590, None, None);
    active.commands.send(WsCommand::RequestSyncV2 { session_index, sync_id:id.clone(), ek, ik, device_name:"ctdozlite for Mac".into(), request:json!({"type":kind,"priority":if kind == "conversation" {0} else {2},"batchSize":2000,"queries":queries}), request_id:format!("req_{id}") }).context("Chưa kết nối tới máy chủ")?;
    Ok(id)
}

fn allowed_download(url: &reqwest::Url) -> bool {
    url.scheme() == "https"
        && url.host_str().is_some_and(|h| {
            ["zdn.vn", "zalo.me", "zaloapp.com"]
                .iter()
                .any(|suffix| h == *suffix || h.ends_with(&format!(".{suffix}")))
        })
}
async fn decode_round(
    codec: &mut Codec,
    id: &str,
    kind: &str,
    mut round: Round,
) -> Result<Vec<Value>> {
    round.batches.sort_by_key(|b| {
        (
            if code(b.get("batchType")) == Some(2) {
                0
            } else {
                1
            },
            code(b.get("idx")).unwrap_or(0),
        )
    });
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 3 || !allowed_download(attempt.url()) {
                attempt.stop()
            } else {
                attempt.follow()
            }
        }))
        .build()?;
    let mut objects = Vec::new();
    for batch in round.batches {
        let url = reqwest::Url::parse(
            batch
                .get("msgUrl")
                .and_then(Value::as_str)
                .context("Thiếu gói lịch sử")?,
        )
        .map_err(|_| anyhow::anyhow!("Địa chỉ lịch sử sai định dạng"))?;
        if !allowed_download(&url) {
            bail!("Địa chỉ tải lịch sử không thuộc dịch vụ Zalo");
        }
        let mut response = client
            .get(url)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("Tải gói lịch sử thất bại"))?
            .error_for_status()
            .map_err(|_| anyhow::anyhow!("Máy chủ từ chối gói lịch sử"))?;
        if response
            .content_length()
            .is_some_and(|n| n > MAX_BLOB as u64)
        {
            bail!("Gói lịch sử vượt giới hạn 64 MiB");
        }
        let mut blob = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| anyhow::anyhow!("Tải gói lịch sử bị gián đoạn"))?
        {
            if blob.len() + chunk.len() > MAX_BLOB {
                bail!("Gói lịch sử vượt giới hạn 64 MiB");
            }
            blob.extend_from_slice(&chunk);
        }
        let result = codec.call(json!({"op":"decode","id":id,"kind":kind,"batchType":batch["batchType"],"blob":STANDARD.encode(blob)})).await?;
        let decoded = result
            .get("objects")
            .and_then(Value::as_array)
            .context("Bộ giải mã thiếu danh sách lịch sử")?;
        objects.extend(decoded.iter().cloned());
    }
    Ok(objects)
}

async fn resolve_ids(
    s: &Session,
    ids: impl IntoIterator<Item = String>,
    mapping: &mut HashMap<String, String>,
) -> Result<()> {
    let missing: HashSet<String> = ids
        .into_iter()
        .filter(|id| !numeric_uid(id) && !id.is_empty() && !mapping.contains_key(id))
        .collect();
    let missing: Vec<_> = missing.into_iter().collect();
    for chunk in missing.chunks(50) {
        let params = s.encoded_params(&json!({"globalUids":serde_json::to_string(chunk)?}))?;
        let url = format!(
            "{}/api/gid/decrypt?{}&params={params}",
            s.domains.profile.trim_end_matches('/'),
            s.common_params()
        );
        let raw = s
            .get(&url)
            .await
            .map_err(|_| anyhow::anyhow!("Tra cứu định danh hội thoại thất bại"))?;
        let decoded = s.maybe_decrypt(raw, s.session_key_bytes()).await;
        if code(decoded.get("error_code")).is_some_and(|c| c != 0) {
            bail!("Máy chủ từ chối tra cứu định danh hội thoại");
        }
        let data = decoded.get("data").unwrap_or(&decoded);
        let data = data.get("data").unwrap_or(data);
        let data = data
            .as_object()
            .context("Phản hồi định danh hội thoại sai định dạng")?;
        for (gid, uid) in data {
            if let Some(uid) = text_id(uid).filter(|v| numeric_uid(v)) {
                mapping.insert(gid.clone(), uid);
            }
        }
    }
    Ok(())
}
fn mapped_id(id: &str, mapping: &HashMap<String, String>) -> Option<String> {
    if numeric_uid(id) {
        Some(id.into())
    } else {
        mapping.get(id).cloned()
    }
}

fn positive_mention_uid(id: &str) -> bool {
    numeric_uid(id) && id.bytes().any(|byte| byte != b'0')
}

fn archive_mention_ids(message: &crate::chat::ChatMessage) -> Vec<String> {
    message
        .history_source
        .as_ref()
        .and_then(|source| source.pointer("/meta/mentionsList"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|mention| mention.get("uid").and_then(text_id))
        .filter(|id| id != "-1" && !id.trim().is_empty() && !numeric_uid(id))
        .collect()
}

fn apply_archive_mention_mapping(
    message: &mut crate::chat::ChatMessage,
    mapping: &HashMap<String, String>,
) {
    for opaque in archive_mention_ids(message) {
        if let Some(uid) = mapping.get(&opaque).filter(|uid| positive_mention_uid(uid)) {
            message.resolved_mention_uids.insert(opaque, uid.clone());
        }
    }
}

/// Decode exact archive mention identities separately from immutable wire payloads.
/// The caller owns account/thread/request freshness checks and persistence.
pub(super) async fn enrich_archive_mentions(
    s: &Session,
    messages: &mut [crate::chat::ChatMessage],
) -> Result<()> {
    let unresolved: HashSet<_> = messages
        .iter()
        .flat_map(|message| {
            archive_mention_ids(message)
                .into_iter()
                .filter(move |opaque| {
                    !message
                        .resolved_mention_uids
                        .get(opaque)
                        .is_some_and(|uid| positive_mention_uid(uid))
                })
        })
        .collect();
    let mut mapping = HashMap::new();
    resolve_ids(s, unresolved, &mut mapping).await?;
    for message in messages {
        apply_archive_mention_mapping(message, &mapping);
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct CachedSidebarSender {
    message_id: String,
    timestamp_ms: i64,
    archive_sender: String,
}

fn cached_sidebar_sender(
    thread: &Thread,
    message: &crate::chat::ChatMessage,
) -> Option<CachedSidebarSender> {
    let timestamp_ms = crate::clock::normalize_ts_ms(message.ts);
    if !thread.is_group
        || message.id.is_empty()
        || thread.presentation.last_message_id != message.id
        || timestamp_ms <= 0
        || crate::clock::normalize_ts_ms(thread.presentation.last_message_ts) != timestamp_ms
    {
        return None;
    }
    // Only the archive adapter owns this identity. Display sender text and
    // cleared archive wire_ref fields never establish a profile UID.
    let archive_sender = message
        .history_source
        .as_ref()
        .and_then(|source| source.get("senderId"))
        .and_then(text_id)?;
    Some(CachedSidebarSender {
        message_id: message.id.clone(),
        timestamp_ms,
        archive_sender,
    })
}

fn apply_cached_sidebar_sender(
    thread: &mut Thread,
    sender: &CachedSidebarSender,
    mapping: &HashMap<String, String>,
) {
    if !thread.is_group
        || thread.presentation.last_message_id != sender.message_id
        || crate::clock::normalize_ts_ms(thread.presentation.last_message_ts) != sender.timestamp_ms
    {
        return;
    }
    if let Some(uid) = mapped_id(&sender.archive_sender, mapping).filter(|uid| numeric_uid(uid)) {
        thread.presentation.last_sender_id = uid;
    }
}

/// Resolve only the indexed latest archive row for each matching group sidebar.
/// This changes presentation identity only; profile names are enriched separately.
pub(super) async fn enrich_cached_sidebar_senders(
    s: &Session,
    threads: &mut [Thread],
) -> Result<()> {
    let account = s
        .login
        .as_ref()
        .context("Thiếu tài khoản để đọc người gửi lịch sử")?
        .uin
        .clone();
    let candidates = {
        let store = super::storage::open_account_store(&account)?;
        let mut candidates = Vec::new();
        for (index, thread) in threads
            .iter()
            .enumerate()
            .filter(|(_, thread)| thread.is_group)
        {
            if let Some(message) = store.latest_message(&thread.id)? {
                if let Some(sender) = cached_sidebar_sender(thread, &message) {
                    candidates.push((index, sender));
                }
            }
        }
        candidates
    };
    let mut mapping = HashMap::new();
    resolve_ids(
        s,
        candidates
            .iter()
            .map(|(_, sender)| sender.archive_sender.clone()),
        &mut mapping,
    )
    .await?;
    for (index, sender) in candidates {
        apply_cached_sidebar_sender(&mut threads[index], &sender, &mapping);
    }
    Ok(())
}

#[derive(Clone, Default)]
struct GroupArchiveIdentity {
    id: String,
    name: String,
    avatar: Option<String>,
}

fn collect_group_identities(data: &Value, groups: &mut HashMap<String, GroupArchiveIdentity>) {
    let data = data.get("data").unwrap_or(data);
    for info in data
        .get("gridInfoMap")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|map| map.values())
    {
        let Some(id) = text_id(&info["groupId"]).filter(|id| numeric_uid(id)) else {
            continue;
        };
        let identity = GroupArchiveIdentity {
            id: id.clone(),
            name: info["name"].as_str().unwrap_or_default().into(),
            avatar: ["avt", "fullAvt", "avatar"].iter().find_map(|key| {
                info[*key]
                    .as_str()
                    .filter(|v| !v.is_empty())
                    .map(str::to_owned)
            }),
        };
        groups.insert(id, identity.clone());
        if let Some(global) = text_id(&info["globalId"]) {
            groups.insert(global, identity);
        }
    }
}

async fn group_archive_identities(s: &Session) -> Result<HashMap<String, GroupArchiveIdentity>> {
    let listed = crate::chat::get_groups(s).await?;
    let mut groups = HashMap::new();
    for thread in &listed {
        groups.insert(
            thread.id.clone(),
            GroupArchiveIdentity {
                id: thread.id.clone(),
                name: thread.name.clone(),
                avatar: thread.avatar.clone(),
            },
        );
    }
    for chunk in listed.chunks(50) {
        let versions: serde_json::Map<String, Value> =
            chunk.iter().map(|t| (t.id.clone(), json!(0))).collect();
        let params = s.encoded_params(&json!({"gridVerMap":serde_json::to_string(&versions)?}))?;
        let url = format!(
            "{}/api/group/getmg-v2?{}",
            s.domains.group.trim_end_matches('/'),
            s.common_params()
        );
        let raw = s
            .post_form(&url, format!("params={params}"))
            .await
            .map_err(|_| anyhow::anyhow!("Tải định danh nhóm thất bại"))?;
        let data = s.maybe_decrypt(raw, s.session_key_bytes()).await;
        if response_code(&data).is_some_and(|code| code != 0) {
            bail!("Máy chủ từ chối tải định danh nhóm");
        }
        collect_group_identities(&data, &mut groups);
    }
    Ok(groups)
}

fn conversation_queries(conversations: &[Value], now: u64) -> Vec<Value> {
    conversations
        .iter()
        .filter_map(|conversation| {
            let id = text_id(&conversation["convId"])?;
            let prefix = if code(conversation.get("convType")) == Some(2) {
                "group"
            } else {
                "oneone"
            };
            let last = code(conversation.get("lastTs"))
                .filter(|n| *n > 0)
                .map(|n| (n as u64).min(now))
                .unwrap_or(now);
            Some(
                json!({"partition":format!("{prefix}/{id}"),"from":0,"to":last,"limit":2147483647}),
            )
        })
        .collect()
}

// Sync-v2 numbers are not the live listener's numeric message type enum.
fn message_content(msg: &Value) -> (&'static str, Value) {
    let kind = code(msg.get("msgType")).unwrap_or(0);
    let caption = msg.get("content").and_then(Value::as_str).unwrap_or("");
    use crate::message_model::MessageKind as K;
    let tag = K::archive_wire_name(kind).unwrap_or("chat.unknown");
    let label = match K::from_archive_code(kind) {
        K::Text => "[Tin nhắn]",
        K::Photo => "[Hình ảnh]",
        K::Voice => "[Tin nhắn thoại]",
        K::Sticker => "[Nhãn dán]",
        K::Contact => "[Danh thiếp]",
        K::Action => "[Nội dung tương tác]",
        K::Location => "[Vị trí]",
        K::Video => "[Video]",
        K::File => "[Tập tin]",
        K::Gif => "[GIF]",
        K::WebContent => "[Nội dung web]",
        K::Delete => "[Thông báo xóa tin nhắn]",
        _ => "[Tin nhắn đặc biệt]",
    };
    let attachments = msg.pointer("/meta/attachsList").and_then(Value::as_array);
    if kind == 15 {
        return (
            tag,
            attachments
                .cloned()
                .map(Value::Array)
                .unwrap_or_else(|| json!([])),
        );
    }
    if matches!(kind, 20 | 21 | 26 | 29 | 32 | 35 | 36) || K::archive_code_name(kind).is_none() {
        // The source has no ji decoder. Preserve opaque metadata, not guessed media/text.
        return (
            tag,
            json!({"description":label,"archive_code":kind,"archive_name":K::archive_code_name(kind),"attachments":attachments}),
        );
    }
    if kind == 0 && !caption.is_empty() {
        return (tag, json!(caption));
    }
    let first = msg
        .get("meta")
        .and_then(|m| m.get("attachsList"))
        .and_then(Value::as_array)
        .and_then(|a| a.first());
    if kind == 24
        && !first
            .and_then(|a| a.get("params"))
            .and_then(Value::as_str)
            .is_some_and(|params| params.contains("pcItem"))
    {
        return (
            tag,
            json!({"description":"[Nội dung web chưa có bộ giải mã]","archive_attachments":attachments}),
        );
    }
    let mut content = first
        .filter(|a| a.is_object())
        .cloned()
        .unwrap_or_else(|| json!({}));
    let displayed = if !caption.is_empty() {
        caption
    } else if kind == 0 {
        first
            .and_then(|a| a.get("title"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or(label)
    } else {
        label
    };
    if kind == 53 {
        content["zinstant_msg"] = first
            .and_then(|a| a.get("zinstantMsg"))
            .cloned()
            .unwrap_or(json!(""));
        content["zinstant_data"] = first
            .and_then(|a| a.get("zinstantData"))
            .cloned()
            .unwrap_or(json!(""));
    }
    if content
        .get("description")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
        || !caption.is_empty()
    {
        content["description"] = json!(displayed);
    }
    content["archive_attachments"] = attachments
        .cloned()
        .map(Value::Array)
        .unwrap_or_else(|| json!([]));
    (tag, content)
}

struct ArchiveConversion {
    threads: Vec<Thread>,
    unresolved_groups: HashSet<String>,
}

fn convert_objects(
    objects: &[Value],
    mapping: &HashMap<String, String>,
    groups: &HashMap<String, GroupArchiveIdentity>,
    own: &str,
) -> Result<ArchiveConversion> {
    let mut messages = Vec::new();
    let mut group_messages = Vec::new();
    let mut unresolved_groups = HashSet::new();
    for object in objects {
        let parts = object
            .get("partitionsList")
            .and_then(Value::as_array)
            .context("Gói lịch sử thiếu phân vùng")?;
        for part in parts {
            let partition = part
                .get("id")
                .and_then(Value::as_str)
                .context("Thiếu định danh phân vùng")?;
            let (gid, is_group) = if let Some(id) = partition.strip_prefix("group/") {
                (id, true)
            } else if let Some(id) = partition.strip_prefix("oneone/") {
                (id, false)
            } else {
                continue;
            };
            let peer = if is_group {
                let Some(identity) = groups.get(gid) else {
                    unresolved_groups.insert(gid.to_owned());
                    continue;
                };
                identity.id.clone()
            } else {
                mapped_id(gid, mapping)
                    .context("Chưa phân giải được định danh hội thoại; hãy thử lại")?
            };
            for msg in part
                .get("messagesList")
                .and_then(Value::as_array)
                .context("Phân vùng thiếu tin nhắn")?
            {
                let id = text_id(&msg["globalId"])
                    .or_else(|| text_id(&msg["clientId"]))
                    .context("Tin lịch sử thiếu định danh")?;
                let sender = text_id(&msg["senderId"]).context("Tin lịch sử thiếu người gửi")?;
                let sender = mapped_id(&sender, mapping)
                    .or_else(|| is_group.then_some(sender))
                    .context("Chưa phân giải được người gửi; hãy thử lại")?;
                if !is_group && sender != own && sender != peer {
                    bail!("Người gửi không khớp hội thoại 1-1");
                }
                let (msg_type, content) = message_content(msg);
                let mut item = json!({"msgId":id,"cliMsgId":msg["clientId"],"uidFrom":if sender == own {"0"} else {&sender},"idTo":if is_group || sender == own {&peer} else {"0"},"content":content,"msgType":msg_type,"ts":msg["timestamp"],"historySource":msg});
                if let Some(quote) = msg
                    .get("meta")
                    .and_then(|m| m.get("quote"))
                    .filter(|q| q.is_object())
                {
                    let mut quote = quote.clone();
                    if text_id(&quote["cliMsgId"]).is_none() {
                        if let Some(id) = text_id(&quote["globalMsgId"]) {
                            quote["cliMsgId"] = json!(id);
                        }
                    }
                    item["quote"] = quote;
                }
                if is_group {
                    item["isGroup"] = json!(true);
                    group_messages.push(item);
                } else {
                    messages.push(item);
                }
            }
        }
    }
    let mut threads = Vec::new();
    crate::chat::merge_previews(
        &mut threads,
        &json!({"data":{"msgs":messages,"groupMsgs":group_messages}}),
    );
    for thread in &mut threads {
        for message in &mut thread.messages {
            let Some(source) = message.history_source.as_ref() else {
                continue;
            };
            let archive_code = code(source.get("msgType")).unwrap_or(0);
            message.kind = crate::message_model::MessageKind::from_archive_code(archive_code);
            // Preserve archive identity without laundering local IDs into live receipt refs.
            message.wire_ref = crate::message_model::WireMessageRef::from_raw(source);
            message.unread_policy = crate::message_model::UnreadPolicy::from_raw(source);
            match message.kind {
                crate::message_model::MessageKind::Photo => {
                    message.link_src = Some("Hình ảnh Zalo".into());
                    if message.link_thumb.is_none() {
                        message.link_thumb = message.link_url.clone();
                    }
                }
                crate::message_model::MessageKind::Video => {
                    message.link_src = Some("Video Zalo".into())
                }
                crate::message_model::MessageKind::Action if message.text.is_empty() => {
                    message.text = "[Nội dung tương tác]".into();
                }
                crate::message_model::MessageKind::Unknown => {
                    message.link_url = None;
                    message.link_thumb = None;
                    message.link_src = None;
                }
                _ => {}
            }
        }
    }
    for thread in threads.iter_mut().filter(|t| t.is_group) {
        if let Some(identity) = groups.values().find(|g| g.id == thread.id) {
            if !identity.name.is_empty() {
                thread.name = identity.name.clone();
            }
            if identity.avatar.is_some() {
                thread.avatar = identity.avatar.clone();
            }
        }
    }
    Ok(ArchiveConversion {
        threads,
        unresolved_groups,
    })
}

pub(super) async fn run(
    s: Session,
    commands: UnboundedSender<WsCommand>,
    mut events: UnboundedReceiver<WsEvent>,
    mut wakeups: UnboundedReceiver<()>,
    tx: super::worker::ScopedSender,
    ctx: eframe::egui::Context,
    generation: u64,
) {
    let account = s.login.as_ref().map(|l| l.uin.clone()).unwrap_or_default();
    let result = run_inner(
        s,
        commands,
        &mut events,
        &mut wakeups,
        &tx,
        &ctx,
        generation,
        &account,
    )
    .await;
    if let Err(error) = result {
        status(
            &tx,
            &ctx,
            format!("Đồng bộ điện thoại tạm dừng: {error}"),
            false,
            generation,
            &account,
        );
    }
}
// This adapter explicitly carries the worker channels and account fence.
#[allow(clippy::too_many_arguments)]
async fn run_inner(
    s: Session,
    commands: UnboundedSender<WsCommand>,
    events: &mut UnboundedReceiver<WsEvent>,
    wakeups: &mut UnboundedReceiver<()>,
    tx: &super::worker::ScopedSender,
    ctx: &eframe::egui::Context,
    generation: u64,
    account: &str,
) -> Result<()> {
    status(
        tx,
        ctx,
        "Đang chuẩn bị đồng bộ lịch sử từ điện thoại",
        true,
        generation,
        account,
    );
    let mut codec = Codec::start().await?;
    let mut active = ActiveSessions {
        commands,
        ids: HashSet::new(),
        next_index: 0,
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis() as u64;
    let id = request_round(
        &mut codec,
        &mut active,
        "conversation",
        vec![json!({"partition":"","from":0,"to":now,"limit":2147483647})],
    )
    .await?;
    status(
        tx,
        ctx,
        "Đã yêu cầu lịch sử; mở Zalo trên điện thoại để xác nhận đồng bộ",
        true,
        generation,
        account,
    );
    let round = wait_round(
        events,
        wakeups,
        &active.commands,
        &id,
        HashSet::new(),
        tx,
        ctx,
        generation,
        account,
    )
    .await?;
    let objects = decode_round(&mut codec, &id, "conversation", round).await?;
    active.dispose(&id);
    codec.call(json!({"op":"releaseSession","id":id})).await?;
    let mut conversations = Vec::new();
    for object in objects {
        conversations.extend(
            object
                .get("conversationsList")
                .and_then(Value::as_array)
                .context("Thiếu danh sách hội thoại")?
                .iter()
                .cloned(),
        );
    }
    let mut mapping = HashMap::new();
    // Group global IDs are not user global IDs: only direct peers go to gid/decrypt.
    resolve_ids(
        &s,
        conversations
            .iter()
            .filter(|c| code(c.get("convType")) != Some(2))
            .filter_map(|c| text_id(&c["convId"])),
        &mut mapping,
    )
    .await?;
    let groups = if conversations
        .iter()
        .any(|c| code(c.get("convType")) == Some(2))
    {
        match group_archive_identities(&s).await {
            Ok(groups) => groups,
            Err(_) => {
                status(tx, ctx, "Chưa phân giải được metadata nhóm; nhóm thiếu định danh sẽ chưa được nhập, các hội thoại khác vẫn tiếp tục", true, generation, account);
                HashMap::new()
            }
        }
    } else {
        HashMap::new()
    };
    let queries = conversation_queries(&conversations, now);
    let own = s.login.as_ref().context("Phiên đã đăng xuất")?.uin.clone();
    let mut count = 0;
    let mut unresolved_groups = HashSet::new();
    for (index, shard) in queries.chunks(30).enumerate() {
        status(
            tx,
            ctx,
            format!(
                "Đang đồng bộ nhóm và 1-1 từ điện thoại · lô {}/{} · đã nhận {count} tin",
                index + 1,
                queries.len().div_ceil(30)
            ),
            true,
            generation,
            account,
        );
        let wanted = shard
            .iter()
            .filter_map(|q| q["partition"].as_str().map(str::to_owned))
            .collect();
        let id = request_round(&mut codec, &mut active, "message", shard.to_vec()).await?;
        let round = wait_round(
            events,
            wakeups,
            &active.commands,
            &id,
            wanted,
            tx,
            ctx,
            generation,
            account,
        )
        .await?;
        let objects = decode_round(&mut codec, &id, "message", round).await?;
        active.dispose(&id);
        let senders = objects
            .iter()
            .flat_map(|o| o["partitionsList"].as_array().into_iter().flatten())
            .flat_map(|p| p["messagesList"].as_array().into_iter().flatten())
            .filter_map(|m| text_id(&m["senderId"]));
        resolve_ids(&s, senders, &mut mapping).await?;
        let converted = convert_objects(&objects, &mapping, &groups, &own)?;
        unresolved_groups.extend(converted.unresolved_groups);
        let mut threads = converted.threads;
        count += threads.iter().map(|t| t.messages.len()).sum::<usize>();
        let _ = tx.send(GuiEvent::PhoneHistoryThreads {
            generation,
            account: account.into(),
            threads: threads.clone(),
        });
        ctx.request_repaint();
        crate::chat::resolve_thread_names(&s, &mut threads).await;
        let _ = tx.send(GuiEvent::PhoneHistoryThreads {
            generation,
            account: account.into(),
            threads,
        });
        ctx.request_repaint();
        codec.call(json!({"op":"releaseSession","id":id})).await?;
    }
    let summary = if unresolved_groups.is_empty() {
        format!(
            "Đã nhận {count} tin lịch sử nhóm và 1-1 từ {} hội thoại trên điện thoại",
            conversations.len()
        )
    } else {
        let mut missing: Vec<_> = unresolved_groups.into_iter().collect();
        missing.sort();
        format!("Đã giữ {count} tin lịch sử; chưa nhập nhóm thiếu ánh xạ định danh: {}. Tải lại metadata nhóm rồi thử đồng bộ lại", missing.join(", "))
    };
    status(tx, ctx, summary, false, generation, account);
    Ok(())
}

pub(super) fn accepts_event(
    logged_in: bool,
    current_generation: u64,
    current_account: &str,
    generation: u64,
    account: &str,
) -> bool {
    logged_in && current_generation == generation && current_account == account
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn archive_mention_mapping_changes_only_exact_uid_sidecar() {
        let mut message = crate::chat::ChatMessage {
            text: "😀 @An @An".into(),
            history_source: Some(json!({"meta":{"mentionsList":[
                {"uid":"opaque-a","pos":3,"len":3},
                {"uid":"opaque-b","pos":7,"len":3},
                {"gUid":"not-a-uid","name":"An","pos":3,"len":3},
                {"uid":"-1","pos":3,"len":3}
            ]}})),
            ..Default::default()
        };
        let before = serde_json::to_value(&message).unwrap();
        assert_eq!(archive_mention_ids(&message), vec!["opaque-a", "opaque-b"]);
        apply_archive_mention_mapping(
            &mut message,
            &HashMap::from([
                ("opaque-a".into(), "11".into()),
                ("opaque-b".into(), "22".into()),
                ("not-a-uid".into(), "33".into()),
                ("-1".into(), "44".into()),
            ]),
        );
        let mut expected = before;
        expected["resolved_mention_uids"] = json!({"opaque-a":"11", "opaque-b":"22"});
        assert_eq!(serde_json::to_value(&message).unwrap(), expected);
        let spans = crate::message_navigation::message_mentions(&message);
        assert_eq!(
            spans
                .iter()
                .map(|span| span.user_id.as_str())
                .collect::<Vec<_>>(),
            vec!["11", "22"]
        );
        assert_eq!(&message.text[spans[1].byte_range.clone()], "@An");
    }

    #[test]
    fn archive_mention_mapping_ignores_invalid_or_unrelated_results() {
        let mut message = crate::chat::ChatMessage {
            history_source: Some(json!({"meta":{"mentionsList":[{"uid":"opaque"}]}})),
            ..Default::default()
        };
        for invalid in ["0", "00", "-1", "not-numeric", "", " 11"] {
            apply_archive_mention_mapping(
                &mut message,
                &HashMap::from([("opaque".into(), invalid.into())]),
            );
            assert!(message.resolved_mention_uids.is_empty());
        }
        apply_archive_mention_mapping(
            &mut message,
            &HashMap::from([("different-key".into(), "11".into())]),
        );
        assert!(message.resolved_mention_uids.is_empty());
    }

    fn cached_sender_fixture(sender: Value) -> (Thread, crate::chat::ChatMessage) {
        let message = crate::chat::ChatMessage {
            id: "archive-message-1".into(),
            ts: 1_700_000_000_000,
            sender: "987654321".into(),
            history_source: Some(
                json!({"senderId": sender, "msgId":"archive-message-1", "opaque":{"keep":true}}),
            ),
            ..Default::default()
        };
        let thread = Thread {
            presentation: crate::chat::SidebarPresentation {
                last_message_id: message.id.clone(),
                last_message_ts: message.ts,
                last_sender: message.sender.clone(),
                ..Default::default()
            },
            id: "g-fixture".into(),
            name: "Fixture".into(),
            preview: "Fixture preview".into(),
            time: "12:00".into(),
            avatar: None,
            is_group: true,
            messages: vec![message.clone()],
            unread: Default::default(),
        };
        (thread, message)
    }

    #[test]
    fn cached_sidebar_sender_resolves_archive_identity_without_rewriting_message() {
        let (mut thread, message) = cached_sender_fixture(json!("opaque-sender"));
        let before = serde_json::to_value(&thread).unwrap();
        let candidate = cached_sidebar_sender(&thread, &message).unwrap();
        let mapping = HashMap::from([("opaque-sender".into(), "123456".into())]);
        apply_cached_sidebar_sender(&mut thread, &candidate, &mapping);
        assert_eq!(thread.presentation.last_sender_id, "123456");
        let mut expected = before;
        expected["presentation"]["last_sender_id"] = json!("123456");
        assert_eq!(serde_json::to_value(&thread).unwrap(), expected);
        assert!(thread.messages[0].wire_ref.fields.is_empty());
    }

    #[test]
    fn cached_sidebar_sender_requires_both_latest_id_and_timestamp() {
        let (mut thread, message) = cached_sender_fixture(json!("opaque-sender"));
        thread.presentation.last_message_ts /= 1000;
        assert!(cached_sidebar_sender(&thread, &message).is_some());
        thread.presentation.last_message_id = "another-message-same-time".into();
        assert!(cached_sidebar_sender(&thread, &message).is_none());
        thread.presentation.last_message_id = message.id.clone();
        thread.presentation.last_message_ts += 1;
        assert!(cached_sidebar_sender(&thread, &message).is_none());
        thread.presentation.last_message_ts = message.ts;
        thread.presentation.last_message_id.clear();
        assert!(cached_sidebar_sender(&thread, &message).is_none());
    }

    #[test]
    fn cached_sidebar_sender_never_uses_numeric_display_sender_or_wire_fallback() {
        let (thread, mut message) = cached_sender_fixture(Value::Null);
        message.wire_ref =
            crate::message_model::WireMessageRef::from_raw(&json!({"uidFrom":"555"}));
        assert!(cached_sidebar_sender(&thread, &message).is_none());
        message.history_source = None;
        assert!(cached_sidebar_sender(&thread, &message).is_none());
    }

    #[test]
    fn cached_sidebar_sender_accepts_verified_numeric_archive_adapter_only() {
        for raw in [json!("123456"), json!(123456)] {
            let (mut thread, message) = cached_sender_fixture(raw);
            let candidate = cached_sidebar_sender(&thread, &message).unwrap();
            apply_cached_sidebar_sender(&mut thread, &candidate, &HashMap::new());
            assert_eq!(thread.presentation.last_sender_id, "123456");
        }
        let (mut thread, message) = cached_sender_fixture(json!("opaque-sender"));
        let candidate = cached_sidebar_sender(&thread, &message).unwrap();
        apply_cached_sidebar_sender(&mut thread, &candidate, &HashMap::new());
        assert!(thread.presentation.last_sender_id.is_empty());
        apply_cached_sidebar_sender(
            &mut thread,
            &candidate,
            &HashMap::from([("opaque-sender".into(), "invalid-uid".into())]),
        );
        assert!(thread.presentation.last_sender_id.is_empty());
    }

    #[test]
    fn cached_sidebar_sender_rechecks_snapshot_and_skips_direct_threads() {
        let (mut thread, message) = cached_sender_fixture(json!("123456"));
        let candidate = cached_sidebar_sender(&thread, &message).unwrap();
        thread.presentation.last_message_id = "newer-message".into();
        apply_cached_sidebar_sender(&mut thread, &candidate, &HashMap::new());
        assert!(thread.presentation.last_sender_id.is_empty());
        thread.presentation.last_message_id = message.id.clone();
        thread.is_group = false;
        assert!(cached_sidebar_sender(&thread, &message).is_none());
        apply_cached_sidebar_sender(&mut thread, &candidate, &HashMap::new());
        assert!(thread.presentation.last_sender_id.is_empty());
    }

    #[test]
    fn phone_media_enums_remain_distinct_from_live_types() {
        for (kind, expected) in [
            (3, "Hình ảnh Zalo"),
            (4, "Hình ảnh Zalo"),
            (19, "Video Zalo"),
            (31, "Hình ảnh Zalo"),
        ] {
            let source = json!({"msgType":kind,"content":"Chú thích 🧵","meta":{"attachsList":[{"href":"https://example.invalid/media?sig=synthetic%2Bvalue","thumb":"https://example.invalid/cover.jpg?sig=synthetic%2Fvalue"}]}});
            let (tag, content) = message_content(&source);
            let mut threads = Vec::new();
            crate::chat::merge_previews(
                &mut threads,
                &json!({"data":{"msgs":[{"msgId":"fixture-id","uidFrom":"456","idTo":"0","msgType":tag,"content":content,"historySource":source}]}}),
            );
            threads[0].messages[0].hydrate_media_metadata();
            let message = &threads[0].messages[0];
            assert_eq!(message.text, "Chú thích 🧵");
            assert_eq!(message.link_src.as_deref(), Some(expected));
            assert_eq!(
                message.link_url.as_deref(),
                Some("https://example.invalid/media?sig=synthetic%2Bvalue")
            );
            assert_eq!(
                message.link_thumb.as_deref(),
                Some("https://example.invalid/cover.jpg?sig=synthetic%2Fvalue")
            );
        }
    }

    #[test]
    fn sync_identifiers_match_official_length_and_are_fresh() {
        let a = generate_sync_id().unwrap();
        let b = generate_sync_id().unwrap();
        assert_eq!(a.len(), 32);
        assert!(a.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(a, b);
        assert_eq!(response_code(&json!({"data":{"error_code":"0"}})), Some(0));
        assert_eq!(response_code(&json!({"error_code":-5})), Some(-5));
        assert_eq!(
            response_code(&json!({"error_code":0,"data":{"error_code":-5}})),
            Some(-5)
        );
        assert!(ack_matches(
            &json!({"reqId":"req_wake_current","error_code":0}),
            592,
            "current"
        ));
        assert!(!ack_matches(
            &json!({"reqId":"req_wake_old","error_code":0}),
            592,
            "current"
        ));
        assert!(!ack_matches(&json!({"error_code":0}), 592, "current"));
    }
    #[tokio::test]
    async fn explicit_wakeup_targets_the_existing_session_and_waits_for_upload() {
        let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
        let (wake_tx, mut wake_rx) = tokio::sync::mpsc::unbounded_channel();
        let (command_tx, mut command_rx) = tokio::sync::mpsc::unbounded_channel();
        let (gui_tx, _gui_rx) = std::sync::mpsc::channel();
        let gui_tx = super::super::worker::ScopedSender::for_test(gui_tx);
        let ctx = eframe::egui::Context::default();
        let task = tokio::spawn(async move {
            wait_round(
                &mut event_rx,
                &mut wake_rx,
                &command_tx,
                "same-session",
                HashSet::new(),
                &gui_tx,
                &ctx,
                1,
                "fixture-account",
            )
            .await
        });
        wake_tx.send(()).unwrap();
        let command = tokio::time::timeout(Duration::from_secs(1), command_rx.recv())
            .await
            .unwrap()
            .unwrap();
        match command {
            WsCommand::WakeupSyncV2 { data, .. } => {
                assert_eq!(data, json!({"syncId":"same-session","toDevice":0}))
            }
            _ => panic!("Expected wakeup, not a new history request"),
        }
        event_tx
            .send(WsEvent {
                version: 1,
                cmd: 592,
                sub_cmd: 0,
                payload: json!({"error_code":0}),
            })
            .unwrap();
        event_tx.send(WsEvent{version:1,cmd:601,sub_cmd:0,payload:json!({"data":{"controls":[{"content":{"act_type":"transfer_sync2","act":"upload_batch","data":{"syncId":"same-session","idx":0,"batchType":2,"msgUrl":"https://fixture.zdn.vn/file","isLast":1,"err":0}}}]}})}).unwrap();
        let round = task.await.unwrap().unwrap();
        assert_eq!(round.batches.len(), 1);
    }
    #[test]
    fn stale_account_and_run_events_are_discarded() {
        assert!(accepts_event(true, 3, "B", 3, "B"));
        assert!(!accepts_event(true, 3, "B", 2, "A"));
        assert!(!accepts_event(true, 3, "B", 2, "B"));
        assert!(!accepts_event(false, 3, "B", 3, "B"));
    }
    #[test]
    fn statuses_reject_and_coverage_are_not_false_success() {
        let mut round = Round::default();
        assert!(round
            .accept("transfer_status", json!({"status":6}))
            .is_err());
        assert!(round
            .accept("transfer_status", json!({"status":5}))
            .unwrap()
            .is_some());
        round.accept("upload_batch", json!({"idx":0,"msgUrl":"https://a.zdn.vn/x","batchType":2,"scopes":[{"partition":"oneone/a"}],"isLast":0})).unwrap();
        assert!(!round.done(&HashSet::from(["oneone/a".into()])));
        assert!(!round.done(&HashSet::from(["oneone/a".into(), "oneone/b".into()])));
        assert!(!round.done(&HashSet::new()));
        assert!(round.accept("upload_batch", json!({"err":-1})).is_err());
    }
    fn completion_batch(idx: i64, batch_type: i64, is_last: bool) -> Value {
        json!({"idx":idx,"batchType":batch_type,"isLast":is_last,"msgUrl":"https://fixture.zdn.vn/batch","scopes":[{"partition":"group/fixture","from":0,"to":1000}]})
    }

    #[test]
    fn batch_completion_final_before_middle_waits_for_every_index() {
        let mut round = Round::default();
        round
            .accept("upload_batch", completion_batch(0, 2, false))
            .unwrap();
        round
            .accept("upload_batch", completion_batch(3, 1, true))
            .unwrap();
        assert!(!round.done(&HashSet::new()));
        round
            .accept("upload_batch", completion_batch(2, 1, false))
            .unwrap();
        assert!(!round.done(&HashSet::new()));
        round
            .accept("upload_batch", completion_batch(1, 1, false))
            .unwrap();
        assert!(round.done(&HashSet::new()));
    }

    #[test]
    fn batch_completion_partition_names_without_final_are_not_complete() {
        let mut round = Round::default();
        round
            .accept("upload_batch", completion_batch(0, 2, false))
            .unwrap();
        round
            .accept("upload_batch", completion_batch(1, 1, false))
            .unwrap();
        assert!(!round.done(&HashSet::from(["group/fixture".into()])));
    }

    #[test]
    fn batch_completion_duplicates_do_not_fill_missing_indices_or_change_kind() {
        let mut round = Round::default();
        round
            .accept("upload_batch", completion_batch(0, 2, false))
            .unwrap();
        round
            .accept("upload_batch", completion_batch(2, 1, true))
            .unwrap();
        let mut duplicate = completion_batch(2, 1, true);
        duplicate["msgUrl"] = json!("https://fixture.zdn.vn/renewed");
        round.accept("upload_batch", duplicate).unwrap();
        assert_eq!(round.batches.len(), 2);
        assert!(!round.done(&HashSet::new()));
        assert!(round
            .accept("upload_batch", completion_batch(0, 1, false))
            .is_err());
        round
            .accept("upload_batch", completion_batch(1, 1, false))
            .unwrap();
        assert!(round.done(&HashSet::new()));
    }

    #[test]
    fn batch_completion_duplicate_can_add_final_marker_but_cannot_conflict() {
        let mut round = Round::default();
        round
            .accept("upload_batch", completion_batch(0, 2, false))
            .unwrap();
        assert!(!round.done(&HashSet::new()));
        round
            .accept("upload_batch", completion_batch(0, 2, true))
            .unwrap();
        assert_eq!(round.batches.len(), 1);
        assert!(round.done(&HashSet::new()));
        let mut multi = Round::default();
        multi
            .accept("upload_batch", completion_batch(0, 2, false))
            .unwrap();
        multi
            .accept("upload_batch", completion_batch(1, 1, false))
            .unwrap();
        multi
            .accept("upload_batch", completion_batch(2, 1, true))
            .unwrap();
        assert!(multi
            .accept("upload_batch", completion_batch(1, 1, true))
            .is_err());
        assert_eq!(multi.final_index, Some(2));
        assert_eq!(multi.batches.len(), 3);
        assert!(multi.done(&HashSet::new()));
    }

    #[test]
    fn batch_completion_rejects_invalid_indices_and_conflicting_final_markers() {
        for index in [
            Value::Null,
            json!(-1),
            json!(4096),
            json!(u64::MAX),
            json!(1.5),
            json!("invalid"),
        ] {
            let mut batch = completion_batch(0, 2, false);
            batch["idx"] = index;
            assert!(Round::default().accept("upload_batch", batch).is_err());
        }
        let mut round = Round::default();
        round
            .accept("upload_batch", completion_batch(3, 1, false))
            .unwrap();
        assert!(round
            .accept("upload_batch", completion_batch(2, 1, true))
            .is_err());
        round
            .accept("upload_batch", completion_batch(4, 1, true))
            .unwrap();
        assert!(round
            .accept("upload_batch", completion_batch(5, 1, false))
            .is_err());
        assert!(round
            .accept("upload_batch", completion_batch(1, 1, true))
            .is_err());
        assert!(!round.done(&HashSet::new()));
        let mut string_index = completion_batch(0, 2, true);
        string_index["idx"] = json!("0");
        let mut single = Round::default();
        single.accept("upload_batch", string_index).unwrap();
        assert!(single.done(&HashSet::new()));
    }

    #[test]
    fn message_end_and_coverage_wait_for_metadata_arriving_late() {
        for wanted in [HashSet::new(), HashSet::from(["group/fixture".into()])] {
            let mut round = Round::default();
            round.accept("upload_batch", json!({"idx":1,"msgUrl":"https://fixture.zdn.vn/message","batchType":1,"scopes":[{"partition":"group/fixture"}],"isLast":1})).unwrap();
            assert!(!round.done(&wanted));
            round.accept("upload_batch", json!({"idx":0,"msgUrl":"https://fixture.zdn.vn/metadata","batchType":2,"isLast":0})).unwrap();
            assert!(round.done(&wanted));
        }
    }

    #[test]
    fn directions_and_id_fallback_do_not_create_zero_collisions() {
        let map = HashMap::from([("opaque".into(), "123".into())]);
        let objects = vec![
            json!({"partitionsList":[{"id":"oneone/opaque","messagesList":[
                {"globalId":"0","clientId":"11","senderId":"123","content":"incoming","timestamp":"1000"},
                {"globalId":"22","clientId":"12","senderId":"456","content":"outgoing","timestamp":"2000"}
            ]}]}),
        ];
        let threads = convert_objects(&objects, &map, &HashMap::new(), "456")
            .unwrap()
            .threads;
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].id, "123");
        assert_eq!(threads[0].messages.len(), 2);
        assert_eq!(threads[0].messages[0].id, "11");
        assert!(!threads[0].messages[0].is_outgoing);
        assert!(threads[0].messages[1].is_outgoing);
        assert!(convert_objects(&objects, &HashMap::new(), &HashMap::new(), "456").is_err());
    }
    #[test]
    fn mixed_phone_queries_include_groups_with_per_conversation_bounds_and_shards() {
        let conversations: Vec<_> = (0..61).map(|index| json!({"convId":format!("c{index}"),"convType":if index % 2 == 0 {2} else {1},"lastTs":100+index})).collect();
        let queries = conversation_queries(&conversations, 150);
        assert_eq!(queries.len(), 61);
        assert_eq!(
            queries.chunks(30).map(|s| s.len()).collect::<Vec<_>>(),
            vec![30, 30, 1]
        );
        assert_eq!(queries[0]["partition"], "group/c0");
        assert_eq!(queries[1]["partition"], "oneone/c1");
        assert_eq!(queries[0]["to"], 100);
        assert_eq!(queries[60]["to"], 150);
    }

    #[test]
    fn mixed_group_archive_preserves_metadata_directions_media_and_replay_identity() {
        let mut groups = HashMap::new();
        collect_group_identities(
            &json!({"data":{"gridInfoMap":{
                "9001":{"globalId":"opaque-a","groupId":"9001","name":"Same group","avt":"https://fixture.zdn.vn/group.png"},
                "9002":{"globalId":"opaque-b","groupId":9002,"name":"Same group"}
            }}}),
            &mut groups,
        );
        let users = HashMap::from([
            ("opaque-peer".into(), "123".into()),
            ("opaque-member".into(), "789".into()),
        ]);
        let objects = vec![json!({"partitionsList":[
            {"id":"oneone/opaque-peer","messagesList":[{"globalId":"11","senderId":"123","content":"direct","timestamp":"1000"}]},
            {"id":"group/opaque-a","messagesList":[
                {"globalId":"12","senderId":"opaque-member","content":"member","timestamp":"2000"},
                {"globalId":"13","senderId":"456","content":"photo caption","msgType":3,"timestamp":"3000","meta":{"attachsList":[{"href":"https://fixture.zdn.vn/old.jpg","thumb":"https://fixture.zdn.vn/thumb.jpg"},{"href":"https://fixture.zdn.vn/second.jpg"}]}}
            ]},
            {"id":"group/9002","messagesList":[{"globalId":"14","senderId":"321","content":"second group","timestamp":"4000"}]},
            {"id":"group/unresolved-group","messagesList":[{"globalId":"15","senderId":"789","content":"unmapped","timestamp":"5000"}]}
        ]})];
        let converted = convert_objects(&objects, &users, &groups, "456").unwrap();
        assert_eq!(
            converted.unresolved_groups,
            HashSet::from(["unresolved-group".into()])
        );
        let threads = converted.threads;
        assert_eq!(threads.len(), 3);
        let direct = threads.iter().find(|t| t.id == "123").unwrap();
        assert!(!direct.is_group);
        let group = threads.iter().find(|t| t.id == "9001").unwrap();
        assert!(group.is_group);
        assert_eq!(group.name, "Same group");
        assert_eq!(
            group.avatar.as_deref(),
            Some("https://fixture.zdn.vn/group.png")
        );
        assert_eq!(group.messages.len(), 2);
        assert!(!group.messages[0].is_outgoing);
        assert!(group.messages[1].is_outgoing);
        assert_eq!(
            group.messages[1].link_url.as_deref(),
            Some("https://fixture.zdn.vn/old.jpg")
        );
        assert_eq!(
            group.messages[1].history_source.as_ref().unwrap()["meta"]["attachsList"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert!(threads.iter().find(|t| t.id == "9002").unwrap().is_group);
        assert!(!threads.iter().any(|t| t.id.contains("unresolved")));
        let mut replay = threads.clone();
        for thread in &threads {
            let existing = replay.iter_mut().find(|t| t.id == thread.id).unwrap();
            crate::chat::merge_thread_into(existing, thread.clone(), true);
        }
        assert_eq!(replay.iter().map(|t| t.messages.len()).sum::<usize>(), 4);
        let mut store = crate::store::MessageStore::open(":memory:").unwrap();
        crate::store::persist_threads(&mut store, &replay).unwrap();
        let mut restored = Vec::new();
        store.hydrate_threads(&mut restored, 100).unwrap();
        assert_eq!(restored.len(), 3);
        assert_eq!(restored.iter().filter(|t| t.is_group).count(), 2);
        assert_eq!(
            restored
                .iter()
                .find(|t| t.id == "9001")
                .unwrap()
                .messages
                .len(),
            2
        );
        // PhoneHistoryThreads is normalized to HistoryThreadsUpdated by gui.rs, never a live update.
        let event = GuiEvent::PhoneHistoryThreads {
            generation: 7,
            account: "456".into(),
            threads: restored,
        };
        assert!(matches!(
            event,
            GuiEvent::PhoneHistoryThreads { generation: 7, .. }
        ));
    }

    #[test]
    fn rich_history_and_repeated_text_survive_merge_and_database_reopen() {
        let objects = vec![json!({"partitionsList":[{"id":"oneone/123","messagesList":[
            {"globalId":"11","clientId":"101","senderId":"123","content":"ok","msgType":0,"timestamp":"1000"},
            {"globalId":"12","clientId":"102","senderId":"123","content":"ok","msgType":0,"timestamp":"1001"},
            {"globalId":"13","clientId":"103","senderId":"456","content":"caption","msgType":3,"timestamp":"2000","meta":{"attachsList":[{"href":"https://f1.zdn.vn/photo.jpg","thumb":"https://f1.zdn.vn/thumb.jpg"},{"href":"https://f1.zdn.vn/second.jpg"}],"quote":{"cliMsgId":"0","globalMsgId":"11","msg":"ok"}}},
            {"globalId":"14","senderId":"456","content":"","msgType":10,"timestamp":"3000","meta":{"attachsList":[{"catId":42,"id":99,"type":7}]}}
        ]}]})];
        let threads = convert_objects(&objects, &HashMap::new(), &HashMap::new(), "456")
            .unwrap()
            .threads;
        assert_eq!(threads[0].messages.len(), 4);
        let photo = &threads[0].messages[2];
        assert_eq!(photo.text, "caption");
        assert_eq!(
            photo.link_url.as_deref(),
            Some("https://f1.zdn.vn/photo.jpg")
        );
        assert_eq!(photo.reply_to_id.as_deref(), Some("11"));
        assert_eq!(photo.link_src.as_deref(), Some("Hình ảnh Zalo"));
        assert_eq!(
            photo.link_thumb.as_deref(),
            Some("https://f1.zdn.vn/thumb.jpg")
        );
        assert_eq!(threads[0].messages[3].sticker_id, Some(99));
        let mut merged = threads.clone();
        crate::chat::merge_thread_into(&mut merged[0], threads[0].clone(), true);
        assert_eq!(merged[0].messages.len(), 4);
        let mut store = crate::store::MessageStore::open(":memory:").unwrap();
        crate::store::persist_threads(&mut store, &merged).unwrap();
        let mut restored = Vec::new();
        store.hydrate_threads(&mut restored, 100).unwrap();
        assert_eq!(restored[0].messages.len(), 4);
        assert_eq!(
            restored[0].messages[2].history_source.as_ref().unwrap()["meta"]["attachsList"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(restored[0].messages[3].sticker_id, Some(99));
    }
    #[test]
    fn cancellation_disposes_every_live_session() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut active = ActiveSessions {
            commands: tx,
            ids: HashSet::from(["a".into(), "b".into()]),
            next_index: 0,
        };
        active.dispose("a");
        drop(active);
        let mut ids = Vec::new();
        while let Ok(WsCommand::DisposeSyncV2 { sync_id }) = rx.try_recv() {
            ids.push(sync_id);
        }
        ids.sort();
        assert_eq!(ids, vec!["a", "b"]);
    }
    #[test]
    fn download_hosts_and_control_envelopes_are_checked() {
        assert!(allowed_download(
            &reqwest::Url::parse("https://f1.zdn.vn/path").unwrap()
        ));
        for url in [
            "http://f1.zdn.vn/x",
            "https://zdn.vn.evil.test/x",
            "https://127.0.0.1/x",
        ] {
            assert!(!allowed_download(&reqwest::Url::parse(url).unwrap()));
        }
        let event = WsEvent {
            version: 1,
            cmd: 601,
            sub_cmd: 0,
            payload: json!({"data":{"controls":[{"content":{"act_type":"transfer_sync2","act":"transfer_status","data":"{\"syncId\":\"fixture\",\"status\":3}"}}]}}),
        };
        let items = controls(&event);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].0, "fixture");
    }
}

#[cfg(test)]
mod archive_adapter_tests {
    use super::*;
    use crate::message_model::MessageKind as K;
    #[test]
    fn archive_new_codes_hydrate_without_losing_any_attachment_or_wire_provenance() {
        let variants = [
            (15, K::Action),
            (24, K::WebContent),
            (31, K::Photo),
            (33, K::Delete),
            (53, K::WebContent),
            (20, K::Unknown),
            (21, K::Unknown),
            (26, K::Unknown),
            (29, K::Unknown),
            (32, K::Unknown),
            (35, K::Unknown),
        ];
        for (archive_code, expected) in variants {
            let source = json!({"globalId":"9001","clientId":"101","senderId":"123","timestamp":"1700000000000","msgType":archive_code,"content":"","meta":{"attachsList":[{"href":"https://fixture.invalid/first?sig=opaque%2Bvalue","params":"{\"pcItem\":{}}","zinstantMsg":"fixture-content","zinstantData":"fixture-data"},{"href":"https://fixture.invalid/second","opaque":{"all":"retained"}}]}});
            let object =
                json!({"partitionsList":[{"id":"oneone/123","messagesList":[source.clone()]}]});
            let converted =
                convert_objects(&[object], &HashMap::new(), &HashMap::new(), "456").unwrap();
            let m = &converted.threads[0].messages[0];
            assert_eq!(m.kind, expected);
            assert_eq!(m.history_source.as_ref(), Some(&source));
            assert_eq!(
                m.history_source.as_ref().unwrap()["meta"]["attachsList"]
                    .as_array()
                    .unwrap()
                    .len(),
                2
            );
            assert!(m.wire_ref.server_id.is_none());
            assert!(!m.wire_ref.fields.contains_key("msgId"));
            assert_eq!(m.wire_ref.original_type, Some(json!(archive_code)));
            if expected == K::Unknown {
                assert_eq!(m.text, "[Tin nhắn đặc biệt]");
                assert!(m.link_url.is_none());
                assert!(m.link_thumb.is_none());
            }
            if archive_code == 31 {
                assert_eq!(m.link_src.as_deref(), Some("Hình ảnh Zalo"));
            }
            if archive_code == 33 {
                assert_ne!(m.text, "[Tin nhắn đã thu hồi]");
            }
        }
    }
    #[test]
    fn oa_arrays_and_webcontent_v2_keys_follow_the_original_decoder() {
        let source = json!({"msgType":15,"meta":{"attachsList":[{"title":"one"},{"title":"two"}]}});
        let (tag, content) = message_content(&source);
        assert_eq!(tag, "chat.list.action");
        assert_eq!(content.as_array().unwrap().len(), 2);
        let source = json!({"msgType":53,"meta":{"attachsList":[{"zinstantMsg":"message","zinstantData":"data","action":"action"}]}});
        let (tag, content) = message_content(&source);
        assert_eq!(tag, "chat.webcontent.v2");
        assert_eq!(content["zinstant_msg"], "message");
        assert_eq!(content["zinstant_data"], "data");
        let (_, invalid) = message_content(
            &json!({"msgType":24,"meta":{"attachsList":[{"href":"https://fixture.invalid/not-decoded"}]}}),
        );
        assert_eq!(invalid["description"], "[Nội dung web chưa có bộ giải mã]");
        assert!(invalid.get("href").is_none());
    }
}

fn codec_assets_override(override_path: Option<PathBuf>, default_path: PathBuf) -> Result<PathBuf> {
    match override_path {
        Some(path) if !path.is_absolute() => {
            bail!("ZL_HISTORY_CODEC_ASSETS phải là đường dẫn tuyệt đối")
        }
        Some(path) => Ok(path),
        None => Ok(default_path),
    }
}

#[cfg(test)]
mod codec_assets_configuration_tests {
    use super::*;
    #[test]
    fn explicit_absolute_path_overrides_bundle_without_touching_files() {
        let path = std::env::temp_dir().join("fixture-codec-assets");
        assert_eq!(
            codec_assets_override(Some(path.clone()), "bundle".into()).unwrap(),
            path
        );
        assert_eq!(
            codec_assets_override(None, "bundle".into()).unwrap(),
            PathBuf::from("bundle")
        );
        assert!(codec_assets_override(Some("relative".into()), "bundle".into()).is_err());
    }
}
