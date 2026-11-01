//! Complete native desktop GUI powered by `eframe` and `egui`.
//!
//! Designed with the UI/UX Pro Max Minimalism & Swiss Style system:
//! - Zero-idle-CPU reactive architecture (event-driven repaint, zero per-frame Thread cloning)
//! - Custom vector icons rendered via `egui::Painter` (zero texture/SVG bloat)
//! - High-contrast Light & Dark semantic color tokens (`#0068FF` / `#2563EB` primary, `#059669` online accent)
//! - Filterable conversation sidebar (`Tất cả`, `Cá nhân`, `Nhóm`), avatar badges, and grouped chat bubbles
//! - Built-in Zalo Sticker rendering & picker (`[Nhãn dán]`)

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;

use eframe::egui::{
    self, Align, Color32, CursorIcon, FontId, Id, Key, Layout, Margin, Pos2, Rect, RichText,
    Rounding, Sense, Stroke, TextureHandle, TextureOptions, Vec2,
};

use crate::chat::{ChatMessage, Thread};
use crate::logger::{self, FindingLevel, LogEntry, ScanFinding};
use crate::qr::QrLogin;
use crate::session::Session;

mod messages;
mod modals;
mod stickers;
mod theme;
mod worker;
mod workspace;

use messages::*;
use stickers::*;
use theme::*;
pub use theme::{emoji_to_tex_key, load_app_icon, lucide, REACTION_LIST};
pub use worker::WorkerCmd;
use worker::*;

pub enum GuiEvent {
    QrReady {
        qr_login: QrLogin,
        qr_image: egui::ColorImage,
    },
    QrScanned {
        name: String,
    },
    LoginSuccess {
        name: String,
        uin: String,
        avatar: Option<String>,
    },
    LoginFailed(String),
    ThreadsUpdated(Vec<Thread>),
    MessageSent {
        thread_id: String,
        text: String,
        msg_id: String,
    },
    StickerSent {
        thread_id: String,
        msg_id: String,
        sticker_id: u64,
        cate_id: u64,
        sticker_type: u32,
    },
    AttachmentSent {
        thread_id: String,
        msg: ChatMessage,
    },
    StickerImageReady {
        key: String,
        image: egui::ColorImage,
    },
    StickerStoreLoaded(Vec<crate::chat::StickerPackInfo>),
    StickerPackItemsLoaded {
        cate_id: u64,
        name: String,
        items: Vec<crate::chat::StickerItem>,
    },
    StickerSearchResults {
        keyword: String,
        items: Vec<crate::chat::StickerItem>,
    },
    StickerSuggestKeywordsLoaded(Vec<String>),
    ComposerStickerSuggestions {
        keyword: String,
        items: Vec<crate::chat::StickerItem>,
    },
    LinkPreviewReady(crate::chat::LinkMetadata),
    PhoneLookupSuccess(crate::chat::PhoneLookupResult),
    PhoneLookupFailed(String),
    FriendStatusChecked {
        user_id: String,
        status: crate::chat::FriendshipStatus,
        online: Option<(bool, String)>,
    },
    QuoteSent {
        thread_id: String,
        text: String,
        msg_id: String,
        quoted: ChatMessage,
    },
    Reacted {
        thread_id: String,
        msg_id: String,
        ts: i64,
        reaction: String,
    },
    MessageDeleted {
        thread_id: String,
        msg_id: String,
        ts: i64,
    },
    MessageRecalled {
        thread_id: String,
        msg_id: String,
        ts: i64,
    },
    SendFailed {
        thread_id: String,
        text: String,
        error: String,
    },
    SessionExpired(String),
    /// The server could not be reached: the session is intact, show it and retry (#21).
    SessionUnreachable(String),
    /// `zalo_history.db` + `threads_cache.json` were removed on request (#21).
    LocalDataWiped,
    CallStateChanged(String),
    IncomingCall {
        call_id: i64,
        caller_id: String,
        caller_name: String,
        is_video: bool,
        is_group: bool,
    },
    CallRejected {
        code: i64,
        reason: String,
    },
    StatusUpdate(String),
    UpdateCheckFinished {
        result: Result<UpdateCheckResult, String>,
        silent: bool,
    },
    UpdateDownloadStatus {
        downloading: bool,
        message: String,
    },
    UpdateDownloadProgress {
        downloaded: u64,
        total: u64,
    },
    UpdateAppliedAndRestarting(String),
    LoggedOut,
}

#[derive(Debug, Clone)]
pub struct UpdateCheckResult {
    pub current_version: String,
    pub latest_version: String,
    pub has_update: bool,
    pub release_title: String,
    pub release_notes: String,
    pub html_url: String,
    pub download_url: Option<String>,
    pub asset_name: Option<String>,
    pub asset_size: u64,
    pub expected_sha256: Option<String>,
    pub published_at: String,
}

/// Compare semantic versions (`"v0.0.2"` > `"0.0.1"`).
pub fn is_newer_semver(remote: &str, current: &str) -> bool {
    let parse = |s: &str| -> (u64, u64, u64) {
        let clean = s
            .trim()
            .trim_start_matches(['v', 'V'])
            .split(['-', '+'])
            .next()
            .unwrap_or("");
        let mut parts = clean
            .split('.')
            .filter_map(|p| p.trim().parse::<u64>().ok());
        (
            parts.next().unwrap_or(0),
            parts.next().unwrap_or(0),
            parts.next().unwrap_or(0),
        )
    };
    parse(remote) > parse(current)
}

#[derive(Debug, Clone)]
struct ActiveCallOverlay {
    partner_id: String,
    partner_name: String,
    is_video: bool,
    is_group: bool,
    state_label: String,
    started_at: std::time::Instant,
    mic_muted: bool,
    cam_enabled: bool,
    is_incoming_ringing: bool,
    error_code: Option<i64>,
    error_reason: Option<String>,
}

#[derive(Debug, Clone)]
struct ImageViewerModal {
    tex_key: String,
    hd_tex_key: Option<String>,
    full_url: String,
    title: String,
    sender: String,
    time: String,
    zoom: f32,
    pan: Vec2,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsTab {
    General,
    Privacy,
    Sync,
    Appearance,
    Notifications,
    Messages,
    Utilities,
    Update,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct AppSettings {
    // 1. Cài đặt chung
    show_all_contacts: bool,
    language: String,
    autostart_on_boot: bool,
    remember_login: bool,
    download_dir: String,
    // 2. Quyền riêng tư
    show_birthday: String,
    show_online_status: bool,
    show_seen_status: bool,
    allow_message_from: String,
    allow_call_from: String,
    allow_search_by_phone: bool,
    friend_src_qr: bool,
    friend_src_group: bool,
    friend_src_card: bool,
    // 3. Giao diện
    theme_mode: String, // "light" | "dark" | "system"
    use_avatar_as_wallpaper: bool,
    // 4. Thông báo
    enable_notifications: bool,
    play_notification_sound: bool,
    // 5. Tin nhắn
    quick_messages_enabled: bool,
    hidden_chat_pin_enabled: bool,
    split_priority_inbox: bool,
    excel_paste_mode: String, // "image" | "text" | "ask"
    double_click_to_reply: bool,
    show_typing_indicator: bool,
    // 6. Tiện ích
    suggest_stickers_while_typing: bool,
    suggest_mentions: bool,
    enable_gif_search: bool,
    enable_sticker_keyword_search: bool,
    // 7. Thiết bị cuộc gọi (Mic & Camera)
    selected_mic_device: String,
    selected_camera_device: String,
}

impl Default for AppSettings {
    fn default() -> Self {
        let default_dl = std::env::var("USERPROFILE")
            .ok()
            .map(|p| format!(r"{p}\Downloads\Zalo Received Files"))
            .or_else(|| {
                std::env::var("HOME")
                    .ok()
                    .map(|h| format!("{h}/Downloads/Zalo Received Files"))
            })
            .unwrap_or_else(|| "Downloads/Zalo Received Files".into());
        Self {
            show_all_contacts: false,
            language: "Tiếng Việt".into(),
            autostart_on_boot: true,
            remember_login: true,
            download_dir: default_dl,
            show_birthday: "Không hiện".into(),
            show_online_status: true,
            show_seen_status: true,
            allow_message_from: "Tất cả mọi người".into(),
            allow_call_from: "Bạn bè và người lạ từng liên hệ".into(),
            allow_search_by_phone: true,
            friend_src_qr: true,
            friend_src_group: true,
            friend_src_card: true,
            theme_mode: "dark".into(),
            use_avatar_as_wallpaper: false,
            enable_notifications: true,
            play_notification_sound: false,
            quick_messages_enabled: true,
            hidden_chat_pin_enabled: true,
            split_priority_inbox: false,
            excel_paste_mode: "ask".into(),
            double_click_to_reply: true,
            show_typing_indicator: true,
            suggest_stickers_while_typing: true,
            suggest_mentions: true,
            enable_gif_search: true,
            enable_sticker_keyword_search: true,
            selected_mic_device: String::new(),
            selected_camera_device: String::new(),
        }
    }
}

impl AppSettings {
    fn load() -> Self {
        std::fs::read_to_string("app_settings.json")
            .ok()
            .and_then(|s| serde_json::from_str::<Self>(&s).ok())
            .unwrap_or_default()
    }

    fn save(&self) {
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write("app_settings.json", json);
        }
    }
}

/// Bubbles rendered per step in the chat view; "Tải thêm tin nhắn cũ" adds another step.
const CHAT_RENDER_STEP: usize = 150;

pub struct ZaloGuiApp {
    // Theme state
    dark_mode: bool,
    theme_dirty: bool,

