//! Responsive TUI interface powered by Ratatui & Crossterm.
//!
//! Design goals:
//! - Non-blocking: every network call runs in a `tokio::spawn` worker and reports
//!   back over an mpsc channel, so the 30 ms render loop never stalls.
//! - ASCII-safe chrome (single-width borders and tags) for Windows consoles while
//!   keeping original UTF-8 message content.
//! - GUI parity: search/filter/unread, scrollback, quote, reactions, delete,
//!   attachments, sticker store, group history and the dev log inspector.

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;

use anyhow::Result;
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
        MouseEvent, MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Frame, Terminal,
};
use serde_json::Value;

use crate::chat::{self, ChatMessage, StickerItem, StickerPackInfo, Thread};
use crate::logger::{self, FindingLevel};
use crate::session::Session;

const POLL_SECS: u64 = 4;

/// Quick-reaction palette for keys `1`..`6`, same order as the GUI picker.
const QUICK_REACTIONS: [&str; 6] = [
    "\u{1F44D}",
    "\u{2764}\u{FE0F}",
    "\u{1F606}",
    "\u{1F62E}",
    "\u{1F622}",
    "\u{1F620}",
];

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Focus {
    Threads,
    Messages,
    Input,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FilterTab {
    All,
    Direct,
    Group,
}

impl FilterTab {
    fn next(self) -> Self {
        match self {
            Self::All => Self::Direct,
            Self::Direct => Self::Group,
            Self::Group => Self::All,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::All => "Tất cả",
            Self::Direct => "Cá nhân",
            Self::Group => "Nhóm",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Overlay {
    None,
    Help,
    Log,
    Sticker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromptKind {
    Attachment,
}

struct PromptState {
    kind: PromptKind,
    text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfirmKind {
    Logout,
    DeleteMessage,
    /// Drop local message history (`zalo_history.db` + `threads_cache.json`) while
    /// keeping the session (#21).
    WipeLocalData,
}

struct ConfirmAction {
    question: String,
    detail: String,
    kind: ConfirmKind,
    thread_id: String,
    msg_id: String,
}

#[derive(Default)]
struct StickerState {
    packs: Vec<StickerPackInfo>,
    items: Vec<StickerItem>,
    pack_idx: usize,
    item_idx: usize,
    stage: u8,
    loading: bool,
    loaded_name: String,
}

enum BgEvent {
    PollDone {
        threads: Vec<Thread>,
        state: chat::PollState,
    },
    PollFailed(String),
    ThreadsRefreshed(Vec<Thread>),
    HistoryLoaded {
        thread_id: String,
        resp: Value,
    },
    MessageSent {
        thread_id: String,
        text: String,
    },
    AttachmentSent {
        thread_id: String,
        msg: Box<ChatMessage>,
    },
    StickerSent {
        thread_id: String,
    },
    ReactionDone {
        thread_id: String,
        msg_id: String,
        icon: String,
    },
    DeleteDone {
        thread_id: String,
        msg_id: String,
    },
    StickerPacks(Vec<StickerPackInfo>),
    StickerItems(Vec<StickerItem>),
    SendFailed(String),
    ActionFailed(String),
}

pub struct TuiApp<'a> {
    pub session: &'a mut Session,
    pub threads: Vec<Thread>,
    pub list_state: ListState,
    pub selected_thread: usize,
    pub input_text: String,
    pub focus: Focus,
    pub status: String,
    pub should_quit: bool,
    pub store: Option<crate::store::MessageStore>,
    pub poll_state: chat::PollState,
    filtered: Vec<usize>,
    filter: FilterTab,
    search: String,
    search_mode: bool,
    unread: HashMap<String, usize>,
    selected_msg: Option<usize>,
    msg_scroll: usize,
    follow_tail: bool,
    msg_ranges: Vec<(usize, usize)>,
    chat_view_h: usize,
    threads_width: u16,
    reply_target: Option<ChatMessage>,
    overlay: Overlay,
    log_scroll: usize,
    sticker: StickerState,
    prompt: Option<PromptState>,
    confirm: Option<ConfirmAction>,
    poll_inflight: bool,
    bg_rx: Receiver<BgEvent>,
    bg_tx: Sender<BgEvent>,
}

impl<'a> TuiApp<'a> {
    pub fn new(session: &'a mut Session, mut threads: Vec<Thread>) -> Self {
        let store = crate::store::MessageStore::open("zalo_history.db").ok();
        if let Some(ref st) = store {
            let _ = st.hydrate_threads(&mut threads, 200);
        }

        let (tx, rx) = channel();

        let mut app = Self {
            session,
            threads,
            list_state: ListState::default(),
            selected_thread: 0,
            input_text: String::new(),
            focus: Focus::Threads,
            status: "Sẵn sàng".into(),
            should_quit: false,
            store,
            poll_state: chat::PollState::default(),
            filtered: Vec::new(),
            filter: FilterTab::All,
            search: String::new(),
            search_mode: false,
            unread: HashMap::new(),
            selected_msg: None,
            msg_scroll: 0,
            follow_tail: true,
            msg_ranges: Vec::new(),
            chat_view_h: 10,
            threads_width: 0,
            reply_target: None,
            overlay: Overlay::None,
            log_scroll: 0,
            sticker: StickerState::default(),
            prompt: None,
            confirm: None,
            poll_inflight: false,
            bg_rx: rx,
            bg_tx: tx,
        };
        app.rebuild_filter();
        app.ensure_msg_selection();
        app
    }

    fn session_clone(&self) -> Session {
        (*self.session).clone()
    }

    fn current_thread(&self) -> Option<&Thread> {
        self.threads.get(self.selected_thread)
    }

    fn rebuild_filter(&mut self) {
        let preferred = self.current_thread().map(|t| t.id.clone());
        self.rebuild_filter_prefer(preferred);
    }

    fn rebuild_filter_prefer(&mut self, preferred: Option<String>) {
        let query = self.search.to_lowercase();
        let mut filtered = Vec::new();
        for (i, t) in self.threads.iter().enumerate() {
            if self.filter == FilterTab::Direct && t.is_group {
                continue;
            }
            if self.filter == FilterTab::Group && !t.is_group {
                continue;
            }
            if !query.is_empty() {
                let hit = t.name.to_lowercase().contains(&query)
                    || t.id.contains(&query)
                    || t.preview.to_lowercase().contains(&query)
                    || t.messages
                        .iter()
                        .rev()
                        .take(20)
                        .any(|m| m.text.to_lowercase().contains(&query));
                if !hit {
                    continue;
                }
            }
            filtered.push(i);
        }
        self.filtered = filtered;

        if self.filtered.is_empty() {
            self.list_state.select(None);
            self.selected_thread = self.threads.len();
            self.selected_msg = None;
            self.msg_ranges.clear();
            return;
        }

        let pos = preferred
            .as_ref()
            .and_then(|id| {
                self.filtered
                    .iter()
                    .position(|&i| self.threads[i].id == *id)
            })
            .unwrap_or(0)
            .min(self.filtered.len() - 1);
        let idx = self.filtered[pos];
        self.list_state.select(Some(pos));

        let resolved = self.threads.get(idx).map(|t| t.id.clone());
        if resolved != preferred {
            self.selected_msg = self.threads[idx].messages.len().checked_sub(1);
            self.follow_tail = true;
            self.msg_scroll = 0;
        }
        self.selected_thread = idx;
    }

    fn select_index(&mut self, thread_idx: usize) {
        let pos = self.filtered.iter().position(|&i| i == thread_idx);
        self.list_state.select(pos);
        self.selected_thread = thread_idx;
        if let Some(t) = self.threads.get(thread_idx) {
            self.unread.remove(&t.id);
            self.selected_msg = t.messages.len().checked_sub(1);
        } else {
            self.selected_msg = None;
        }
        self.msg_scroll = 0;
        self.follow_tail = true;
        self.reply_target = None;
        self.msg_ranges.clear();
    }

    fn move_thread(&mut self, delta: isize) {
        if self.filtered.is_empty() {
            return;
        }
        let cur = self
            .list_state
            .selected()
            .unwrap_or(0)
            .min(self.filtered.len() - 1);
        let n = self.filtered.len() as isize;
        let next = if delta.abs() > 1 {
            (cur as isize + delta).clamp(0, n - 1) as usize
        } else {
            (cur as isize + delta).rem_euclid(n) as usize
        };
        let idx = self.filtered[next];
        self.select_index(idx);
    }

    fn select_first(&mut self) {
        if let Some(&i) = self.filtered.first() {
            self.select_index(i);
        }
    }

    fn select_last(&mut self) {
        if let Some(&i) = self.filtered.last() {
            self.select_index(i);
        }
    }

    fn cycle_filter(&mut self) {
        self.filter = self.filter.next();
        self.rebuild_filter();
        self.status = format!("Lọc: {}", self.filter.label());
    }

    fn cycle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Threads => Focus::Messages,
            Focus::Messages => Focus::Input,
            Focus::Input => Focus::Threads,
        };
        if self.focus == Focus::Messages {
            self.ensure_msg_selection();
        }
    }

    fn cycle_focus_rev(&mut self) {
        self.focus = match self.focus {
            Focus::Threads => Focus::Input,
            Focus::Messages => Focus::Threads,
            Focus::Input => Focus::Messages,
        };
        if self.focus == Focus::Messages {
            self.ensure_msg_selection();
        }
    }

    fn escape(&mut self) {
        if self.reply_target.is_some() {
            self.reply_target = None;
            self.status = "Đã huỷ trả lời".into();
            return;
        }
        self.focus = match self.focus {
            Focus::Input => Focus::Messages,
            Focus::Messages => Focus::Threads,
            Focus::Threads => Focus::Threads,
        };
    }

    fn ensure_msg_selection(&mut self) {
        let len = self
            .threads
            .get(self.selected_thread)
            .map(|t| t.messages.len())
            .unwrap_or(0);
        if len == 0 {
            self.selected_msg = None;
            return;
        }
        if self.selected_msg.map(|i| i >= len).unwrap_or(true) {
            self.selected_msg = Some(len - 1);
        }
    }

    fn move_message(&mut self, delta: isize) {
        let Some(len) = self
            .threads
            .get(self.selected_thread)
            .map(|t| t.messages.len())
        else {
            return;
        };
        if len == 0 {
            return;
        }
        let cur = self.selected_msg.unwrap_or(len - 1).min(len - 1);
        let next = ((cur as isize + delta).clamp(0, (len - 1) as isize)) as usize;
        self.selected_msg = Some(next);
        self.follow_tail = false;
        self.ensure_msg_visible();
    }

    fn ensure_msg_visible(&mut self) {
        let Some(idx) = self.selected_msg else {
            return;
        };
        let Some(&(start, end)) = self.msg_ranges.get(idx) else {
            return;
        };
        let h = self.chat_view_h.max(1);
        if start < self.msg_scroll {
            self.msg_scroll = start;
        } else if end > self.msg_scroll + h {
            self.msg_scroll = end.saturating_sub(h);
        }
    }

    fn scroll_by(&mut self, delta: isize) {
        let step = delta.unsigned_abs();
        if delta < 0 {
            self.msg_scroll = self.msg_scroll.saturating_sub(step);
        } else {
            self.msg_scroll = self.msg_scroll.saturating_add(step);
        }
        self.follow_tail = false;
    }

    fn scroll_page(&mut self, dir: isize) {
        let step = self.chat_view_h.max(1);
        self.scroll_by(dir * step as isize);
    }

    fn bump_thread(&mut self, thread_id: &str) {
        let selected_id = self.current_thread().map(|t| t.id.clone());
        if let Some(pos) = self.threads.iter().position(|t| t.id == thread_id) {
            if pos != 0 {
                let t = self.threads.remove(pos);
                self.threads.insert(0, t);
            }
        }
        self.rebuild_filter_prefer(selected_id);
    }

    fn persist_snapshot(&mut self, threads: &[Thread]) {
        if let Some(ref mut st) = self.store {
            let _ = crate::store::persist_threads(st, threads);
        } else if let Ok(mut st) = crate::store::MessageStore::open("zalo_history.db") {
            let _ = crate::store::persist_threads(&mut st, threads);
            self.store = Some(st);
        }
    }

    fn persist_current_snapshot(&mut self) {
        let Some(t) = self.threads.get(self.selected_thread) else {
            return;
        };
        let snapshot = vec![t.clone()];
        self.persist_snapshot(&snapshot);
    }

    fn persist_thread_by_id(&mut self, thread_id: &str) {
        let Some(t) = self.threads.iter().find(|t| t.id == thread_id) else {
            return;
        };
        let snapshot = vec![t.clone()];
        self.persist_snapshot(&snapshot);
    }

    fn persist_async(&self) {
        let snapshot = self.threads.clone();
        tokio::spawn(async move {
            let _ = tokio::task::spawn_blocking(move || {
                if let Ok(mut st) = crate::store::MessageStore::open("zalo_history.db") {
                    let _ = crate::store::persist_threads(&mut st, &snapshot);
                }
            })
            .await;
        });
    }

    fn spawn_poll(&mut self) {
        if self.poll_inflight || self.session.login.is_none() {
            return;
        }
        self.poll_inflight = true;
        let session = self.session_clone();
        let mut state = self.poll_state.clone();
        let mut threads = self.threads.clone();
        let tx = self.bg_tx.clone();
        tokio::spawn(async move {
            match chat::poll_new_messages(&session, &mut state, &mut threads).await {
                Ok(()) => {
                    let _ = tx.send(BgEvent::PollDone { threads, state });
                }
                Err(e) => {
                    logger::log_error("tui::poll_new_messages", &e.to_string());
                    let _ = tx.send(BgEvent::PollFailed(e.to_string()));
                }
            }
        });
    }

    fn spawn_refresh(&mut self) {
        self.status = "Đang làm mới...".into();
        let session = self.session_clone();
        let tx = self.bg_tx.clone();
        tokio::spawn(async move {
            let result = async {
                let mut list = chat::get_friends(&session).await?;
                let ids: Vec<String> = list.iter().map(|t| t.id.clone()).collect();
                match chat::get_last_msgs(&session, &ids).await {
                    Ok(resp) => chat::merge_previews(&mut list, &resp),
                    Err(e) => logger::log_error("tui::refresh::get_last_msgs", &e.to_string()),
                }
                Ok::<Vec<Thread>, anyhow::Error>(list)
            }
            .await;
            match result {
                Ok(list) => {
                    let _ = tx.send(BgEvent::ThreadsRefreshed(list));
                }
                Err(e) => {
                    logger::log_error("tui::refresh::get_friends", &e.to_string());
                    if crate::session::SessionStatus::from_error(&e)
                        == crate::session::SessionStatus::Expired
                    {
                        // Credentials are still on disk: re-login replaces them (#21).
                        let _ = tx.send(BgEvent::ActionFailed(
                            "Phiên đã hết hạn (máy chủ từ chối zpw_sek). Bấm 'x' để đăng xuất rồi đăng nhập lại."
                                .into(),
                        ));
                    } else {
                        let _ = tx.send(BgEvent::ActionFailed(format!("Lỗi làm mới: {e}")));
                    }
                }
            }
        });
    }

    fn spawn_history(&mut self) {
        let Some(t) = self.threads.get(self.selected_thread) else {
            return;
        };
        if !t.is_group {
            self.status = "Chỉ áp dụng cho nhóm chat".into();
            return;
        }
        let group_id = t.id.clone();
        self.status = "Đang tải lịch sử nhóm...".into();
        let session = self.session_clone();
        let tx = self.bg_tx.clone();
        let gid = group_id.clone();
        tokio::spawn(async move {
            match chat::get_group_history(&session, &gid, 50).await {
                Ok(resp) => {
                    let _ = tx.send(BgEvent::HistoryLoaded {
                        thread_id: gid,
                        resp,
                    });
                }
                Err(e) => {
                    logger::log_error("tui::get_group_history", &e.to_string());
                    let _ = tx.send(BgEvent::ActionFailed(format!("Lỗi tải lịch sử nhóm: {e}")));
                }
            }
        });
    }

    fn open_sticker_store(&mut self) {
        self.overlay = Overlay::Sticker;
        self.sticker.stage = 0;
        if !self.sticker.packs.is_empty() || self.sticker.loading {
            return;
        }
        self.sticker.loading = true;
        let session = self.session_clone();
        let tx = self.bg_tx.clone();
        tokio::spawn(async move {
            match chat::fetch_sticker_store_packs(&session).await {
                Ok(packs) => {
                    let _ = tx.send(BgEvent::StickerPacks(packs));
                }
                Err(e) => {
                    logger::log_error("tui::fetch_sticker_store_packs", &e.to_string());
                    let _ = tx.send(BgEvent::ActionFailed(format!("Lỗi tải kho nhãn dán: {e}")));
                }
            }
        });
    }

    fn sticker_move(&mut self, delta: isize) {
        let (len, idx) = if self.sticker.stage == 0 {
            (self.sticker.packs.len(), &mut self.sticker.pack_idx)
        } else {
            (self.sticker.items.len(), &mut self.sticker.item_idx)
        };
        if len == 0 {
            return;
        }
        let cur = (*idx).min(len - 1) as isize;
        *idx = ((cur + delta).clamp(0, (len - 1) as isize)) as usize;
    }

    fn sticker_enter(&mut self) {
        if self.sticker.stage == 0 {
            let Some(pack) = self.sticker.packs.get(self.sticker.pack_idx) else {
                return;
            };
            let cate_id = pack.id;
            let name = pack.name.clone();
            if self.sticker.loading {
                return;
            }
            self.sticker.loading = true;
            self.sticker.loaded_name = name.clone();
            let session = self.session_clone();
            let tx = self.bg_tx.clone();
            tokio::spawn(async move {
                match chat::fetch_sticker_pack_items(&session, cate_id, &name).await {
                    Ok(items) => {
                        let _ = tx.send(BgEvent::StickerItems(items));
                    }
                    Err(e) => {
                        logger::log_error("tui::fetch_sticker_pack_items", &e.to_string());
                        let _ = tx.send(BgEvent::ActionFailed(format!("Lỗi tải bộ nhãn dán: {e}")));
                    }
                }
            });
        } else {
            let Some(item) = self.sticker.items.get(self.sticker.item_idx).cloned() else {
                return;
            };
            self.overlay = Overlay::None;
            self.send_sticker(item);
        }
    }

    fn handle_sticker_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Esc | KeyCode::F(2) => {
                self.overlay = Overlay::None;
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
                if !self.sticker.items.is_empty() {
                    self.sticker.stage = 1 - self.sticker.stage;
                }
            }
            KeyCode::Up | KeyCode::Char('k') => self.sticker_move(-1),
            KeyCode::Down | KeyCode::Char('j') => self.sticker_move(1),
            KeyCode::PageUp => self.sticker_move(-10),
            KeyCode::PageDown => self.sticker_move(10),
            KeyCode::Enter => self.sticker_enter(),
            _ => {}
        }
    }

    fn send_sticker(&mut self, item: StickerItem) {
        let Some(t) = self.threads.get(self.selected_thread) else {
            return;
        };
        let to_id = t.id.clone();
        let is_group = t.is_group;

        let now = crate::ui::clock();
        let ts = unix_now_ms();
        let msg = ChatMessage {
            id: format!("local_{ts}"),
            sender: "Tôi".into(),
            text: String::new(),
            time: now.clone(),
            is_outgoing: true,
            ts,
            sticker_id: Some(item.sticker_id),
            sticker_cate_id: Some(item.cate_id),
            sticker_type: Some(item.sticker_type),
            ..Default::default()
        };
        if let Some(t) = self.threads.get_mut(self.selected_thread) {
            t.messages.push(msg);
            t.preview = "[Nhãn dán]".into();
            t.time = now;
        }
        self.selected_msg = None;
        self.follow_tail = true;
        self.persist_current_snapshot();
        chat::save_threads_cache(&self.threads);

        let session = self.session_clone();
        let tx = self.bg_tx.clone();
        tokio::spawn(async move {
            match chat::send_sticker(
                &session,
                &to_id,
                item.sticker_id,
                item.cate_id,
                item.sticker_type,
                is_group,
            )
            .await
            {
                Ok(_) => {
                    let _ = tx.send(BgEvent::StickerSent { thread_id: to_id });
                }
                Err(e) => {
                    logger::log_error("tui::send_sticker", &e.to_string());
                    let _ = tx.send(BgEvent::ActionFailed(format!("Lỗi gửi nhãn dán: {e}")));
                }
            }
        });
        self.status = "Đã gửi nhãn dán".into();
    }

    fn send_attachment(&mut self, path: PathBuf) {
        if !path.is_file() {
            self.status = format!("Không tìm thấy tệp: {}", path.display());
            return;
        }
        let Some(t) = self.threads.get(self.selected_thread) else {
            return;
        };
        let to_id = t.id.clone();
        let is_group = t.is_group;
        let caption = self.input_text.trim().to_string();
        self.input_text.clear();
        self.status = format!("Đang gửi tệp {}...", path.display());

        let session = self.session_clone();
        let tx = self.bg_tx.clone();
        tokio::spawn(async move {
            match chat::send_attachment(&session, &to_id, is_group, &path, &caption).await {
                Ok((_resp, msg)) => {
                    let _ = tx.send(BgEvent::AttachmentSent {
                        thread_id: to_id,
                        msg: Box::new(msg),
                    });
                }
                Err(e) => {
                    logger::log_error("tui::send_attachment", &e.to_string());
                    let _ = tx.send(BgEvent::ActionFailed(format!("Lỗi gửi tệp: {e}")));
                }
            }
        });
    }

    fn submit_prompt(&mut self, kind: PromptKind, text: String) {
        match kind {
            PromptKind::Attachment => {
                let trimmed = text.trim().to_string();
                if trimmed.is_empty() {
                    self.status = "Chưa nhập đường dẫn tệp".into();
                    return;
                }
                self.send_attachment(PathBuf::from(trimmed));
            }
        }
    }

    fn send_current(&mut self) {
        let text = self.input_text.trim().to_string();
        if text.is_empty() {
            return;
        }
        let Some(t) = self.threads.get(self.selected_thread) else {
            self.status = "Chưa chọn hội thoại".into();
            return;
        };
        let to_id = t.id.clone();
        let is_group = t.is_group;

        self.input_text.clear();
        let reply = self.reply_target.take();
        let now = crate::ui::clock();
        let ts = unix_now_ms();
        let msg = ChatMessage {
            id: format!("local_{ts}"),
            sender: "Tôi".into(),
            text: text.clone(),
            time: now.clone(),
            is_outgoing: true,
            ts,
            reply_to_id: reply.as_ref().map(|m| m.id.clone()),
            reply_to_sender: reply.as_ref().map(|m| m.sender.clone()),
            reply_to_text: reply.as_ref().map(|m| m.text.clone()),
            ..Default::default()
        };
        if let Some(t) = self.threads.get_mut(self.selected_thread) {
            t.messages.push(msg);
            t.preview = text.clone();
            t.time = now;
        }
        self.selected_msg = None;
        self.follow_tail = true;
        self.persist_current_snapshot();
        chat::save_threads_cache(&self.threads);
        self.bump_thread(&to_id);

        let session = self.session_clone();
        let tx = self.bg_tx.clone();
        tokio::spawn(async move {
            let result = match (&reply, is_group) {
                (Some(q), true) => chat::send_quote_message(&session, &to_id, &text, q, true)
                    .await
                    .map(|_| ()),
                (Some(q), false) => chat::send_quote_message(&session, &to_id, &text, q, false)
                    .await
                    .map(|_| ()),
                (None, true) => chat::send_group_message(&session, &to_id, &text)
                    .await
                    .map(|_| ()),
                (None, false) => chat::send_message(&session, &to_id, &text)
                    .await
                    .map(|_| ()),
            };
            match result {
                Ok(()) => {
                    let _ = tx.send(BgEvent::MessageSent {
                        thread_id: to_id,
                        text,
                    });
                }
                Err(e) => {
                    logger::log_error("tui::send_message", &e.to_string());
                    let _ = tx.send(BgEvent::SendFailed(e.to_string()));
                }
            }
        });
        self.status = "Đang gửi tin nhắn...".into();
    }

    fn react_quick(&mut self, key: char) {
        let Some(idx) = key
            .to_digit(10)
            .map(|d| d as usize)
            .and_then(|d| d.checked_sub(1))
        else {
            return;
        };
        let Some(emoji) = QUICK_REACTIONS.get(idx).copied() else {
            return;
        };
        let Some(mi) = self.selected_msg else {
            return;
        };
        let (to_id, is_group, msg) = {
            let Some(t) = self.threads.get(self.selected_thread) else {
                return;
            };
            let Some(msg) = t.messages.get(mi) else {
                return;
            };
            (t.id.clone(), t.is_group, msg.clone())
        };
        if let Some(m) = self
            .threads
            .get_mut(self.selected_thread)
            .and_then(|t| t.messages.get_mut(mi))
        {
            let code = chat::reaction_code_and_type(emoji).0;
            if !m.reactions.iter().any(|r| r == code) {
                m.reactions.push(code.to_string());
            }
        }
        let session = self.session_clone();
        let tx = self.bg_tx.clone();
        let msg_id = msg.id.clone();
        tokio::spawn(async move {
            match chat::send_reaction(&session, &to_id, is_group, &msg, emoji).await {
                Ok(_) => {
                    let _ = tx.send(BgEvent::ReactionDone {
                        thread_id: to_id,
                        msg_id,
                        icon: emoji.to_string(),
                    });
                }
                Err(e) => {
                    logger::log_error("tui::send_reaction", &e.to_string());
                    let _ = tx.send(BgEvent::ActionFailed(format!("Lỗi thả cảm xúc: {e}")));
                }
            }
        });
        self.status = format!("Đã thả cảm xúc {emoji}");
    }

    fn quote_selected(&mut self) {
        let Some(mi) = self.selected_msg else {
            return;
        };
        let Some(t) = self.threads.get(self.selected_thread) else {
            return;
        };
        let Some(m) = t.messages.get(mi) else {
            return;
        };
        self.reply_target = Some(m.clone());
        self.focus = Focus::Input;
        self.status = "Đang trả lời tin nhắn (Ctrl+X để huỷ)".into();
    }

    fn confirm_delete_selected(&mut self) {
        let Some(mi) = self.selected_msg else {
            return;
        };
        let Some(t) = self.threads.get(self.selected_thread) else {
            return;
        };
        let Some(m) = t.messages.get(mi) else {
            return;
        };
        if m.id.is_empty() || m.id.starts_with("local_") {
            self.status = "Tin nhắn chưa đồng bộ, chưa thể xoá trên server".into();
            return;
        }
        let preview = m.text.chars().take(60).collect::<String>();
        self.confirm = Some(ConfirmAction {
            question: "Xoá tin nhắn này (chỉ phía bạn)?".into(),
            detail: preview,
            kind: ConfirmKind::DeleteMessage,
            thread_id: t.id.clone(),
            msg_id: m.id.clone(),
        });
    }

    fn open_selected_link(&mut self) {
        let Some(mi) = self.selected_msg else {
            return;
        };
        let url = self
            .threads
            .get(self.selected_thread)
            .and_then(|t| t.messages.get(mi))
            .and_then(|m| m.link_url.clone().or_else(|| m.link_src.clone()));
        match url {
            Some(u) if !u.is_empty() => {
                open_external_url(&u);
                self.status = format!("Mở liên kết: {u}");
            }
            _ => self.status = "Tin nhắn không có liên kết".into(),
        }
    }

    fn run_confirm(&mut self, action: ConfirmAction) {
        match action.kind {
            ConfirmKind::Logout => {
                let _ = std::fs::remove_file("session.json");
                let _ = std::fs::remove_file("threads_cache.json");
                self.session.logout();
                self.threads.clear();
                self.filtered.clear();
                self.list_state.select(None);
                self.unread.clear();
                self.selected_thread = 0;
                self.selected_msg = None;
                self.status = "Đã đăng xuất. Bấm 'q' để thoát.".into();
            }
            ConfirmKind::WipeLocalData => {
                // The custom channel keeps account-scoped history until an atomic wipe is available.
                self.status = "Chức năng xóa dữ liệu chưa hỗ trợ kho lịch sử cá nhân".into();
            }
            ConfirmKind::DeleteMessage => {
                let thread_id = action.thread_id;
                let msg_id = action.msg_id;
                let Some(t) = self.threads.iter().find(|t| t.id == thread_id) else {
                    return;
                };
                let Some(msg) = t.messages.iter().find(|m| m.id == msg_id).cloned() else {
                    return;
                };
                let is_group = t.is_group;
                let session = self.session_clone();
                let tx = self.bg_tx.clone();
                tokio::spawn(async move {
                    match chat::delete_message_for_me(&session, &thread_id, is_group, &msg).await {
                        Ok(_) => {
                            let _ = tx.send(BgEvent::DeleteDone { thread_id, msg_id });
                        }
                        Err(e) => {
                            logger::log_error("tui::delete_message_for_me", &e.to_string());
                            let _ =
                                tx.send(BgEvent::ActionFailed(format!("Lỗi xoá tin nhắn: {e}")));
                        }
                    }
                });
                self.status = "Đang xoá tin nhắn...".into();
            }
        }
    }

    fn handle_key(&mut self, code: KeyCode, mods: KeyModifiers) {
        if mods.contains(KeyModifiers::CONTROL) {
            match code {
                KeyCode::Char('c') => {
                    self.should_quit = true;
                    return;
                }
                KeyCode::Char('u') => {
                    self.input_text.clear();
                    return;
                }
                KeyCode::Char('x') => {
                    if self.reply_target.is_some() {
                        self.reply_target = None;
                        self.status = "Đã huỷ trả lời".into();
                    }
                    return;
                }
                _ => {}
            }
        }

        if let Some(action) = self.confirm.take() {
            match code {
                KeyCode::Char('y') | KeyCode::Enter => self.run_confirm(action),
                KeyCode::Char('n') | KeyCode::Esc => self.status = "Đã huỷ thao tác".into(),
                _ => self.confirm = Some(action),
            }
            return;
        }

        if let Some(mut prompt) = self.prompt.take() {
            match code {
                KeyCode::Esc => {
                    self.status = "Đã huỷ nhập".into();
                }
                KeyCode::Enter => {
                    let text = prompt.text.clone();
                    self.submit_prompt(prompt.kind, text);
                }
                KeyCode::Backspace => {
                    prompt.text.pop();
                    self.prompt = Some(prompt);
                }
                KeyCode::Char(c) => {
                    if !c.is_control() && !mods.contains(KeyModifiers::CONTROL) {
                        prompt.text.push(c);
                    }
                    self.prompt = Some(prompt);
                }
                _ => self.prompt = Some(prompt),
            }
            return;
        }

        match self.overlay {
            Overlay::Help => {
                if code == KeyCode::Esc
                    || code == KeyCode::F(1)
                    || code == KeyCode::Char('?')
                    || code == KeyCode::Char('q')
                {
                    self.overlay = Overlay::None;
                }
                return;
            }
            Overlay::Log => {
                match code {
                    KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('l') => {
                        self.overlay = Overlay::None;
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        self.log_scroll = self.log_scroll.saturating_sub(1);
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        self.log_scroll = self.log_scroll.saturating_add(1);
                    }
                    KeyCode::PageUp => self.log_scroll = self.log_scroll.saturating_sub(10),
                    KeyCode::PageDown => self.log_scroll = self.log_scroll.saturating_add(10),
                    _ => {}
                }
                return;
            }
            Overlay::Sticker => {
                self.handle_sticker_key(code);
                return;
            }
            Overlay::None => {}
        }

        if self.search_mode {
            match code {
                KeyCode::Esc => {
                    self.search.clear();
                    self.search_mode = false;
                    self.rebuild_filter();
                }
                KeyCode::Enter => self.search_mode = false,
                KeyCode::Backspace => {
                    self.search.pop();
                    self.rebuild_filter();
                }
                KeyCode::Up | KeyCode::Char('k') => self.move_thread(-1),
                KeyCode::Down | KeyCode::Char('j') => self.move_thread(1),
                KeyCode::Char(c) => {
                    self.search.push(c);
                    self.rebuild_filter();
                }
                _ => {}
            }
            return;
        }

        match code {
            KeyCode::F(1) => {
                self.overlay = Overlay::Help;
                return;
            }
            KeyCode::Char('?') if self.focus != Focus::Input => {
                self.overlay = Overlay::Help;
                return;
            }
            KeyCode::F(2) => {
                self.open_sticker_store();
                return;
            }
            KeyCode::F(3) => {
                self.prompt = Some(PromptState {
                    kind: PromptKind::Attachment,
                    text: String::new(),
                });
                return;
            }
            KeyCode::Char('/') if self.focus != Focus::Input => {
                self.search_mode = true;
                self.status = "Tìm kiếm: gõ từ khoá, Enter để xong, Esc để xoá".into();
                return;
            }
            KeyCode::Char('f') if self.focus != Focus::Input => {
                self.cycle_filter();
                return;
            }
            KeyCode::Char('u') if self.focus != Focus::Input => {
                self.unread.clear();
                self.status = "Đã xoá thông báo chưa đọc".into();
                return;
            }
            KeyCode::Char('r') if self.focus != Focus::Input => {
                self.spawn_refresh();
                return;
            }
            KeyCode::Char('h') if self.focus != Focus::Input => {
                self.spawn_history();
                return;
            }
            KeyCode::Char('l') if self.focus != Focus::Input => {
                if logger::is_dev_mode() {
                    self.overlay = Overlay::Log;
                    self.log_scroll = 0;
                } else {
                    self.status = "Nhật ký chỉ có trong chế độ --dev".into();
                }
                return;
            }
            KeyCode::Char('w') if self.focus != Focus::Input => {
                self.confirm = Some(ConfirmAction {
                    question: "Xoá lịch sử tin nhắn cục bộ (zalo_history.db, threads_cache.json)?"
                        .into(),
                    detail: "Session đăng nhập giữ nguyên. Thao tác này không thể hoàn tác.".into(),
                    kind: ConfirmKind::WipeLocalData,
                    thread_id: String::new(),
                    msg_id: String::new(),
                });
                return;
            }
            KeyCode::Char('x') if self.focus != Focus::Input => {
                self.confirm = Some(ConfirmAction {
                    question: "Đăng xuất và xoá session.json, threads_cache.json?".into(),
                    detail: "Thao tác này không thể hoàn tác.".into(),
                    kind: ConfirmKind::Logout,
                    thread_id: String::new(),
                    msg_id: String::new(),
                });
                return;
            }
            KeyCode::Char('q') if self.focus != Focus::Input => {
                self.should_quit = true;
                return;
            }
            KeyCode::Tab => {
                self.cycle_focus();
                return;
            }
            KeyCode::BackTab => {
                self.cycle_focus_rev();
                return;
            }
            KeyCode::Esc => {
                self.escape();
                return;
            }
            _ => {}
        }

        match self.focus {
            Focus::Threads => match code {
                KeyCode::Up | KeyCode::Char('k') => self.move_thread(-1),
                KeyCode::Down | KeyCode::Char('j') => self.move_thread(1),
                KeyCode::PageUp => self.move_thread(-10),
                KeyCode::PageDown => self.move_thread(10),
                KeyCode::Home => self.select_first(),
                KeyCode::End => self.select_last(),
                KeyCode::Enter => {
                    self.focus = Focus::Messages;
                    self.ensure_msg_selection();
                }
                KeyCode::Char(c) if !c.is_control() && !mods.contains(KeyModifiers::CONTROL) => {
                    self.focus = Focus::Input;
                    self.input_text.push(c);
                }
                _ => {}
            },
            Focus::Messages => match code {
                KeyCode::Up | KeyCode::Char('k') => self.move_message(-1),
                KeyCode::Down | KeyCode::Char('j') => self.move_message(1),
                KeyCode::PageUp => self.scroll_page(-1),
                KeyCode::PageDown => self.scroll_page(1),
                KeyCode::Home => {
                    self.msg_scroll = 0;
                    self.follow_tail = false;
                }
                KeyCode::End => self.follow_tail = true,
                KeyCode::Enter => self.focus = Focus::Input,
                KeyCode::Char(c @ '1'..='6') => self.react_quick(c),
                KeyCode::Char('y') => self.quote_selected(),
                KeyCode::Char('d') => self.confirm_delete_selected(),
                KeyCode::Char('o') => self.open_selected_link(),
                KeyCode::Char(c) if !c.is_control() && !mods.contains(KeyModifiers::CONTROL) => {
                    self.focus = Focus::Input;
                    self.input_text.push(c);
                }
                _ => {}
            },
            Focus::Input => match code {
                KeyCode::Enter => self.send_current(),
                KeyCode::Backspace => {
                    self.input_text.pop();
                }
                KeyCode::Char(c) => {
                    if !c.is_control() && !mods.contains(KeyModifiers::CONTROL) {
                        self.input_text.push(c);
                    }
                }
                _ => {}
            },
        }
    }

    fn handle_mouse(&mut self, m: MouseEvent) {
        let in_threads = (m.column as usize) < self.threads_width as usize;
        match m.kind {
            MouseEventKind::ScrollUp => {
                if in_threads {
                    self.move_thread(-1);
                } else {
                    self.scroll_by(-3);
                }
            }
            MouseEventKind::ScrollDown => {
                if in_threads {
                    self.move_thread(1);
                } else {
                    self.scroll_by(3);
                }
            }
            _ => {}
        }
    }

    fn apply_polled(&mut self, polled: Vec<Thread>, state: chat::PollState) -> bool {
        self.poll_state = state;
        let current_id = self.current_thread().map(|t| t.id.clone());

        let mut old: HashMap<String, (HashSet<String>, i64, usize, String)> = HashMap::new();
        for t in &self.threads {
            old.insert(
                t.id.clone(),
                (
                    t.messages.iter().map(|m| m.id.clone()).collect(),
                    t.messages.iter().map(|m| m.ts).max().unwrap_or(0),
                    t.messages.len(),
                    t.preview.clone(),
                ),
            );
        }

        let mut changed = false;
        for t in &polled {
            match old.get(&t.id) {
                Some((prev_ids, prev_max_ts, prev_len, prev_preview)) => {
                    if t.messages.len() != *prev_len || &t.preview != prev_preview {
                        changed = true;
                    }
                    let fresh = t
                        .messages
                        .iter()
                        .filter(|m| {
                            !m.is_outgoing
                                && !m.id.is_empty()
                                && !m.id.starts_with("local_")
                                && !prev_ids.contains(&m.id)
                                && (*prev_max_ts <= 0 || m.ts <= 0 || m.ts > *prev_max_ts)
                        })
                        .count();
                    if fresh > 0 && current_id.as_deref() != Some(t.id.as_str()) {
                        *self.unread.entry(t.id.clone()).or_insert(0) += fresh;
                        changed = true;
                    }
                }
                None => changed = true,
            }
        }

        // Merge into the live threads instead of replacing them, so optimistic
        // messages and history loaded while the poll was in flight survive.
        for incoming in polled {
            if let Some(pos) = self.threads.iter().position(|x| x.id == incoming.id) {
                chat::merge_thread_into(&mut self.threads[pos], incoming, false);
            } else {
                self.threads.push(incoming);
                changed = true;
            }
        }

        if let Some(id) = &current_id {
            self.unread.remove(id);
        }
        self.rebuild_filter_prefer(current_id);
        self.msg_ranges.clear();
        changed
    }

    fn apply_refresh(&mut self, list: Vec<Thread>) {
        if list.is_empty() {
            self.status = "Không lấy được danh sách hội thoại".into();
            return;
        }
        for ft in list {
            if let Some(pos) = self.threads.iter().position(|t| t.id == ft.id) {
                chat::merge_thread_into(&mut self.threads[pos], ft, false);
            } else {
                self.threads.push(ft);
            }
        }
        self.rebuild_filter();
        chat::save_threads_cache(&self.threads);
        self.persist_async();
    }

    fn check_background_events(&mut self) {
        while let Ok(event) = self.bg_rx.try_recv() {
            match event {
                BgEvent::PollDone { threads, state } => {
                    self.poll_inflight = false;
                    let changed = self.apply_polled(threads, state);
                    self.status = format!("Cập nhật {}", crate::ui::clock());
                    if changed {
                        chat::save_threads_cache(&self.threads);
                        self.persist_async();
                    }
                }
                BgEvent::PollFailed(err) => {
                    self.poll_inflight = false;
                    self.status = format!("Lỗi cập nhật: {err}");
                }
                BgEvent::ThreadsRefreshed(list) => {
                    let n = list.len();
                    self.apply_refresh(list);
                    self.status = format!("Đã làm mới ({n} hội thoại)");
                }
                BgEvent::HistoryLoaded { thread_id, resp } => {
                    let mut count = 0usize;
                    if let Some(t) = self.threads.iter_mut().find(|t| t.id == thread_id) {
                        chat::merge_group_history(t, &resp);
                        count = t.messages.len();
                    }
                    if count > 0 {
                        self.persist_thread_by_id(&thread_id);
                        chat::save_threads_cache(&self.threads);
                        self.selected_msg = None;
                        self.follow_tail = true;
                        self.status = format!("Đã tải lịch sử nhóm ({count} tin)");
                    } else {
                        self.status = "Không có tin nhắn mới từ nhóm".into();
                    }
                }
                BgEvent::MessageSent { thread_id, text } => {
                    if let Some(t) = self.threads.iter_mut().find(|t| t.id == thread_id) {
                        t.preview = text;
                    }
                    self.bump_thread(&thread_id);
                    self.status = "Đã gửi tin nhắn".into();
                }
                BgEvent::AttachmentSent { thread_id, msg } => {
                    let msg = *msg;
                    let mut appended = false;
                    if let Some(t) = self.threads.iter_mut().find(|t| t.id == thread_id) {
                        if !t.messages.iter().any(|m| m.id == msg.id) {
                            t.messages.push(msg.clone());
                            appended = true;
                        }
                        t.preview = if msg.text.is_empty() {
                            "[Tệp đính kèm]".into()
                        } else {
                            msg.text.clone()
                        };
                        t.time = msg.time.clone();
                    }
                    if appended {
                        self.bump_thread(&thread_id);
                        self.persist_thread_by_id(&thread_id);
                        chat::save_threads_cache(&self.threads);
                        self.follow_tail = true;
                        self.status = "Đã gửi tệp đính kèm".into();
                    }
                }
                BgEvent::StickerSent { thread_id } => {
                    self.bump_thread(&thread_id);
                    self.status = "Đã gửi nhãn dán".into();
                }
                BgEvent::ReactionDone {
                    thread_id,
                    msg_id,
                    icon,
                } => {
                    if let Some(m) = self
                        .threads
                        .iter_mut()
                        .find(|t| t.id == thread_id)
                        .and_then(|t| t.messages.iter_mut().find(|m| m.id == msg_id))
                    {
                        let code = chat::reaction_code_and_type(&icon).0;
                        if !m.reactions.iter().any(|r| r == code) {
                            m.reactions.push(code.to_string());
                        }
                    }
                    self.status = "Đã thả cảm xúc".into();
                }
                BgEvent::DeleteDone { thread_id, msg_id } => {
                    if let Some(t) = self.threads.iter_mut().find(|t| t.id == thread_id) {
                        t.messages.retain(|m| m.id != msg_id);
                    }
                    self.selected_msg = None;
                    self.persist_thread_by_id(&thread_id);
                    chat::save_threads_cache(&self.threads);
                    self.status = "Đã xoá tin nhắn".into();
                }
                BgEvent::StickerPacks(packs) => {
                    self.sticker.loading = false;
                    self.sticker.items.clear();
                    self.sticker.packs = packs;
                    self.sticker.pack_idx = 0;
                    self.sticker.item_idx = 0;
                    self.sticker.stage = 0;
                    self.status = format!("Kho nhãn dán: {} bộ", self.sticker.packs.len());
                }
                BgEvent::StickerItems(items) => {
                    self.sticker.loading = false;
                    let n = items.len();
                    self.sticker.items = items;
                    self.sticker.item_idx = 0;
                    self.sticker.stage = 1;
                    self.status = if n == 0 {
                        "Bộ nhãn dán trống".into()
                    } else {
                        format!("Bộ nhãn dán: {n} sticker")
                    };
                }
                BgEvent::SendFailed(err) => {
                    self.status = format!("Gửi tin nhắn thất bại: {err}");
                }
                BgEvent::ActionFailed(err) => {
                    self.status = err;
                }
            }
        }
    }
}

pub async fn run_tui(mut app: TuiApp<'_>) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_loop(&mut terminal, &mut app).await;

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    result
}

async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut TuiApp<'_>,
) -> Result<()> {
    let mut last_poll = std::time::Instant::now();

    while !app.should_quit {
        app.check_background_events();
        terminal.draw(|f| ui_draw(f, app))?;

        if event::poll(Duration::from_millis(30))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    app.handle_key(key.code, key.modifiers);
                }
                Event::Mouse(m) => app.handle_mouse(m),
                _ => {}
            }
        }

        if last_poll.elapsed() >= Duration::from_secs(POLL_SECS) {
            last_poll = std::time::Instant::now();
            app.spawn_poll();
        }
    }
    Ok(())
}

