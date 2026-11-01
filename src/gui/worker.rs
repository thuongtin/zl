use super::*;

pub(super) struct ScopedSender {
    raw: Sender<GuiEvent>,
    scope: Option<crate::runtime_state::SessionScope>,
    #[cfg(test)]
    passthrough: bool,
}
impl Clone for ScopedSender {
    fn clone(&self) -> Self {
        Self {
            raw: self.raw.clone(),
            scope: Some(
                self.scope
                    .clone()
                    .unwrap_or_else(|| crate::runtime_state::registry().current()),
            ),
            #[cfg(test)]
            passthrough: self.passthrough,
        }
    }
}
impl ScopedSender {
    #[cfg(test)]
    pub(super) fn for_test(raw: Sender<GuiEvent>) -> Self {
        Self {
            raw,
            scope: None,
            passthrough: true,
        }
    }
    fn current(&self) -> bool {
        self.scope
            .as_ref()
            .is_none_or(|s| crate::runtime_state::registry().accepts(s))
    }
    fn refreshed(&self) -> Self {
        Self {
            raw: self.raw.clone(),
            scope: None,
            #[cfg(test)]
            passthrough: self.passthrough,
        }
    }
    fn send_names_metadata(&self, threads: Vec<Thread>) {
        let _fence = storage::mutation_fence()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if !self.current() {
            return;
        }
        let scope = self
            .scope
            .clone()
            .unwrap_or_else(|| crate::runtime_state::registry().current());
        let threads = names_metadata(&threads);
        if !scope.account.is_empty()
            && storage::open_account_store(&scope.account)
                .and_then(|mut store| store.persist_threads(&threads))
                .is_err()
        {
            let _ = self.raw.send(GuiEvent::Scoped {
                scope: scope.clone(),
                event: Box::new(GuiEvent::PersistenceStatus {
                    component: "history",
                    error: Some(
                        "Chưa lưu được tên hội thoại; lịch sử đã nhận vẫn được giữ.".into(),
                    ),
                }),
            });
        }
        // The metadata event deliberately bypasses unread derivation from a potentially older DB view.
        let _ = self.raw.send(GuiEvent::Scoped {
            scope,
            event: Box::new(GuiEvent::HistoryThreadsUpdated(threads)),
        });
    }
    pub(super) fn send(
        &self,
        mut event: GuiEvent,
    ) -> Result<(), Box<std::sync::mpsc::SendError<GuiEvent>>> {
        #[cfg(test)]
        if self.passthrough {
            return self.raw.send(event).map_err(Box::new);
        }
        let _fence = storage::mutation_fence()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if !self.current() {
            return Err(Box::new(std::sync::mpsc::SendError(event)));
        }
        if matches!(&event, GuiEvent::LoggedOut | GuiEvent::SessionExpired(_)) {
            crate::runtime_state::registry().invalidate();
            return self.raw.send(event).map_err(Box::new);
        }
        if let GuiEvent::LoginSuccess { uin, .. } = &event {
            crate::runtime_state::registry().bind(uin);
            return self.raw.send(event).map_err(Box::new);
        }
        let scope = self
            .scope
            .clone()
            .unwrap_or_else(|| crate::runtime_state::registry().current());
        if let GuiEvent::DeliveryEvents(events) = &event {
            if !scope.account.is_empty() {
                if let Ok(mut store) = storage::open_account_store(&scope.account) {
                    for delivery in events {
                        if let Some(id) = delivery.message_id.as_deref() {
                            if let Ok(Some(message)) = store.get_message(&delivery.thread_id, id) {
                                let mut threads = vec![Thread {
                                    presentation: Default::default(),
                                    id: delivery.thread_id.clone(),
                                    name: String::new(),
                                    preview: String::new(),
                                    time: String::new(),
                                    avatar: None,
                                    is_group: delivery.thread_id.starts_with('g'),
                                    messages: vec![message],
                                    unread: Default::default(),
                                }];
                                if !crate::delivery_receipts::apply_delivery_events(
                                    &mut threads,
                                    std::slice::from_ref(delivery),
                                )
                                .is_empty()
                                {
                                    let _ = store.persist_threads(&threads);
                                }
                            }
                        }
                    }
                }
            }
        }
        if let GuiEvent::ReactionEvents(events) = &event {
            if !scope.account.is_empty()
                && crate::reaction_journal::ReactionJournal::open(
                    std::path::Path::new("."),
                    &scope.account,
                )
                .and_then(|mut j| {
                    let added = j.append(events)?;
                    if let Ok(mut store) = storage::open_account_store(&scope.account) {
                        for event in events {
                            if !event.message_id.is_empty() {
                                if let Some(message) =
                                    store.get_message(&event.thread_id, &event.message_id)?
                                {
                                    let mut threads = vec![Thread {
                                        presentation: Default::default(),
                                        id: event.thread_id.clone(),
                                        name: String::new(),
                                        preview: String::new(),
                                        time: String::new(),
                                        avatar: None,
                                        is_group: event.thread_id.starts_with('g'),
                                        messages: vec![message],
                                        unread: Default::default(),
                                    }];
                                    j.apply_loaded(&mut threads)?;
                                    store.persist_threads(&threads)?;
                                }
                            }
                        }
                    }
                    Ok(added)
                })
                .is_err()
            {
                let _ = self.raw.send(GuiEvent::Scoped {
                    scope: scope.clone(),
                    event: Box::new(GuiEvent::PersistenceStatus {
                        component: "reactions",
                        error: Some(
                            "Chưa ghi được sự kiện thả tim; giữ sự kiện trong bộ nhớ".into(),
                        ),
                    }),
                });
            }
        }
        let incoming = match &mut event {
            GuiEvent::ThreadsUpdated(t) | GuiEvent::HistoryThreadsUpdated(t) => Some(t),
            GuiEvent::PhoneHistoryThreads { threads, .. } => Some(threads),
            _ => None,
        };
        if let Some(threads) = incoming {
            if !scope.account.is_empty() {
                let result = storage::open_account_store(&scope.account).and_then(|mut store| {
                    for t in threads.iter_mut() {
                        for m in &mut t.messages {
                            storage::hydrate_model(m);
                        }
                    }
                    if storage::legacy_import_enabled(std::path::Path::new("."), &scope.account) {
                        if let Ok(Some(cache)) =
                            crate::original_reactions::load_validated_private_cache(
                                std::path::Path::new("original_reactions.json"),
                                &scope.account,
                            )
                        {
                            cache.apply_to_threads(threads);
                        }
                    }
                    if let Ok(mut journal) = crate::reaction_journal::ReactionJournal::open(
                        std::path::Path::new("."),
                        &scope.account,
                    ) {
                        journal.apply_loaded(threads)?;
                    }
                    store.persist_threads(threads)?;
                    for thread in threads.iter_mut() {
                        store.derive_unread(&thread.id, &mut thread.unread)?;
                    }
                    Ok(())
                });
                if result.is_err() {
                    let _ = self.raw.send(GuiEvent::Scoped {
                        scope: scope.clone(),
                        event: Box::new(GuiEvent::PersistenceStatus {
                            component: "history",
                            error: Some(
                                "Lưu lịch sử thất bại; dữ liệu chưa ghi vẫn được giữ trong bộ nhớ"
                                    .into(),
                            ),
                        }),
                    });
                }
            }
        }

        self.raw
            .send(GuiEvent::Scoped {
                scope,
                event: Box::new(event),
            })
            .map_err(Box::new)
    }
}
fn names_metadata(threads: &[Thread]) -> Vec<Thread> {
    threads
        .iter()
        .map(|thread| Thread {
            id: thread.id.clone(),
            name: thread.name.clone(),
            avatar: thread.avatar.clone(),
            is_group: thread.is_group,
            preview: String::new(),
            time: String::new(),
            messages: Vec::new(),
            unread: Default::default(),
            presentation: crate::chat::SidebarPresentation {
                members: thread.presentation.members.clone(),
                member_count: thread.presentation.member_count,
                ..Default::default()
            },
        })
        .collect()
}
fn publish_received_before_lookup(
    threads: &[Thread],
    is_history: bool,
    mut publish: impl FnMut(GuiEvent),
) -> Vec<Thread> {
    publish(if is_history {
        GuiEvent::HistoryThreadsUpdated(threads.to_vec())
    } else {
        GuiEvent::ThreadsUpdated(threads.to_vec())
    });
    names_metadata(threads)
}

fn payload_has_read_points(payload: &serde_json::Value) -> bool {
    let data = payload.get("data").unwrap_or(payload);
    data.get("clearUnreads").is_some()
        || data.get("clearUnreadInfo").is_some()
        || data
            .get("statusMap")
            .is_some_and(|map| map.get("clearUnreads").is_some())
}
fn merge_known_remote_update(mut known: Vec<Thread>, payload: &serde_json::Value) -> Vec<Thread> {
    crate::chat::merge_previews(&mut known, payload);
    known.retain(|thread| !thread.messages.is_empty() || thread.unread.fresh);
    known
}