    // Session state
    is_logged_in: bool,
    user_name: String,
    user_uin: String,
    user_avatar: Option<String>,

    // Login screen state
    login_tab: LoginTab,
    phone_input: String,
    password_input: String,
    login_in_progress: bool,
    login_status_msg: String,

    // QR state
    qr_texture: Option<TextureHandle>,
    qr_status: String,

    // Chat screen state
    threads: Vec<Thread>,
    selected_thread_index: usize,
    thread_filter: ThreadFilter,
    search_query: String,
    input_message: String,
    status_msg: String,

    // Reply / forward state
    replying_to: Option<ChatMessage>,
    forward_msg: Option<ChatMessage>,
    forward_filter: String,
    filter_starred_only: bool,
    active_call: Option<ActiveCallOverlay>,
    image_viewer: Option<ImageViewerModal>,

    info_drawer_open: bool,
    profile_menu_open: bool,
    /// "Xoá dữ liệu tin nhắn cục bộ" is pending confirmation (#21).
    wipe_confirm_open: bool,
    toast_notice: Option<(String, std::time::Instant)>,
    phone_modal_open: bool,
    phone_modal_input: String,
    phone_lookup_loading: bool,
    phone_lookup_result: Option<crate::chat::PhoneLookupResult>,
    phone_lookup_error: Option<String>,
    in_chat_search_open: bool,
    in_chat_search_query: String,
    in_chat_search_focus_pending: bool,
    // Sticker picker & image cache state
    sticker_picker_open: bool,
    sticker_tab: StickerTab,
    sticker_textures: HashMap<String, TextureHandle>,
    loading_stickers: HashSet<String>,
    emoji_textures: HashMap<String, TextureHandle>,
    app_logo_texture: Option<TextureHandle>,
    sticker_store_packs: Vec<crate::chat::StickerPackInfo>,
    sticker_store_loading: bool,
    sticker_store_filter: String,
    sticker_pack_items: HashMap<u64, Vec<crate::chat::StickerItem>>,
    loading_sticker_packs: HashSet<u64>,
    pinned_sticker_packs: Vec<(u64, String)>,
    sticker_search_query: String,
    sticker_search_results: Vec<crate::chat::StickerItem>,
    sticker_search_loading: bool,
    last_sticker_search_dispatched: String,
    last_sticker_search_edit_at: std::time::Instant,
    sticker_suggest_keywords: HashSet<String>,
    composer_sticker_cache: HashMap<String, Vec<crate::chat::StickerItem>>,
    composer_sticker_loading: HashSet<String>,
    dismissed_sticker_suggest_for: String,
    last_composer_text: String,
    last_composer_edit_at: std::time::Instant,
    composer_sticker_selected: Option<usize>,
    composer_focus_pending: bool,
    pending_clipboard_image: Option<(std::path::PathBuf, u32, u32, TextureHandle)>,
    unread_counts: HashMap<String, usize>,
    link_metadata_cache: HashMap<String, crate::chat::LinkMetadata>,
    loading_link_previews: HashSet<String>,
    dismissed_link_preview_for: String,
    friend_status_map: HashMap<String, crate::chat::FriendshipStatus>,
    checking_friend_status: HashSet<String>,
    user_online_map: HashMap<String, (bool, String)>,
    allow_quit: bool,
    settings_modal_open: bool,
    settings_tab: SettingsTab,
    settings: AppSettings,
    settings_blur_tex: Option<TextureHandle>,
    settings_blur_req_at: Option<std::time::Instant>,
    update_checking: bool,
    update_downloading: bool,
    update_progress: Option<(u64, u64)>,
    update_status: String,
    update_info: Option<UpdateCheckResult>,
    dev_mode: bool,
    dev_panel_open: bool,
    dev_tab: DevTab,
    selected_log: Option<LogEntry>,
    scanner_findings: Vec<ScanFinding>,

    // Background channels
    event_rx: Receiver<GuiEvent>,
    #[allow(dead_code)]
    event_tx: Sender<GuiEvent>,
    /// (thread id, number of newest bubbles rendered) — see `CHAT_RENDER_STEP`.
    chat_render_window: (String, usize),

