//! Standalone tool to test fetching real data from Zalo using a saved `session.json`.
//!
//! Run with: `cargo run --release --bin test_fetch`
//! Optional: `cargo run --release --bin test_fetch -- --send <TO_ID> <TEXT>`

use anyhow::{bail, Result};
use zalo_rust::session::Session;

#[tokio::main]
async fn main() -> Result<()> {
    println!("=== ZALO REAL DATA FETCH TEST ===");

    let mut s = Session::new(false, false, false)?;

    if let Err(e) = s.load_from_file("session.json") {
        bail!("Không tìm thấy hoặc không đọc được session.json ({e}). Vui lòng quét QR đăng nhập trước!");
    }

    let li = s.login.as_ref().unwrap();
    println!("✓ Đã tải session.json:");
    println!("  UIN:          {}", li.uin);
    println!(
        "  Tên:          {}",
        if li.display_name.is_empty() {
            "(chưa có)"
        } else {
            &li.display_name
        }
    );
    println!(
        "  zpw_enk:      {}",
        zalo_rust::session::redact_secret(&li.zpw_enk)
    );
    println!(
        "  zpw_sek:      {}",
        zalo_rust::session::redact_secret(&li.zpw_sek)
    );
    println!("  Send2Me ID:   {}", li.send2me_id);
    println!("  Profile Host: {}", s.domains.profile);
    println!("  Convers Host: {}", s.domains.convers_api);
    println!("  Chat Host:    {}", s.domains.chat);

    // 1. Test getfriends
    println!("\n--- [1] Thử gọi /api/social/friend/getfriends ---");
    let mut friends = match zalo_rust::chat::get_friends(&s).await {
        Ok(f) => {
            println!("✓ getfriends THÀNH CÔNG! Số lượng: {}", f.len());
            for (i, item) in f.iter().enumerate() {
                println!(
                    "  [{i}] ID: {:<20} Tên: {:<25} Tin nhắn: {}",
                    item.id, item.name, item.preview
                );
            }
            f
        }
        Err(e) => {
            println!(
                "! getfriends lỗi ({e}) -> Khởi tạo danh sách mặc định từ session để test tiếp..."
            );
            let mut list = Vec::new();
            if !li.send2me_id.is_empty() {
                list.push(zalo_rust::chat::Thread {
                    presentation: Default::default(),
                    unread: Default::default(),
                    id: li.send2me_id.clone(),
                    name: "Cloud của tôi (Lưu trữ)".into(),
                    preview: String::new(),
                    time: "12:00".into(),
                    avatar: None,
                    is_group: false,
                    messages: Vec::new(),
                });
            }
            list.push(zalo_rust::chat::Thread {
                presentation: Default::default(),
                unread: Default::default(),
                id: "1000000000000000001".into(),
                name: "Zalo".into(),
                preview: String::new(),
                time: "12:00".into(),
                avatar: None,
                is_group: false,
                messages: Vec::new(),
            });
            list
        }
    };

    // 2. Test get-last-msgs
    println!("\n--- [2] Thử gọi poll_new_messages (lp_chat + get-last-msgs + Cloud của tôi) ---");
    let mut ps = zalo_rust::chat::PollState::default();
    if let Err(e) = zalo_rust::chat::poll_new_messages(&s, &mut ps, &mut friends).await {
        println!("! poll_new_messages lỗi: {e}");
    }
    println!("\n=== KẾT QUẢ ĐỒNG BỘ TIN NHẮN VÀO HỘI THOẠI ===");
    for (i, f) in friends.iter().enumerate() {
        println!(
            "  [{i}] ID: {:<20} Tên: {:<25} Tin mới nhất: '{}' (Tổng tin: {})",
            f.id,
            f.name,
            f.preview,
            f.messages.len()
        );
        for (mi, m) in f.messages.iter().enumerate() {
            println!(
                "      ({mi}) [{}] {}: {}",
                m.time,
                if m.is_outgoing { "Tôi" } else { &m.sender },
                m.text
            );
        }
    }

    // 3. Test get_groups
    println!("\n--- [3] Thử gọi /api/group/getlg/v4 (danh sách nhóm) ---");
    match zalo_rust::chat::get_groups(&s).await {
        Ok(groups) => {
            println!("✓ get_groups THÀNH CÔNG! Số lượng nhóm: {}", groups.len());
            for (i, g) in groups.iter().enumerate() {
                println!("  [{i}] Group ID: {:<20} Tên: {}", g.id, g.name);
            }
            // 4. Test get_group_history on first group if available
            if let Some(first_g) = groups.first() {
                println!(
                    "\n--- [4] Thử gọi /api/group/history cho nhóm {} ---",
                    first_g.id
                );
                match zalo_rust::chat::get_group_history(&s, &first_g.id, 10).await {
                    Ok(resp) => {
                        println!("✓ get_group_history THÀNH CÔNG!");
                        let mut test_thread = first_g.clone();
                        zalo_rust::chat::merge_group_history(&mut test_thread, &resp);
                        println!("  Số tin nhắn tải được: {}", test_thread.messages.len());
                        for (mi, m) in test_thread.messages.iter().enumerate() {
                            println!("    ({mi}) [{}] {}: {}", m.time, m.sender, m.text);
                        }
                    }
                    Err(e) => println!("! get_group_history lỗi: {e}"),
                }
            }
        }
        Err(e) => {
            println!("! get_groups lỗi: {e}");
        }
    }

    // 5. Cache persistence test
    println!("\n--- [5] Kiểm tra lưu và đọc threads_cache.json ---");
    zalo_rust::chat::save_threads_cache(&friends);
    let reloaded = zalo_rust::chat::load_threads_cache();
    println!(
        "✓ threads_cache.json: Đã lưu và nạp lại thành công {} hội thoại.",
        reloaded.len()
    );

    // 6. Optional send test (1-1 or group)
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(pos) = args.iter().position(|a| a == "--send") {
        if let (Some(to_id), Some(text)) = (args.get(pos + 1), args.get(pos + 2)) {
            println!("\n--- [6] Thử gửi tin nhắn cá nhân tới {to_id}: '{text}' ---");
            match zalo_rust::chat::send_message(&s, to_id, text).await {
                Ok(resp) => {
                    println!("✓ Gửi tin nhắn THÀNH CÔNG!");
                    println!("  Response: {}", serde_json::to_string_pretty(&resp)?);
                }
                Err(e) => {
                    println!("! Gửi tin nhắn lỗi: {e}");
                }
            }
        }
    } else if let Some(pos) = args.iter().position(|a| a == "--send-group") {
        if let (Some(grid), Some(text)) = (args.get(pos + 1), args.get(pos + 2)) {
            println!("\n--- [6] Thử gửi tin nhắn nhóm tới {grid}: '{text}' ---");
            match zalo_rust::chat::send_group_message(&s, grid, text).await {
                Ok(resp) => {
                    println!("✓ Gửi tin nhắn nhóm THÀNH CÔNG!");
                    println!("  Response: {}", serde_json::to_string_pretty(&resp)?);
                }
                Err(e) => {
                    println!("! Gửi tin nhắn nhóm lỗi: {e}");
                }
            }
        }
    }

    Ok(())
}