struct PollLease(&'static std::sync::atomic::AtomicBool);
impl Drop for PollLease {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}
struct CloudLease {
    active: std::sync::Arc<std::sync::Mutex<std::collections::HashSet<(String, String)>>>,
    key: (String, String),
}
impl Drop for CloudLease {
    fn drop(&mut self) {
        self.active
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&self.key);
    }
}
fn persist_outgoing(
    account: &str,
    thread_id: &str,
    message: ChatMessage,
    is_group: bool,
) -> anyhow::Result<()> {
    let mut store = storage::open_account_store(account)?;
    store.persist_threads(&[Thread {
        presentation: Default::default(),
        id: thread_id.into(),
        name: String::new(),
        preview: message.text.clone(),
        time: message.time.clone(),
        avatar: None,
        is_group,
        messages: vec![message],
        unread: Default::default(),
    }])?;
    Ok(())
}
fn media_job_queue() -> &'static crate::media_resources::MediaJobQueue {
    static QUEUE: std::sync::OnceLock<crate::media_resources::MediaJobQueue> =
        std::sync::OnceLock::new();
    QUEUE.get_or_init(Default::default)
}
fn scoped_tasks() -> &'static std::sync::Mutex<Vec<tokio::task::AbortHandle>> {
    static TASKS: std::sync::OnceLock<std::sync::Mutex<Vec<tokio::task::AbortHandle>>> =
        std::sync::OnceLock::new();
    TASKS.get_or_init(Default::default)
}
static BLOCKING_JOBS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
fn tracked_blocking<F, R>(work: F) -> tokio::task::JoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    BLOCKING_JOBS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    struct Lease;
    impl Drop for Lease {
        fn drop(&mut self) {
            BLOCKING_JOBS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        }
    }
    let lease = Lease;
    tokio::task::spawn_blocking(move || {
        let _lease = lease;
        work()
    })
}
async fn cancel_account_jobs() -> bool {
    let tasks = {
        let mut registry = scoped_tasks().lock().unwrap_or_else(|e| e.into_inner());
        let tasks: Vec<_> = registry.drain(..).collect();
        for task in &tasks {
            task.abort();
        }
        tasks
    };
    // Aborting an async parent does not stop its already-submitted blocking closure.
    tokio::task::yield_now().await;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while tasks.iter().any(|task| !task.is_finished())
        || BLOCKING_JOBS.load(std::sync::atomic::Ordering::SeqCst) != 0
    {
        if std::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    true
}
async fn flush_shutdown_and_ack(
    root: std::path::PathBuf,
    account: String,
    threads: Vec<Thread>,
    read_deltas: Vec<(String, Vec<ChatMessage>)>,
    draft: Option<(String, crate::runtime_state::Draft)>,
    events: Sender<GuiEvent>,
) -> bool {
    let result = tracked_blocking(move || {
        storage::flush_shutdown(&root, &account, &threads, &read_deltas, draft)
    })
    .await;
    let success = matches!(result, Ok(Ok(())));
    let error = (!success).then(|| {
        "Chưa lưu được dữ liệu cuối; ứng dụng vẫn mở. Hãy kiểm tra ổ đĩa rồi thử thoát lại."
            .to_string()
    });
    let _ = events.send(GuiEvent::ShutdownComplete { error });
    success
}
fn spawn_scoped(
    sender: ScopedSender,
    future: impl std::future::Future<Output = ()> + Send + 'static,
) -> tokio::task::JoinHandle<()> {
    let scope = sender
        .scope
        .clone()
        .unwrap_or_else(|| crate::runtime_state::registry().current());
    let task = tokio::spawn(async move {
        tokio::select! {biased; _=crate::runtime_state::registry().cancelled(&scope)=>{}, _=future=>{}}
    });
    let mut tasks = scoped_tasks().lock().unwrap_or_else(|e| e.into_inner());
    tasks.retain(|task| !task.is_finished());
    tasks.push(task.abort_handle());
    task
}

fn resolve_page_mentions(
    sender: ScopedSender,
    session: Session,
    ctx: egui::Context,
    thread_id: String,
    request_id: u64,
    mut messages: Vec<ChatMessage>,
) {
    spawn_scoped(sender.clone(), async move {
        if super::phone_history::enrich_archive_mentions(&session, &mut messages)
            .await
            .is_err()
        {
            return;
        }
        let mappings: Vec<_> = messages
            .iter()
            .filter(|m| !m.resolved_mention_uids.is_empty())
            .map(|m| (m.id.clone(), m.resolved_mention_uids.clone()))
            .collect();
        if mappings.is_empty() {
            return;
        }
        let account = session
            .login
            .as_ref()
            .map(|l| l.uin.clone())
            .unwrap_or_default();
        let tid = thread_id.clone();
        let saved = mappings.clone();
        let persist = tracked_blocking(move || -> anyhow::Result<()> {
            let mut store = storage::open_account_store(&account)?;
            store.persist_mention_mappings(&tid, &saved)?;
            Ok(())
        })
        .await;
        if !matches!(persist, Ok(Ok(()))) {
            let _ = sender.send(GuiEvent::StatusUpdate(
                "Chưa lưu được định danh tag lịch sử".into(),
            ));
            return;
        }
        let _ = sender.send(GuiEvent::ResolvedMentions {
            thread_id,
            request_id,
            mappings,
        });
        ctx.request_repaint();
    });
}



pub enum WorkerCmd {
    Shutdown {
        account: String,
        threads: Vec<Thread>,
        draft: Option<(String, crate::runtime_state::Draft)>,
    },
    LocalReadDelta {
        account: String,
        thread_id: String,
        messages: Vec<ChatMessage>,
    },

    LoadLocalPage {
        thread_id: String,
        cursor: Option<crate::store_v2::HistoryCursor>,
        direction: crate::store_v2::PageDirection,
        replace: bool,
        request_id: u64,
    },

    NotifyBackground {
        opaque_context: String,
        sound: bool,
    },

    RequestNotificationAuthorization,

    RequestQr,
    LoginPassword {
        phone: String,
        pass: String,
    },
    RefreshThreads,
    PollMessages {
        known_threads: Vec<(String, String, bool)>,
    },
    LoadGroupHistory {
        group_id: String,
        is_group: bool,
    },
    SendMessage {
        to_id: String,
        text: String,
        is_group: bool,
    },
    SendQuote {
        to_id: String,
        text: String,
        quoted: ChatMessage,
        is_group: bool,
    },
    Forward {
        target_id: String,
        target_is_group: bool,
        msg: ChatMessage,
    },
    React {
        to_id: String,
        is_group: bool,
        msg: ChatMessage,
        reaction: String,
    },
    DeleteForMe {
        to_id: String,
        is_group: bool,
        msg: ChatMessage,
    },
    RecallMessage {
        to_id: String,
        is_group: bool,
        msg: ChatMessage,
    },
    SendSticker {
        to_id: String,
        sticker_id: u64,
        cate_id: u64,
        sticker_type: u32,
        is_group: bool,
    },
    PickAndSendAttachment {
        to_id: String,
        is_group: bool,
        images_only: bool,
        caption: String,
        preset_path: Option<std::path::PathBuf>,
    },
    FetchStickerImage {
        key: String,
        url: String,
    },
    FetchStickerStore,
    FetchStickerPack {
        cate_id: u64,
        name: String,
    },
    SearchStickers {
        keyword: String,
    },
    SuggestComposerStickers {
        keyword: String,
    },
    ParseLinkPreview {
        url: String,
    },
    LookupPhone {
        phone: String,
    },
    StartCall {
        partner_id: String,
        partner_name: String,
        partner_avatar: Option<String>,
        is_video: bool,
        is_group: bool,
    },
    AnswerCall {
        accept: bool,
    },
    SetCallMediaState {
        is_on_mic: bool,
        is_on_cam: bool,
    },
    EndCall,
    AddFriend {
        user_id: String,
        user_name: String,
    },
    CheckFriendStatus {
        user_id: String,
    },
    AcceptFriend {
        user_id: String,
        user_name: String,
    },
    RejectFriend {
        user_id: String,
        user_name: String,
    },
    UndoFriend {
        user_id: String,
        user_name: String,
    },
    PersistThreads(u64),
    CheckForUpdates {
        silent: bool,
    },
    DownloadAndApplyUpdate {
        download_url: String,
        asset_name: String,
        target_version: String,
        expected_sha256: Option<String>,
    },
    Logout,
    /// Delete local message data (`zalo_history.db` + `threads_cache.json`) without
    /// touching credentials. Separate from [`WorkerCmd::Logout`] by design (#21).
    WipeLocalData,
}

fn report_session_save(session: &Session, gui_tx: &ScopedSender, ctx: &egui::Context) {
    if !gui_tx.current() {
        return;
    }
    let error = session.save_to_file("session.json").err().map(|_| {
        "Không lưu được phiên đăng nhập vào ổ đĩa; phiên hiện tại vẫn hoạt động. Kiểm tra thư mục dữ liệu và dung lượng trống trước khi thoát.".to_string()
    });
    if let Some(message) = &error {
        let _ = gui_tx.send(GuiEvent::StatusUpdate(message.clone()));
    }
    let _ = gui_tx.send(GuiEvent::PersistenceStatus {
        component: "session",
        error,
    });
    ctx.request_repaint();
}

pub(super) static PERSISTENCE: std::sync::OnceLock<persistence::LatestSnapshot<Vec<Thread>>> =
    std::sync::OnceLock::new();

pub(super) static WORKER_TX: std::sync::OnceLock<tokio::sync::mpsc::UnboundedSender<WorkerCmd>> =
    std::sync::OnceLock::new();

static WORKER_HANDLE: std::sync::OnceLock<std::sync::Mutex<Option<std::thread::JoinHandle<()>>>> =
    std::sync::OnceLock::new();
/// Join only after the native event loop has stopped, with a bounded wait.
pub fn join_worker_after_close() -> Result<(), String> {
    let Some(handle) = WORKER_HANDLE.get().and_then(|h| h.lock().ok()?.take()) else {
        return Ok(());
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while !handle.is_finished() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    if handle.is_finished() {
        handle
            .join()
            .map_err(|_| "Tác vụ nền kết thúc bất thường".into())
    } else {
        Err("Tác vụ nền chưa kết thúc; chưa khởi chạy bản cập nhật".into())
    }
}


pub(super) fn extract_msg_id(resp: &serde_json::Value) -> String {
    resp.get("data")
        .and_then(|d| d.get("msgId"))
        .and_then(|v| match v {
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Number(n) => Some(n.to_string()),
            _ => None,
        })
        .unwrap_or_default()
}

pub(super) fn resp_error_code(resp: &serde_json::Value) -> Option<i64> {
    resp.get("error_code")
        .and_then(serde_json::Value::as_i64)
        .filter(|c| *c != 0)
}

/// Immediately decode any disk-cached sticker thumbnails (`stickers_cache/stk_{cid}_{sid}.png`)
/// and push `GuiEvent::StickerImageReady` *before* sending sticker list metadata, returning any
/// missing `(key, url, cache_path)` entries for concurrent network prefetching.
pub(super) fn preload_disk_sticker_thumbnails(
    items: &[crate::chat::StickerItem],
    gui_tx: &ScopedSender,
    limit: usize,
) -> Vec<(String, String, std::path::PathBuf)> {
    let cache_dir = std::path::Path::new("stickers_cache");
    let _ = std::fs::create_dir_all(cache_dir);

    let mut missing = Vec::new();
    for item in items.iter().take(limit) {
        let key = format!("stk_{}_{}", item.cate_id, item.sticker_id);
        let cache_path = cache_dir.join(format!("{key}.png"));
        if let Ok(disk_bytes) = std::fs::read(&cache_path) {
            if let Ok(dyn_img) = image::load_from_memory(&disk_bytes) {
                let rgba = dyn_img.to_rgba8();
                let size = [rgba.width() as usize, rgba.height() as usize];
                let pixels = rgba.as_flat_samples();
                let color_img = egui::ColorImage::from_rgba_unmultiplied(size, pixels.as_slice());
                let _ = gui_tx.send(GuiEvent::StickerImageReady {
                    key,
                    image: color_img,
                });
                continue;
            } else {
                let _ = std::fs::remove_file(&cache_path);
            }
        }
        let url = if item.cate_id == 0 && (1..=30).contains(&item.sticker_id) {
            format!(
                "https://stc-chat.zdn.vn/images/stickers/default/thumb/{}.png",
                item.sticker_id
            )
        } else {
            format!(
                "https://api.zaloapp.com/api/emoticon/sticker/webpc?eid={}&size=130",
                item.sticker_id
            )
        };
        missing.push((key, url, cache_path));
    }
    missing
}

/// Concurrently download, cache, and decode missing sticker thumbnails so the popover and search
/// grid render smoothly without waiting for frame-by-frame lazy requests.
async fn prefetch_missing_sticker_thumbnails(
    http: &reqwest::Client,
    missing: Vec<(String, String, std::path::PathBuf)>,
    gui_tx: &ScopedSender,
    egui_ctx: &egui::Context,
) {
    if missing.is_empty() {
        return;
    }
    let futs = missing.into_iter().map(|(key, url, cache_path)| {
        let client = http.clone();
        let tx = gui_tx.clone();
        let ctx = egui_ctx.clone();
        async move {
            if let Ok(resp) = client.get(&url).send().await {
                if resp.status().is_success() {
                    if let Ok(b) = resp.bytes().await {
                        let vec = b.to_vec();
                        if let Ok(dyn_img) = image::load_from_memory(&vec) {
                            let _ = std::fs::write(&cache_path, &vec);
                            let rgba = dyn_img.to_rgba8();
                            let size = [rgba.width() as usize, rgba.height() as usize];
                            let pixels = rgba.as_flat_samples();
                            let color_img =
                                egui::ColorImage::from_rgba_unmultiplied(size, pixels.as_slice());
                            let _ = tx.send(GuiEvent::StickerImageReady {
                                key,
                                image: color_img,
                            });
                            ctx.request_repaint();
                        }
                    }
                }
            }
        }
    });
    futures_util::future::join_all(futs).await;
}

pub(super) fn spawn_session_worker(
    session: Session,
    gui_tx: Sender<GuiEvent>,
    egui_ctx: egui::Context,
) {
    crate::runtime_state::registry().bind(
        session
            .login
            .as_ref()
            .map(|l| l.uin.as_str())
            .unwrap_or_default(),
    );
    let gui_tx = ScopedSender {
        raw: gui_tx,
        scope: None,
        #[cfg(test)]
        passthrough: false,
    };
    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel::<WorkerCmd>();
    let _ = WORKER_TX.set(cmd_tx);

    let session = std::sync::Arc::new(tokio::sync::Mutex::new(session));
    let poll_state =
        std::sync::Arc::new(tokio::sync::Mutex::new(crate::chat::PollState::default()));
    let ctrl_poll_state = std::sync::Arc::new(tokio::sync::Mutex::new(
        crate::call::CtrlPollState::default(),
    ));

    let handle = std::thread::Builder::new()
        .name("zalo-worker".into())
        .spawn(move || {
            // Lightweight 2-thread async runtime to keep memory & CPU footprint minimal
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap();

            rt.block_on(async move {
                {
                    let s = session.lock().await.clone();
                    let _ = crate::login::get_server_info(&s).await;
                }
                {
                    let s_kw = session.clone();
                    let gui_tx_kw = gui_tx.clone();
                    let ctx_kw = egui_ctx.clone();
                    tokio::spawn(async move {
                        let s = s_kw.lock().await.clone();
                        if s.login.is_some() {
                            if let Some(tx) = WORKER_TX.get() {
                                let _ = tx.send(WorkerCmd::RefreshThreads);
                            }
                            if let Ok(kws) = crate::chat::fetch_sticker_suggest_keywords(&s).await {
                                if !kws.is_empty() {
                                    let _ = gui_tx_kw
                                        .send(GuiEvent::StickerSuggestKeywordsLoaded(kws));
                                    ctx_kw.request_repaint();
                                }
                            }
                        }
                    });
                }

                let call_bridge = std::sync::Arc::new(crate::call::CallBridge::spawn(
                    session.clone(),
                    gui_tx.raw.clone(),
                    egui_ctx.clone(),
                ));

                // 1) Real-time `zpw_ws` WebSocket loop for instant chat messages + VoIP events
                {
                    let s_ws = session.clone();
                    let cb = call_bridge.clone();
                    let gui_tx_ws = gui_tx.clone();
                    let ctx_ws = egui_ctx.clone();
                    tokio::spawn(async move {
                        let mut ws_rx: Option<
                            tokio::sync::mpsc::UnboundedReceiver<crate::socket::WsEvent>,
                        > = None;
                        let mut ws_sek = String::new();
                        loop {
                            let s_snap = s_ws.lock().await.clone();
                            if s_snap.login.is_some() {
                                let cur_sek = s_snap.zpw_sek.clone().unwrap_or_default();
                                if !cur_sek.is_empty() && (ws_rx.is_none() || ws_sek != cur_sek) {
                                    ws_sek = cur_sek;
                                    ws_rx = Some(crate::socket::spawn(s_snap.clone()));
                                }
                                if let Some(rx) = ws_rx.as_mut() {
                                    match tokio::time::timeout(
                                        Duration::from_millis(1500),
                                        rx.recv(),
                                    )
                                    .await
                                    {
                                        Ok(Some(ev)) => {
                                            let data_node = ev
                                                .payload
                                                .get("data")
                                                .unwrap_or(&ev.payload);
                                            for voip_ev in
                                                crate::call::extract_voip_controls(data_node)
                                            {
                                                cb.handle_control_event(voip_ev);
                                            }
                                            if ev.is_chat_ish() {
                                                let mut ws_threads = Vec::new();
                                                if let Some(li) = s_snap.login.as_ref() {
                                                    if !li.send2me_id.is_empty() {
                                                        ws_threads.push(Thread {
                                                            presentation: Default::default(),
                                                            unread: Default::default(),
                                                            id: li.send2me_id.clone(),
                                                            name: "Cloud của tôi (Lưu trữ)"
                                                                .into(),
                                                            preview: String::new(),
                                                            time: String::new(),
                                                            avatar: None,
                                                            is_group: false,
                                                            messages: Vec::new(),
                                                        });
                                                    }
                                                }
                                                crate::chat::merge_previews(
                                                    &mut ws_threads,
                                                    &ev.payload,
                                                );
                                                ws_threads.retain(|t| !t.messages.is_empty());
                                                if !ws_threads.is_empty() {
                                                    crate::chat::resolve_thread_names(
                                                        &s_snap,
                                                        &mut ws_threads,
                                                    )
                                                    .await;
                                                    let _ = gui_tx_ws.send(
                                                        GuiEvent::ThreadsUpdated(ws_threads),
                                                    );
                                                    ctx_ws.request_repaint();
                                                }
                                            }
                                        }
                                        Ok(None) => {
                                            ws_rx = None;
                                            tokio::time::sleep(Duration::from_secs(2)).await;
                                        }
                                        Err(_) => {}
                                    }
                                } else {
                                    tokio::time::sleep(Duration::from_secs(2)).await;
                                }
                            } else {
                                ws_rx = None;
                                ws_sek.clear();
                                tokio::time::sleep(Duration::from_secs(3)).await;
                            }
                        }
                    });
                }

                // 2) Background HTTP long-poll fallback for VoIP control events (`lp_control`)
                {
                    let s_ctrl = session.clone();
                    let cps = ctrl_poll_state.clone();
                    let cb = call_bridge.clone();
                    tokio::spawn(async move {
                        loop {
                            let s_snap = s_ctrl.lock().await.clone();
                            if s_snap.login.is_some() {
                                let voip_res = {
                                    let mut st = cps.lock().await;
                                    crate::call::poll_voip_controls(&s_snap, &mut st).await
                                };
                                if let Ok(events) = voip_res {
                                    for ev in events {
                                        cb.handle_control_event(ev);
                                    }
                                } else {
                                    tokio::time::sleep(Duration::from_secs(5)).await;
                                }
                            } else {
                                tokio::time::sleep(Duration::from_secs(3)).await;
                            }
                        }
                    });
                }

                let mut current_qr_task: Option<tokio::task::JoinHandle<()>> = None;

                let mut failed_reads: std::collections::HashMap<(String, String), Vec<ChatMessage>> = Default::default();
                while let Some(cmd) = cmd_rx.recv().await {
                    let gui_tx = gui_tx.refreshed();
                    match cmd {
                        WorkerCmd::Shutdown { account, threads, draft } => {
                            // Fence producers first, then drain submitted blocking work before the final FIFO snapshot.
                            {
                                let _fence = storage::mutation_fence().lock().unwrap_or_else(|e| e.into_inner());
                                crate::runtime_state::registry().invalidate();
                            }
                            if !cancel_account_jobs().await {
                                let _ = gui_tx.raw.send(GuiEvent::ShutdownComplete { error: Some("Tác vụ ghi chưa kết thúc; ứng dụng vẫn mở. Hãy thử lại.".into()) });
                                crate::runtime_state::registry().bind(&account);
                                egui_ctx.request_repaint();
                                continue;
                            }
                            if let Some(task) = current_qr_task.take() { task.abort(); }
                            let read_deltas = failed_reads.iter().filter(|((owner, _), _)| owner == &account).map(|((_, thread), messages)| (thread.clone(), messages.clone())).collect();
                            let success = flush_shutdown_and_ack(std::path::PathBuf::from("."), account, threads, read_deltas, draft, gui_tx.raw.clone()).await;
                            egui_ctx.request_repaint();
                            if success { break; }
                            let account = session.lock().await.login.as_ref().map(|l| l.uin.clone()).unwrap_or_default();
                            crate::runtime_state::registry().bind(&account);
                        }

                        WorkerCmd::LocalReadDelta { account, thread_id, messages } => {
                            // FIFO command completion precedes subsequent GUI snapshots and logout.
                            let persistence_session = session.lock().await;
                            if persistence_session.login.as_ref().is_none_or(|login| login.uin != account) { continue; }
                            let retry_key = (account.clone(), thread_id.clone());
                            let mut messages = messages;
                            if let Some(previous) = failed_reads.remove(&retry_key) { messages.extend(previous); }
                            let retry_messages = messages.clone();
                            let result = tracked_blocking(move || {
                                storage::open_account_store(&account)
                                    .and_then(|mut store| store.read_local_visible(&thread_id, &messages))
                            }).await;
                            drop(persistence_session);
                            if matches!(result, Ok(Ok(_))) { failed_reads.remove(&retry_key); } else { failed_reads.insert(retry_key, retry_messages); }
                            let error = (!matches!(result, Ok(Ok(_)))).then(|| "Chưa lưu được trạng thái đọc local; trạng thái trong bộ nhớ vẫn được giữ.".to_string());
                            if let Some(message) = &error { let _ = gui_tx.send(GuiEvent::StatusUpdate(message.clone())); }
                            let _ = gui_tx.send(GuiEvent::PersistenceStatus { component:"local-read", error });
                            egui_ctx.request_repaint();
                        }

                        WorkerCmd::LoadLocalPage{thread_id,cursor,direction,replace,request_id}=>{
                            let tx=gui_tx.clone();let ctx=egui_ctx.clone();let message_session=session.lock().await.clone();let account=message_session.login.as_ref().map(|l|l.uin.clone()).unwrap_or_default();
                            spawn_scoped(gui_tx.clone(),async move{let tid=thread_id.clone();let result=tracked_blocking(move||storage::load_page(&account,&tid,cursor.as_ref(),direction)).await;
                                match result {Ok(Ok((messages,has_more)))=>{let markup=messages.clone();let _=tx.send(GuiEvent::LocalPage{thread_id:thread_id.clone(),messages,has_more,replace,request_id});resolve_page_mentions(tx.clone(),message_session,ctx.clone(),thread_id,request_id,markup);},_=>{let _=tx.send(GuiEvent::StatusUpdate("Tải lịch sử local thất bại".into()));}}ctx.request_repaint();});
                        }

                        WorkerCmd::NotifyBackground{opaque_context,sound}=>{
                            spawn_scoped(gui_tx.clone(),async move{let context=crate::native_notifications::NotificationContext::new(&opaque_context);let _=crate::native_notifications::notify_background(&context,sound).await;});
                        }

                        WorkerCmd::RequestNotificationAuthorization=>{
                            let tx=gui_tx.clone();let ctx=egui_ctx.clone();spawn_scoped(gui_tx.clone(),async move{let status=crate::native_notifications::request_if_user_action(true).await;let _=tx.send(GuiEvent::StatusUpdate(match status{Ok(s)=>format!("Quyền thông báo macOS: {s:?}"),Err(_)=>"Chưa xác nhận quyền thông báo macOS".into()}));ctx.request_repaint();});
                        }

                        WorkerCmd::PersistThreads(ticket) => {
                            let Some((account, threads_snapshot)) = PERSISTENCE.get_or_init(Default::default).take(ticket) else { continue; };
                            // Tickets left behind by logout/account changes must never recreate old data.
                            let persistence_session = session.lock().await;
                            if persistence_session.login.as_ref().is_none_or(|login| login.uin != account) {
                                continue;
                            }
                            let gui_tx_persist = gui_tx.clone();
                            let ctx_persist = egui_ctx.clone();
                            // Serialize snapshots and logout behind completed disk writes.
                            let persistence = tracked_blocking(move || {
                                let database_result = storage::open_account_store(&account)
                                    .and_then(|mut store| store.persist_threads(&threads_snapshot));
                                let error = database_result.is_err().then(|| {
                                    "Không lưu được lịch sử vào ổ đĩa; tin nhắn hiện vẫn còn trong bộ nhớ. Kiểm tra thư mục dữ liệu và dung lượng trống.".to_string()
                                });
                                if let Some(message) = &error {
                                    let _ = gui_tx_persist.send(GuiEvent::StatusUpdate(message.clone()));
                                }
                                let _ = gui_tx_persist.send(GuiEvent::PersistenceStatus { component: "history", error });
                                ctx_persist.request_repaint();
                            }).await;
                            drop(persistence_session);
                            if persistence.is_err() {
                                let message = "Không lưu được lịch sử: tác vụ ghi dữ liệu bị gián đoạn; tin nhắn hiện vẫn còn trong bộ nhớ.".to_string();
                                let _ = gui_tx.send(GuiEvent::StatusUpdate(message.clone()));
                                let _ = gui_tx.send(GuiEvent::PersistenceStatus { component: "history", error: Some(message) });
                                egui_ctx.request_repaint();
                            }
                        }
                        WorkerCmd::Logout => {
                            PERSISTENCE.get_or_init(Default::default).invalidate();
                            if let Some(task) = current_qr_task.take() {
                                task.abort();
                            }
                            // Credentials and the account-scoped thread cache only:
                            // message history is removed by `WipeLocalData` (#21).
                            let _ = std::fs::remove_file("session.json");


                            {
                                let mut s = session.lock().await;
                                s.logout();
                            }
                            {
                                let mut ps = poll_state.lock().await;
                                *ps = crate::chat::PollState::default();
                            }
                            let _ = gui_tx.send(GuiEvent::LoggedOut);
                            egui_ctx.request_repaint();
                        }
                        WorkerCmd::WipeLocalData => {
                            let account = session.lock().await.login.as_ref().map(|l| l.uin.clone()).unwrap_or_default();
                            if account.is_empty() { continue; }
                            {
                                let _fence = storage::mutation_fence().lock().unwrap_or_else(|e| e.into_inner());
                                crate::runtime_state::registry().invalidate();
                            }
                            PERSISTENCE.get_or_init(Default::default).invalidate();
                            if !cancel_account_jobs().await {
                                let _ = gui_tx.raw.send(GuiEvent::LocalDataWipeFailed);
                                let _ = gui_tx.raw.send(GuiEvent::StatusUpdate("Tác vụ ghi chưa kết thúc; dữ liệu được giữ nguyên. Hãy thử lại.".into()));
                                crate::runtime_state::registry().bind(&account);
                                egui_ctx.request_repaint();
                                continue;
                            }
                            let owner = account.clone();
                            let result = tracked_blocking(move || storage::wipe_account(std::path::Path::new("."), &owner)).await;
                            while cmd_rx.try_recv().is_ok() {}
                            crate::runtime_state::registry().bind(&account);
                            if matches!(result, Ok(Ok(()))) {
                                failed_reads.retain(|(owner, _), _| owner != &account);
                                *poll_state.lock().await = crate::chat::PollState::default();
                                let _ = gui_tx.raw.send(GuiEvent::LocalDataWiped);
                            } else {
                                let _ = gui_tx.raw.send(GuiEvent::LocalDataWipeFailed);
                                let _ = gui_tx.raw.send(GuiEvent::StatusUpdate("Xóa dữ liệu chưa hoàn tất; hãy kiểm tra ổ đĩa và thử lại.".into()));
                            }
                            egui_ctx.request_repaint();
                        }
                        WorkerCmd::RequestQr => {
                            crate::runtime_state::registry().invalidate();
                            PERSISTENCE.get_or_init(Default::default).invalidate();
                            if let Some(task) = current_qr_task.take() {
                                task.abort();
                            }
                            let session_clone = session.clone();
                            let poll_clone = poll_state.clone();
                            let gui_tx_clone = gui_tx.clone();
                            let ctx_clone = egui_ctx.clone();

                            let handle = spawn_scoped(gui_tx.clone(), async move {
                                {
                                    let mut s = session_clone.lock().await;
                                    if !gui_tx_clone.current(){return;}
                                    s.logout();
                                }
                                {
                                    let mut ps = poll_clone.lock().await;
                                    *ps = crate::chat::PollState::default();
                                }
                                let _ = gui_tx_clone
                                    .send(GuiEvent::StatusUpdate("Đang xin mã QR mới…".into()));
                                ctx_clone.request_repaint();

                                let s_snap = { session_clone.lock().await.clone() };
                                let q_res = crate::qr::request_qr(&s_snap, "vi").await;

                                match q_res {
                                    Ok(q) => {
                                        if let Some(payload) = q.payload() {
                                            if let Ok(code) = qrcode::QrCode::new(payload.as_bytes())
                                            {
                                                let w = code.width();
                                                let quiet_zone = 4;
                                                let total_w = w + quiet_zone * 2;
                                                let mut pixels =
                                                    Vec::with_capacity(total_w * total_w * 4);
                                                for y in 0..total_w {
                                                    for x in 0..total_w {
                                                        let in_qr = x >= quiet_zone
                                                            && x < quiet_zone + w
                                                            && y >= quiet_zone
                                                            && y < quiet_zone + w;
                                                        let dark = in_qr
                                                            && (code[(
                                                                x - quiet_zone,
                                                                y - quiet_zone,
                                                            )] == qrcode::Color::Dark);
                                                        let c = if dark {
                                                            [15, 23, 42, 255]
                                                        } else {
                                                            [255, 255, 255, 255]
                                                        };
                                                        pixels.extend_from_slice(&c);
                                                    }
                                                }
                                                let img = egui::ColorImage::from_rgba_unmultiplied(
                                                    [total_w, total_w],
                                                    &pixels,
                                                );
                                                let _ = gui_tx_clone.send(GuiEvent::QrReady {
                                                    qr_login: q.clone(),
                                                    qr_image: img,
                                                });
                                                ctx_clone.request_repaint();

                                                let scan_res =
                                                    crate::qr::wait_for_scan(&s_snap, &q).await;

                                                if let Ok(scanned) = scan_res {
                                                    let name = scanned
                                                        .get("display_name")
                                                        .and_then(|v| v.as_str())
                                                        .unwrap_or("Zalo User");
                                                    let _ =
                                                        gui_tx_clone.send(GuiEvent::QrScanned {
                                                            name: name.to_string(),
                                                        });
                                                    ctx_clone.request_repaint();

                                                    let confirm_res =
                                                        crate::qr::confirm(&s_snap, &q).await;

                                                    if let Ok(resp) = confirm_res {
                                                        let mut s = session_clone.lock().await;
                                    if !gui_tx_clone.current(){return;}
                                                        if let Some(sek) =
                                                            crate::qr::extract_zpw_sek(&resp)
                                                        {
                                                            s.set_zpw_sek(sek);
                                                        }
                                                        if let Some(data) = resp.get("data") {
                                                            crate::login::apply_login_config(
                                                                &mut s, data,
                                                            );
                                                        }
                                                        report_session_save(&s, &gui_tx_clone, &ctx_clone);

                                                        if let Ok(info) =
                                                            crate::login::get_login_info(&s).await
                                                        {
                                                            if let Some(cfg) = info.get("data") {
                                                                crate::login::apply_login_config(
                                                                    &mut s, cfg,
                                                                );
                                                                s.apply_server_info(cfg);
                                                            }
                                                        }
                                                        let _ = s.sync_profile_me().await;
                                                        report_session_save(&s, &gui_tx_clone, &ctx_clone);

                                                        let uin = s
                                                            .login
                                                            .as_ref()
                                                            .map(|l| l.uin.clone())
                                                            .unwrap_or_default();
                                                        let dname = s
                                                            .login
                                                            .as_ref()
                                                            .map(|l| l.display_name.clone())
                                                            .unwrap_or_default();
                                                        let u_ava = s
                                                            .login
                                                            .as_ref()
                                                            .map(|l| l.avatar.clone())
                                                            .filter(|a| !a.is_empty());
                                                        let _ = gui_tx_clone.send(
                                                            GuiEvent::LoginSuccess {
                                                                name: dname,
                                                                uin,
                                                                avatar: u_ava,
                                                            },
                                                        );
                                                        ctx_clone.request_repaint();

                                                        let bootstrap_session = s.clone();
                                                        drop(s);
                                                        let s = bootstrap_session;
                                                        let mut list = crate::chat::get_friends_remote(&s)
                                                            .await
                                                            .unwrap_or_default();

                                                        {
                                                            let mut ps = poll_clone.lock().await;
                                                            let _ = crate::chat::poll_new_messages_remote(
                                                                &s, &mut ps, &mut list,
                                                            )
                                                            .await;
                                                        }
                                                        let _ = gui_tx_clone
                                                            .send(GuiEvent::ThreadsUpdated(list));
                                                        ctx_clone.request_repaint();

                                                        let s_kw = s.clone();
                                                        let tx_kw = gui_tx_clone.clone();
                                                        let ctx_kw = ctx_clone.clone();
                                                        spawn_scoped(gui_tx.clone(), async move {
                                                            if let Ok(kws) = crate::chat::fetch_sticker_suggest_keywords(&s_kw).await {
                                                                if !kws.is_empty() {
                                                                    let _ = tx_kw.send(GuiEvent::StickerSuggestKeywordsLoaded(kws));
                                                                    ctx_kw.request_repaint();
                                                                }
                                                            }
                                                        });
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        let _ = gui_tx_clone
                                            .send(GuiEvent::LoginFailed(e.to_string()));
                                        ctx_clone.request_repaint();
                                    }
                                }
                            });
                            current_qr_task = Some(handle);
                        }
                        WorkerCmd::RefreshThreads => {
                            let session_clone = session.clone();
                            let poll_clone = poll_state.clone();
                            let gui_tx_clone = gui_tx.clone();
                            let ctx_clone = egui_ctx.clone();
                            tokio::spawn(async move {
                                let mut s = session_clone.lock().await.clone();
                                if s.login.is_some() {
                                    match s.session_status().await {
                                        crate::session::SessionStatus::Valid => {}
                                        crate::session::SessionStatus::Expired => {
                                            // Keep `session.json`: the old credentials stay
                                            // readable until a new login replaces the file (#21).
                                            {
                                                let mut sl = session_clone.lock().await;
                                                sl.mark_expired();
                                                let _ = sl.save_to_file("session.json");
                                                sl.logout();
                                            }
                                            let _ = gui_tx_clone.send(GuiEvent::SessionExpired(
                                                "Phiên đăng nhập (zpw_sek) đã hết hạn trên máy chủ Zalo"
                                                    .into(),
                                            ));
                                            ctx_clone.request_repaint();
                                            if let Some(tx) = WORKER_TX.get() {
                                                let _ = tx.send(WorkerCmd::RequestQr);
                                            }
                                            return;
                                        }
                                        crate::session::SessionStatus::Unreachable => {
                                            // Offline: keep the session, say so, and let the
                                            // next poll retry.
                                            crate::logger::log_error(
                                                "gui::refresh_threads",
                                                "không kết nối được máy chủ Zalo",
                                            );
                                            let _ = gui_tx_clone.send(GuiEvent::SessionUnreachable(
                                                "Không kết nối được máy chủ Zalo".into(),
                                            ));
                                            ctx_clone.request_repaint();
                                            return;
                                        }
                                    }
                                    if let Some((uid, name)) = s.sync_profile_me().await {
                                        {
                                            let mut sl = session_clone.lock().await;
                                            if let Some(ref mut li) = sl.login {
                                                li.uin = uid.clone();
                                                if !name.is_empty() {
                                                    li.display_name = name.clone();
                                                }
                                            }
                                            let _ = sl.save_to_file("session.json");
                                        }
                                        let u_ava = s
                                            .login
                                            .as_ref()
                                            .map(|l| l.avatar.clone())
                                            .filter(|a| !a.is_empty());
                                        let _ = gui_tx_clone.send(GuiEvent::LoginSuccess {
                                            name,
                                            uin: uid,
                                            avatar: u_ava,
                                        });
                                    }
                                    let _ = gui_tx_clone.send(GuiEvent::StatusUpdate(
                                        "⏳ Đang đồng bộ danh bạ & nhóm từ máy chủ Zalo…".into(),
                                    ));
                                    ctx_clone.request_repaint();
                                    let mut list = crate::chat::load_threads_cache();
                                    if let Ok(friends) = crate::chat::get_friends(&s).await {
                                        for ft in friends {
                                            if let Some(ex) = list.iter_mut().find(|t| t.id == ft.id) {
                                                if !ft.name.is_empty() {
                                                    ex.name = ft.name;
                                                }
                                                if crate::chat::should_replace_avatar(
                                                    ex.avatar.as_deref(),
                                                    ft.avatar.as_deref(),
                                                ) {
                                                    ex.avatar = ft.avatar;
                                                }
                                                ex.is_group = ex.is_group || ft.is_group;
                                            } else {
                                                list.push(ft);
                                            }
                                        }
                                    }
                                    if let Ok(groups) = crate::chat::get_groups(&s).await {
                                        for gt in groups {
                                            if let Some(ex) = list.iter_mut().find(|t| t.id == gt.id) {
                                                if !gt.name.is_empty() {
                                                    ex.name = gt.name;
                                                }
                                                if crate::chat::should_replace_avatar(
                                                    ex.avatar.as_deref(),
                                                    gt.avatar.as_deref(),
                                                ) {
                                                    ex.avatar = gt.avatar;
                                                }
                                                ex.is_group = true;
                                            } else {
                                                list.push(gt);
                                            }
                                        }
                                    }
                                    crate::store::load_history_into_threads(
                                        "zalo_history.db",
                                        &mut list,
                                        crate::store::MessageStore::PRELOAD_MSGS,
                                    );
                                    let _ = gui_tx_clone.send(GuiEvent::StatusUpdate(
                                        format!("⏳ Đang tải tin nhắn cho {} hội thoại…", list.len()),
                                    ));
                                    ctx_clone.request_repaint();
                                    let mut fresh_ps = crate::chat::PollState::default();
                                    let _ =
                                        crate::chat::poll_new_messages(&s, &mut fresh_ps, &mut list)
                                            .await;
                                    {
                                        let mut ps = poll_clone.lock().await;
                                        *ps = fresh_ps;
                                    }
                                    let total_msgs: usize =
                                        list.iter().map(|t| t.messages.len()).sum();
                                    let n_threads = list.len();
                                    let _ = gui_tx_clone.send(GuiEvent::ThreadsUpdated(list));
                                    let _ = gui_tx_clone.send(GuiEvent::StatusUpdate(format!(
                                        "✅ Đã đồng bộ {n_threads} hội thoại ({total_msgs} tin nhắn)"
                                    )));
                                    ctx_clone.request_repaint();
                                }
                            });
                        }
                        WorkerCmd::PollMessages { known_threads } => {
                            static IS_POLLING: std::sync::atomic::AtomicBool =
                                std::sync::atomic::AtomicBool::new(false);
                            if IS_POLLING.swap(true, std::sync::atomic::Ordering::SeqCst) {
                                continue;
                            }

                            let session_clone = session.clone();
                            let poll_clone = poll_state.clone();
                            let gui_tx_clone = gui_tx.clone();
                            let ctx_clone = egui_ctx.clone();
                            tokio::spawn(async move {
                                let s = session_clone.lock().await.clone();
                                if s.login.is_some() {
                                    let mut temp_threads: Vec<Thread> = known_threads
                                        .into_iter()
                                        .map(|(id, name, is_group)| Thread {
                                            presentation: Default::default(),
                                            unread: Default::default(),
                                            id,
                                            name,
                                            preview: String::new(),
                                            time: String::new(),
                                            avatar: None,
                                            is_group,
                                            messages: Vec::new(),
                                        })
                                        .collect();

                                    let mut poll_err: Option<anyhow::Error> = None;
                                    for _ in 0..3 {
                                        let poll_res = {
                                            let mut ps = poll_clone.lock().await;
                                            crate::chat::get_chat_from_polling(&s, &mut ps).await
                                        };
                                        match poll_res {
                                            Ok(lp_resp) => {
                                                crate::chat::merge_previews(
                                                    &mut temp_threads,
                                                    &lp_resp,
                                                );
                                                let data_node =
                                                    lp_resp.get("data").unwrap_or(&lp_resp);
                                                let more = data_node
                                                    .get("more")
                                                    .and_then(serde_json::Value::as_i64)
                                                    .unwrap_or(0);
                                                let has_reset = data_node
                                                    .get("resetLastActionId")
                                                    .and_then(serde_json::Value::as_str)
                                                    .map(|r| r != "0" && !r.is_empty())
                                                    .unwrap_or(false);
                                                if more == 0 && !has_reset {
                                                    break;
                                                }
                                            }
                                            Err(e) => {
                                                poll_err = Some(e);
                                                break;
                                            }
                                        }
                                    }

                                    match poll_err {
                                        Some(e) => {
                                            if crate::session::SessionStatus::from_error(&e)
                                                == crate::session::SessionStatus::Expired
                                            {
                                                {
                                                    let mut sl = session_clone.lock().await;
                                                    sl.mark_expired();
                                                    let _ = sl.save_to_file("session.json");
                                                    sl.logout();
                                                }
                                                let _ = gui_tx_clone.send(GuiEvent::SessionExpired(
                                                    "Phiên đăng nhập (zpw_sek) đã hết hạn".into(),
                                                ));
                                                ctx_clone.request_repaint();
                                                if let Some(tx) = WORKER_TX.get() {
                                                    let _ = tx.send(WorkerCmd::RequestQr);
                                                }
                                            } else {
                                                // The session is untouched: report and retry on
                                                // the next poll instead of logging the user out.
                                                crate::logger::log_error(
                                                    "gui::poll_messages",
                                                    &e.to_string(),
                                                );
                                                let _ = gui_tx_clone.send(
                                                    GuiEvent::SessionUnreachable(e.to_string()),
                                                );
                                                ctx_clone.request_repaint();
                                            }
                                        }
                                        None => {
                                            temp_threads.retain(|t| !t.messages.is_empty());
                                            if !temp_threads.is_empty() {
                                                crate::chat::resolve_thread_names(
                                                    &s,
                                                    &mut temp_threads,
                                                )
                                                .await;
                                                let _ = gui_tx_clone
                                                    .send(GuiEvent::ThreadsUpdated(temp_threads));
                                                ctx_clone.request_repaint();
                                            }
                                        }
                                    }
                                }
                                IS_POLLING.store(false, std::sync::atomic::Ordering::SeqCst);
                            });
                        }
                        WorkerCmd::LoadGroupHistory { group_id, is_group } => {
                            let session_clone = session.clone();
                            let gui_tx_clone = gui_tx.clone();
                            let ctx_clone = egui_ctx.clone();
                            tokio::spawn(async move {
                                let s = session_clone.lock().await.clone();
                                if s.login.is_some() {
                                    let is_send2me = s
                                        .login
                                        .as_ref()
                                        .map(|l| !l.send2me_id.is_empty() && l.send2me_id == group_id)
                                        .unwrap_or(false);
                                    let t = Thread {
                                        presentation: Default::default(),
                                        unread: Default::default(),
                                        id: group_id.clone(),
                                        name: String::new(),
                                        preview: String::new(),
                                        time: String::new(),
                                        avatar: None,
                                        is_group: is_group && !is_send2me,
                                        messages: Vec::new(),
                                    };
                                    let mut t_list = vec![t];
                                    crate::store::load_history_into_threads(
                                        "zalo_history.db",
                                        &mut t_list,
                                        500,
                                    );
                                    let Some(mut t) =
                                        t_list.into_iter().find(|x| x.id == group_id)
                                    else {
                                        return;
                                    };
                                    if !is_group && !is_send2me {
                                        let mut one_list = vec![t.clone()];
                                        if let Ok(resp) = crate::chat::get_last_msgs(
                                            &s,
                                            std::slice::from_ref(&group_id),
                                        )
                                        .await
                                        {
                                            crate::chat::merge_previews(&mut one_list, &resp);
                                        }
                                        if let Ok(resp_all) =
                                            crate::chat::get_last_msgs(&s, &[]).await
                                        {
                                            crate::chat::merge_previews(&mut one_list, &resp_all);
                                        }
                                        if let Some(updated) =
                                            one_list.into_iter().find(|x| x.id == group_id)
                                        {
                                            t = updated;
                                        }
                                    }
                                    if is_group || is_send2me {
                                        if let Ok(cm_resp) = crate::chat::get_cloud_messages(
                                            &s,
                                            &group_id,
                                            "9223372036854775808",
                                            false,
                                            50,
                                        )
                                        .await
                                        {
                                            crate::chat::merge_group_history(&mut t, &cm_resp);
                                        }
                                        // Backfill up to 5 pages (250 messages) of older cloud history
                                        let mut cursor_id = t
                                            .messages
                                            .first()
                                            .map(|m| m.id.as_str())
                                            .filter(|id| {
                                                !id.is_empty()
                                                    && id.chars().all(|c| c.is_ascii_digit())
                                            })
                                            .unwrap_or("9223372036854775808")
                                            .to_string();
                                        for page in 1..=5 {
                                            let prev_len = t.messages.len();
                                            let Ok(cm_old) = crate::chat::get_cloud_messages(
                                                &s,
                                                &group_id,
                                                &cursor_id,
                                                true,
                                                50,
                                            )
                                            .await
                                            else {
                                                break;
                                            };
                                            crate::chat::merge_group_history(&mut t, &cm_old);
                                            let new_len = t.messages.len();
                                            let _ = gui_tx_clone.send(GuiEvent::StatusUpdate(
                                                format!(
                                                    "⏳ Đang đồng bộ lịch sử cũ (trang {page}/5 — {new_len} tin nhắn)…"
                                                ),
                                            ));
                                            ctx_clone.request_repaint();
                                            let data_node = cm_old
                                                .pointer("/data/data")
                                                .or_else(|| cm_old.get("data"))
                                                .unwrap_or(&cm_old);
                                            let has_more = data_node
                                                .get("hasMore")
                                                .and_then(|v| v.as_i64())
                                                .unwrap_or(0);
                                            let next_cursor = data_node
                                                .get("lastMsgId")
                                                .and_then(|v| {
                                                    v.as_str().map(str::to_string).or_else(|| {
                                                        v.as_u64().map(|n| n.to_string())
                                                    })
                                                })
                                                .filter(|s| {
                                                    !s.is_empty()
                                                        && s != "0"
                                                        && s.chars().all(|c| c.is_ascii_digit())
                                                })
                                                .or_else(|| {
                                                    t.messages
                                                        .first()
                                                        .map(|m| m.id.clone())
                                                        .filter(|id| {
                                                            !id.is_empty()
                                                                && id
                                                                    .chars()
                                                                    .all(|c| c.is_ascii_digit())
                                                        })
                                                });
                                            let Some(nc) = next_cursor else {
                                                break;
                                            };
                                            if has_more == 0 || nc == cursor_id || new_len == prev_len {
                                                break;
                                            }
                                            cursor_id = nc;
                                        }
                                    }
                                    if is_group
                                        && !is_send2me
                                        && (crate::chat::is_generic_thread_name(&t.name)
                                            || t.avatar.as_deref().unwrap_or("").is_empty())
                                    {
                                        crate::chat::resolve_thread_names(
                                            &s,
                                            std::slice::from_mut(&mut t),
                                        )
                                        .await;
                                    }
                                    let cnt = t.messages.len();
                                    if !t.messages.is_empty() || !t.name.is_empty() {
                                        if let Ok(mut store) =
                                            crate::store::MessageStore::open("zalo_history.db")
                                        {
                                            let _ = crate::store::persist_threads(
                                                &mut store,
                                                std::slice::from_ref(&t),
                                            );
                                        }
                                        let _ = gui_tx_clone.send(GuiEvent::ThreadsUpdated(vec![t]));
                                    }
                                    let _ = gui_tx_clone.send(GuiEvent::StatusUpdate(format!(
                                        "✅ Đã đồng bộ {cnt} tin nhắn lịch sử"
                                    )));
                                    ctx_clone.request_repaint();
                                }
                            });
                        }
                        WorkerCmd::SendMessage {
                            to_id,
                            text,
                            is_group,
                        } => {
                            let session_clone = session.clone();
                            let gui_tx_clone = gui_tx.clone();
                            let ctx_clone = egui_ctx.clone();
                            tokio::spawn(async move {
                                let s = session_clone.lock().await.clone();
                                if let Some(found_url) = crate::chat::extract_first_url(&text) {
                                    let meta = crate::chat::parse_link_metadata(&s, &found_url)
                                        .await
                                        .unwrap_or_else(|_| crate::chat::LinkMetadata {
                                            url: found_url.clone(),
                                            title: found_url.clone(),
                                            desc: String::new(),
                                            thumb: String::new(),
                                            src: "Liên kết".into(),
                                        });
                                    let _ =
                                        gui_tx_clone.send(GuiEvent::LinkPreviewReady(meta.clone()));
                                    match crate::chat::send_link_message(
                                        &s, &to_id, &text, &meta, is_group,
                                    )
                                    .await
                                    {
                                        Ok(resp) => {
                                            let msg_id = extract_msg_id(&resp);
                                            let now = crate::ui::clock();
                                            let final_id = if msg_id.is_empty() {
                                                format!("out_{}", crate::login::unix_now())
                                            } else {
                                                msg_id
                                            };
                                            let msg = ChatMessage {
                                                id: final_id,
                                                sender: "Tôi".into(),
                                                text,
                                                time: now,
                                                is_outgoing: true,
                                                ts: (crate::login::unix_now() * 1000) as i64,
                                                link_url: Some(meta.url),
                                                link_title: if meta.title.is_empty() {
                                                    None
                                                } else {
                                                    Some(meta.title)
                                                },
                                                link_desc: if meta.desc.is_empty() {
                                                    None
                                                } else {
                                                    Some(meta.desc)
                                                },
                                                link_thumb: if meta.thumb.is_empty() {
                                                    None
                                                } else {
                                                    Some(meta.thumb)
                                                },
                                                link_src: if meta.src.is_empty() {
                                                    None
                                                } else {
                                                    Some(meta.src)
                                                },
                                                ..Default::default()
                                            };
                                            let _ = gui_tx_clone.send(GuiEvent::AttachmentSent {
                                                thread_id: to_id,
                                                msg,
                                            });
                                            ctx_clone.request_repaint();
                                            return;
                                        }
                                        Err(e) => {
                                            let err_s = e.to_string();
                                            crate::logger::log_error("gui::send_link", &err_s);
                                            let _ = gui_tx_clone.send(GuiEvent::SendFailed {
                                                thread_id: to_id,
                                                text,
                                                error: err_s,
                                            });
                                            ctx_clone.request_repaint();
                                            return;
                                        }
                                    }
                                }

                                let res = if is_group {
                                    crate::chat::send_group_message(&s, &to_id, &text).await
                                } else {
                                    crate::chat::send_message(&s, &to_id, &text).await
                                };
                                match res {
                                    Ok(resp) => {
                                        let msg_id = extract_msg_id(&resp);
                                        let _ = gui_tx_clone.send(GuiEvent::MessageSent {
                                            thread_id: to_id,
                                            text,
                                            msg_id,
                                        });
                                        ctx_clone.request_repaint();
                                    }
                                    Err(e) => {
                                        let err_s = e.to_string();
                                        crate::logger::log_error("gui::send_message", &err_s);
                                        let _ = gui_tx_clone.send(GuiEvent::SendFailed {
                                            thread_id: to_id,
                                            text,
                                            error: err_s.clone(),
                                        });
                                        if crate::session::SessionStatus::from_error(&e)
                                            == crate::session::SessionStatus::Expired
                                        {
                                            {
                                                let mut sl = session_clone.lock().await;
                                                sl.mark_expired();
                                                let _ = sl.save_to_file("session.json");
                                                sl.logout();
                                            }
                                            let _ = gui_tx_clone.send(GuiEvent::SessionExpired(
                                                "Không thể gửi vì phiên zpw_sek đã hết hạn".into(),
                                            ));
                                            if let Some(tx) = WORKER_TX.get() {
                                                let _ = tx.send(WorkerCmd::RequestQr);
                                            }
                                        }
                                        ctx_clone.request_repaint();
                                    }
                                }
                            });
                        }
                        WorkerCmd::SendQuote {
                            to_id,
                            text,
                            quoted,
                            is_group,
                        } => {
                            let session_clone = session.clone();
                            let gui_tx_clone = gui_tx.clone();
                            let ctx_clone = egui_ctx.clone();
                            tokio::spawn(async move {
                                let s = session_clone.lock().await.clone();
                                match crate::chat::send_quote_message(
                                    &s, &to_id, &text, &quoted, is_group,
                                )
                                .await
                                {
                                    Ok(resp) => {
                                        let _ = gui_tx_clone.send(GuiEvent::QuoteSent {
                                            thread_id: to_id,
                                            text,
                                            msg_id: extract_msg_id(&resp),
                                            quoted,
                                        });
                                    }
                                    Err(e) => {
                                        let err_s = e.to_string();
                                        crate::logger::log_error("gui::send_quote", &err_s);
                                        let _ = gui_tx_clone.send(GuiEvent::SendFailed {
                                            thread_id: to_id,
                                            text,
                                            error: err_s,
                                        });
                                    }
                                }
                                ctx_clone.request_repaint();
                            });
                        }
                        WorkerCmd::Forward {
                            target_id,
                            target_is_group,
                            msg,
                        } => {
                            let session_clone = session.clone();
                            let gui_tx_clone = gui_tx.clone();
                            let ctx_clone = egui_ctx.clone();
                            tokio::spawn(async move {
                                let s = session_clone.lock().await.clone();
                                let res = crate::chat::forward_message(
                                    &s,
                                    &target_id,
                                    target_is_group,
                                    &msg,
                                )
                                .await;
                                match res {
                                    Ok(resp) if resp_error_code(&resp).is_none() => {
                                        let msg_id = extract_msg_id(&resp);
                                        if let Some(sid) = msg.sticker_id.filter(|v| *v > 0) {
                                            let _ = gui_tx_clone.send(GuiEvent::StickerSent {
                                                thread_id: target_id,
                                                msg_id,
                                                sticker_id: sid,
                                                cate_id: msg.sticker_cate_id.unwrap_or(0),
                                                sticker_type: msg.sticker_type.unwrap_or(7),
                                            });
                                        } else {
                                            let _ = gui_tx_clone.send(GuiEvent::MessageSent {
                                                thread_id: target_id,
                                                text: msg.text.clone(),
                                                msg_id,
                                            });
                                        }
                                        let _ = gui_tx_clone.send(GuiEvent::StatusUpdate(
                                            "Đã chuyển tiếp tin nhắn".into(),
                                        ));
                                    }
                                    Ok(resp) => {
                                        let err_s = format!(
                                            "server error {}",
                                            resp_error_code(&resp).unwrap_or(0)
                                        );
                                        crate::logger::log_error("gui::forward", &err_s);
                                        let _ = gui_tx_clone.send(GuiEvent::StatusUpdate(format!(
                                            "Chuyển tiếp thất bại: {err_s}"
                                        )));
                                    }
                                    Err(e) => {
                                        crate::logger::log_error("gui::forward", &e.to_string());
                                        let _ = gui_tx_clone.send(GuiEvent::StatusUpdate(format!(
                                            "Chuyển tiếp thất bại: {e}"
                                        )));
                                    }
                                }
                                ctx_clone.request_repaint();
                            });
                        }
                        WorkerCmd::React {
                            to_id,
                            is_group,
                            msg,
                            reaction,
                        } => {
                            let session_clone = session.clone();
                            let gui_tx_clone = gui_tx.clone();
                            let ctx_clone = egui_ctx.clone();
                            tokio::spawn(async move {
                                let s = session_clone.lock().await.clone();
                                let res = crate::chat::send_reaction(
                                    &s, &to_id, is_group, &msg, &reaction,
                                )
                                .await;
                                match res {
                                    Ok(resp) if resp_error_code(&resp).is_none() => {
                                        let _ = gui_tx_clone.send(GuiEvent::Reacted {
                                            thread_id: to_id,
                                            msg_id: msg.id.clone(),
                                            ts: msg.ts,
                                            reaction,
                                        });
                                    }
                                    Ok(resp) => {
                                        let err_s = format!(
                                            "server error {}",
                                            resp_error_code(&resp).unwrap_or(0)
                                        );
                                        crate::logger::log_error("gui::react", &err_s);
                                        let _ = gui_tx_clone.send(GuiEvent::StatusUpdate(format!(
                                            "Thả cảm xúc thất bại: {err_s}"
                                        )));
                                    }
                                    Err(e) => {
                                        crate::logger::log_error("gui::react", &e.to_string());
                                        let _ = gui_tx_clone.send(GuiEvent::StatusUpdate(format!(
                                            "Thả cảm xúc thất bại: {e}"
                                        )));
                                    }
                                }
                                ctx_clone.request_repaint();
                            });
                        }
                        WorkerCmd::DeleteForMe {
                            to_id,
                            is_group,
                            msg,
                        } => {
                            let session_clone = session.clone();
                            let gui_tx_clone = gui_tx.clone();
                            let ctx_clone = egui_ctx.clone();
                            tokio::spawn(async move {
                                let s = session_clone.lock().await.clone();
                                let local_only = msg.id.is_empty()
                                    || msg.id.starts_with("out_")
                                    || msg.id.starts_with("local_");
                                let res: anyhow::Result<serde_json::Value> = if local_only {
                                    Ok(serde_json::json!({}))
                                } else {
                                    crate::chat::delete_message_for_me(&s, &to_id, is_group, &msg)
                                        .await
                                };
                                match res {
                                    Ok(resp) if resp_error_code(&resp).is_none() => {
                                        let (tid, mid, ts) =
                                            (to_id.clone(), msg.id.clone(), msg.ts);
                                        let _ = tokio::task::spawn_blocking(move || {
                                            if let Ok(mut store) =
                                                crate::store::MessageStore::open("zalo_history.db")
                                            {
                                                let _ = store.delete_message(&tid, &mid, ts);
                                            }
                                        })
                                        .await;
                                        let _ = gui_tx_clone.send(GuiEvent::MessageDeleted {
                                            thread_id: to_id,
                                            msg_id: msg.id.clone(),
                                            ts: msg.ts,
                                        });
                                    }
                                    Ok(resp) => {
                                        let err_s = format!(
                                            "server error {}",
                                            resp_error_code(&resp).unwrap_or(0)
                                        );
                                        crate::logger::log_error("gui::delete", &err_s);
                                        let _ = gui_tx_clone.send(GuiEvent::StatusUpdate(format!(
                                            "Xóa tin nhắn thất bại: {err_s}"
                                        )));
                                    }
                                    Err(e) => {
                                        crate::logger::log_error("gui::delete", &e.to_string());
                                        let _ = gui_tx_clone.send(GuiEvent::StatusUpdate(format!(
                                            "Xóa tin nhắn thất bại: {e}"
                                        )));
                                    }
                                }
                                ctx_clone.request_repaint();
                            });
                        }
                        WorkerCmd::RecallMessage {
                            to_id,
                            is_group,
                            msg,
                        } => {
                            let session_clone = session.clone();
                            let gui_tx_clone = gui_tx.clone();
                            let ctx_clone = egui_ctx.clone();
                            tokio::spawn(async move {
                                let s = session_clone.lock().await.clone();
                                let local_only = msg.id.is_empty()
                                    || msg.id.starts_with("out_")
                                    || msg.id.starts_with("local_");
                                let res: anyhow::Result<serde_json::Value> = if local_only {
                                    Ok(serde_json::json!({}))
                                } else {
                                    crate::chat::recall_message(&s, &to_id, is_group, &msg).await
                                };
                                match res {
                                    Ok(resp) if resp_error_code(&resp).is_none() => {
                                        let _ = gui_tx_clone.send(GuiEvent::MessageRecalled {
                                            thread_id: to_id,
                                            msg_id: msg.id.clone(),
                                            ts: msg.ts,
                                        });
                                    }
                                    Ok(resp) => {
                                        let err_s = format!(
                                            "server error {}",
                                            resp_error_code(&resp).unwrap_or(0)
                                        );
                                        crate::logger::log_error("gui::recall", &err_s);
                                        let _ = gui_tx_clone.send(GuiEvent::StatusUpdate(format!(
                                            "Đã thu hồi cục bộ (Máy chủ: {err_s})"
                                        )));
                                    }
                                    Err(e) => {
                                        crate::logger::log_error("gui::recall", &e.to_string());
                                        let _ = gui_tx_clone.send(GuiEvent::StatusUpdate(format!(
                                            "Không thể thu hồi trên máy chủ: {e}"
                                        )));
                                    }
                                }
                                ctx_clone.request_repaint();
                            });
                        }
                        WorkerCmd::SendSticker {
                            to_id,
                            sticker_id,
                            cate_id,
                            sticker_type,
                            is_group,
                        } => {
                            let session_clone = session.clone();
                            let gui_tx_clone = gui_tx.clone();
                            let ctx_clone = egui_ctx.clone();
                            tokio::spawn(async move {
                                let s = session_clone.lock().await.clone();
                                let res = crate::chat::send_sticker(
                                    &s,
                                    &to_id,
                                    sticker_id,
                                    cate_id,
                                    sticker_type,
                                    is_group,
                                )
                                .await;
                                match res {
                                    Ok(resp) => {
                                        let msg_id = resp
                                            .get("data")
                                            .and_then(|d| d.get("msgId"))
                                            .and_then(|v| match v {
                                                serde_json::Value::String(s) => Some(s.clone()),
                                                serde_json::Value::Number(n) => Some(n.to_string()),
                                                _ => None,
                                            })
                                            .unwrap_or_default();
                                        let _ = gui_tx_clone.send(GuiEvent::StickerSent {
                                            thread_id: to_id,
                                            msg_id,
                                            sticker_id,
                                            cate_id,
                                            sticker_type,
                                        });
                                        ctx_clone.request_repaint();
                                    }
                                    Err(e) => {
                                        let err_s = e.to_string();
                                        crate::logger::log_error("gui::send_sticker", &err_s);
                                        let _ = gui_tx_clone.send(GuiEvent::SendFailed {
                                            thread_id: to_id,
                                            text: String::new(),
                                            error: err_s.clone(),
                                        });
                                        if crate::session::SessionStatus::from_error(&e)
                                            == crate::session::SessionStatus::Expired
                                        {
                                            {
                                                let mut sl = session_clone.lock().await;
                                                sl.mark_expired();
                                                let _ = sl.save_to_file("session.json");
                                                sl.logout();
                                            }
                                            let _ = gui_tx_clone.send(GuiEvent::SessionExpired(
                                                "Không thể gửi nhãn dán vì phiên zpw_sek đã hết hạn"
                                                    .into(),
                                            ));
                                            if let Some(tx) = WORKER_TX.get() {
                                                let _ = tx.send(WorkerCmd::RequestQr);
                                            }
                                        }
                                        ctx_clone.request_repaint();
                                    }
                                }
                            });
                        }
                        WorkerCmd::FetchStickerImage { key, url } => {
                            let session_clone = session.clone();
                            let gui_tx_clone = gui_tx.clone();
                            let ctx_clone = egui_ctx.clone();
                            tokio::spawn(async move {
                                let cache_dir = std::path::Path::new("stickers_cache");
                                let _ = std::fs::create_dir_all(cache_dir);
                                let cache_path = cache_dir.join(format!("{key}.png"));

                                // Bytes come from the disk cache, a local file or the network. They
                                // used to be decoded once to validate and again to use; now once, on
                                // the blocking pool so a large photo cannot stall the 2-thread runtime.
                                let (bytes_opt, from_network) =
                                    if let Ok(disk_bytes) = std::fs::read(&cache_path) {
                                        (Some(disk_bytes), false)
                                    } else if !url.starts_with("http://")
                                        && !url.starts_with("https://")
                                    {
                                        (std::fs::read(&url).ok(), false)
                                    } else {
                                        let client = { session_clone.lock().await.clone().http };
                                        let body = match client.get(&url).send().await {
                                            Ok(resp) if resp.status().is_success() => {
                                                resp.bytes().await.ok()
                                            }
                                            _ => None,
                                        };
                                        (body.map(|b| b.to_vec()), true)
                                    };
                                let Some(bytes) = bytes_opt else {
                                    return;
                                };

                                let _ = tokio::task::spawn_blocking(move || {
                                    let Ok(mut dyn_img) = image::load_from_memory(&bytes) else {
                                        if !from_network {
                                            // Corrupt cache entry: drop it so the next request refetches.
                                            let _ = std::fs::remove_file(&cache_path);
                                        }
                                        return;
                                    };
                                    if from_network {
                                        let _ = std::fs::write(&cache_path, &bytes);
                                    }
                                    // Bubbles draw images at most 240 pt wide; full-size photos kept as
                                    // textures cost ~8 MB each (1080p) and are never freed.
                                    // The viewer's `hd_` textures stay full size.
                                    if !key.starts_with("ava_")
                                        && !key.starts_with("hd_")
                                        && (dyn_img.width() > 480 || dyn_img.height() > 480)
                                    {
                                        dyn_img = dyn_img.thumbnail(480, 480);
                                    }
                                    {
                                        let mut rgba = dyn_img.to_rgba8();
                                        // Apply smooth anti-aliased circular alpha mask for avatar textures (`ava_*`)
                                        if key.starts_with("ava_") {
                                            let side = rgba.width().min(rgba.height()).clamp(32, 160);
                                            let x0 = (rgba.width() - side) / 2;
                                            let y0 = (rgba.height() - side) / 2;
                                            let cropped = image::imageops::crop_imm(
                                                &rgba, x0, y0, side, side,
                                            )
                                            .to_image();
                                            rgba = if side > 128 {
                                                image::imageops::resize(
                                                    &cropped,
                                                    128,
                                                    128,
                                                    image::imageops::FilterType::Triangle,
                                                )
                                            } else {
                                                cropped
                                            };
                                            let w = rgba.width() as f32;
                                            let h = rgba.height() as f32;
                                            let cx = (w - 1.0) * 0.5;
                                            let cy = (h - 1.0) * 0.5;
                                            let r = w.min(h) * 0.5;
                                            for (x, y, px) in rgba.enumerate_pixels_mut() {
                                                let dx = x as f32 - cx;
                                                let dy = y as f32 - cy;
                                                let dist = (dx * dx + dy * dy).sqrt();
                                                if dist > r {
                                                    px[3] = 0;
                                                } else if dist > r - 1.4 {
                                                    let cov = ((r - dist) / 1.4).clamp(0.0, 1.0);
                                                    px[3] = (px[3] as f32 * cov) as u8;
                                                }
                                            }
                                        }
                                        let size =
                                            [rgba.width() as usize, rgba.height() as usize];
                                        let pixels = rgba.as_flat_samples();
                                        let color_img = egui::ColorImage::from_rgba_unmultiplied(
                                            size,
                                            pixels.as_slice(),
                                        );
                                        let _ = gui_tx_clone.send(GuiEvent::StickerImageReady {
                                            key,
                                            image: color_img,
                                        });
                                        ctx_clone.request_repaint();
                                    }
                                })
                                .await;
                            });
                        }
                        WorkerCmd::FetchStickerStore => {
                            let session_clone = session.clone();
                            let gui_tx_clone = gui_tx.clone();
                            let ctx_clone = egui_ctx.clone();
                            tokio::spawn(async move {
                                let s = session_clone.lock().await.clone();
                                match crate::chat::fetch_sticker_store_packs(&s).await {
                                    Ok(packs) => {
                                        let _ = gui_tx_clone
                                            .send(GuiEvent::StickerStoreLoaded(packs));
                                    }
                                    Err(e) => {
                                        crate::logger::log_error(
                                            "gui::fetch_sticker_store",
                                            &e.to_string(),
                                        );
                                        let _ = gui_tx_clone
                                            .send(GuiEvent::StickerStoreLoaded(Vec::new()));
                                        let _ = gui_tx_clone.send(GuiEvent::StatusUpdate(format!(
                                            "Không tải được kho nhãn dán: {e}"
                                        )));
                                    }
                                }
                                ctx_clone.request_repaint();
                            });
                        }
                        WorkerCmd::FetchStickerPack { cate_id, name } => {
                            let session_clone = session.clone();
                            let gui_tx_clone = gui_tx.clone();
                            let ctx_clone = egui_ctx.clone();
                            tokio::spawn(async move {
                                let s = session_clone.lock().await.clone();
                                match crate::chat::fetch_sticker_pack_items(&s, cate_id, &name)
                                    .await
                                {
                                    Ok(items) => {
                                        let _ = gui_tx_clone.send(
                                            GuiEvent::StickerPackItemsLoaded {
                                                cate_id,
                                                name,
                                                items,
                                            },
                                        );
                                    }
                                    Err(e) => {
                                        crate::logger::log_error(
                                            "gui::fetch_sticker_pack",
                                            &e.to_string(),
                                        );
                                        let _ = gui_tx_clone.send(GuiEvent::StatusUpdate(format!(
                                            "Không tải được bộ nhãn \"{name}\": {e}"
                                        )));
                                    }
                                }
                                ctx_clone.request_repaint();
                            });
                        }
                        WorkerCmd::SearchStickers { keyword } => {
                            let session_clone = session.clone();
                            let gui_tx_clone = gui_tx.clone();
                            let ctx_clone = egui_ctx.clone();
                            tokio::spawn(async move {
                                let s = session_clone.lock().await.clone();
                                match crate::chat::search_stickers_by_keyword(&s, &keyword).await {
                                    Ok(items) => {
                                        let missing = preload_disk_sticker_thumbnails(
                                            &items,
                                            &gui_tx_clone,
                                            24,
                                        );
                                        let _ = gui_tx_clone.send(GuiEvent::StickerSearchResults {
                                            keyword,
                                            items,
                                        });
                                        ctx_clone.request_repaint();
                                        prefetch_missing_sticker_thumbnails(
                                            &s.http,
                                            missing,
                                            &gui_tx_clone,
                                            &ctx_clone,
                                        )
                                        .await;
                                    }
                                    Err(e) => {
                                        crate::logger::log_error(
                                            "gui::search_stickers",
                                            &e.to_string(),
                                        );
                                        let _ = gui_tx_clone.send(GuiEvent::StickerSearchResults {
                                            keyword,
                                            items: Vec::new(),
                                        });
                                        ctx_clone.request_repaint();
                                    }
                                }
                            });
                        }
                        WorkerCmd::SuggestComposerStickers { keyword } => {
                            let session_clone = session.clone();
                            let gui_tx_clone = gui_tx.clone();
                            let ctx_clone = egui_ctx.clone();
                            tokio::spawn(async move {
                                let s = session_clone.lock().await.clone();
                                let items = crate::chat::search_stickers_by_keyword(&s, &keyword)
                                    .await
                                    .unwrap_or_default();
                                let missing =
                                    preload_disk_sticker_thumbnails(&items, &gui_tx_clone, 16);
                                let _ =
                                    gui_tx_clone.send(GuiEvent::ComposerStickerSuggestions {
                                        keyword,
                                        items,
                                    });
                                ctx_clone.request_repaint();
                                prefetch_missing_sticker_thumbnails(
                                    &s.http,
                                    missing,
                                    &gui_tx_clone,
                                    &ctx_clone,
                                )
                                .await;
                            });
                        }
                        WorkerCmd::ParseLinkPreview { url } => {
                            let session_clone = session.clone();
                            let gui_tx_clone = gui_tx.clone();
                            let ctx_clone = egui_ctx.clone();
                            tokio::spawn(async move {
                                let s = session_clone.lock().await.clone();
                                if let Ok(meta) = crate::chat::parse_link_metadata(&s, &url).await {
                                    let _ = gui_tx_clone.send(GuiEvent::LinkPreviewReady(meta));
                                    ctx_clone.request_repaint();
                                }
                            });
                        }
                        WorkerCmd::PickAndSendAttachment {
                            to_id,
                            is_group,
                            images_only,
                            caption,
                            preset_path,
                        } => {
                            let session_clone = session.clone();
                            let gui_tx_clone = gui_tx.clone();
                            let ctx_clone = egui_ctx.clone();
                            tokio::spawn(async move {
                                let picked_path = if let Some(p) = preset_path {
                                    Some(p)
                                } else {
                                    tokio::task::spawn_blocking(move || {
                                        pick_native_file_dialog(images_only)
                                    })
                                    .await
                                    .ok()
                                    .flatten()
                                };

                                let Some(path) = picked_path else {
                                    return;
                                };
                                let fname = path
                                    .file_name()
                                    .and_then(|n| n.to_str())
                                    .unwrap_or("tập tin")
                                    .to_string();
                                let _ = gui_tx_clone.send(GuiEvent::StatusUpdate(format!(
                                    "⏳ Đang tải lên và gửi '{fname}'…"
                                )));
                                ctx_clone.request_repaint();

                                let s = session_clone.lock().await.clone();
                                match crate::chat::send_attachment(
                                    &s, &to_id, is_group, &path, &caption,
                                )
                                .await
                                {
                                    Ok((_resp, cm)) => {
                                        let _ = gui_tx_clone.send(GuiEvent::AttachmentSent {
                                            thread_id: to_id,
                                            msg: cm,
                                        });
                                    }
                                    Err(e) => {
                                        let err_s = e.to_string();
                                        crate::logger::log_error("gui::send_attachment", &err_s);
                                        let _ = gui_tx_clone.send(GuiEvent::SendFailed {
                                            thread_id: to_id,
                                            text: caption,
                                            error: format!("Lỗi gửi '{fname}': {err_s}"),
                                        });
                                    }
                                }
                                ctx_clone.request_repaint();
                            });
                        }
                        WorkerCmd::StartCall {
                            partner_id,
                            partner_name,
                            partner_avatar,
                            is_video,
                            is_group,
                        } => {
                            let kind_label = if is_video { "Gọi video" } else { "Gọi thoại" };
                            call_bridge.make_call(
                                &partner_id,
                                &partner_name,
                                partner_avatar.as_deref(),
                                is_video,
                                is_group,
                            );
                            let status_msg = format!(
                                "📞 Đang khởi tạo {kind_label} trực tiếp (Pure-Rust) tới {partner_name} ({partner_id})…"
                            );
                            let _ = gui_tx.send(GuiEvent::StatusUpdate(status_msg));
                            egui_ctx.request_repaint();
                        }
                        WorkerCmd::AnswerCall { accept } => {
                            call_bridge.answer_call(accept);
                            egui_ctx.request_repaint();
                        }
                        WorkerCmd::SetCallMediaState {
                            is_on_mic,
                            is_on_cam,
                        } => {
                            call_bridge.set_media_state(is_on_mic, is_on_cam);
                            egui_ctx.request_repaint();
                        }
                        WorkerCmd::EndCall => {
                            call_bridge.end_call();
                            let _ = gui_tx.send(GuiEvent::CallStateChanged("free".into()));
                            let _ = gui_tx.send(GuiEvent::StatusUpdate(
                                "Đã kết thúc cuộc gọi".into(),
                            ));
                            egui_ctx.request_repaint();
                        }
                        WorkerCmd::CheckFriendStatus { user_id } => {
                            let session_clone = session.clone();
                            let gui_tx_clone = gui_tx.clone();
                            let ctx_clone = egui_ctx.clone();
                            tokio::spawn(async move {
                                let s = session_clone.lock().await.clone();
                                let status_res =
                                    crate::chat::check_friendship_status(&s, &user_id).await;
                                let online_opt =
                                    crate::chat::get_user_online_status(&s, &user_id).await.ok();
                                if let Ok(status) = status_res {
                                    let _ = gui_tx_clone.send(GuiEvent::FriendStatusChecked {
                                        user_id,
                                        status,
                                        online: online_opt,
                                    });
                                    ctx_clone.request_repaint();
                                }
                            });
                        }
                        WorkerCmd::AcceptFriend { user_id, user_name } => {
                            let session_clone = session.clone();
                            let gui_tx_clone = gui_tx.clone();
                            let ctx_clone = egui_ctx.clone();
                            tokio::spawn(async move {
                                let s = session_clone.lock().await.clone();
                                match crate::chat::accept_friend_request(&s, &user_id).await {
                                    Ok(_) => {
                                        let _ = gui_tx_clone.send(GuiEvent::FriendStatusChecked {
                                            user_id,
                                            status: crate::chat::FriendshipStatus::Friend,
                                            online: None,
                                        });
                                        let _ = gui_tx_clone.send(GuiEvent::StatusUpdate(format!(
                                            "Đã đồng ý kết bạn với {user_name}!"
                                        )));
                                    }
                                    Err(e) => {
                                        let _ = gui_tx_clone.send(GuiEvent::StatusUpdate(format!(
                                            "Không thể đồng ý kết bạn với {user_name}: {e}"
                                        )));
                                    }
                                }
                                ctx_clone.request_repaint();
                            });
                        }
                        WorkerCmd::RejectFriend { user_id, user_name } => {
                            let session_clone = session.clone();
                            let gui_tx_clone = gui_tx.clone();
                            let ctx_clone = egui_ctx.clone();
                            tokio::spawn(async move {
                                let s = session_clone.lock().await.clone();
                                if crate::chat::reject_friend_request(&s, &user_id)
                                    .await
                                    .is_ok()
                                {
                                    let _ = gui_tx_clone.send(GuiEvent::FriendStatusChecked {
                                        user_id,
                                        status: crate::chat::FriendshipStatus::Stranger,
                                        online: None,
                                    });
                                    let _ = gui_tx_clone.send(GuiEvent::StatusUpdate(format!(
                                        "Đã từ chối lời mời kết bạn của {user_name}."
                                    )));
                                    ctx_clone.request_repaint();
                                }
                            });
                        }
                        WorkerCmd::UndoFriend { user_id, user_name } => {
                            let session_clone = session.clone();
                            let gui_tx_clone = gui_tx.clone();
                            let ctx_clone = egui_ctx.clone();
                            tokio::spawn(async move {
                                let s = session_clone.lock().await.clone();
                                if crate::chat::undo_friend_request(&s, &user_id).await.is_ok() {
                                    let _ = gui_tx_clone.send(GuiEvent::FriendStatusChecked {
                                        user_id,
                                        status: crate::chat::FriendshipStatus::Stranger,
                                        online: None,
                                    });
                                    let _ = gui_tx_clone.send(GuiEvent::StatusUpdate(format!(
                                        "Đã hủy lời mời kết bạn tới {user_name}."
                                    )));
                                    ctx_clone.request_repaint();
                                }
                            });
                        }
                        WorkerCmd::AddFriend { user_id, user_name } => {
                            let session_clone = session.clone();
                            let gui_tx_clone = gui_tx.clone();
                            let ctx_clone = egui_ctx.clone();
                            tokio::spawn(async move {
                                let s = session_clone.lock().await.clone();
                                // Check if there is an incoming request we can accept directly
                                if crate::chat::accept_friend_request(&s, &user_id).await.is_ok() {
                                    let _ = gui_tx_clone.send(GuiEvent::FriendStatusChecked {
                                        user_id,
                                        status: crate::chat::FriendshipStatus::Friend,
                                        online: None,
                                    });
                                    let _ = gui_tx_clone.send(GuiEvent::StatusUpdate(format!(
                                        "Đã đồng ý kết bạn với {user_name}! Bạn có thể gọi lại ngay."
                                    )));
                                    ctx_clone.request_repaint();
                                    return;
                                }
                                match crate::chat::send_friend_request(
                                    &s,
                                    &user_id,
                                    "Xin chào, kết bạn với mình nhé!",
                                )
                                .await
                                {
                                    Ok(_) => {
                                        let _ = gui_tx_clone.send(GuiEvent::FriendStatusChecked {
                                            user_id: user_id.clone(),
                                            status: crate::chat::FriendshipStatus::OutgoingPending,
                                            online: None,
                                        });
                                        let _ = gui_tx_clone.send(GuiEvent::StatusUpdate(format!(
                                            "Đã gửi lời mời kết bạn tới {user_name} ({user_id})."
                                        )));
                                    }
                                    Err(e) => {
                                        let _ = gui_tx_clone.send(GuiEvent::StatusUpdate(format!(
                                            "Không thể gửi lời mời kết bạn tới {user_name}: {e}"
                                        )));
                                    }
                                }
                                ctx_clone.request_repaint();
                            });
                        }
                        WorkerCmd::LookupPhone { phone } => {
                            let session_clone = session.clone();
                            let gui_tx_clone = gui_tx.clone();
                            let ctx_clone = egui_ctx.clone();
                            tokio::spawn(async move {
                                let s = session_clone.lock().await.clone();
                                match crate::chat::find_user_by_phone(&s, &phone).await {
                                    Ok(profile) => {
                                        let _ = gui_tx_clone
                                            .send(GuiEvent::PhoneLookupSuccess(profile));
                                    }
                                    Err(e) => {
                                        crate::logger::log_error(
                                            "gui::lookup_phone",
                                            &e.to_string(),
                                        );
                                        let _ = gui_tx_clone
                                            .send(GuiEvent::PhoneLookupFailed(e.to_string()));
                                    }
                                }
                                ctx_clone.request_repaint();
                            });
                        }
                        WorkerCmd::LoginPassword { phone, pass } => {
                            crate::runtime_state::registry().invalidate();
                            PERSISTENCE.get_or_init(Default::default).invalidate();
                            let session_clone = session.clone();
                            let poll_clone = poll_state.clone();
                            let gui_tx_clone = gui_tx.clone();
                            let ctx_clone = egui_ctx.clone();
                            spawn_scoped(gui_tx.clone(), async move {
                                let _ = gui_tx_clone.send(GuiEvent::StatusUpdate(
                                    "Đang xác thực tài khoản…".into(),
                                ));
                                ctx_clone.request_repaint();
                                let mut s = session_clone.lock().await;
                                    if !gui_tx_clone.current(){return;}
                                match crate::login::login_by_password(&mut s, &phone, &pass, "VN")
                                    .await
                                {
                                    Ok(_) => {
                                        if let Ok(info) = crate::login::get_login_info(&s).await {
                                            if let Some(cfg) = info.get("data") {
                                                crate::login::apply_login_config(&mut s, cfg);
                                                s.apply_server_info(cfg);
                                                report_session_save(&s, &gui_tx_clone, &ctx_clone);

                                                let uin = s
                                                    .login
                                                    .as_ref()
                                                    .map(|l| l.uin.clone())
                                                    .unwrap_or_default();
                                                let name = s
                                                    .login
                                                    .as_ref()
                                                    .map(|l| l.display_name.clone())
                                                    .unwrap_or_default();
                                                let u_ava = s
                                                    .login
                                                    .as_ref()
                                                    .map(|l| l.avatar.clone())
                                                    .filter(|a| !a.is_empty());
                                                let _ = gui_tx_clone.send(GuiEvent::LoginSuccess {
                                                    name,
                                                    uin,
                                                    avatar: u_ava,
                                                });
                                                ctx_clone.request_repaint();

                                                let bootstrap_session = s.clone();
                                                drop(s);
                                                let s = bootstrap_session;
                                                if let Ok(mut list) =
                                                    crate::chat::get_friends_remote(&s).await
                                                {
                                                    {
                                                        let mut ps = poll_clone.lock().await;
                                                        *ps = crate::chat::PollState::default();
                                                        let _ = crate::chat::poll_new_messages_remote(
                                                            &s, &mut ps, &mut list,
                                                        )
                                                        .await;
                                                    }
                                                    let _ = gui_tx_clone
                                                        .send(GuiEvent::ThreadsUpdated(list));
                                                    ctx_clone.request_repaint();
                                                }
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        let _ = gui_tx_clone
                                            .send(GuiEvent::LoginFailed(e.to_string()));
                                        ctx_clone.request_repaint();
                                    }
                                }
                            });
                        }
                        WorkerCmd::CheckForUpdates { silent } => {
                            let gui_tx_clone = gui_tx.clone();
                            let ctx_clone = egui_ctx.clone();
                            tokio::spawn(async move {
                                let res = fetch_github_latest_release().await;
                                let _ = gui_tx_clone.send(GuiEvent::UpdateCheckFinished {
                                    result: res,
                                    silent,
                                });
                                ctx_clone.request_repaint();
                            });
                        }
                        WorkerCmd::DownloadAndApplyUpdate {
                            download_url,
                            asset_name,
                            target_version,
                            expected_sha256,
                        } => {
                            let gui_tx_clone = gui_tx.clone();
                            let ctx_clone = egui_ctx.clone();
                            spawn_scoped(gui_tx.clone(), async move {
                                let _ = gui_tx_clone.send(GuiEvent::UpdateDownloadStatus {
                                    downloading: true,
                                    message: format!(
                                        "Đang tải bản cập nhật v{target_version}…"
                                    ),
                                });
                                ctx_clone.request_repaint();
                                match apply_github_release_update(
                                    &download_url,
                                    &asset_name,
                                    &target_version,
                                    expected_sha256.as_deref(),
                                    &gui_tx_clone,
                                    &ctx_clone,
                                )
                                .await
                                {
                                    Ok(restarting) => {
                                        if restarting {
                                            let _ = gui_tx_clone.send(
                                                GuiEvent::UpdateAppliedAndRestarting(target_version),
                                            );
                                        } else {
                                            let _ = gui_tx_clone.send(
                                                GuiEvent::UpdateDownloadStatus {
                                                    downloading: false,
                                                    message: format!(
                                                        "Đã tải và mở bộ cài đặt {asset_name} (v{target_version})"
                                                    ),
                                                },
                                            );
                                        }
                                    }
                                    Err(e) => {
                                        let _ = gui_tx_clone.send(GuiEvent::UpdateDownloadStatus {
                                            downloading: false,
                                            message: format!("Lỗi cập nhật: {e}"),
                                        });
                                    }
                                }
                                ctx_clone.request_repaint();
                            });
                        }
                    }
                }
            });
            rt.shutdown_timeout(Duration::from_secs(2));
        })
        .expect("Failed to spawn zalo-worker thread");
    let _ = WORKER_HANDLE.set(std::sync::Mutex::new(Some(handle)));
}

const GITHUB_REPO: &str = "ctdoteam1110/ctdozlite";

async fn fetch_github_latest_release() -> Result<UpdateCheckResult, String> {
    let current_ver = env!("CARGO_PKG_VERSION").to_string();
    let client = reqwest::Client::builder()
        .user_agent(format!("ctdozlite/{current_ver}"))
        .timeout(Duration::from_secs(12))
        .build()
        .map_err(|e| format!("Khởi tạo HTTP client thất bại: {e}"))?;

    let latest_url = format!("https://api.github.com/repos/{GITHUB_REPO}/releases/latest");
    let mut rel_json: Option<serde_json::Value> = None;

    if let Ok(resp) = client
        .get(&latest_url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
    {
        if resp.status().is_success() {
            if let Ok(v) = resp.json::<serde_json::Value>().await {
                rel_json = Some(v);
            }
        }
    }

    if rel_json.is_none() {
        let list_url = format!("https://api.github.com/repos/{GITHUB_REPO}/releases?per_page=5");
        if let Ok(resp) = client
            .get(&list_url)
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
        {
            if resp.status().is_success() {
                if let Ok(arr) = resp.json::<serde_json::Value>().await {
                    if let Some(first) = arr.as_array().and_then(|a| a.first()).cloned() {
                        rel_json = Some(first);
                    }
                }
            }
        }
    }

    // Rate-limit resilient fallback: fetch static `latest.json` directly from GitHub Release CDN
    if rel_json.is_none() {
        let cdn_manifest_url =
            format!("https://github.com/{GITHUB_REPO}/releases/latest/download/latest.json");
        if let Ok(resp) = client.get(&cdn_manifest_url).send().await {
            if resp.status().is_success() {
                if let Ok(manifest) = resp.json::<serde_json::Value>().await {
                    let ver = manifest
                        .get("version")
                        .and_then(|v| v.as_str())
                        .unwrap_or(&current_ver)
                        .trim_start_matches(['v', 'V'])
                        .to_string();
                    let asset_name = if cfg!(target_os = "windows") {
                        format!("ctdozlite-{ver}-portable-windows-x86_64.exe")
                    } else if cfg!(target_os = "macos") {
                        if cfg!(target_arch = "aarch64") {
                            format!("ctdozlite-{ver}-macos-arm64.dmg")
                        } else if cfg!(target_arch = "x86_64") {
                            format!("ctdozlite-{ver}-macos-x86_64.dmg")
                        } else {
                            format!("ctdozlite-{ver}-macos.dmg")
                        }
                    } else {
                        format!("ctdozlite-{ver}-x86_64.AppImage")
                    };
                    let dl_url = format!(
                        "https://github.com/{GITHUB_REPO}/releases/download/v{ver}/{asset_name}"
                    );
                    return Ok(UpdateCheckResult {
                        current_version: current_ver.clone(),
                        has_update: is_newer_semver(&ver, &current_ver),
                        latest_version: ver.clone(),
                        release_title: format!("ctdozlite v{ver}"),
                        release_notes: manifest
                            .get("notes")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        html_url: format!("https://github.com/{GITHUB_REPO}/releases/tag/v{ver}"),
                        download_url: Some(dl_url),
                        asset_name: Some(asset_name),
                        asset_size: 0,
                        expected_sha256: manifest
                            .get("sha256")
                            .and_then(|v| v.as_str())
                            .map(str::to_string),
                        published_at: String::new(),
                    });
                }
            }
        }
    }

    let Some(rel) = rel_json else {
        return Err("Không truy cập được máy chủ GitHub Releases".into());
    };

    let raw_tag = rel
        .get("tag_name")
        .and_then(|v| v.as_str())
        .unwrap_or(&current_ver);
    let latest_ver = raw_tag.trim().trim_start_matches(['v', 'V']).to_string();
    let has_update = is_newer_semver(&latest_ver, &current_ver);
    let release_title = rel
        .get("name")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(raw_tag)
        .to_string();
    let release_notes = rel
        .get("body")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let html_url = rel
        .get("html_url")
        .and_then(|v| v.as_str())
        .unwrap_or(&format!("https://github.com/{GITHUB_REPO}/releases"))
        .to_string();
    let published_at = rel
        .get("published_at")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .split('T')
        .next()
        .unwrap_or("")
        .to_string();

    let mut best_url: Option<String> = None;
    let mut best_name: Option<String> = None;
    let mut best_size: u64 = 0;
    let mut best_score: i32 = -1;
    let mut sha256_url: Option<String> = None;

    if let Some(assets) = rel.get("assets").and_then(|v| v.as_array()) {
        for a in assets {
            let name = a.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let dl = a
                .get("browser_download_url")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let sz = a.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
            if name.eq_ignore_ascii_case("SHA256SUMS.txt") && !dl.is_empty() {
                sha256_url = Some(dl.to_string());
                continue;
            }
            if name.is_empty() || dl.is_empty() || name.ends_with(".txt") || name.ends_with(".json")
            {
                continue;
            }
            let n_low = name.to_lowercase();
            let score = if cfg!(target_os = "windows") {
                if n_low.ends_with(".exe") && n_low.contains("portable") {
                    100
                } else if n_low.ends_with(".exe") {
                    80
                } else {
                    0
                }
            } else if cfg!(target_os = "macos") {
                if n_low.ends_with(".dmg") {
                    if (cfg!(target_arch = "aarch64")
                        && (n_low.contains("arm64") || n_low.contains("aarch64")))
                        || (cfg!(target_arch = "x86_64")
                            && (n_low.contains("x86_64") || n_low.contains("intel")))
                    {
                        110
                    } else if !n_low.contains("arm64")
                        && !n_low.contains("aarch64")
                        && !n_low.contains("x86_64")
                        && !n_low.contains("intel")
                    {
                        100
                    } else {
                        50
                    }
                } else {
                    0
                }
            } else if n_low.ends_with(".appimage") {
                100
            } else if n_low.ends_with(".deb") && !n_low.contains("legacy") {
                90
            } else if n_low.ends_with(".tar.gz") && !n_low.contains("legacy") {
                80
            } else {
                0
            };
            if score > best_score {
                best_score = score;
                best_url = Some(dl.to_string());
                best_name = Some(name.to_string());
                best_size = sz;
            }
        }
    }

    let mut expected_sha256: Option<String> = None;
    if let (Some(ref s_url), Some(ref b_name)) = (sha256_url, &best_name) {
        if let Ok(resp) = client.get(s_url).send().await {
            if let Ok(txt) = resp.text().await {
                for line in txt.lines() {
                    let mut parts = line.split_whitespace();
                    let Some(hash) = parts.next() else { continue };
                    let Some(fname) = parts.next() else { continue };
                    let clean_fname = fname.trim_start_matches('*').trim_start_matches("./");
                    if clean_fname == b_name && hash.len() == 64 {
                        expected_sha256 = Some(hash.to_lowercase());
                        break;
                    }
                }
            }
        }
    }

    Ok(UpdateCheckResult {
        current_version: current_ver,
        latest_version: latest_ver,
        has_update,
        release_title,
        release_notes,
        html_url,
        download_url: best_url,
        asset_name: best_name,
        asset_size: best_size,
        expected_sha256,
        published_at,
    })
}

/// Download before staging, then launch portable updates only after shutdown/join.
async fn apply_github_release_update(
    download_url: &str,
    asset_name: &str,
    target_version: &str,
    expected_sha256: Option<&str>,
    gui_tx: &ScopedSender,
    ctx: &egui::Context,
) -> Result<bool, String> {
    let client = reqwest::Client::builder()
        .user_agent(format!("ctdozlite/{target_version}"))
        .timeout(Duration::from_secs(240))
        .build()
        .map_err(|e| format!("Khởi tạo HTTP client thất bại: {e}"))?;
    let bytes = crate::updater::download_release(
        &client,
        &crate::updater::DownloadRequest {
            url: download_url,
            asset_name,
            expected_sha256,
            expected_size: None,
            limits: Default::default(),
        },
        |downloaded, total| {
            let _ = gui_tx.send(GuiEvent::UpdateDownloadProgress { downloaded, total });
            ctx.request_repaint();
        },
    )
    .await?;
    let name = asset_name.to_ascii_lowercase();
    let portable =
        (cfg!(target_os = "windows") && name.ends_with(".exe") && !name.contains("setup"))
            || (cfg!(target_os = "linux") && name.ends_with(".appimage"));
    let asset_name = asset_name.to_owned();
    tokio::task::spawn_blocking(move || -> Result<bool, String> {
        if portable {
            let executable =
                std::env::current_exe().map_err(|e| format!("Thiếu đường dẫn ứng dụng: {e}"))?;
            let pending = crate::updater::stage_portable_release(&bytes, &executable)?;
            let mut slot = pending_update()
                .lock()
                .map_err(|_| "Trạng thái cập nhật chưa sẵn sàng")?;
            if slot.is_some() {
                return Err("Một bản cập nhật đang chờ ứng dụng đóng và lưu dữ liệu".into());
            }
            *slot = Some(pending);
            return Ok(true);
        }
        let home = std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(std::path::PathBuf::from)
            .ok_or("Thiếu thư mục người dùng để lưu bộ cài")?;
        let directory = home.join("Downloads");
        std::fs::create_dir_all(&directory)
            .map_err(|e| format!("Không tạo được thư mục tải xuống: {e}"))?;
        crate::updater::install_release(
            &bytes,
            &crate::updater::InstallTarget::Installer(directory.join(asset_name)),
            |path| {
                #[cfg(target_os = "windows")]
                let result = std::process::Command::new(path).spawn();
                #[cfg(target_os = "macos")]
                let result = std::process::Command::new("open").arg(path).spawn();
                #[cfg(not(any(target_os = "windows", target_os = "macos")))]
                let result = std::process::Command::new("xdg-open").arg(path).spawn();
                result
                    .map(|_| ())
                    .map_err(|e| format!("Khởi chạy bộ cài thất bại: {e}"))
            },
        )?;
        Ok(false)
    })
    .await
    .map_err(|e| format!("Tác vụ cài cập nhật bị gián đoạn: {e}"))?
}

fn pending_update() -> &'static std::sync::Mutex<Option<crate::updater::DeferredPortableInstall>> {
    static PENDING: std::sync::OnceLock<
        std::sync::Mutex<Option<crate::updater::DeferredPortableInstall>>,
    > = std::sync::OnceLock::new();
    PENDING.get_or_init(Default::default)
}

/// Commit and launch a staged update only after the worker was successfully joined.
///
/// # Errors
/// Returns installation/launch errors; failed launches restore the previous executable.
pub fn launch_pending_update() -> Result<(), String> {
    let pending = pending_update()
        .lock()
        .map_err(|_| "Trạng thái cập nhật chưa sẵn sàng")?
        .take();
    if let Some(pending) = pending {
        pending.launch_and_finish(|path| {
            std::process::Command::new(path)
                .args(std::env::args().skip(1))
                .spawn()
                .map(|_| ())
                .map_err(|e| format!("Khởi chạy bản mới thất bại: {e}"))
        })?;
    }
    Ok(())
}

/// Discard staging after a failed shutdown without changing the running executable.
pub fn abort_pending_update() {
    if let Ok(mut slot) = pending_update().lock() {
        if let Some(pending) = slot.take() {
            if let Err(error) = pending.abort() {
                crate::logger::log_error("updater::abort", &error);
            }
        }
    }
}

#[cfg(test)]
mod lifecycle_regressions {
    use super::*;

    fn root(label: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("lifecycle-{label}-{}", std::process::id()));
        if root.exists() {
            std::fs::remove_dir_all(&root).unwrap();
        }
        std::fs::create_dir_all(&root).unwrap();
        root
    }
    fn thread() -> Thread {
        Thread {
            id: "fixture-thread".into(),
            name: "Fixture".into(),
            preview: "last message".into(),
            time: String::new(),
            avatar: None,
            is_group: false,
            presentation: Default::default(),
            unread: Default::default(),
            messages: vec![ChatMessage {
                id: "42".into(),
                text: "last message".into(),
                ts: 1_700_000_000_000,
                ..Default::default()
            }],
        }
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_ack_waits_for_consumer_write_and_reopens_final_snapshot() {
        let root = root("shutdown");
        let (events, rx) = channel();
        let (entered, entered_rx) = channel();
        let (release, release_rx) = channel();
        let blocker = std::thread::spawn(move || {
            let _guard = storage::mutation_fence().lock().unwrap();
            entered.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        entered_rx.recv().unwrap();
        let destination = root.clone();
        let task = tokio::spawn(async move {
            flush_shutdown_and_ack(
                destination,
                "fixture-account".into(),
                vec![thread()],
                vec![("fixture-thread".into(), thread().messages)],
                Some((
                    "fixture-thread".into(),
                    crate::runtime_state::Draft {
                        text: "unsent draft".into(),
                        ..Default::default()
                    },
                )),
                events,
            )
            .await
        });
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(
            rx.try_recv().is_err(),
            "close acknowledged before filesystem consumer completed"
        );
        assert!(!task.is_finished());
        release.send(()).unwrap();
        blocker.join().unwrap();
        assert!(task.await.unwrap());
        assert!(matches!(
            rx.recv().unwrap(),
            GuiEvent::ShutdownComplete { error: None }
        ));
        let store = crate::store_v2::AccountStore::open(&root, "fixture-account").unwrap();
        assert_eq!(
            store
                .get_message("fixture-thread", "42")
                .unwrap()
                .unwrap()
                .text,
            "last message"
        );
        assert_eq!(
            store
                .unread("fixture-thread")
                .unwrap()
                .last_seen_id
                .as_deref(),
            Some("42")
        );
        let composer = crate::runtime_state::ComposerStore::open(&root, "fixture-account").unwrap();
        assert_eq!(composer.draft("fixture-thread").text, "unsent draft");
        drop(store);
        drop(composer);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn shutdown_write_failure_emits_error_instead_of_close_ack() {
        let root = root("shutdown-failure");
        let destination = root.join("not-directory");
        std::fs::write(&destination, b"fixture").unwrap();
        let (events, rx) = channel();
        assert!(
            !flush_shutdown_and_ack(
                destination,
                "fixture-account".into(),
                vec![thread()],
                Vec::new(),
                None,
                events
            )
            .await
        );
        assert!(matches!(
            rx.recv().unwrap(),
            GuiEvent::ShutdownComplete { error: Some(_) }
        ));
        std::fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn wipe_waits_for_aborted_parent_blocking_write_before_deleting() {
        let root = root("wipe-inflight");
        let destination = root.clone();
        let (entered, entered_rx) = channel();
        let (release, release_rx) = channel();
        let (events, _) = channel();
        let task = spawn_scoped(ScopedSender::for_test(events), async move {
            tracked_blocking(move || {
                entered.send(()).unwrap();
                release_rx.recv().unwrap();
                crate::store_v2::AccountStore::open(&destination, "fixture-account")
                    .unwrap()
                    .persist_threads(&[thread()])
                    .unwrap();
            })
            .await
            .unwrap();
        });
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let (barrier_done, barrier_rx) = channel();
        let wipe_root = root.clone();
        let barrier = tokio::spawn(async move {
            assert!(cancel_account_jobs().await);
            storage::wipe_account(&wipe_root, "fixture-account").unwrap();
            barrier_done.send(()).unwrap();
        });
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(
            barrier_rx.try_recv().is_err(),
            "wipe ran while an aborted parent's blocking writer was active"
        );
        release.send(()).unwrap();
        barrier.await.unwrap();
        assert!(task.await.unwrap_err().is_cancelled());
        let store = crate::store_v2::AccountStore::open(&root, "fixture-account").unwrap();
        assert!(store.get_message("fixture-thread", "42").unwrap().is_none());
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
    async fn received_delta_survives_cancelled_followup(label: &str) {
        let root = root(label);
        let (events, event_rx) = channel();
        let account = "fixture-account";
        let mut store = crate::store_v2::AccountStore::open(&root, account).unwrap();
        let metadata = publish_received_before_lookup(&[thread()], false, |event| {
            let GuiEvent::ThreadsUpdated(threads) = &event else {
                panic!("expected live delta");
            };
            store.persist_threads(threads).unwrap();
            events.send(event).unwrap();
        });
        drop(store);
        assert!(matches!(
            event_rx.recv().unwrap(),
            GuiEvent::ThreadsUpdated(_)
        ));
        assert!(metadata
            .iter()
            .all(|thread| thread.messages.is_empty() && thread.unread == Default::default()));
        let (lookup_entered, lookup_rx) = channel();
        let (_release, blocked_lookup) = tokio::sync::oneshot::channel::<()>();
        let lookup = tokio::spawn(async move {
            lookup_entered.send(()).unwrap();
            let _ = blocked_lookup.await;
            panic!("blocked follow-up must not complete before shutdown");
        });
        lookup_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        lookup.abort();
        assert!(lookup.await.unwrap_err().is_cancelled());
        // The final GUI snapshot intentionally contains no copy of the newly received row.
        assert!(
            flush_shutdown_and_ack(
                root.clone(),
                account.into(),
                Vec::new(),
                Vec::new(),
                None,
                events
            )
            .await
        );
        assert!(matches!(
            event_rx.recv().unwrap(),
            GuiEvent::ShutdownComplete { error: None }
        ));
        let store = crate::store_v2::AccountStore::open(&root, account).unwrap();
        assert_eq!(
            store
                .get_message("fixture-thread", "42")
                .unwrap()
                .unwrap()
                .text,
            "last message"
        );
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_delta_persists_before_blocked_profile_lookup_and_shutdown() {
        received_delta_survives_cancelled_followup("live-before-profile").await;
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn polling_page_persists_before_blocked_next_page_and_shutdown() {
        received_delta_survives_cancelled_followup("poll-before-next-page").await;
    }
    #[test]
    fn delayed_names_never_replay_messages_or_unread_state() {
        let root = root("names-only");
        let mut stale = thread();
        let mut store = crate::store_v2::AccountStore::open(&root, "fixture-account").unwrap();
        store.persist_threads(&[stale.clone()]).unwrap();
        store
            .read_local_visible(&stale.id, &stale.messages)
            .unwrap();
        stale.unread.last_seen_id = Some("1".into());
        stale.unread.marked_unread = true;
        stale.unread.fresh = true;
        stale.unread.settings_fresh = true;
        stale.name = "Resolved name".into();
        stale.messages[0].text = "stale message body".into();
        stale.messages[0].reaction_snapshot_fresh = true;
        let metadata = names_metadata(&[stale]);
        assert!(metadata[0].messages.is_empty());
        assert_eq!(metadata[0].unread, Default::default());
        assert_eq!(metadata[0].presentation.last_message_ts, 0);
        store.persist_threads(&metadata).unwrap();
        assert_eq!(
            store
                .get_message("fixture-thread", "42")
                .unwrap()
                .unwrap()
                .text,
            "last message"
        );
        let unread = store.unread("fixture-thread").unwrap();
        assert_eq!(unread.last_seen_id.as_deref(), Some("42"));
        assert!(!unread.marked_unread);
        assert_eq!(
            store.all_thread_metadata().unwrap()[0].name,
            "Resolved name"
        );
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
}