    // Background poll trigger
    last_poll: std::time::Instant,
}

#[derive(Debug, PartialEq, Eq)]
enum LoginTab {
    QrCode,
    Password,
}

#[derive(Debug, PartialEq, Eq)]
enum DevTab {
    Logs,
    Scanner,
    CryptoState,
}

/// Every user-visible string in `threads` (names + message bodies), for font coverage checks.
fn thread_texts(threads: &[Thread]) -> impl Iterator<Item = &str> {
    threads.iter().flat_map(|t| {
        std::iter::once(t.name.as_str()).chain(t.messages.iter().map(|m| m.text.as_str()))
    })
}

impl ZaloGuiApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        session: Session,
        dev_mode: bool,
        initial_threads: Vec<Thread>,
    ) -> Self {
        setup_custom_fonts(&cc.egui_ctx);
        let loaded_settings = AppSettings::load();
        crate::media::set_selected_mic_device(&loaded_settings.selected_mic_device);
        crate::media::set_selected_camera_device(&loaded_settings.selected_camera_device);
        let dark_mode = loaded_settings.theme_mode != "light";
        if dark_mode {
            ThemePalette::dark().apply_to_ctx(&cc.egui_ctx);
        } else {
            ThemePalette::light().apply_to_ctx(&cc.egui_ctx);
        }

        let (tx, rx) = channel();

        // A session the server already refused is not "logged in": jump straight to the
        // login prompt instead of re-testing it (#21).
        let is_logged_in = session.login.is_some() && !session.is_expired_marked();
        let (user_name, user_uin, user_avatar) = session
            .login
            .as_ref()
            .map(|l| {
                (
                    l.display_name.clone(),
                    l.uin.clone(),
                    if l.avatar.is_empty() {
                        None
                    } else {
                        Some(l.avatar.clone())
                    },
                )
            })
            .unwrap_or_default();

        // main() already read threads_cache.json and hydrated it from SQLite; reading, merging,
        // hydrating and rewriting the cache again here doubled startup I/O.
        let mut threads = initial_threads;
        if let Some(li) = session.login.as_ref() {
            if !li.send2me_id.is_empty() {
                for t in threads.iter_mut() {
                    if !t.is_group && t.name.contains("Cloud của tôi") {
                        t.id = li.send2me_id.clone();
                    }
                }
            }
        }
        crate::chat::deduplicate_threads(&mut threads, None);

        let selected_thread_index = threads
            .iter()
            .enumerate()
            .max_by_key(|(_, t)| t.messages.last().map(|m| m.ts).unwrap_or(0))
            .map(|(idx, _)| idx)
            .unwrap_or(0);

        let app = Self {
            dark_mode,
            theme_dirty: false,
            is_logged_in,
            user_name,
            user_uin,
            user_avatar,
            login_tab: LoginTab::QrCode,
            phone_input: String::new(),
            password_input: String::new(),
            login_in_progress: false,
            login_status_msg: String::new(),
            qr_texture: None,
            qr_status: "Đang tạo mã QR đăng nhập…".into(),
            threads,
            selected_thread_index,
            thread_filter: ThreadFilter::All,
            search_query: String::new(),
            input_message: String::new(),
            replying_to: None,
            forward_msg: None,
            forward_filter: String::new(),
            filter_starred_only: false,
            active_call: None,
            image_viewer: None,
            status_msg: "Trực tuyến · Sẵn sàng".into(),
            info_drawer_open: true,
            profile_menu_open: false,
            wipe_confirm_open: false,
            toast_notice: None,
            phone_modal_open: false,
            phone_modal_input: String::new(),
            phone_lookup_loading: false,
            phone_lookup_result: None,
            phone_lookup_error: None,
            in_chat_search_open: false,
            in_chat_search_query: String::new(),
            in_chat_search_focus_pending: false,
            sticker_picker_open: false,
            sticker_tab: StickerTab::Popular,
            sticker_textures: HashMap::new(),
            loading_stickers: HashSet::new(),
            emoji_textures: load_default_emoji_textures(&cc.egui_ctx),
            app_logo_texture: load_app_logo_texture(&cc.egui_ctx),
            sticker_store_packs: Vec::new(),
            sticker_store_loading: false,
            sticker_store_filter: String::new(),
            sticker_pack_items: FEATURED_PACK_TABS
                .iter()
                .filter_map(|&(cid, _)| builtin_pack_stickers(cid).map(|items| (cid, items)))
                .collect(),
            loading_sticker_packs: HashSet::new(),
            pinned_sticker_packs: FEATURED_PACK_TABS
                .iter()
                .map(|&(cid, title)| (cid, title.to_string()))
                .collect(),
            sticker_search_query: String::new(),
            sticker_search_results: Vec::new(),
            sticker_search_loading: false,
            last_sticker_search_dispatched: String::new(),
            last_sticker_search_edit_at: std::time::Instant::now(),
            sticker_suggest_keywords: HashSet::new(),
            composer_sticker_cache: HashMap::new(),
            composer_sticker_loading: HashSet::new(),
            dismissed_sticker_suggest_for: String::new(),
            last_composer_text: String::new(),
            last_composer_edit_at: std::time::Instant::now(),
            composer_sticker_selected: None,
            composer_focus_pending: false,
            pending_clipboard_image: None,
            unread_counts: HashMap::new(),
            link_metadata_cache: HashMap::new(),
            loading_link_previews: HashSet::new(),
            dismissed_link_preview_for: String::new(),
            friend_status_map: HashMap::new(),
            checking_friend_status: HashSet::new(),
            user_online_map: HashMap::new(),
            allow_quit: false,
            settings_modal_open: false,
            settings_tab: SettingsTab::General,
            settings: loaded_settings,
            settings_blur_tex: None,
            settings_blur_req_at: None,
            update_checking: false,
            update_downloading: false,
            update_progress: None,
            update_status: String::new(),
            update_info: None,
            dev_mode,
            dev_panel_open: false,
            dev_tab: DevTab::Logs,
            selected_log: None,
            scanner_findings: Vec::new(),
            event_rx: rx,
            event_tx: tx.clone(),
            chat_render_window: (String::new(), CHAT_RENDER_STEP),
            last_poll: std::time::Instant::now(),
        };

        // Clean up any leftover `.old` executable from a previous in-place self-update
        if let Ok(exe_path) = std::env::current_exe() {
            let old_path = exe_path.with_extension("exe.old");
            let _ = std::fs::remove_file(&old_path);
            let old_unix = exe_path.with_extension("old");
            let _ = std::fs::remove_file(&old_unix);
        }

        spawn_session_worker(session, tx, cc.egui_ctx.clone());
        ensure_cjk_font(&cc.egui_ctx, thread_texts(&app.threads));

        if let Some(wtx) = WORKER_TX.get() {
            if is_logged_in {
                let _ = wtx.send(WorkerCmd::RefreshThreads);
            } else {
                let _ = wtx.send(WorkerCmd::RequestQr);
            }
            let _ = wtx.send(WorkerCmd::CheckForUpdates { silent: true });
        }
        app
    }

    fn palette(&self) -> ThemePalette {
        if self.dark_mode {
            ThemePalette::dark()
        } else {
            ThemePalette::light()
        }
    }

    fn persist_threads_async(&self) {
        let snapshot = self.threads.clone();
        if let Some(wtx) = WORKER_TX.get() {
            let _ = wtx.send(WorkerCmd::PersistThreads(snapshot));
        }
    }

    fn check_events(&mut self, ctx: &egui::Context) {
        while let Ok(event) = self.event_rx.try_recv() {
            match event {
                GuiEvent::QrReady { qr_image, .. } => {
                    self.qr_texture =
                        Some(ctx.load_texture("qr_code_tex", qr_image, TextureOptions::NEAREST));
                    self.qr_status = "Mở ứng dụng Zalo trên điện thoại và quét mã QR".into();
                    self.login_in_progress = false;
                }
                GuiEvent::QrScanned { name } => {
                    self.qr_status =
                        format!("Đã quét bởi {name} — Vui lòng bấm 'Đăng nhập' trên điện thoại…");
                }
                GuiEvent::LoginSuccess { name, uin, avatar } => {
                    self.is_logged_in = true;
                    self.user_name = name;
                    self.user_uin = uin;
                    if avatar.is_some() {
                        self.user_avatar = avatar;
                    }
                    self.login_in_progress = false;
                    self.composer_sticker_cache.retain(|_, v| !v.is_empty());
                    self.status_msg = "Trực tuyến · Đồng bộ thời gian thực".into();
                }
                GuiEvent::LoginFailed(err) => {
                    self.login_in_progress = false;
                    self.login_status_msg = format!("Lỗi đăng nhập: {err}");
                }
                GuiEvent::ThreadsUpdated(updates) => {
                    ensure_cjk_font(ctx, thread_texts(&updates));
                    if self.is_logged_in && !updates.is_empty() {
                        let prev_selected_name = self
                            .threads
                            .get(self.selected_thread_index)
                            .map(|t| (t.name.clone(), t.is_group));
                        let live_ids: HashSet<String> = updates
                            .iter()
                            .filter(|u| !u.id.is_empty() && u.id != "0")
                            .map(|u| u.id.clone())
                            .collect();

                        let active_tid = self
                            .threads
                            .get(self.selected_thread_index)
                            .map(|t| t.id.clone())
                            .unwrap_or_default();
                        let is_focused = ctx.input(|i| i.focused);
                        let mut new_incoming_alert = false;

                        for nt in updates {
                            if let Some(existing) = self.threads.iter_mut().find(|t| t.id == nt.id)
                            {
                                let old_len = existing.messages.len();
                                crate::chat::merge_thread_into(existing, nt, true);
                                let added = existing.messages.len().saturating_sub(old_len);
                                if added > 0 {
                                    let inc_cnt = existing
                                        .messages
                                        .iter()
                                        .rev()
                                        .take(added)
                                        .filter(|m| !m.is_outgoing)
                                        .count();
                                    if inc_cnt > 0 && (existing.id != active_tid || !is_focused) {
                                        *self
                                            .unread_counts
                                            .entry(existing.id.clone())
                                            .or_insert(0) += inc_cnt;
                                        new_incoming_alert = true;
                                    }
                                }
                            } else if let Some(existing) = self.threads.iter_mut().find(|t| {
                                !live_ids.contains(&t.id)
                                    && crate::chat::threads_represent_same_contact(t, &nt)
                            }) {
                                crate::chat::merge_thread_into(existing, nt, true);
                            } else if !nt.id.is_empty() && nt.id != "0" {
                                self.threads.push(nt);
                            }
                        }

                        if new_incoming_alert && self.settings.enable_notifications {
                            let total_unread: usize = self.unread_counts.values().sum();
                            let title = if total_unread > 0 {
                                format!("({total_unread}) ctdozlite")
                            } else {
                                "ctdozlite".to_string()
                            };
                            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
                            if !is_focused {
                                ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
                                    egui::UserAttentionType::Informational,
                                ));
                            }
                        }
                        crate::chat::deduplicate_threads(&mut self.threads, Some(&live_ids));

                        if let Some((sel_name, sel_grp)) = prev_selected_name {
                            if let Some(idx) = self
                                .threads
                                .iter()
                                .position(|t| t.is_group == sel_grp && t.name == sel_name)
                            {
                                self.selected_thread_index = idx;
                            }
                        }
                        if self.selected_thread_index >= self.threads.len() {
                            self.selected_thread_index = self.threads.len().saturating_sub(1);
                        }
                        self.persist_threads_async();
                    }
                    self.status_msg = format!("Trực tuyến · Cập nhật {}", crate::ui::clock());
                }
                GuiEvent::SessionExpired(reason) => {
                    self.is_logged_in = false;
                    self.qr_texture = None;
                    self.qr_status =
                        format!("{reason} — Quét mã QR bên dưới để khôi phục phiên trực tuyến.");
                    self.status_msg = format!("{reason} (Lịch sử tin nhắn vẫn được bảo toàn)");
                }
                GuiEvent::SessionUnreachable(reason) => {
                    // Only the network failed: stay logged in, keep the thread list, and
                    // let the next poll retry (#21).
                    self.status_msg = format!("⚠️ {reason} — đang thử lại…");
                }
                GuiEvent::LocalDataWiped => {
                    self.threads.clear();
                    self.selected_thread_index = 0;
                    self.status_msg = "Đã xoá dữ liệu tin nhắn cục bộ (session giữ nguyên)".into();
                }
                GuiEvent::LoggedOut => {
                    self.is_logged_in = false;
                    self.user_name.clear();
                    self.user_uin.clear();
                    self.threads.clear();
                    self.selected_thread_index = 0;
                    self.qr_texture = None;
                    self.qr_status = "Đang xin mã QR mới…".into();
                    self.status_msg = "Đã đăng xuất".into();
                }
                GuiEvent::MessageSent {
                    thread_id,
                    text,
                    msg_id,
                } => {
                    if let Some(t) = self.threads.iter_mut().find(|t| t.id == thread_id) {
                        t.preview = text.clone();
                        let now = crate::ui::clock();
                        t.time = now.clone();
                        let final_id = if msg_id.is_empty() {
                            format!("out_{}", crate::login::unix_now())
                        } else {
                            msg_id
                        };
                        if !t
                            .messages
                            .iter()
                            .any(|m| !m.id.is_empty() && m.id == final_id)
                        {
                            t.messages.push(ChatMessage {
                                id: final_id,
                                sender: "Tôi".into(),
                                text,
                                time: now,
                                is_outgoing: true,
                                ts: (crate::login::unix_now() * 1000) as i64,
                                ..Default::default()
                            });
                            t.messages.sort_by_key(|m| m.ts);
                        }
                        self.persist_threads_async();
                    }
                    self.status_msg = format!("Đã gửi tin nhắn lúc {}", crate::ui::clock());
                }
                GuiEvent::StickerSent {
                    thread_id,
                    msg_id,
                    sticker_id,
                    cate_id,
                    sticker_type,
                } => {
                    if let Some(t) = self.threads.iter_mut().find(|t| t.id == thread_id) {
                        t.preview = "[Nhãn dán]".into();
                        let now = crate::ui::clock();
                        t.time = now.clone();
                        let final_id = if msg_id.is_empty() {
                            format!("out_{}", crate::login::unix_now())
                        } else {
                            msg_id
                        };
                        if !t
                            .messages
                            .iter()
                            .any(|m| !m.id.is_empty() && m.id == final_id)
                        {
                            t.messages.push(ChatMessage {
                                id: final_id,
                                sender: "Tôi".into(),
                                text: "[Nhãn dán]".into(),
                                time: now,
                                is_outgoing: true,
                                ts: (crate::login::unix_now() * 1000) as i64,
                                sticker_id: Some(sticker_id),
                                sticker_cate_id: Some(cate_id),
                                sticker_type: Some(sticker_type),
                                ..Default::default()
                            });
                            t.messages.sort_by_key(|m| m.ts);
                        }
                        self.persist_threads_async();
                    }
                    self.status_msg = format!("Đã gửi nhãn dán lúc {}", crate::ui::clock());
                }
                GuiEvent::AttachmentSent { thread_id, msg } => {
                    if let Some(t) = self.threads.iter_mut().find(|t| t.id == thread_id) {
                        t.preview = msg.text.clone();
                        t.time = msg.time.clone();
                        if !t
                            .messages
                            .iter()
                            .any(|m| !m.id.is_empty() && m.id == msg.id)
                        {
                            t.messages.push(msg);
                            t.messages.sort_by_key(|m| m.ts);
                        }
                        self.persist_threads_async();
                    }
                    self.status_msg = format!("Đã gửi đính kèm lúc {}", crate::ui::clock());
                }
                GuiEvent::QuoteSent {
                    thread_id,
                    text,
                    msg_id,
                    quoted,
                } => {
                    if let Some(t) = self.threads.iter_mut().find(|t| t.id == thread_id) {
                        t.preview = text.clone();
                        let now = crate::ui::clock();
                        t.time = now.clone();
                        let final_id = if msg_id.is_empty() {
                            format!("out_{}", crate::login::unix_now())
                        } else {
                            msg_id
                        };
                        if !t
                            .messages
                            .iter()
                            .any(|m| !m.id.is_empty() && m.id == final_id)
                        {
                            t.messages.push(ChatMessage {
                                id: final_id,
                                sender: "Tôi".into(),
                                text,
                                time: now,
                                is_outgoing: true,
                                ts: (crate::login::unix_now() * 1000) as i64,
                                reply_to_id: Some(quoted.id.clone()),
                                reply_to_sender: Some(quoted.sender.clone()),
                                reply_to_text: Some(quoted.text.clone()),
                                ..Default::default()
                            });
                            t.messages.sort_by_key(|m| m.ts);
                        }
                        self.persist_threads_async();
                    }
                    self.status_msg = format!("Đã gửi trả lời lúc {}", crate::ui::clock());
                }
                GuiEvent::Reacted {
                    thread_id,
                    msg_id,
                    ts,
                    reaction,
                } => {
                    if let Some(t) = self.threads.iter_mut().find(|t| t.id == thread_id) {
                        if let Some(m) = t.messages.iter_mut().find(|m| {
                            (!m.id.is_empty() && m.id == msg_id) || (m.id.is_empty() && m.ts == ts)
                        }) {
                            m.reactions = vec![reaction];
                        }
                        self.persist_threads_async();
                    }
                    self.status_msg = "Đã thả cảm xúc".into();
                }
                GuiEvent::MessageDeleted {
                    thread_id,
                    msg_id,
                    ts,
                } => {
                    if let Some(t) = self.threads.iter_mut().find(|t| t.id == thread_id) {
                        t.messages.retain(|m| {
                            !((!msg_id.is_empty() && m.id == msg_id)
                                || (msg_id.is_empty() && m.ts == ts))
                        });
                        if let Some(last) = t.messages.last() {
                            t.preview = last.text.clone();
                        }
                        self.persist_threads_async();
                    }
                    self.status_msg = "Đã xóa tin nhắn ở phía bạn".into();
                }
                GuiEvent::MessageRecalled {
                    thread_id,
                    msg_id,
                    ts,
                } => {
                    if let Some(t) = self.threads.iter_mut().find(|t| t.id == thread_id) {
                        if let Some(m) = t.messages.iter_mut().find(|m| {
                            (!msg_id.is_empty() && m.id == msg_id)
                                || (msg_id.is_empty() && m.ts == ts)
                        }) {
                            m.text = "[Tin nhắn đã thu hồi]".into();
                            m.sticker_id = None;
                            m.link_url = None;
                            m.link_thumb = None;
                        }
                        if let Some(last) = t.messages.last() {
                            t.preview = last.text.clone();
                        }
                        self.persist_threads_async();
                    }
                    self.status_msg = "Đã thu hồi tin nhắn với mọi người".into();
                }
                GuiEvent::StickerImageReady { key, image } => {
                    self.loading_stickers.remove(&key);
                    let tex = ctx.load_texture(&key, image, TextureOptions::LINEAR);
                    self.sticker_textures.insert(key, tex);
                }
                GuiEvent::StickerStoreLoaded(packs) => {
                    self.sticker_store_loading = false;
                    let count = packs.len();
                    self.sticker_store_packs = packs;
                    self.status_msg = format!("Đã tải Kho Nhãn Dán Zalo ({count} bộ nhãn)");
                }
                GuiEvent::StickerPackItemsLoaded {
                    cate_id,
                    name,
                    items,
                } => {
                    self.loading_sticker_packs.remove(&cate_id);
                    let count = items.len();
                    if !self
                        .pinned_sticker_packs
                        .iter()
                        .any(|(id, _)| *id == cate_id)
                    {
                        self.pinned_sticker_packs.insert(0, (cate_id, name.clone()));
                    }
                    self.sticker_pack_items.insert(cate_id, items);
                    self.status_msg = format!("Đã mở bộ nhãn \"{name}\" ({count} nhãn dán)");
                }
                GuiEvent::StickerSearchResults { keyword, items } => {
                    self.sticker_search_loading = false;
                    let count = items.len();
                    self.sticker_search_results = items;
                    self.status_msg =
                        format!("Tìm thấy {count} nhãn dán cho từ khóa \"{keyword}\"");
                }
                GuiEvent::StickerSuggestKeywordsLoaded(kws) => {
                    for k in kws {
                        self.sticker_suggest_keywords.insert(k);
                    }
                }
                GuiEvent::ComposerStickerSuggestions { keyword, items } => {
                    self.composer_sticker_loading.remove(&keyword);
                    self.composer_sticker_cache.insert(keyword, items);
                }
                GuiEvent::LinkPreviewReady(meta) => {
                    self.loading_link_previews.remove(&meta.url);
                    if !meta.thumb.is_empty() {
                        let key = format!("thumb_{}", crate::crypto::md5_hex(&meta.thumb));
                        if !self.sticker_textures.contains_key(&key)
                            && self.loading_stickers.insert(key.clone())
                        {
                            if let Some(tx) = WORKER_TX.get() {
                                let _ = tx.send(WorkerCmd::FetchStickerImage {
                                    key,
                                    url: meta.thumb.clone(),
                                });
                            }
                        }
                    }
                    let mut updated_any = false;
                    for t in &mut self.threads {
                        for m in &mut t.messages {
                            let matches_url = m.link_url.as_deref() == Some(meta.url.as_str())
                                || crate::chat::extract_first_url(&m.text).as_deref()
                                    == Some(meta.url.as_str());
                            if matches_url {
                                if m.link_url.is_none() {
                                    m.link_url = Some(meta.url.clone());
                                    updated_any = true;
                                }
                                if (m.link_title.is_none()
                                    || m.link_title.as_deref() == Some(meta.url.as_str()))
                                    && !meta.title.is_empty()
                                {
                                    m.link_title = Some(meta.title.clone());
                                    updated_any = true;
                                }
                                if m.link_desc.is_none() && !meta.desc.is_empty() {
                                    m.link_desc = Some(meta.desc.clone());
                                    updated_any = true;
                                }
                                let old_thumb_expired = m
                                    .link_thumb
                                    .as_deref()
                                    .map(|th| {
                                        th.is_empty()
                                            || (th.contains("zpcloud.zdn.vn")
                                                && !self.sticker_textures.contains_key(&format!(
                                                    "thumb_{}",
                                                    crate::crypto::md5_hex(th)
                                                )))
                                    })
                                    .unwrap_or(true);
                                if old_thumb_expired && !meta.thumb.is_empty() {
                                    m.link_thumb = Some(meta.thumb.clone());
                                    updated_any = true;
                                }
                                if m.link_src.is_none() && !meta.src.is_empty() {
                                    m.link_src = Some(meta.src.clone());
                                    updated_any = true;
                                }
                            }
                        }
                    }
                    self.link_metadata_cache.insert(meta.url.clone(), meta);
                    if updated_any {
                        self.persist_threads_async();
                    }
                }
                GuiEvent::FriendStatusChecked {
                    user_id,
                    status,
                    online,
                } => {
                    self.checking_friend_status.remove(&user_id);
                    self.friend_status_map.insert(user_id.clone(), status);
                    if let Some(onl) = online {
                        self.user_online_map.insert(user_id.clone(), onl);
                    }
                    if let Some(ref mut prof) = self.phone_lookup_result {
                        if prof.user_id == user_id {
                            prof.friendship = status;
                        }
                    }
                }
                GuiEvent::PhoneLookupSuccess(res) => {
                    self.phone_lookup_loading = false;
                    self.phone_lookup_error = None;
                    self.phone_modal_open = true;
                    self.show_toast(format!("Đã tìm thấy tài khoản: {}", res.display_name));
                    self.friend_status_map
                        .insert(res.user_id.clone(), res.friendship);
                    self.phone_lookup_result = Some(res);
                }
                GuiEvent::PhoneLookupFailed(err) => {
                    self.phone_lookup_loading = false;
                    self.phone_lookup_result = None;
                    self.phone_modal_open = true;
                    self.phone_lookup_error = Some(err);
                }
                GuiEvent::SendFailed {
                    thread_id: _,
                    text,
                    error,
                } => {
                    if self.input_message.is_empty() && !text.is_empty() {
                        self.input_message = text;
                    }
                    self.status_msg = format!("Không gửi được: {error}");
                }
                GuiEvent::CallStateChanged(state) => {
                    if state == "free" {
                        // Keep modal open if there is a rejection reason so the user can read it
                        let has_error = self
                            .active_call
                            .as_ref()
                            .and_then(|c| c.error_reason.as_ref())
                            .is_some();
                        self.sticker_textures.remove("call_remote_video");
                        self.sticker_textures.remove("call_remote_video_1");
                        self.sticker_textures.remove("call_remote_video_2");
                        self.sticker_textures.remove("call_remote_video_3");
                        self.sticker_textures.remove("call_local_video");
                        if !has_error {
                            self.active_call = None;
                        }
                    } else if let Some(ref mut call) = self.active_call {
                        if state == "incall" || state == "connected" {
                            call.is_incoming_ringing = false;
                        }
                        if call.error_reason.is_none() {
                            call.state_label = match state.as_str() {
                                "calling" | "ringing" => "Đang đổ chuông…".into(),
                                "incoming_ringing" => "Cuộc gọi đến đang đổ chuông…".into(),
                                "connecting" => "Đang kết nối luồng âm thanh/video…".into(),
                                "incall" | "connected" => {
                                    "Đang trong cuộc gọi (Pure-Rust VoIP)".into()
                                }
                                _ => format!("Đang kết nối ({state})…"),
                            };
                        }
                    }
                }
                GuiEvent::IncomingCall {
                    call_id: _,
                    caller_id,
                    caller_name,
                    is_video,
                    is_group,
                } => {
                    let resolved_name = self
                        .threads
                        .iter()
                        .find(|t| t.id == caller_id)
                        .map(|t| t.name.clone())
                        .unwrap_or(caller_name);
                    self.status_msg = format!(
                        "📲 Cuộc gọi {} đến từ {resolved_name}…",
                        if is_video { "video" } else { "thoại" }
                    );
                    self.active_call = Some(ActiveCallOverlay {
                        partner_id: caller_id,
                        partner_name: resolved_name,
                        is_video,
                        is_group,
                        state_label: "Cuộc gọi đến đang đổ chuông…".into(),
                        started_at: std::time::Instant::now(),
                        mic_muted: false,
                        cam_enabled: is_video,
                        is_incoming_ringing: true,
                        error_code: None,
                        error_reason: None,
                    });
                    ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
                        egui::UserAttentionType::Critical,
                    ));
                }
                GuiEvent::CallRejected { code, reason } => {
                    self.status_msg = format!("⚠️ Từ chối cuộc gọi (mã {code}): {reason}");
                    if let Some(ref mut call) = self.active_call {
                        call.state_label = "Không thể thực hiện cuộc gọi".into();
                        call.error_code = Some(code);
                        call.error_reason = Some(reason);
                    }
                }
                GuiEvent::StatusUpdate(msg) => {
                    self.status_msg = msg;
                }
                GuiEvent::UpdateCheckFinished { result, silent } => {
                    self.update_checking = false;
                    match result {
                        Ok(info) => {
                            if info.has_update {
                                let msg = format!(
                                    "🚀 Có bản cập nhật mới v{} trên GitHub (hiện tại: v{})",
                                    info.latest_version, info.current_version
                                );
                                self.update_status = msg.clone();
                                self.show_toast(msg);
                            } else {
                                self.update_status = format!(
                                    "✅ Bạn đang sử dụng phiên bản mới nhất (v{})",
                                    info.current_version
                                );
                                if !silent {
                                    self.show_toast(&self.update_status.clone());
                                }
                            }
                            self.update_info = Some(info);
                        }
                        Err(e) => {
                            self.update_status = format!("⚠️ Không kiểm tra được cập nhật: {e}");
                            if !silent {
                                self.show_toast(&self.update_status.clone());
                            }
                        }
                    }
                }
                GuiEvent::UpdateDownloadStatus {
                    downloading,
                    message,
                } => {
                    self.update_downloading = downloading;
                    if !downloading {
                        self.update_progress = None;
                    }
                    self.update_status = message.clone();
                    self.show_toast(message);
                }
                GuiEvent::UpdateDownloadProgress { downloaded, total } => {
                    self.update_downloading = true;
                    self.update_progress = Some((downloaded, total));
                }
                GuiEvent::UpdateAppliedAndRestarting(ver) => {
                    self.update_downloading = false;
                    self.show_toast(format!("✅ Đã cập nhật lên v{ver} — Đang khởi động lại…"));
                    self.allow_quit = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    std::process::exit(0);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Lightweight Background Session Worker (2-thread Tokio pool, off-UI disk I/O)
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Main eframe::App Implementation
// ---------------------------------------------------------------------------

impl eframe::App for ZaloGuiApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if ctx.input(|i| i.modifiers.ctrl && i.key_pressed(Key::Q))
            || ctx.input(|i| i.viewport().close_requested())
        {
            self.allow_quit = true;
            if let Some(tx) = WORKER_TX.get() {
                let _ = tx.send(WorkerCmd::EndCall);
            }
            std::process::exit(0);
        }
        if self.theme_dirty {
            self.palette().apply_to_ctx(ctx);
            self.theme_dirty = false;
        }

        self.check_events(ctx);

        // Non-overlapping long-poll trigger; wakes only every 1.2s to keep idle CPU ~0%
        if self.is_logged_in && self.last_poll.elapsed() >= Duration::from_millis(1200) {
            self.last_poll = std::time::Instant::now();
            if let Some(tx) = WORKER_TX.get() {
                let known: Vec<(String, String, bool)> = self
                    .threads
                    .iter()
                    .map(|t| (t.id.clone(), t.name.clone(), t.is_group))
                    .collect();
                let _ = tx.send(WorkerCmd::PollMessages {
                    known_threads: known,
                });
            }
        }

        ctx.request_repaint_after(Duration::from_millis(1200));

        let pal = self.palette();

        Self::handle_frameless_resize(ctx);
        self.render_custom_title_bar(ctx, &pal);
        // Dev Inspector Panel
        if self.dev_mode && self.dev_panel_open {
            egui::TopBottomPanel::bottom("dev_inspector")
                .resizable(true)
                .default_height(250.0)
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.strong("Dev Inspector & Protocol Scanner");
                        ui.separator();
                        ui.selectable_value(
                            &mut self.dev_tab,
                            DevTab::Logs,
                            "Lưu lượng mạng (Logs)",
                        );
                        ui.selectable_value(
                            &mut self.dev_tab,
                            DevTab::Scanner,
                            "Quét lỗi hệ thống (Scanner)",
                        );
                        ui.selectable_value(
                            &mut self.dev_tab,
                            DevTab::CryptoState,
                            "Trạng thái Crypto & Keys",
                        );

                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.button("Làm mới quét").clicked() {
                                self.scanner_findings = logger::scan_logs();
                            }
                            if ui.button("Xóa log").clicked() {
                                logger::clear_entries();
                                self.selected_log = None;
                                self.scanner_findings.clear();
                            }
                        });
                    });
                    ui.separator();

                    match self.dev_tab {
                        DevTab::Logs => self.render_dev_logs(ui),
                        DevTab::Scanner => self.render_dev_scanner(ui),
                        DevTab::CryptoState => self.render_dev_crypto(ui),
                    }
                });
        }

        // Main Workspace
        if !self.is_logged_in {
            egui::CentralPanel::default()
                .frame(egui::Frame::none().fill(pal.bg_chat_canvas))
                .show(ctx, |ui| {
                    self.render_login_screen(ui, &pal);
                });
        } else {
            self.render_chat_workspace(ctx, &pal);
        }

        if !ctx.input(|i| i.viewport().maximized.unwrap_or(false)) {
            let screen = ctx.screen_rect();
            ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                Id::new("window_border_overlay"),
            ))
            .rect_stroke(
                screen.shrink(0.5),
                Rounding::ZERO,
                Stroke::new(1.0_f32, pal.border),
            );
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        std::process::exit(0);
    }
}

