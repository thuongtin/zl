//! Zalo client in Rust — complete TUI & GUI.
//!
//! Protocol reimplemented from Zalo PC 26.10.11.

#![allow(dead_code)]

mod audio;
mod call;
mod chat;
mod clock;
mod crypto;
mod data_dir;
mod endpoints;
mod gui;
mod logger;
mod login;
mod media;
mod message_model;
mod param_cipher;
mod qr;
mod reactions;
mod session;
mod socket;
mod store;
mod tui;
mod ui;
mod unread;
mod voip_log;

use anyhow::Result;
use session::Session;

pub const POLL_SECS: u64 = 4;

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("ctdozlite v{}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let data_path = data_dir::initialize(&args)?;
    if args.iter().any(|a| a == "--print-data-dir") {
        println!("{}", data_path.display());
        return Ok(());
    }
    std::panic::set_hook(Box::new(|info| {
        use std::io::Write;
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let msg = format!("[{ts}] PANIC: {info}\n");
        eprint!("{msg}");
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("crash.log")
        {
            let _ = f.write_all(msg.as_bytes());
        }
    }));
    let dev_mode = args
        .iter()
        .any(|a| a == "--dev" || a == "-v" || a == "--verbose");
    let dump = args.iter().any(|a| a == "-d" || a == "--dump");
    let dump_raw = args.iter().any(|a| a == "--dump-raw");
    let force_tui = args.iter().any(|a| a == "--tui");
    let force_gui = args.iter().any(|a| a == "--gui");
    let force_qr = args.iter().any(|a| a == "--qr");
    let force_logout = args.iter().any(|a| a == "--logout");

    if force_logout {
        let _ = std::fs::remove_file("session.json");
        let _ = std::fs::remove_file("threads_cache.json");
        println!("✓ Đã đăng xuất và xóa phiên lưu trữ cũ (session.json, threads_cache.json).");
        if !force_gui && !force_tui && !force_qr {
            return Ok(());
        }
    }

    logger::init(dev_mode);

    if dev_mode {
        println!(
            "🔧 [DEV MODE ACTIVATED] – Quét log, chẩn đoán giao thức và trace chi tiết được bật."
        );
    } else {
        println!("🔒 [PRODUCTION MODE] – Toàn bộ nhật ký và dev inspector được ẩn.");
    }

    let mut s = Session::new(dev_mode, dump, dump_raw)?;

    // Attempt to restore persistent session from disk
    let session_restored = if !force_logout {
        if s.load_from_file("session.json").is_ok() {
            if let Some(li) = &s.login {
                let name = if li.display_name.is_empty() {
                    "Zalo User"
                } else {
                    &li.display_name
                };
                println!(
                    "✓ Đã khôi phục phiên từ session.json: {name} (UIN: {})",
                    li.uin
                );
                true
            } else {
                false
            }
        } else {
            false
        }
    } else {
        false
    };

    // Determine launch mode:
    // If `--gui` is specified (or default when not `--tui` / `--qr` / `--phone`):
    // Open the native GUI window IMMEDIATELY without blocking on network requests!
    // Background worker in `gui.rs` handles `get_server_info`, `get_friends`, and `poll_new_messages`.
    if force_gui || (!force_tui && !force_qr && !args.iter().any(|a| a.starts_with("--phone"))) {
        println!("🚀 Đang khởi chạy giao diện đồ họa ctdozlite (egui)…");
        #[cfg(target_os = "windows")]
        if !dev_mode {
            #[link(name = "kernel32")]
            extern "system" {
                fn GetConsoleWindow() -> *mut std::ffi::c_void;
            }
            #[link(name = "user32")]
            extern "system" {
                fn ShowWindow(hwnd: *mut std::ffi::c_void, n_cmd_show: i32) -> i32;
            }
            unsafe {
                let hwnd = GetConsoleWindow();
                if !hwnd.is_null() {
                    ShowWindow(hwnd, 0); // SW_HIDE
                }
            }
        }
        let mut viewport = eframe::egui::ViewportBuilder::default()
            .with_app_id("ctdozlite")
            .with_decorations(false)
            .with_inner_size([1020.0, 700.0])
            .with_min_inner_size([480.0, 360.0])
            .with_title("ctdozlite");
        if let Some(icon) = gui::load_app_icon() {
            viewport = viewport.with_icon(std::sync::Arc::new(icon));
        }
        let native_options = eframe::NativeOptions {
            viewport,
            ..Default::default()
        };

        let mut initial_threads = chat::load_threads_cache();
        store::load_history_into_threads(
            "zalo_history.db",
            &mut initial_threads,
            store::MessageStore::PRELOAD_MSGS,
        );

        let app_session = s;
        let res = eframe::run_native(
            "ctdozlite",
            native_options,
            Box::new(move |cc| {
                Ok(Box::new(gui::ZaloGuiApp::new(
                    cc,
                    app_session,
                    dev_mode,
                    initial_threads,
                )))
            }),
        )
        .map_err(|e| anyhow::anyhow!("Không thể khởi chạy GUI: {e}"));
        if res.is_ok() {
            std::process::exit(0);
        }
        return res;
    }

    // 1. Initial server info check for TUI mode
    if let Ok(resp) = login::get_server_info(&s).await {
        if let Some(d) = resp.get("data") {
            s.apply_server_info(d);
        }
        if dev_mode {
            println!(
                "✓ getServerInfo: chat {} • convers {}",
                s.domains.chat, s.domains.conversation
            );
        }
    }

    // Otherwise, run in TUI mode:
    println!("🖥️ Khởi chạy chế độ dòng lệnh TUI…");

    // Validate the stored session, but never destroy it for anything but a server
    // verdict: only `600` / `-102` mean the credentials are gone (#21).
    let mut session_restored = session_restored;
    let mut server_ok = false;
    if session_restored && s.is_expired_marked() {
        // A previous run already read `600` / `-102`; the file still holds the old
        // credentials until a new login replaces them.
        println!(
            "⚠️ Phiên trong session.json đã bị máy chủ từ chối ở lần chạy trước. Cần đăng nhập lại."
        );
        session_restored = false;
    } else if session_restored {
        match s.session_status().await {
            session::SessionStatus::Valid => server_ok = true,
            session::SessionStatus::Expired => {
                println!(
                    "⚠️ Phiên đăng nhập đã hết hạn (máy chủ từ chối zpw_sek). Cần đăng nhập lại."
                );
                // Keep every credential and only record the verdict; the file is not
                // replaced until a fresh login succeeds.
                s.mark_expired();
                if let Err(e) = s.save_to_file("session.json") {
                    eprintln!("! Không ghi được dấu hết hạn vào session.json: {e}");
                }
                session_restored = false;
            }
            session::SessionStatus::Unreachable => {
                // Offline: keep the session and start with cached threads and local
                // history. `r` retries inside the TUI; no QR, no logout.
                println!(
                    "⚠️ Không kết nối được máy chủ Zalo — giữ nguyên phiên, chạy tiếp với dữ liệu cục bộ. Bấm 'r' để thử lại."
                );
            }
        }
    }

    // Login via QR or credentials if not already restored
    let mut logged_in_now = false;
    if force_qr || !session_restored {
        if force_qr {
            println!("… Đang tạo mã QR");
            let _ = qr::login_via_qr(&mut s, "vi").await?;
            logged_in_now = true;
        } else {
            let arg_val = |name: &str| -> Option<String> {
                args.iter()
                    .position(|a| a == name)
                    .and_then(|i| args.get(i + 1))
                    .cloned()
            };

            let phone = match arg_val("--phone") {
                Some(v) => v,
                None => ui::prompt("Số điện thoại: ")?,
            };
            let password = match arg_val("--pass") {
                Some(v) => v,
                None => ui::prompt_password("Mật khẩu: ")?,
            };
            let iso = match arg_val("--iso") {
                Some(v) => v,
                None => ui::prompt_default("Mã quốc gia", "VN")?,
            };

            if !phone.trim().is_empty() && !password.is_empty() {
                println!("… Đang đăng nhập mật khẩu");
                let v1 = args.iter().any(|a| a == "--v1");
                let _ = if v1 {
                    login::login_by_password_v1(&mut s, phone.trim(), &password).await?
                } else {
                    login::login_by_password(&mut s, phone.trim(), &password, iso.trim()).await?
                };
                logged_in_now = true;
            }
        }
    }

    if logged_in_now {
        // First moment the previous `session.json` may be replaced (#21). `qr::login_via_qr`
        // already saved once; the password path never persisted before this.
        if let Err(e) = s.save_to_file("session.json") {
            eprintln!("! Không lưu được session.json: {e}");
        }
        server_ok = true;
    }

    // Fetch initial threads and hydrate with local SQLite history
    let mut initial_threads = chat::load_threads_cache();
    store::load_history_into_threads("zalo_history.db", &mut initial_threads, 200);
    if s.login.is_some() && server_ok {
        match chat::get_friends(&s).await {
            Ok(list) => {
                for ft in list {
                    if let Some(ex) = initial_threads.iter_mut().find(|t| t.id == ft.id) {
                        if !ft.name.is_empty() {
                            ex.name = ft.name;
                        }
                        ex.is_group = ex.is_group || ft.is_group;
                    } else {
                        initial_threads.push(ft);
                    }
                }
                let mut poll_state = chat::PollState::default();
                if let Err(e) =
                    chat::poll_new_messages(&s, &mut poll_state, &mut initial_threads).await
                {
                    crate::logger::log_error("main::poll_new_messages", &e.to_string());
                    eprintln!("! Lỗi tải tin nhắn gần nhất: {e}");
                }
            }
            Err(e) => {
                crate::logger::log_error("main::get_friends", &e.to_string());
                eprintln!("! Lỗi tải danh sách bạn bè: {e}");
            }
        }
    }

    let tui_app = tui::TuiApp::new(&mut s, initial_threads);
    tui::run_tui(tui_app).await?;

    Ok(())
}

mod message_navigation;

pub mod delivery_receipts;
mod media_resources;
pub mod native_notifications;
pub mod original_reactions;
pub mod presence;
pub mod reaction_journal;
pub mod read_receipts;
pub mod runtime_state;
pub mod store_v2;