fn ui_draw(f: &mut Frame, app: &mut TuiApp<'_>) {
    let size = f.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(4),
        ])
        .split(size);

    // 1. Header bar
    let user_name = app
        .session
        .login
        .as_ref()
        .map(|l| {
            if l.display_name.is_empty() {
                format!("UIN: {}", l.uin)
            } else {
                format!("{} (UIN: {})", l.display_name, l.uin)
            }
        })
        .unwrap_or_else(|| "Chưa đăng nhập".into());

    let mode_span = if logger::is_dev_mode() {
        Span::styled(
            " [DEV] ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(
            " [PROD] ",
            Style::default().fg(Color::White).bg(Color::Green),
        )
    };

    let total_unread: usize = app.unread.values().sum();
    let mut header_spans = vec![
        Span::styled(
            " ctdozlite ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("| Tài khoản: "),
        Span::styled(
            user_name,
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" | Chưa đọc: "),
        Span::styled(
            total_unread.to_string(),
            Style::default()
                .fg(Color::LightRed)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" | "),
        Span::styled(&app.status, Style::default().fg(Color::Yellow)),
        Span::raw(" "),
        mode_span,
    ];
    if app.poll_inflight {
        header_spans.push(Span::styled(
            " [đang cập nhật] ",
            Style::default().fg(Color::Yellow),
        ));
    }

    let header = Paragraph::new(Line::from(header_spans))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Plain)
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .alignment(Alignment::Left);
    f.render_widget(header, chunks[0]);

    // 2. Body: thread list | chat area
    let body_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(32), Constraint::Percentage(68)])
        .split(chunks[1]);
    app.threads_width = body_chunks[0].width;

    let selected_pos = app.list_state.selected();
    let thread_items: Vec<ListItem> = app
        .filtered
        .iter()
        .enumerate()
        .map(|(pos, &i)| {
            let t = &app.threads[i];
            let is_sel = Some(pos) == selected_pos;
            let tag = if t.name.contains("Cloud") {
                "[Cloud] "
            } else if t.is_group {
                "[Group] "
            } else {
                "[User]  "
            };
            let cursor = if is_sel { "> " } else { "  " };
            let name_style = if is_sel {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            let preview_text = if t.preview.is_empty() {
                "- chưa có tin nhắn -".to_string()
            } else {
                let p = t.preview.chars().take(22).collect::<String>();
                format!("{p}...")
            };
            let mut line1 = vec![
                Span::raw(cursor),
                Span::styled(tag, Style::default().fg(Color::DarkGray)),
                Span::styled(t.name.clone(), name_style),
                Span::raw("  "),
                Span::styled(t.time.clone(), Style::default().fg(Color::DarkGray)),
            ];
            if let Some(n) = app.unread.get(&t.id).copied().filter(|n| *n > 0) {
                line1.push(Span::raw(" "));
                line1.push(Span::styled(
                    format!("({n})"),
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::LightRed)
                        .add_modifier(Modifier::BOLD),
                ));
            }
            let line2 = Line::from(vec![
                Span::raw("    "),
                Span::styled(preview_text, Style::default().fg(Color::Gray)),
            ]);
            ListItem::new(vec![Line::from(line1), line2])
        })
        .collect();

    let threads_title = if app.search_mode {
        format!(" Tìm: {}_ ", app.search)
    } else if !app.search.is_empty() {
        format!(
            " Hội thoại {}/{} [{}] tìm '{}' ",
            app.filtered.len(),
            app.threads.len(),
            app.filter.label(),
            app.search
        )
    } else {
        format!(
            " Hội thoại {}/{} [{}] ",
            app.filtered.len(),
            app.threads.len(),
            app.filter.label()
        )
    };

    let threads_border_style = if app.focus == Focus::Threads {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let threads_list = List::new(thread_items)
        .block(
            Block::default()
                .title(threads_title)
                .borders(Borders::ALL)
                .border_type(BorderType::Plain)
                .border_style(threads_border_style),
        )
        .highlight_style(
            Style::default()
                .bg(Color::Rgb(25, 40, 65))
                .add_modifier(Modifier::BOLD),
        );
    f.render_stateful_widget(threads_list, body_chunks[0], &mut app.list_state);

    // 2b. Right column: contact header, messages, reply bar, input
    let reply_h = if app.reply_target.is_some() { 2 } else { 0 };
    let chat_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(4),
            Constraint::Length(reply_h),
            Constraint::Length(3),
        ])
        .split(body_chunks[1]);

    let (name, id, is_group, msg_count) = match app.threads.get(app.selected_thread) {
        Some(t) => (t.name.clone(), t.id.clone(), t.is_group, t.messages.len()),
        None => ("Chưa chọn hội thoại".to_string(), String::new(), false, 0),
    };
    let kind = if is_group { "Nhóm" } else { "Cá nhân" };
    let chat_header = Paragraph::new(Line::from(vec![
        Span::styled(
            format!(" {name} "),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("[{kind}] "), Style::default().fg(Color::DarkGray)),
        Span::raw("| ID: "),
        Span::styled(id, Style::default().fg(Color::DarkGray)),
        Span::raw(format!(" | {msg_count} tin | ")),
        if app.follow_tail {
            Span::styled("theo dõi cuối", Style::default().fg(Color::Green))
        } else {
            Span::styled("đang xem lịch sử", Style::default().fg(Color::Yellow))
        },
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Plain)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    f.render_widget(chat_header, chat_chunks[0]);

    if app.focus == Focus::Messages {
        app.ensure_msg_selection();
    }

    let inner_w = chat_chunks[1].width.saturating_sub(2) as usize;
    let inner_h = chat_chunks[1].height.saturating_sub(2) as usize;
    let highlight = app.focus == Focus::Messages;
    let (lines, ranges) = match app.threads.get(app.selected_thread) {
        Some(t) => build_chat_lines(t, inner_w, app.selected_msg, highlight),
        None => (
            vec![Line::from(Span::styled(
                "  Chọn một hội thoại để bắt đầu.",
                Style::default().fg(Color::DarkGray),
            ))],
            Vec::new(),
        ),
    };
    let max_scroll = lines.len().saturating_sub(inner_h);
    if app.follow_tail {
        app.msg_scroll = max_scroll;
    } else {
        app.msg_scroll = app.msg_scroll.min(max_scroll);
        if app.msg_scroll >= max_scroll {
            app.follow_tail = true;
        }
    }
    app.msg_ranges = ranges;
    app.chat_view_h = inner_h;

    let pct = if lines.len() <= inner_h {
        "100%".to_string()
    } else {
        format!(
            "{}%",
            ((app.msg_scroll + inner_h) * 100 / lines.len().max(1)).min(100)
        )
    };

    let chat_border_style = if app.focus == Focus::Messages {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let chat_body = Paragraph::new(lines)
        .block(
            Block::default()
                .title(format!(" Nội dung tin nhắn [{pct}] "))
                .borders(Borders::ALL)
                .border_type(BorderType::Plain)
                .border_style(chat_border_style),
        )
        .scroll(((app.msg_scroll.min(u16::MAX as usize)) as u16, 0));
    f.render_widget(chat_body, chat_chunks[1]);

    if let Some(q) = &app.reply_target {
        let quote = format!(
            " {}: {}",
            q.sender,
            q.text
                .replace('\n', " ")
                .chars()
                .take(120)
                .collect::<String>()
        );
        let reply = Paragraph::new(quote)
            .block(
                Block::default()
                    .title(" Đang trả lời [Ctrl+X huỷ] ")
                    .borders(Borders::ALL)
                    .border_type(BorderType::Plain)
                    .border_style(Style::default().fg(Color::Magenta)),
            )
            .style(Style::default().fg(Color::Gray));
        f.render_widget(reply, chat_chunks[2]);
    }

    let input_title = if app.focus == Focus::Input {
        " Soạn tin nhắn: [ĐANG GÕ] (Enter: Gửi | Esc: Lùi | Ctrl+X: Huỷ trả lời) "
    } else {
        " Soạn tin nhắn: [Tab để nhập] "
    };
    let input_border_style = if app.focus == Focus::Input {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let cursor = if app.focus == Focus::Input { "_" } else { "" };
    let input_widget = Paragraph::new(format!(" {}{cursor}", app.input_text)).block(
        Block::default()
            .title(input_title)
            .borders(Borders::ALL)
            .border_type(BorderType::Plain)
            .border_style(input_border_style),
    );
    f.render_widget(input_widget, chat_chunks[3]);

    // 3. Footer
    let (hint1, hint2) = match app.focus {
        Focus::Threads => (
            "[Up/Down j/k] chọn | [PgUp/PgDn] nhảy 10 | [Enter] xem tin | [Tab] đổi vùng | [/] tìm | [f] lọc",
            "[r] làm mới | [h] lịch sử nhóm | [u] xoá chưa đọc | [F2] nhãn dán | [F3] tệp | [x] đăng xuất | [F1] trợ giúp | [q] thoát",
        ),
        Focus::Messages => (
            "[Up/Down j/k] chọn tin | [PgUp/PgDn] cuộn | [Home/End] đầu/cuối | [Enter] soạn | [Tab] đổi vùng",
            "[1-6] cảm xúc | [y] trả lời | [d] xoá | [o] mở link | [F2] nhãn dán | [F3] tệp | [F1] trợ giúp | [q] thoát",
        ),
        Focus::Input => (
            "[Enter] gửi | [Ctrl+U] xoá hết | [Ctrl+X] huỷ trả lời | [Esc] lùi | [Tab] đổi vùng",
            "[F2] nhãn dán | [F3] tệp đính kèm | [F1] trợ giúp | [Ctrl+C] thoát",
        ),
    };
    let footer_text = if app.search_mode {
        format!("Tìm kiếm: {}_   (Enter: xong | Esc: xoá)", app.search)
    } else {
        String::new()
    };
    let footer = Paragraph::new(vec![
        Line::from(Span::styled(hint1, Style::default().fg(Color::DarkGray))),
        Line::from(Span::styled(
            footer_text,
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(hint2, Style::default().fg(Color::DarkGray))),
    ])
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Plain)
            .title(" Phím tắt "),
    )
    .alignment(Alignment::Center);
    f.render_widget(footer, chunks[2]);

    // 4. Overlays
    match app.overlay {
        Overlay::Help => render_help(f, size),
        Overlay::Log => render_log_modal(f, size, app.log_scroll),
        Overlay::Sticker => render_sticker_overlay(f, size, app),
        Overlay::None => {}
    }
    if let Some(action) = &app.confirm {
        render_confirm(f, size, action);
    }
    if let Some(prompt) = &app.prompt {
        render_prompt(f, size, prompt);
    }
}

fn build_chat_lines(
    thread: &Thread,
    width: usize,
    selected: Option<usize>,
    highlight: bool,
) -> (Vec<Line<'static>>, Vec<(usize, usize)>) {
    let width = width.max(20);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut ranges: Vec<(usize, usize)> = Vec::new();

    if thread.messages.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            format!("  Hội thoại '{}' đã sẵn sàng.", thread.name),
            Style::default().fg(Color::DarkGray),
        )));
        lines.push(Line::from(Span::styled(
            "  Bấm Tab để soạn tin nhắn, Enter để gửi.",
            Style::default().fg(Color::Cyan),
        )));
        return (lines, ranges);
    }

    let mut last_day: Option<i64> = None;
    for (idx, m) in thread.messages.iter().enumerate() {
        if let Some(day) = message_day(m.ts) {
            if last_day != Some(day) {
                if !lines.is_empty() {
                    lines.push(Line::raw(""));
                }
                lines.push(Line::from(Span::styled(
                    format!("--- {} ---", day_label(day)),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD),
                )));
                last_day = Some(day);
            }
        }

        let start = lines.len();
        let sel = highlight && selected == Some(idx);

        let sender_name = if m.is_outgoing {
            "Tôi".to_string()
        } else if !m.sender.trim().is_empty() {
            m.sender.clone()
        } else if thread.is_group {
            "Thành viên".to_string()
        } else {
            thread.name.clone()
        };
        let time_disp = if m.time.is_empty() {
            "--:--".to_string()
        } else {
            m.time.clone()
        };
        let pin_part = if m.is_pinned { "* " } else { "" };
        let time_part = format!("[{time_disp}] ");
        let name_part = format!("{sender_name}: ");
        let prefix_len =
            pin_part.chars().count() + time_part.chars().count() + name_part.chars().count();

        let mut prefix: Vec<Span<'static>> = Vec::new();
        if m.is_pinned {
            prefix.push(Span::styled("* ", Style::default().fg(Color::Yellow)));
        }
        prefix.push(Span::styled(
            time_part,
            Style::default().fg(Color::DarkGray),
        ));
        let sender_style = if m.is_outgoing {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD)
        };
        prefix.push(Span::styled(name_part, sender_style));

        if let (Some(qs), Some(qt)) = (&m.reply_to_sender, &m.reply_to_text) {
            let quote = format!("| {}: {}", qs, qt.replace('\n', " "));
            for chunk in wrap_text(&quote, width.saturating_sub(prefix_len).max(16)) {
                lines.push(Line::from(vec![
                    Span::styled(" ".repeat(prefix_len), Style::default()),
                    Span::styled(
                        chunk,
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::ITALIC),
                    ),
                ]));
            }
        }

        let mut body = if m.sticker_id.unwrap_or(0) > 0 {
            format!("[Nhãn dán #{}]", m.sticker_id.unwrap_or(0))
        } else {
            m.text.clone()
        };
        if let Some(url) = &m.link_url {
            if !url.is_empty() && !body.contains(url.as_str()) {
                if !body.is_empty() {
                    body.push(' ');
                }
                body.push_str(url);
            }
        }
        if !m.reactions.is_empty() {
            let icons: Vec<&str> = m.reactions.iter().map(|r| chat::icon_to_emoji(r)).collect();
            body.push_str("  ");
            body.push_str(&icons.join(" "));
        }
        if body.trim().is_empty() {
            body = "(không có nội dung)".to_string();
        }

        let avail = width.saturating_sub(prefix_len).max(10);
        let chunks = wrap_text(&body, avail);
        for (ci, chunk) in chunks.iter().enumerate() {
            if ci == 0 {
                let mut spans = prefix.clone();
                spans.push(Span::styled(
                    chunk.clone(),
                    Style::default().fg(Color::White),
                ));
                lines.push(Line::from(spans));
            } else {
                lines.push(Line::from(vec![
                    Span::styled(" ".repeat(prefix_len), Style::default()),
                    Span::styled(chunk.clone(), Style::default().fg(Color::White)),
                ]));
            }
        }

        if sel {
            let bg = Style::default().bg(Color::Rgb(35, 48, 70));
            for line in lines.iter_mut().skip(start) {
                *line = line.clone().style(bg);
            }
        }
        ranges.push((start, lines.len()));
    }

    (lines, ranges)
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let width = width.max(4);
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut cur_len = 0usize;

    for word in text.split_whitespace() {
        let wl = word.chars().count();
        if wl > width {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
                cur_len = 0;
            }
            let mut buf = String::new();
            let mut n = 0usize;
            for ch in word.chars() {
                buf.push(ch);
                n += 1;
                if n == width {
                    out.push(std::mem::take(&mut buf));
                    n = 0;
                }
            }
            if !buf.is_empty() {
                cur = buf;
                cur_len = n;
            }
            continue;
        }
        if cur_len > 0 && cur_len + 1 + wl > width {
            out.push(std::mem::take(&mut cur));
            cur_len = 0;
        }
        if cur_len > 0 {
            cur.push(' ');
            cur_len += 1;
        }
        cur.push_str(word);
        cur_len += wl;
    }

    if !cur.is_empty() {
        out.push(cur);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

fn normalize_ts_ms(ts: i64) -> i64 {
    if ts <= 0 {
        0
    } else if ts < 10_000_000_000 {
        ts * 1000
    } else {
        ts
    }
}

fn message_day(ts: i64) -> Option<i64> {
    let ms = normalize_ts_ms(ts);
    if ms <= 0 {
        return None;
    }
    Some((ms / 1000 + 7 * 3600).div_euclid(86_400))
}

fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = (y + if m <= 2 { 1 } else { 0 }) as i32;
    (year, m, d)
}