// ---------------------------------------------------------------------------
// Screen Renderers
// ---------------------------------------------------------------------------

impl ZaloGuiApp {
    fn handle_frameless_resize(ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().maximized.unwrap_or(false)) {
            return;
        }
        let screen = ctx.screen_rect();
        let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else {
            return;
        };
        let border = 5.0_f32;
        let corner = 9.0_f32;
        let left = (pos.x - screen.left()).abs() <= border;
        let right = (screen.right() - pos.x).abs() <= border;
        let top = (pos.y - screen.top()).abs() <= border;
        let bottom = (screen.bottom() - pos.y).abs() <= border;

        let left_c = (pos.x - screen.left()).abs() <= corner;
        let right_c = (screen.right() - pos.x).abs() <= corner;
        let top_c = (pos.y - screen.top()).abs() <= corner;
        let bottom_c = (screen.bottom() - pos.y).abs() <= corner;

        let dir_and_cursor = if top_c && left_c && (top || left) {
            Some((egui::ResizeDirection::NorthWest, CursorIcon::ResizeNwSe))
        } else if top_c && right_c && (top || right) {
            Some((egui::ResizeDirection::NorthEast, CursorIcon::ResizeNeSw))
        } else if bottom_c && left_c && (bottom || left) {
            Some((egui::ResizeDirection::SouthWest, CursorIcon::ResizeNeSw))
        } else if bottom_c && right_c && (bottom || right) {
            Some((egui::ResizeDirection::SouthEast, CursorIcon::ResizeNwSe))
        } else if left {
            Some((egui::ResizeDirection::West, CursorIcon::ResizeHorizontal))
        } else if right {
            Some((egui::ResizeDirection::East, CursorIcon::ResizeHorizontal))
        } else if top {
            Some((egui::ResizeDirection::North, CursorIcon::ResizeVertical))
        } else if bottom {
            Some((egui::ResizeDirection::South, CursorIcon::ResizeVertical))
        } else {
            None
        };

        if let Some((dir, cursor)) = dir_and_cursor {
            ctx.set_cursor_icon(cursor);
            if ctx.input(|i| i.pointer.button_pressed(egui::PointerButton::Primary)) {
                ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(dir));
            }
        }
    }

    fn render_custom_title_bar(&mut self, ctx: &egui::Context, pal: &ThemePalette) {
        let is_maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
        let total_unread: usize = self.unread_counts.values().sum();

        egui::TopBottomPanel::top("custom_title_bar")
            .exact_height(32.0)
            .frame(
                egui::Frame::none()
                    .fill(pal.bg_app)
                    .inner_margin(Margin {
                        left: 12.0,
                        right: 0.0,
                        top: 0.0,
                        bottom: 0.0,
                    })
                    .stroke(Stroke::new(1.0_f32, pal.border)),
            )
            .show(ctx, |ui| {
                let bar_rect = ui.max_rect();
                let mut buttons_left_x = bar_rect.right();

                ui.horizontal_centered(|ui| {
                    // 1. App Logo (20x20)
                    let (logo_rect, _) =
                        ui.allocate_exact_size(Vec2::new(20.0, 20.0), Sense::hover());
                    if let Some(tex) = &self.app_logo_texture {
                        ui.painter().image(
                            tex.id(),
                            logo_rect,
                            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    } else {
                        ui.painter()
                            .rect_filled(logo_rect, Rounding::same(5.0), pal.primary);
                    }

                    ui.add_space(6.0);
                    ui.label(
                        RichText::new("ctdozlite")
                            .color(pal.text_primary)
                            .size(13.5)
                            .strong(),
                    );

                    // Subtle "Native v<version>" pill badge
                    egui::Frame::none()
                        .fill(pal.primary.linear_multiply(0.14))
                        .rounding(Rounding::same(4.0))
                        .inner_margin(Margin::symmetric(5.0, 1.5))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(format!("Native v{}", env!("CARGO_PKG_VERSION")))
                                    .color(pal.primary)
                                    .size(10.5)
                                    .strong(),
                            );
                        });

                    if total_unread > 0 {
                        ui.add_space(4.0);
                        egui::Frame::none()
                            .fill(pal.danger)
                            .rounding(Rounding::same(8.0))
                            .inner_margin(Margin::symmetric(6.0, 1.5))
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new(format!("{total_unread} tin mới"))
                                        .color(Color32::WHITE)
                                        .size(10.5)
                                        .strong(),
                                );
                            });
                    } else if self.is_logged_in && !self.user_name.is_empty() {
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new(format!("·  {}", self.user_name))
                                .color(pal.text_muted)
                                .size(12.0),
                        );
                    }

                    // 2. Right-side Window Control Buttons
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;

                        // Close Button (46x32)
                        let (close_rect, close_resp) =
                            ui.allocate_exact_size(Vec2::new(46.0, 32.0), Sense::click());
                        if close_resp.hovered() {
                            ui.painter().rect_filled(
                                close_rect,
                                Rounding::ZERO,
                                Color32::from_rgb(232, 17, 35),
                            );
                        }
                        let close_fg = if close_resp.hovered() {
                            Color32::WHITE
                        } else {
                            pal.text_secondary
                        };
                        let cc = close_rect.center();
                        let d = 4.6_f32;
                        ui.painter().line_segment(
                            [Pos2::new(cc.x - d, cc.y - d), Pos2::new(cc.x + d, cc.y + d)],
                            Stroke::new(1.4_f32, close_fg),
                        );
                        ui.painter().line_segment(
                            [Pos2::new(cc.x + d, cc.y - d), Pos2::new(cc.x - d, cc.y + d)],
                            Stroke::new(1.4_f32, close_fg),
                        );
                        let close_resp = close_resp.on_hover_text("Đóng ứng dụng (Ctrl+Q)");
                        if close_resp.clicked() || close_resp.secondary_clicked() {
                            self.allow_quit = true;
                            if let Some(tx) = WORKER_TX.get() {
                                let _ = tx.send(WorkerCmd::EndCall);
                            }
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            std::process::exit(0);
                        }

                        // Maximize / Restore Button (46x32)
                        let (max_rect, max_resp) =
                            ui.allocate_exact_size(Vec2::new(46.0, 32.0), Sense::click());
                        if max_resp.hovered() {
                            ui.painter()
                                .rect_filled(max_rect, Rounding::ZERO, pal.bg_hover);
                        }
                        let max_fg = if max_resp.hovered() {
                            pal.text_primary
                        } else {
                            pal.text_secondary
                        };
                        let mc = max_rect.center();
                        if is_maximized {
                            let back = Rect::from_center_size(
                                Pos2::new(mc.x + 1.2, mc.y - 1.2),
                                Vec2::splat(7.8),
                            );
                            let front = Rect::from_center_size(
                                Pos2::new(mc.x - 1.2, mc.y + 1.2),
                                Vec2::splat(7.8),
                            );
                            ui.painter().rect_stroke(
                                back,
                                Rounding::same(1.0),
                                Stroke::new(1.25_f32, max_fg),
                            );
                            ui.painter().rect_filled(
                                front,
                                Rounding::same(1.0),
                                if max_resp.hovered() {
                                    pal.bg_hover
                                } else {
                                    pal.bg_app
                                },
                            );
                            ui.painter().rect_stroke(
                                front,
                                Rounding::same(1.0),
                                Stroke::new(1.25_f32, max_fg),
                            );
                        } else {
                            let sq = Rect::from_center_size(mc, Vec2::splat(9.5));
                            ui.painter().rect_stroke(
                                sq,
                                Rounding::same(1.2),
                                Stroke::new(1.3_f32, max_fg),
                            );
                        }
                        if max_resp
                            .on_hover_text(if is_maximized {
                                "Khôi phục kích thước"
                            } else {
                                "Phóng to toàn màn hình"
                            })
                            .clicked()
                        {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!is_maximized));
                        }

                        // Minimize Button (46x32)
                        let (min_rect, min_resp) =
                            ui.allocate_exact_size(Vec2::new(46.0, 32.0), Sense::click());
                        if min_resp.hovered() {
                            ui.painter()
                                .rect_filled(min_rect, Rounding::ZERO, pal.bg_hover);
                        }
                        let min_fg = if min_resp.hovered() {
                            pal.text_primary
                        } else {
                            pal.text_secondary
                        };
                        let mnc = min_rect.center();
                        ui.painter().line_segment(
                            [Pos2::new(mnc.x - 5.0, mnc.y), Pos2::new(mnc.x + 5.0, mnc.y)],
                            Stroke::new(1.4_f32, min_fg),
                        );
                        if min_resp.on_hover_text("Thu nhỏ").clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                        }

                        // Quick Theme Toggle (36x32)
                        let (th_rect, th_resp) =
                            ui.allocate_exact_size(Vec2::new(36.0, 32.0), Sense::click());
                        if th_resp.hovered() {
                            ui.painter()
                                .rect_filled(th_rect, Rounding::ZERO, pal.bg_hover);
                        }
                        let th_icon = if self.dark_mode {
                            lucide::SUN
                        } else {
                            lucide::MOON
                        };
                        ui.painter().text(
                            th_rect.center(),
                            egui::Align2::CENTER_CENTER,
                            th_icon,
                            FontId::proportional(13.5),
                            if th_resp.hovered() {
                                pal.text_primary
                            } else {
                                pal.text_muted
                            },
                        );
                        if th_resp
                            .on_hover_cursor(CursorIcon::PointingHand)
                            .on_hover_text("Chuyển đổi giao diện Sáng / Tối")
                            .clicked()
                        {
                            self.dark_mode = !self.dark_mode;
                            self.theme_dirty = true;
                        }

                        buttons_left_x = th_rect.left();

                        if !self.is_logged_in && !self.threads.is_empty() {
                            ui.add_space(8.0);
                            let hist_btn = egui::Button::new(
                                RichText::new("Xem lịch sử đã lưu")
                                    .size(11.5)
                                    .color(pal.primary)
                                    .strong(),
                            )
                            .fill(pal.primary.linear_multiply(0.12))
                            .stroke(Stroke::new(1.0_f32, pal.primary))
                            .rounding(Rounding::same(5.0));
                            let r = ui.add(hist_btn).on_hover_cursor(CursorIcon::PointingHand);
                            buttons_left_x = r.rect.left() - 8.0;
                            if r.clicked() {
                                self.is_logged_in = true;
                            }
                        }
                    });
                });

                // 3. Interactive drag / double-click region covering everything left of the control buttons
                let drag_rect = Rect::from_min_max(
                    bar_rect.min,
                    Pos2::new(
                        (buttons_left_x - 2.0).max(bar_rect.left() + 40.0),
                        bar_rect.max.y,
                    ),
                );
                let drag_resp = ui.interact(
                    drag_rect,
                    Id::new("custom_title_bar_drag"),
                    Sense::click_and_drag(),
                );
                if drag_resp.double_clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!is_maximized));
                } else if drag_resp.drag_started_by(egui::PointerButton::Primary) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
            });
    }

    fn render_login_screen(&mut self, ui: &mut egui::Ui, pal: &ThemePalette) {
        ui.vertical_centered(|ui| {
            let avail_h = ui.available_height();
            ui.add_space((avail_h * 0.08).max(20.0));
            if let Some(tex) = &self.app_logo_texture {
                ui.image((tex.id(), Vec2::new(64.0, 64.0)));
                ui.add_space(10.0);
            }
            ui.label(
                RichText::new("Đăng nhập Zalo")
                    .color(pal.primary)
                    .size(26.0)
                    .strong(),
            );
            ui.add_space(4.0);
            ui.label(
                RichText::new("Khởi chạy nhẹ, bảo mật và đồng bộ tin nhắn thời gian thực")
                    .color(pal.text_secondary)
                    .size(13.5),
            );
            ui.add_space(18.0);

            egui::Frame::none()
                .fill(pal.bg_elevated)
                .rounding(Rounding::same(16.0))
                .stroke(Stroke::new(1.0_f32, pal.border))
                .inner_margin(Margin::symmetric(28.0, 24.0))
                .show(ui, |ui| {
                    ui.set_width(360.0);
                    ui.vertical_centered(|ui| {
                        // Segmented Tab Switcher
                        egui::Frame::none()
                            .fill(pal.bg_input)
                            .rounding(Rounding::same(10.0))
                            .inner_margin(Margin::same(4.0))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    let tab_w = (ui.available_width() - 8.0) / 2.0;
                                    for (tab, title) in [
                                        (LoginTab::QrCode, "Quét mã QR"),
                                        (LoginTab::Password, "Số điện thoại"),
                                    ] {
                                        let active = self.login_tab == tab;
                                        let btn = egui::Button::new(
                                            RichText::new(title)
                                                .size(13.0)
                                                .color(if active {
                                                    Color32::WHITE
                                                } else {
                                                    pal.text_secondary
                                                })
                                                .strong(),
                                        )
                                        .fill(if active {
                                            pal.primary
                                        } else {
                                            Color32::TRANSPARENT
                                        })
                                        .stroke(Stroke::NONE)
                                        .rounding(Rounding::same(7.0))
                                        .min_size(Vec2::new(tab_w, 32.0));

                                        if ui
                                            .add(btn)
                                            .on_hover_cursor(CursorIcon::PointingHand)
                                            .clicked()
                                        {
                                            self.login_tab = tab;
                                        }
                                    }
                                });
                            });

                        ui.add_space(18.0);

                        match self.login_tab {
                            LoginTab::QrCode => {
                                if let Some(tex) = &self.qr_texture {
                                    egui::Frame::none()
                                        .fill(Color32::WHITE)
                                        .rounding(Rounding::same(12.0))
                                        .stroke(Stroke::new(1.0_f32, pal.border))
                                        .inner_margin(Margin::same(14.0))
                                        .show(ui, |ui| {
                                            ui.image((tex.id(), Vec2::new(240.0, 240.0)));
                                        });
                                } else {
                                    let (rect, _) = ui.allocate_exact_size(
                                        Vec2::new(268.0, 268.0),
                                        Sense::hover(),
                                    );
                                    ui.painter().rect_filled(
                                        rect,
                                        Rounding::same(12.0),
                                        pal.bg_input,
                                    );
                                    ui.painter().rect_stroke(
                                        rect,
                                        Rounding::same(12.0),
                                        Stroke::new(1.0_f32, pal.border),
                                    );
                                    ui.painter().text(
                                        rect.center(),
                                        egui::Align2::CENTER_CENTER,
                                        "Đang tải mã QR…",
                                        FontId::proportional(14.5),
                                        pal.text_secondary,
                                    );
                                }

                                ui.add_space(14.0);
                                ui.label(
                                    RichText::new(&self.qr_status)
                                        .color(pal.text_primary)
                                        .size(13.5),
                                );
                                ui.add_space(12.0);

                                let refresh_btn = egui::Button::new(
                                    RichText::new("Tạo / Làm mới mã QR")
                                        .size(13.5)
                                        .color(Color32::WHITE)
                                        .strong(),
                                )
                                .fill(pal.primary)
                                .rounding(Rounding::same(8.0))
                                .min_size(Vec2::new(200.0, 36.0));

                                if ui
                                    .add(refresh_btn)
                                    .on_hover_cursor(CursorIcon::PointingHand)
                                    .clicked()
                                {
                                    self.login_in_progress = true;
                                    self.qr_status = "Đang xin mã QR…".into();
                                    if let Some(tx) = WORKER_TX.get() {
                                        let _ = tx.send(WorkerCmd::RequestQr);
                                    }
                                }
                            }
                            LoginTab::Password => {
                                ui.with_layout(Layout::top_down(Align::Min), |ui| {
                                    ui.label(
                                        RichText::new("Số điện thoại")
                                            .size(13.0)
                                            .color(pal.text_secondary)
                                            .strong(),
                                    );
                                    ui.add_space(4.0);
                                    ui.add_sized(
                                        [ui.available_width(), 34.0],
                                        egui::TextEdit::singleline(&mut self.phone_input)
                                            .hint_text("Ví dụ: 0912345678"),
                                    );
                                    ui.add_space(12.0);

                                    ui.label(
                                        RichText::new("Mật khẩu")
                                            .size(13.0)
                                            .color(pal.text_secondary)
                                            .strong(),
                                    );
                                    ui.add_space(4.0);
                                    ui.add_sized(
                                        [ui.available_width(), 34.0],
                                        egui::TextEdit::singleline(&mut self.password_input)
                                            .password(true)
                                            .hint_text("Nhập mật khẩu Zalo"),
                                    );
                                    ui.add_space(16.0);

                                    if !self.login_status_msg.is_empty() {
                                        ui.label(
                                            RichText::new(&self.login_status_msg)
                                                .color(pal.danger)
                                                .size(12.5),
                                        );
                                        ui.add_space(8.0);
                                    }

                                    let btn = egui::Button::new(
                                        RichText::new("Đăng nhập")
                                            .color(Color32::WHITE)
                                            .size(14.0)
                                            .strong(),
                                    )
                                    .fill(pal.primary)
                                    .rounding(Rounding::same(8.0))
                                    .min_size(Vec2::new(ui.available_width(), 38.0));

                                    if ui
                                        .add_enabled(!self.login_in_progress, btn)
                                        .on_hover_cursor(CursorIcon::PointingHand)
                                        .clicked()
                                    {
                                        self.login_in_progress = true;
                                        self.login_status_msg = "Đang kết nối…".into();
                                        if let Some(tx) = WORKER_TX.get() {
                                            let _ = tx.send(WorkerCmd::LoginPassword {
                                                phone: self.phone_input.clone(),
                                                pass: self.password_input.clone(),
                                            });
                                        }
                                    }
                                });
                            }
                        }
                    });
                });
        });
    }

    fn show_toast(&mut self, msg: impl Into<String>) {
        let text = msg.into();
        self.status_msg = text.clone();
        self.toast_notice = Some((text, std::time::Instant::now()));
    }
}

#[cfg(test)]
mod tests {
    use super::is_newer_semver;

    #[test]
    fn semver_comparison_handles_v_prefix_and_prerelease() {
        assert!(is_newer_semver("v0.0.2", "0.0.1"));
        assert!(is_newer_semver("0.1.0", "0.0.9"));
        assert!(is_newer_semver("V1.0.0-beta.1", "0.9.9"));
        assert!(!is_newer_semver("v0.0.1", "0.0.1"));
        assert!(!is_newer_semver("0.0.1", "0.0.2"));
    }
}