fn day_label(day: i64) -> String {
    let today = (crate::login::unix_now() + 7 * 3600).div_euclid(86_400);
    let (y, m, d) = civil_from_days(day);
    if day == today {
        format!("Hôm nay {d:02}/{m:02}/{y}")
    } else if day == today - 1 {
        format!("Hôm qua {d:02}/{m:02}/{y}")
    } else {
        format!("{d:02}/{m:02}/{y}")
    }
}

fn unix_now_ms() -> i64 {
    crate::login::unix_now() * 1000
}

fn open_external_url(url: &str) {
    let trimmed = url.trim();
    if !(trimmed.starts_with("https://") || trimmed.starts_with("http://")) {
        return;
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let _ = std::process::Command::new("rundll32.exe")
            .args(["url.dll,FileProtocolHandler", trimmed])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = std::process::Command::new("xdg-open").arg(trimmed).spawn();
    }
}

fn help_line(keys: &str, desc: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("  {keys:<14}"),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(desc.to_string(), Style::default().fg(Color::White)),
    ])
}

fn render_help(f: &mut Frame, area: Rect) {
    let modal_area = centered_rect(80, 80, area);
    f.render_widget(Clear, modal_area);

    let content: Vec<Line<'static>> = vec![
        Line::from(Span::styled(
            "=== PHÍM TẮT ZALO TUI ===",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::raw(""),
        help_line(
            "Tab / BackTab",
            "Chuyển vùng: Danh sách -> Tin nhắn -> Soạn tin",
        ),
        help_line("F1 / ?", "Mở trợ giúp này"),
        help_line("q / Ctrl+C", "Thoát (q chỉ khi không gõ tin)"),
        Line::raw(""),
        Line::from(Span::styled(
            "-- Danh sách hội thoại --",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )),
        help_line("Up/Down j/k", "Chọn hội thoại (cuộn chuột cũng được)"),
        help_line("PgUp/PgDn", "Nhảy 10 hội thoại"),
        help_line("Home / End", "Hội thoại đầu / cuối"),
        help_line("/", "Tìm theo tên, ID, nội dung, tin nhắn"),
        help_line("f", "Lọc: Tất cả -> Cá nhân -> Nhóm"),
        help_line("u", "Xoá tất cả thông báo chưa đọc"),
        help_line("r", "Làm mới danh sách từ server (nên)"),
        help_line("h", "Tải lịch sử nhóm (50 tin)"),
        help_line("x", "Đăng xuất (có xác nhận)"),
        help_line("w", "Xoá lịch sử tin nhắn cục bộ (có xác nhận)"),
        Line::raw(""),
        Line::from(Span::styled(
            "-- Tin nhắn --",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )),
        help_line("Up/Down j/k", "Chọn tin nhắn"),
        help_line("PgUp/PgDn", "Cuộn nhanh"),
        help_line("Home / End", "Đầu / cuối hội thoại"),
        help_line(
            "1 .. 6",
            "Thả cảm xúc nhanh (Thích, Yêu, Haha, Wow, Buồn, Phẫn nộ)",
        ),
        help_line("y", "Trả lời tin đang chọn"),
        help_line("d", "Xoá tin nhắn (chỉ phía bạn)"),
        help_line("o", "Mở liên kết của tin nhắn"),
        Line::raw(""),
        Line::from(Span::styled(
            "-- Soạn tin / đính kèm --",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )),
        help_line("Enter", "Gửi tin nhắn"),
        help_line("Ctrl+U", "Xoá toàn bộ ô soạn"),
        help_line("Ctrl+X", "Huỷ trả lời"),
        help_line("Esc", "Lùi về vùng trước"),
        help_line("F2", "Kho nhãn dán: Enter chọn bộ, Enter lần 2 để gửi"),
        help_line("F3", "Gửi tệp đính kèm bằng đường dẫn"),
        help_line("l", "Dev log inspector (chỉ chế độ --dev)"),
        Line::raw(""),
        Line::from(Span::styled(
            "Esc hoặc F1 để đóng trợ giúp.",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let block = Block::default()
        .title(" Trợ giúp (Esc/F1 để đóng) ")
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(Color::Cyan));
    let paragraph = Paragraph::new(content)
        .block(block)
        .wrap(Wrap { trim: true });
    f.render_widget(paragraph, modal_area);
}

fn render_sticker_overlay(f: &mut Frame, area: Rect, app: &TuiApp<'_>) {
    let modal_area = centered_rect(84, 80, area);
    f.render_widget(Clear, modal_area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(modal_area);

    let loading = if app.sticker.loading {
        " (đang tải...)"
    } else {
        ""
    };
    let packs_title = if app.sticker.stage == 0 {
        format!(
            " Kho nhãn dán ({} bộ){loading} [ĐANG CHỌN] — Enter để mở bộ ",
            app.sticker.packs.len()
        )
    } else {
        format!(
            " Kho nhãn dán ({} bộ){loading} — Left/Right đổi cột ",
            app.sticker.packs.len()
        )
    };
    let pack_items: Vec<ListItem> = app
        .sticker
        .packs
        .iter()
        .map(|p| {
            ListItem::new(Line::from(vec![
                Span::styled(format!(" {} ", p.name), Style::default().fg(Color::White)),
                Span::styled(
                    format!("({} sticker)", p.total_image),
                    Style::default().fg(Color::DarkGray),
                ),
            ]))
        })
        .collect();
    let mut packs_state = ListState::default();
    if !app.sticker.packs.is_empty() {
        packs_state.select(Some(app.sticker.pack_idx.min(app.sticker.packs.len() - 1)));
    }
    let packs_border = if app.sticker.stage == 0 {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let packs_list = List::new(pack_items)
        .block(
            Block::default()
                .title(packs_title)
                .borders(Borders::ALL)
                .border_type(BorderType::Plain)
                .border_style(packs_border),
        )
        .highlight_style(
            Style::default()
                .bg(Color::Rgb(25, 40, 65))
                .add_modifier(Modifier::BOLD),
        );
    f.render_stateful_widget(packs_list, chunks[0], &mut packs_state);

    let items_title = if app.sticker.items.is_empty() {
        format!(
            " Bộ '{}' — chọn một bộ bên trên rồi Enter ",
            app.sticker.loaded_name
        )
    } else if app.sticker.stage == 1 {
        format!(
            " Bộ '{}' ({} sticker) [ĐANG CHỌN] — Enter để gửi ",
            app.sticker.loaded_name,
            app.sticker.items.len()
        )
    } else {
        format!(
            " Bộ '{}' ({} sticker) ",
            app.sticker.loaded_name,
            app.sticker.items.len()
        )
    };
    let item_rows: Vec<ListItem> = app
        .sticker
        .items
        .iter()
        .enumerate()
        .map(|(i, it)| {
            let label = if it.label.trim().is_empty() {
                format!("Sticker #{}", it.sticker_id)
            } else {
                it.label.clone()
            };
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!(" {:>3}. ", i + 1),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(label, Style::default().fg(Color::White)),
                Span::styled(
                    format!("  id={} cate={}", it.sticker_id, it.cate_id),
                    Style::default().fg(Color::DarkGray),
                ),
            ]))
        })
        .collect();
    let mut items_state = ListState::default();
    if !app.sticker.items.is_empty() {
        items_state.select(Some(app.sticker.item_idx.min(app.sticker.items.len() - 1)));
    }
    let items_border = if app.sticker.stage == 1 {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let items_list = List::new(item_rows)
        .block(
            Block::default()
                .title(items_title)
                .borders(Borders::ALL)
                .border_type(BorderType::Plain)
                .border_style(items_border),
        )
        .highlight_style(
            Style::default()
                .bg(Color::Rgb(25, 40, 65))
                .add_modifier(Modifier::BOLD),
        );
    f.render_stateful_widget(items_list, chunks[1], &mut items_state);
}

fn render_confirm(f: &mut Frame, area: Rect, action: &ConfirmAction) {
    let modal_area = centered_rect(60, 26, area);
    f.render_widget(Clear, modal_area);

    let content = vec![
        Line::raw(""),
        Line::from(Span::styled(
            format!("  {}", action.question),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            format!("  {}", action.detail),
            Style::default().fg(Color::White),
        )),
        Line::raw(""),
        Line::from(Span::styled(
            "  [y/Enter] Xác nhận    [n/Esc] Huỷ",
            Style::default().fg(Color::Cyan),
        )),
    ];
    let paragraph = Paragraph::new(content).block(
        Block::default()
            .title(" Xác nhận ")
            .borders(Borders::ALL)
            .border_type(BorderType::Plain)
            .border_style(Style::default().fg(Color::Yellow)),
    );
    f.render_widget(paragraph, modal_area);
}

fn render_prompt(f: &mut Frame, area: Rect, prompt: &PromptState) {
    let modal_area = centered_rect(70, 24, area);
    f.render_widget(Clear, modal_area);

    let title = match prompt.kind {
        PromptKind::Attachment => " Gửi tệp đính kèm: nhập đường dẫn tệp ",
    };
    let content = vec![
        Line::raw(""),
        Line::from(Span::styled(
            format!("  {}_", prompt.text),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::raw(""),
        Line::from(Span::styled(
            "  [Enter] Gửi    [Esc] Huỷ",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    let paragraph = Paragraph::new(content).block(
        Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_type(BorderType::Plain)
            .border_style(Style::default().fg(Color::Cyan)),
    );
    f.render_widget(paragraph, modal_area);
}

fn render_log_modal(f: &mut Frame, area: Rect, scroll: usize) {
    let modal_area = centered_rect(88, 80, area);
    f.render_widget(Clear, modal_area);

    let findings = logger::scan_logs();
    let entries = logger::get_entries();

    let mut content = Vec::new();

    content.push(Line::from(vec![Span::styled(
        "=== KẾT QUẢ QUÉT LOG & CHẨN ĐOÁN GIAO THỨC ===",
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )]));
    for finding in &findings {
        let (icon, color) = match finding.level {
            FindingLevel::Info => ("[INFO] ", Color::Green),
            FindingLevel::Warning => ("[WARN] ", Color::Yellow),
            FindingLevel::Critical => ("[CRIT] ", Color::Red),
        };
        content.push(Line::from(vec![
            Span::styled(
                icon,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                finding.title.clone(),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
        ]));
        content.push(Line::from(vec![
            Span::raw("  Chỉ tiết: "),
            Span::styled(finding.detail.clone(), Style::default().fg(Color::White)),
        ]));
        content.push(Line::from(vec![
            Span::raw("  Gợi ý: "),
            Span::styled(finding.suggestion.clone(), Style::default().fg(Color::Cyan)),
        ]));
        content.push(Line::raw(""));
    }

    content.push(Line::from(vec![Span::styled(
        format!(
            "=== LƯU LƯỢNG MẠNG THỜI GIAN THỰC ({} requests) ===",
            entries.len()
        ),
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )]));
    for e in entries.iter().rev() {
        let status_color = if e.is_error { Color::Red } else { Color::Green };
        let status_str = e
            .status
            .map(|s| s.to_string())
            .unwrap_or_else(|| "ERR".into());

        content.push(Line::from(vec![
            Span::styled(
                format!("[{}] ", e.timestamp),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                format!("{:<4} ", e.method),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{:<3} ", status_str),
                Style::default().fg(status_color),
            ),
            Span::styled(
                format!("({}ms) ", e.latency_ms),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(e.url.clone(), Style::default().fg(Color::White)),
        ]));
        if let Some(dec) = &e.decrypted_preview {
            let preview = dec.chars().take(80).collect::<String>();
            content.push(Line::from(vec![
                Span::raw("  + Decrypted: "),
                Span::styled(format!("{preview}..."), Style::default().fg(Color::Gray)),
            ]));
        }
        if let Some(err) = &e.error_msg {
            content.push(Line::from(vec![Span::styled(
                format!("  + Lỗi: {err}"),
                Style::default().fg(Color::Red),
            )]));
        }
    }

    let block = Block::default()
        .title(" Dev Inspector & Protocol Scanner (Esc/q để đóng, Up/Down để cuộn) ")
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(Color::Yellow));

    let p = Paragraph::new(content)
        .block(block)
        .scroll((scroll.min(u16::MAX as usize) as u16, 0))
        .wrap(Wrap { trim: true });

    f.render_widget(p, modal_area);
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_words_to_width() {
        assert_eq!(
            wrap_text("a b c", 3),
            vec!["a b".to_string(), "c".to_string()]
        );
    }

    #[test]
    fn breaks_long_words() {
        assert_eq!(
            wrap_text("abcdef", 4),
            vec!["abcd".to_string(), "ef".to_string()]
        );
    }

    #[test]
    fn message_day_skips_missing_ts() {
        assert_eq!(message_day(0), None);
        assert!(message_day(1_700_000_000_000).is_some());
    }

    #[test]
    fn draws_thread_and_messages_without_panicking() {
        let mut session = crate::session::Session::new(false, false, false).unwrap();
        let (tx, rx) = channel();
        let thread = Thread {
            presentation: Default::default(),
            unread: Default::default(),
            id: "u1".into(),
            name: "Test".into(),
            preview: "xin chao".into(),
            time: "12:00".into(),
            avatar: None,
            is_group: true,
            messages: vec![
                ChatMessage {
                    id: "m1".into(),
                    sender: "An".into(),
                    text: "xin chao".into(),
                    time: "12:00".into(),
                    is_outgoing: false,
                    ts: 1_700_000_000_000,
                    ..Default::default()
                },
                ChatMessage {
                    id: "m2".into(),
                    sender: "Tôi".into(),
                    text: "chao ban".into(),
                    time: "12:01".into(),
                    is_outgoing: true,
                    ts: 1_700_000_060_000,
                    reply_to_sender: Some("An".into()),
                    reply_to_text: Some("xin chao".into()),
                    reactions: vec!["/-strong".into()],
                    ..Default::default()
                },
            ],
        };
        let mut app = TuiApp {
            session: &mut session,
            threads: vec![thread],
            list_state: ListState::default(),
            selected_thread: 0,
            input_text: "hello".into(),
            focus: Focus::Messages,
            status: "test".into(),
            should_quit: false,
            store: None,
            poll_state: chat::PollState::default(),
            filtered: vec![0],
            filter: FilterTab::All,
            search: String::new(),
            search_mode: false,
            unread: HashMap::new(),
            selected_msg: Some(1),
            msg_scroll: 0,
            follow_tail: true,
            msg_ranges: Vec::new(),
            chat_view_h: 10,
            threads_width: 0,
            reply_target: None,
            overlay: Overlay::None,
            log_scroll: 0,
            sticker: StickerState::default(),
            prompt: None,
            confirm: None,
            poll_inflight: false,
            bg_rx: rx,
            bg_tx: tx,
        };
        let backend = ratatui::backend::TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| ui_draw(f, &mut app)).unwrap();
        assert!(app.msg_ranges.len() >= 2);
    }

    /// The "clear local data" confirmation must reach the terminal with its Vietnamese
    /// text intact — it is the only guard in front of an irreversible delete (#21).
    #[test]
    fn wipe_confirmation_renders_its_vietnamese_text() {
        let mut session = crate::session::Session::new(false, false, false).unwrap();
        let (tx, rx) = channel();
        let mut app = TuiApp {
            session: &mut session,
            threads: Vec::new(),
            list_state: ListState::default(),
            selected_thread: 0,
            input_text: String::new(),
            focus: Focus::Threads,
            status: "test".into(),
            should_quit: false,
            store: None,
            poll_state: chat::PollState::default(),
            filtered: Vec::new(),
            filter: FilterTab::All,
            search: String::new(),
            search_mode: false,
            unread: HashMap::new(),
            selected_msg: None,
            msg_scroll: 0,
            follow_tail: true,
            msg_ranges: Vec::new(),
            chat_view_h: 10,
            threads_width: 0,
            reply_target: None,
            overlay: Overlay::None,
            log_scroll: 0,
            sticker: StickerState::default(),
            prompt: None,
            confirm: Some(ConfirmAction {
                question: "Xoá lịch sử tin nhắn cục bộ (zalo_history.db, threads_cache.json)?"
                    .into(),
                detail: "Session đăng nhập giữ nguyên. Thao tác này không thể hoàn tác.".into(),
                kind: ConfirmKind::WipeLocalData,
                thread_id: String::new(),
                msg_id: String::new(),
            }),
            poll_inflight: false,
            bg_rx: rx,
            bg_tx: tx,
        };
        let backend = ratatui::backend::TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| ui_draw(f, &mut app)).unwrap();

        let screen: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(
            screen.contains("Xác nhận"),
            "confirmation title missing from the frame"
        );
        assert!(
            screen.contains("Xoá lịch sử tin nhắn cục bộ"),
            "confirmation question missing from the frame"
        );
    }
}
