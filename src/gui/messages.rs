use super::*;

// ---------------------------------------------------------------------------
// Chat Bubble & Sticker Widget Rendering
// ---------------------------------------------------------------------------

pub(super) fn is_sticker_message(msg: &ChatMessage) -> Option<(u64, u64)> {
    if let Some(sid) = msg.sticker_id {
        if sid > 0 {
            return Some((sid, msg.sticker_cate_id.unwrap_or(0)));
        }
    }
    if msg.text.trim() == "[Nhãn dán]" {
        return Some((18009, 10130));
    }
    None
}

pub(super) fn render_sticker_widget(
    ui: &mut egui::Ui,
    msg: &ChatMessage,
    sticker_textures: &HashMap<String, TextureHandle>,
    loading_stickers: &mut HashSet<String>,
) -> bool {
    let Some((sid, cid)) = is_sticker_message(msg) else {
        return false;
    };

    let key = format!("stk_{cid}_{sid}");
    let url = if cid == 0 && (1..=30).contains(&sid) {
        format!("https://stc-chat.zdn.vn/images/stickers/default/thumb/{sid}.png")
    } else {
        format!("https://api.zaloapp.com/api/emoticon/sticker/webpc?eid={sid}&size=130")
    };

    if !sticker_textures.contains_key(&key) && loading_stickers.insert(key.clone()) {
        if let Some(tx) = WORKER_TX.get() {
            let _ = tx.send(WorkerCmd::FetchStickerImage {
                key: key.clone(),
                url,
            });
        }
    }

    if let Some(tex) = sticker_textures.get(&key) {
        ui.image((tex.id(), Vec2::new(116.0, 116.0)));
        true
    } else {
        false
    }
}

#[derive(Debug, Clone)]
pub(super) struct CallCardView {
    pub(super) title: String,
    pub(super) status_badge: String,
    pub(super) reason: Option<String>,
    pub(super) is_video: bool,
    pub(super) is_missed_or_failed: bool,
}

/// Detect whether `msg` is a voice/video call log entry and parse its structured metadata.
pub(super) fn extract_call_card_view(msg: &ChatMessage) -> Option<CallCardView> {
    let raw = msg.text.trim();
    let is_call_id = msg.id.starts_with("call_");
    let is_call_text = raw == "Cuộc gọi"
        || raw == "Cuộc gọi thoại"
        || raw == "Cuộc gọi video"
        || raw.starts_with("📞")
        || raw.starts_with("📹")
        || raw.starts_with("Cuộc gọi đi")
        || raw.starts_with("Cuộc gọi đến")
        || raw.starts_with("Cuộc gọi video");
    if !is_call_id && !is_call_text {
        return None;
    }

    let lower = raw.to_lowercase();
    let is_video = lower.contains("video") || raw.starts_with("📹");
    let is_incoming = lower.contains("gọi đến") || (!lower.contains("gọi đi") && !msg.is_outgoing);

    // Parse optional duration `(Ns)`
    let secs_opt: Option<u64> = raw.find('(').and_then(|open| {
        let rest = &raw[open + 1..];
        let close = rest.find("s)")?;
        rest[..close].trim().parse::<u64>().ok()
    });

    // Parse optional failure/rejection reason after `—` or `-`
    let raw_reason = raw
        .split_once(" — ")
        .or_else(|| raw.split_once(" - "))
        .map(|(_, r)| r.trim().to_string())
        .filter(|r| !r.is_empty());

    let is_missed_or_failed = match secs_opt {
        Some(0) => true,
        Some(_) => raw_reason.is_some(),
        None => raw_reason.is_some() || (!msg.is_outgoing && raw == "Cuộc gọi"),
    };

    let title = if is_video {
        if is_incoming {
            if is_missed_or_failed {
                "Cuộc gọi video nhỡ".to_string()
            } else {
                "Cuộc gọi video đến".to_string()
            }
        } else {
            "Cuộc gọi video đi".to_string()
        }
    } else if is_incoming {
        if is_missed_or_failed {
            "Cuộc gọi thoại nhỡ".to_string()
        } else {
            "Cuộc gọi thoại đến".to_string()
        }
    } else {
        "Cuộc gọi thoại đi".to_string()
    };

    let arrow = if is_incoming { "↙" } else { "↗" };
    let status_badge = match (is_missed_or_failed, secs_opt) {
        // The title already names the missed/failed state; show the reason instead of repeating it.
        (true, _) => {
            let reason_txt = raw_reason
                .as_deref()
                .filter(|r| !r.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| {
                    if is_incoming {
                        "Bạn đã bỏ lỡ cuộc gọi này".to_string()
                    } else {
                        "Người nhận không nghe máy".to_string()
                    }
                });
            format!("{arrow} {reason_txt}")
        }
        (false, Some(s)) if s >= 60 => {
            format!("{arrow} Thời lượng: {} phút {:02} giây", s / 60, s % 60)
        }
        (false, Some(s)) => format!("{arrow} Thời lượng: {s} giây"),
        (false, None) => format!("{arrow} Đã kết thúc"),
    };

    // When the call is missed/failed the badge already carries the reason, so only surface a
    // separate reason line for calls that otherwise look successful.
    let reason = if is_missed_or_failed {
        None
    } else {
        raw_reason.map(|r| {
            if r.contains("gián đoạn") || r.contains("từ chối") {
                "Người nhận từ chối hoặc kết nối gián đoạn".to_string()
            } else if r.contains("đang bận") {
                "Người nhận đang bận hoặc từ chối".to_string()
            } else {
                r
            }
        })
    };

    Some(CallCardView {
        title,
        status_badge,
        reason,
        is_video,
        is_missed_or_failed,
    })
}

pub(super) fn render_call_card_widget(
    ui: &mut egui::Ui,
    card: &CallCardView,
    pal: &ThemePalette,
    card_width: f32,
) -> Option<BubbleAction> {
    let mut action = None;
    let accent = if card.is_missed_or_failed {
        Color32::from_rgb(239, 68, 68)
    } else {
        Color32::from_rgb(34, 197, 94)
    };

    ui.set_width(card_width);
    let text_col_w = (card_width - 52.0).max(110.0);
    ui.vertical(|ui| {
        ui.set_width(card_width);
        ui.horizontal(|ui| {
            let (icon_rect, _) = ui.allocate_exact_size(Vec2::new(40.0, 40.0), Sense::hover());
            ui.painter().circle_filled(
                icon_rect.center(),
                20.0,
                accent.linear_multiply(if pal.is_dark { 0.18 } else { 0.14 }),
            );
            ui.painter().circle_stroke(
                icon_rect.center(),
                20.0,
                Stroke::new(1.2_f32, accent.linear_multiply(0.55)),
            );
            if card.is_video {
                draw_icon_video(ui.painter(), icon_rect.center(), 18.0, accent);
            } else {
                draw_icon_phone(ui.painter(), icon_rect.center(), 18.0, accent);
            }

            ui.add_space(6.0);
            ui.vertical(|ui| {
                ui.set_width(text_col_w);
                ui.add(
                    egui::Label::new(
                        RichText::new(&card.title)
                            .size(13.8)
                            .color(pal.text_primary)
                            .strong(),
                    )
                    .wrap(),
                );
                ui.add_space(2.0);
                ui.add(
                    egui::Label::new(
                        RichText::new(&card.status_badge)
                            .size(11.8)
                            .color(accent)
                            .strong(),
                    )
                    .wrap(),
                );
                if let Some(reason) = &card.reason {
                    ui.add_space(3.0);
                    ui.add(
                        egui::Label::new(
                            RichText::new(reason).size(11.2).color(pal.text_secondary),
                        )
                        .wrap(),
                    );
                }
            });
        });
        ui.add_space(8.0);
        let (sep_rect, _) = ui.allocate_exact_size(Vec2::new(card_width, 1.0), Sense::hover());
        ui.painter().line_segment(
            [sep_rect.left_center(), sep_rect.right_center()],
            Stroke::new(1.0_f32, pal.border),
        );
        ui.add_space(6.0);

        let btn_label = if card.is_video {
            format!("{}  Gọi video lại", lucide::VIDEO)
        } else {
            format!("{}  Gọi lại", lucide::PHONE)
        };
        let call_btn = egui::Button::new(
            RichText::new(btn_label)
                .size(12.5)
                .color(Color32::WHITE)
                .strong(),
        )
        .fill(pal.primary)
        .stroke(Stroke::new(1.0_f32, pal.primary_hover))
        .rounding(Rounding::same(7.0))
        .min_size(Vec2::new(card_width, 30.0));

        if ui
            .add(call_btn)
            .on_hover_cursor(CursorIcon::PointingHand)
            .clicked()
        {
            action = Some(BubbleAction::CallAgain {
                is_video: card.is_video,
            });
        }
    });
    action
}

/// Open an `http://` or `https://` URL in the user's default system browser.
pub(super) fn open_external_url(url: &str) {
    let trimmed = url.trim();
    if !(trimmed.starts_with("https://") || trimmed.starts_with("http://")) {
        return;
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        let _ = std::process::Command::new("rundll32.exe")
            .args(["url.dll,FileProtocolHandler", trimmed])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(trimmed).spawn();
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(trimmed).spawn();
    }
}
/// Open a native OS file-picker dialog on a worker thread and return the selected path.
pub(super) fn pick_native_file_dialog(images_only: bool) -> Option<std::path::PathBuf> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        let filter = if images_only {
            "Image Files (*.png;*.jpg;*.jpeg;*.webp;*.gif;*.bmp)|*.png;*.jpg;*.jpeg;*.webp;*.gif;*.bmp|All Files (*.*)|*.*"
        } else {
            "All Files (*.*)|*.*|Video & Media (*.mp4;*.mov;*.mkv;*.mp3;*.wav)|*.mp4;*.mov;*.mkv;*.mp3;*.wav|Documents (*.pdf;*.zip;*.docx;*.xlsx;*.txt)|*.pdf;*.zip;*.docx;*.xlsx;*.txt"
        };
        let title = if images_only {
            "Chọn hình ảnh để gửi qua Zalo"
        } else {
            "Chọn video hoặc tập tin đính kèm để gửi qua Zalo"
        };
        let script = format!(
            "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; \
             Add-Type -AssemblyName System.Windows.Forms; \
             $f = New-Object System.Windows.Forms.OpenFileDialog; \
             $f.Title = '{title}'; \
             $f.Filter = '{filter}'; \
             $top = New-Object System.Windows.Forms.Form; \
             $top.TopMost = $true; \
             if ($f.ShowDialog($top) -eq [System.Windows.Forms.DialogResult]::OK) {{ Write-Output $f.FileName }}"
        );
        let out = std::process::Command::new("powershell")
            .args(["-NoProfile", "-STA", "-Command", &script])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()?;
        let path_str = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if path_str.is_empty() {
            None
        } else {
            Some(std::path::PathBuf::from(path_str))
        }
    }
    #[cfg(target_os = "macos")]
    {
        let prompt = if images_only {
            "POSIX path of (choose file with prompt \"Chọn hình ảnh để gửi qua Zalo\" of type {\"public.image\"})"
        } else {
            "POSIX path of (choose file with prompt \"Chọn tập tin đính kèm để gửi qua Zalo\")"
        };
        if let Ok(out) = std::process::Command::new("osascript")
            .args(["-e", prompt])
            .output()
        {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !s.is_empty() {
                    return Some(std::path::PathBuf::from(s));
                }
            }
        }
        None
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let mut zcmd = std::process::Command::new("zenity");
        zcmd.arg("--file-selection");
        if images_only {
            zcmd.arg("--file-filter=Images | *.png *.jpg *.jpeg *.webp *.gif *.bmp");
        }
        if let Ok(out) = zcmd.output() {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !s.is_empty() {
                    return Some(std::path::PathBuf::from(s));
                }
            }
        }
        if let Ok(out) = std::process::Command::new("kdialog")
            .arg("--getopenfilename")
            .output()
        {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !s.is_empty() {
                    return Some(std::path::PathBuf::from(s));
                }
            }
        }
        None
    }
}

/// Explicit user-action picker. Call only from a background worker, never during startup.
pub(super) fn pick_native_files_dialog(images_only: bool) -> Vec<std::path::PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let selection = if images_only {
            "choose file with prompt \"Chọn hình ảnh để gửi qua Zalo\" of type {\"public.image\"} with multiple selections allowed"
        } else {
            "choose file with prompt \"Chọn tập tin đính kèm để gửi qua Zalo\" with multiple selections allowed"
        };
        // Foundation JSON serialization preserves commas, Unicode, and embedded newlines in paths.
        let script = format!(
            r#"use framework "Foundation"
use scripting additions
set chosenFiles to {selection}
set filePaths to {{}}
repeat with selectedFile in chosenFiles
    set end of filePaths to POSIX path of selectedFile
end repeat
set jsonData to current application's NSJSONSerialization's dataWithJSONObject:filePaths options:0 |error|:(missing value)
set jsonText to current application's NSString's alloc()'s initWithData:jsonData encoding:(current application's NSUTF8StringEncoding)
return jsonText as text"#
        );
        if let Ok(output) = std::process::Command::new("osascript")
            .args(["-e", &script])
            .output()
        {
            if output.status.success() {
                return decode_picker_paths(&output.stdout);
            }
        }
        Vec::new()
    }
    #[cfg(not(target_os = "macos"))]
    {
        pick_native_file_dialog(images_only).into_iter().collect()
    }
}

fn decode_picker_paths(bytes: &[u8]) -> Vec<std::path::PathBuf> {
    if bytes.len() > 256 * 1024 {
        return Vec::new();
    }
    serde_json::from_slice::<Vec<String>>(bytes)
        .unwrap_or_default()
        .into_iter()
        .map(std::path::PathBuf::from)
        .filter(|path| path.is_absolute())
        .collect()
}

#[cfg(test)]
mod native_picker_tests {
    use super::decode_picker_paths;
    #[test]
    fn json_paths_preserve_commas_unicode_and_newlines() {
        let paths = vec!["/tmp/hello,world.png", "/tmp/ảnh bé.png", "/tmp/a\nb.png"];
        let encoded = serde_json::to_vec(&paths).unwrap();
        let decoded = decode_picker_paths(&encoded);
        assert_eq!(
            decoded,
            paths
                .iter()
                .map(std::path::PathBuf::from)
                .collect::<Vec<_>>()
        );
        assert!(decode_picker_paths(b"not-json").is_empty());
        assert!(decode_picker_paths(br#"["relative.png"]"#).is_empty());
    }
}

/// Detect `Ctrl+V` or `Shift+Insert` reliably on Windows even when `egui-winit` swallows `Key::V`
/// because the system clipboard contains an image instead of UTF-8 text.
pub(super) fn is_clipboard_paste_triggered(ctx: &egui::Context) -> bool {
    let egui_paste = ctx.input(|i| {
        (i.modifiers.ctrl && i.key_pressed(Key::V))
            || (i.modifiers.shift && i.key_pressed(Key::Insert))
            || i.events.iter().any(|e| matches!(e, egui::Event::Paste(_)))
    });

    #[cfg(target_os = "windows")]
    {
        #[link(name = "user32")]
        extern "system" {
            fn GetAsyncKeyState(v_key: i32) -> i16;
        }
        static WAS_PASTE_DOWN: std::sync::atomic::AtomicBool =
            std::sync::atomic::AtomicBool::new(false);

        let is_focused = ctx.input(|i| i.focused);
        if !is_focused {
            WAS_PASTE_DOWN.store(false, std::sync::atomic::Ordering::SeqCst);
            return egui_paste;
        }
        let (win32_down, modifier_held) = unsafe {
            let ctrl = (GetAsyncKeyState(0x11) as u16 & 0x8000) != 0; // VK_CONTROL
            let v_key = (GetAsyncKeyState(0x56) as u16 & 0x8000) != 0; // 'V'
            let shift = (GetAsyncKeyState(0x10) as u16 & 0x8000) != 0; // VK_SHIFT
            let ins = (GetAsyncKeyState(0x2D) as u16 & 0x8000) != 0; // VK_INSERT
            ((ctrl && v_key) || (shift && ins), ctrl || shift)
        };
        if modifier_held {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
        let prev = WAS_PASTE_DOWN.swap(win32_down, std::sync::atomic::Ordering::SeqCst);
        let edge_triggered = win32_down && !prev;
        return egui_paste || edge_triggered;
    }

    #[cfg(not(target_os = "windows"))]
    {
        egui_paste
    }
}

pub(super) fn finalize_clipboard_rgba_image(
    rgba: image::RgbaImage,
) -> Option<(std::path::PathBuf, u32, u32, egui::ColorImage)> {
    let (width, height) = rgba.dimensions();
    if width == 0 || height == 0 {
        return None;
    }
    let _ = std::fs::create_dir_all("stickers_cache");
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let path = std::path::PathBuf::from(format!("stickers_cache/clipboard_{now_ms}.png"));
    rgba.save(&path).ok()?;

    let thumb = if width > 240 || height > 240 {
        image::imageops::thumbnail(&rgba, 240, 240)
    } else {
        rgba
    };
    let (tw, th) = thumb.dimensions();
    let col_img =
        egui::ColorImage::from_rgba_unmultiplied([tw as usize, th as usize], thumb.as_raw());
    Some((path, width, height, col_img))
}

/// Read an image from the Windows Clipboard supporting:
/// 1. Registered `"PNG"` / `"image/png"` streams (Snipping Tool, ShareX, Chrome/Edge, Discord)
/// 2. `CF_HDROP` (`15`) copied image files from Windows Explorer or ShareX
/// 3. `CF_DIB` (`8`) / `CF_DIBV5` (`17`) 24-bit & 32-bit bitmaps (`BI_RGB` & `BI_BITFIELDS`)
pub(super) fn read_clipboard_image_to_png(
) -> Option<(std::path::PathBuf, u32, u32, egui::ColorImage)> {
    #[cfg(target_os = "windows")]
    {
        const CF_DIB: u32 = 8;
        const CF_HDROP: u32 = 15;
        #[link(name = "user32")]
        extern "system" {
            fn IsClipboardFormatAvailable(format: u32) -> i32;
            fn RegisterClipboardFormatA(lpsz_format: *const u8) -> u32;
            fn OpenClipboard(h_wnd_new_owner: *mut std::ffi::c_void) -> i32;
            fn CloseClipboard() -> i32;
            fn GetClipboardData(u_format: u32) -> *mut std::ffi::c_void;
        }
        #[link(name = "kernel32")]
        extern "system" {
            fn GlobalLock(h_mem: *mut std::ffi::c_void) -> *const u8;
            fn GlobalUnlock(h_mem: *mut std::ffi::c_void) -> i32;
            fn GlobalSize(h_mem: *mut std::ffi::c_void) -> usize;
        }
        #[link(name = "shell32")]
        extern "system" {
            fn DragQueryFileW(
                h_drop: *mut std::ffi::c_void,
                i_file: u32,
                lpsz_file: *mut u16,
                cch: u32,
            ) -> u32;
        }

        unsafe {
            let cf_png = RegisterClipboardFormatA(b"PNG\0".as_ptr());
            let cf_mime_png = RegisterClipboardFormatA(b"image/png\0".as_ptr());
            let has_png = cf_png != 0 && IsClipboardFormatAvailable(cf_png) != 0;
            let has_mime_png = cf_mime_png != 0 && IsClipboardFormatAvailable(cf_mime_png) != 0;
            let has_dib = IsClipboardFormatAvailable(CF_DIB) != 0;
            let has_hdrop = IsClipboardFormatAvailable(CF_HDROP) != 0;

            if !has_png && !has_mime_png && !has_dib && !has_hdrop {
                return None;
            }

            // Retry OpenClipboard briefly in case egui-winit/arboard or ShareX is releasing the lock
            let mut opened = false;
            for _ in 0..6 {
                if OpenClipboard(std::ptr::null_mut()) != 0 {
                    opened = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(8));
            }
            if !opened {
                return None;
            }

            // 1. Try raw PNG stream first (preserves exact RGBA and avoids DIB header quirks)
            for fmt in [cf_png, cf_mime_png] {
                if fmt != 0 && IsClipboardFormatAvailable(fmt) != 0 {
                    let hmem = GetClipboardData(fmt);
                    if !hmem.is_null() {
                        let ptr = GlobalLock(hmem);
                        let len = GlobalSize(hmem);
                        if !ptr.is_null() && len > 8 {
                            let bytes = std::slice::from_raw_parts(ptr, len).to_vec();
                            GlobalUnlock(hmem);
                            if let Ok(dyn_img) = image::load_from_memory(&bytes) {
                                CloseClipboard();
                                return finalize_clipboard_rgba_image(dyn_img.to_rgba8());
                            }
                        } else if !ptr.is_null() {
                            GlobalUnlock(hmem);
                        }
                    }
                }
            }

            // 2. Try CF_HDROP (copied image file in Windows Explorer or ShareX)
            if has_hdrop {
                let hdrop = GetClipboardData(CF_HDROP);
                if !hdrop.is_null() {
                    let mut buf = vec![0u16; 1024];
                    let chars =
                        DragQueryFileW(hdrop, 0, buf.as_mut_ptr(), buf.len() as u32) as usize;
                    if chars > 0 && chars < buf.len() {
                        let file_path =
                            std::path::PathBuf::from(String::from_utf16_lossy(&buf[..chars]));
                        if let Ok(dyn_img) = image::open(&file_path) {
                            CloseClipboard();
                            let rgba = dyn_img.to_rgba8();
                            let (width, height) = rgba.dimensions();
                            let thumb = if width > 240 || height > 240 {
                                image::imageops::thumbnail(&rgba, 240, 240)
                            } else {
                                rgba
                            };
                            let (tw, th) = thumb.dimensions();
                            let col_img = egui::ColorImage::from_rgba_unmultiplied(
                                [tw as usize, th as usize],
                                thumb.as_raw(),
                            );
                            return Some((file_path, width, height, col_img));
                        }
                    }
                }
            }

            // 3. Fallback to CF_DIB (24-bit BGR or 32-bit BGRA/BGRX, BI_RGB or BI_BITFIELDS)
            if has_dib {
                let hmem = GetClipboardData(CF_DIB);
                if !hmem.is_null() {
                    let ptr = GlobalLock(hmem);
                    let len = GlobalSize(hmem);
                    if !ptr.is_null() && len >= 40 {
                        let slice = std::slice::from_raw_parts(ptr, len);
                        let bi_size =
                            u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]) as usize;
                        let width_i = i32::from_le_bytes([slice[4], slice[5], slice[6], slice[7]]);
                        let height_i =
                            i32::from_le_bytes([slice[8], slice[9], slice[10], slice[11]]);
                        let bit_count = u16::from_le_bytes([slice[14], slice[15]]);
                        let compression =
                            u32::from_le_bytes([slice[16], slice[17], slice[18], slice[19]]);

                        if width_i > 0
                            && height_i != 0
                            && (bit_count == 24 || bit_count == 32)
                            && bi_size >= 40
                            && bi_size <= slice.len()
                        {
                            let width = width_i as u32;
                            let top_down = height_i < 0;
                            let height = height_i.unsigned_abs();
                            let mut offset = bi_size;
                            if (compression == 3 || compression == 6) && bi_size == 40 {
                                offset += if compression == 6 { 16 } else { 12 };
                            }
                            let bytes_per_px = (bit_count / 8) as usize;
                            let row_stride = (width as usize * bytes_per_px + 3) & !3;
                            let needed =
                                offset.saturating_add(row_stride.saturating_mul(height as usize));
                            if needed <= slice.len() {
                                let mut rgba = image::RgbaImage::new(width, height);
                                let mut any_nonzero_alpha = false;
                                if bytes_per_px == 4 {
                                    for y in 0..height {
                                        let src_y =
                                            if top_down { y } else { height - 1 - y } as usize;
                                        let row = &slice[offset + src_y * row_stride..];
                                        for x in 0..width {
                                            if row[x as usize * 4 + 3] != 0 {
                                                any_nonzero_alpha = true;
                                                break;
                                            }
                                        }
                                        if any_nonzero_alpha {
                                            break;
                                        }
                                    }
                                }
                                for y in 0..height {
                                    let src_y = if top_down { y } else { height - 1 - y } as usize;
                                    let row = &slice[offset + src_y * row_stride..];
                                    for x in 0..width {
                                        let p = x as usize * bytes_per_px;
                                        let b = row[p];
                                        let g = row[p + 1];
                                        let r = row[p + 2];
                                        let a = if bytes_per_px == 4 && any_nonzero_alpha {
                                            row[p + 3]
                                        } else {
                                            255
                                        };
                                        rgba.put_pixel(x, y, image::Rgba([r, g, b, a]));
                                    }
                                }
                                GlobalUnlock(hmem);
                                CloseClipboard();
                                return finalize_clipboard_rgba_image(rgba);
                            }
                        }
                    }
                    if !ptr.is_null() {
                        GlobalUnlock(hmem);
                    }
                }
            }

            CloseClipboard();
            None
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        // Wayland (`wl-paste --type image/png`) or X11 (`xclip -selection clipboard -t image/png -o`)
        for (prog, args) in [
            ("wl-paste", &["--no-newline", "--type", "image/png"][..]),
            (
                "xclip",
                &["-selection", "clipboard", "-t", "image/png", "-o"][..],
            ),
        ] {
            if let Ok(out) = std::process::Command::new(prog).args(args).output() {
                if out.status.success() && out.stdout.len() > 8 {
                    if let Ok(dyn_img) = image::load_from_memory(&out.stdout) {
                        return finalize_clipboard_rgba_image(dyn_img.to_rgba8());
                    }
                }
            }
        }
        None
    }
}

/// Format a URL for clean UI display by stripping tracking query strings (`?_r=1&utm_...`)
/// and truncating to `max_chars`.
pub(super) fn clean_display_url(url: &str, max_chars: usize) -> String {
    let trimmed = url.trim();
    let no_scheme = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
        .unwrap_or(trimmed);
    let base = no_scheme
        .split('?')
        .next()
        .unwrap_or(no_scheme)
        .trim_end_matches('/');
    truncate_chars(base, max_chars).into_owned()
}

/// Extract `(user_comment, url, domain)` from a message when `msg.link_url` is set or when
/// `msg.text` contains an embedded `http://` / `https://` URL.
///
/// Note: `user_comment` only contains extra text typed by the sender *outside* the link's own
/// `link_title` / `link_desc` (so `link_desc` is rendered inside the Link Preview Card rather than
/// duplicated as a large paragraph above the card).
pub(super) fn extract_clean_domain(url: &str) -> String {
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or("Liên kết")
        .trim()
        .strip_prefix("www.")
        .unwrap_or_else(|| {
            url.split("://")
                .nth(1)
                .unwrap_or(url)
                .split('/')
                .next()
                .unwrap_or("Liên kết")
                .trim()
        })
        .to_string()
}

pub(super) fn extract_message_link_view(msg: &ChatMessage) -> Option<(String, String, String)> {
    if let Some(url) = msg.link_url.as_deref().filter(|u| !u.is_empty()) {
        let desc_trim = msg.link_desc.as_deref().unwrap_or("").trim();
        let title_trim = msg.link_title.as_deref().unwrap_or("").trim();
        let url_base = url.split('?').next().unwrap_or(url);

        let mut kept_lines = Vec::new();
        for raw_line in msg.text.lines() {
            let mut line = raw_line.trim().to_string();
            if let Some(pos) = line.find("http://").or_else(|| line.find("https://")) {
                let after = &line[pos..];
                let end = after.find(char::is_whitespace).unwrap_or(after.len());
                let found = &after[..end];
                if found == url || found.starts_with(url_base) {
                    line = format!("{}{}", &line[..pos], &after[end..])
                        .trim()
                        .to_string();
                }
            }
            if line.is_empty()
                || line == url
                || line == title_trim
                || line == desc_trim
                || line == "[Chia sẻ]"
                || line == "[Liên kết]"
                || (!desc_trim.is_empty() && desc_trim.contains(&line))
                || (!title_trim.is_empty() && title_trim.contains(&line))
            {
                continue;
            }
            kept_lines.push(line);
        }
        let user_comment = kept_lines.join("\n").trim().to_string();
        let domain = extract_clean_domain(url);
        return Some((user_comment, url.to_string(), domain));
    }
    let text = msg.text.trim();
    let pos = text.find("https://").or_else(|| text.find("http://"))?;
    let after = &text[pos..];
    let end = after.find(char::is_whitespace).unwrap_or(after.len());
    let url = after[..end].trim_end_matches([')', ']', ',', ';']);
    if url.len() < 10 {
        return None;
    }
    let caption = format!("{}{}", &text[..pos], &after[end..])
        .trim()
        .to_string();
    let domain = extract_clean_domain(url);
    Some((caption, url.to_string(), domain))
}

/// Normalize a millisecond or second timestamp to milliseconds (`> 0`).
pub(super) fn normalize_ts_ms(ts: i64) -> i64 {
    crate::clock::normalize_ts_ms(ts)
}

/// Return `true` when a date/time separator pill should be inserted before `curr`.
pub(super) fn should_show_time_divider(prev: Option<&ChatMessage>, curr: &ChatMessage) -> bool {
    let Some(prev) = prev else {
        return true;
    };
    let prev_ms = normalize_ts_ms(prev.ts);
    let curr_ms = normalize_ts_ms(curr.ts);
    if prev_ms > 0 && curr_ms > 0 {
        let prev_day = crate::clock::local_day(prev.ts);
        let curr_day = crate::clock::local_day(curr.ts);
        if prev_day != curr_day {
            return true;
        }
        return (curr_ms - prev_ms).abs() >= 30 * 60 * 1000;
    }
    false
}

/// Format the label for a centered date/time separator pill.
pub(super) fn format_time_divider_label(msg: &ChatMessage) -> String {
    let ts_ms = normalize_ts_ms(msg.ts);
    if ts_ms <= 0 {
        return if msg.time.is_empty() {
            "Cuộc trò chuyện".to_string()
        } else {
            format!("Hôm nay • {}", msg.time)
        };
    }
    let Some(local) = crate::clock::local_datetime(msg.ts) else {
        return "Cuộc trò chuyện".into();
    };
    let msg_day = crate::clock::local_day(msg.ts).unwrap_or(0);
    let today_day = crate::clock::local_day(crate::login::unix_now() * 1000).unwrap_or(0);
    let hm = local.format("%H:%M");

    if msg_day == today_day {
        format!("Hôm nay • {hm}")
    } else if msg_day == today_day - 1 {
        format!("Hôm qua • {hm}")
    } else {
        local.format("%d/%m/%Y").to_string()
    }
}

/// Render a centered Zalo-style compact date pill (no side hairlines).
pub(super) fn render_date_divider(ui: &mut egui::Ui, msg: &ChatMessage, pal: &ThemePalette) {
    let label = format_time_divider_label(msg);
    let avail_w = ui.available_width();
    ui.horizontal(|ui| {
        let text_w = ui.fonts(|f| {
            f.layout_no_wrap(
                label.clone(),
                FontId::proportional(11.5),
                pal.text_secondary,
            )
            .size()
            .x
        });
        let pill_w = text_w + 26.0;
        let left_space = ((avail_w - pill_w) * 0.5).max(0.0);
        if left_space > 0.0 {
            ui.add_space(left_space);
        }
        let pill_bg = if pal.is_dark {
            Color32::from_rgba_unmultiplied(30, 34, 41, 235)
        } else {
            Color32::from_rgba_unmultiplied(219, 228, 240, 215)
        };
        egui::Frame::none()
            .fill(pill_bg)
            .rounding(Rounding::same(10.0))
            .inner_margin(Margin::symmetric(13.0, 3.5))
            .show(ui, |ui| {
                ui.label(
                    RichText::new(label)
                        .size(11.4)
                        .color(if pal.is_dark {
                            Color32::from_rgb(215, 218, 224)
                        } else {
                            pal.text_secondary
                        })
                        .strong(),
                );
            });
    });
}

/// Detect whether `msg` is a system/event notification rather than a user speech bubble.
pub(super) fn is_system_notice_message(msg: &ChatMessage) -> bool {
    if msg.id.starts_with("sys_") {
        return true;
    }
    let t = msg.text.trim();
    t.starts_with("[Hệ thống]")
        || t.starts_with("[Thông báo]")
        || (t.starts_with("Bạn và ") && t.contains("đã trở thành bạn bè"))
        || t.contains("đã thêm bạn vào nhóm")
        || t.contains("đã tham gia nhóm")
        || t.contains("đã rời khỏi nhóm")
        || t.contains("đã đổi tên nhóm")
}

/// Render a centered system event pill (e.g., friend connected, group event).
pub(super) fn render_system_notice_pill(ui: &mut egui::Ui, msg: &ChatMessage, pal: &ThemePalette) {
    let avail_w = ui.available_width();
    let text = msg.text.trim();
    ui.horizontal(|ui| {
        let text_w = ui.fonts(|f| {
            f.layout_no_wrap(
                text.to_string(),
                FontId::proportional(12.0),
                pal.text_secondary,
            )
            .size()
            .x
        });
        let pill_w = (text_w + 28.0).min(avail_w - 40.0);
        let left_pad = ((avail_w - pill_w) * 0.5).max(0.0);
        if left_pad > 0.0 {
            ui.add_space(left_pad);
        }
        egui::Frame::none()
            .fill(pal.bg_elevated)
            .stroke(Stroke::new(1.0_f32, pal.border))
            .rounding(Rounding::same(12.0))
            .inner_margin(Margin::symmetric(12.0, 4.0))
            .show(ui, |ui| {
                ui.label(
                    RichText::new(format!("{} {text}", lucide::SPARKLES))
                        .size(12.0)
                        .color(pal.text_secondary),
                );
            });
    });
}

/// Return `true` when two adjacent messages belong to the same visual bubble group.
pub(super) fn same_message_cluster(a: &ChatMessage, b: &ChatMessage) -> bool {
    if is_system_notice_message(a) || is_system_notice_message(b) {
        return false;
    }
    if a.is_outgoing != b.is_outgoing || a.sender != b.sender {
        return false;
    }
    if should_show_time_divider(Some(a), b) {
        return false;
    }
    let a_ms = normalize_ts_ms(a.ts);
    let b_ms = normalize_ts_ms(b.ts);
    if a_ms > 0 && b_ms > 0 && (b_ms - a_ms).abs() > 10 * 60 * 1000 {
        return false;
    }
    true
}

#[derive(Debug, PartialEq, Eq)]
enum MediaActivation {
    RetryPreview,
    OpenVideo,
    OpenPhoto,
}
fn media_activation(is_video: bool, failed: bool) -> MediaActivation {
    if is_video {
        MediaActivation::OpenVideo
    } else if failed {
        MediaActivation::RetryPreview
    } else {
        MediaActivation::OpenPhoto
    }
}

fn native_media_mime(url: &str, is_video: bool) -> &'static str {
    let path = url
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    if is_video {
        if path.ends_with(".mov") {
            "video/quicktime"
        } else if path.ends_with(".webm") {
            "video/webm"
        } else {
            "video/mp4"
        }
    } else if path.ends_with(".png") {
        "image/png"
    } else if path.ends_with(".webp") {
        "image/webp"
    } else if path.ends_with(".heic") || path.ends_with(".heif") {
        "image/heic"
    } else if path.ends_with(".jxl") {
        "image/jxl"
    } else if path.ends_with(".jpg") || path.ends_with(".jpeg") {
        "image/jpeg"
    } else {
        "image/*"
    }
}

/// Render photos and video posters as visual media, never as raw URL link cards.
// Keep the existing renderer boundary stable; rendering state is caller-owned.
#[allow(clippy::too_many_arguments)]
fn render_inline_media(
    ui: &mut egui::Ui,
    msg: &ChatMessage,
    max_width: f32,
    text_col: Color32,
    textures: &HashMap<String, TextureHandle>,
    loading: &mut HashSet<String>,
    failures: &HashSet<String>,
    is_video: bool,
    _sender: String,
    target: (&str, bool),
) -> Option<BubbleAction> {
    let preview_url = msg
        .link_thumb
        .as_deref()
        .filter(|s| !s.is_empty())
        .or_else(|| (!is_video).then_some(msg.link_url.as_deref()).flatten());
    let key = preview_url.map(|url| format!("thumb_{}", crate::crypto::md5_hex(url)));
    let texture = key.as_ref().and_then(|key| textures.get(key));
    let width_limit = if max_width.is_nan() {
        1.0
    } else {
        max_width.clamp(1.0, 320.0)
    };
    let size = texture
        .map(|tex| {
            let [w, h] = tex.size();
            let scale = (width_limit / w.max(1) as f32)
                .min(300.0 / h.max(1) as f32)
                .min(1.0);
            Vec2::new(w.max(1) as f32 * scale, h.max(1) as f32 * scale)
        })
        .unwrap_or(Vec2::new(
            width_limit.min(240.0),
            if is_video { 135.0 } else { 180.0 },
        ));
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let failed = key.as_ref().is_some_and(|key| failures.contains(key));
    let visible = ui.is_rect_visible(rect);
    if let Some(tex) = texture {
        ui.painter().image(
            tex.id(),
            rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
    } else {
        ui.painter()
            .rect_filled(rect, Rounding::same(10.0), text_col.linear_multiply(0.07));
        let label = if failed && is_video {
            "Chưa tải được preview"
        } else if failed {
            "Chưa tải được preview\nBấm để thử lại"
        } else if preview_url.is_none() {
            "Video không có ảnh xem trước"
        } else {
            "Đang tải preview…"
        };
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            label,
            FontId::proportional(12.0),
            text_col,
        );
    }
    if is_video {
        let center = rect.center();
        ui.painter()
            .circle_filled(center, 20.0, Color32::from_black_alpha(150));
        ui.painter().add(egui::Shape::convex_polygon(
            vec![
                center + Vec2::new(-5.0, -9.0),
                center + Vec2::new(-5.0, 9.0),
                center + Vec2::new(10.0, 0.0),
            ],
            Color32::WHITE,
            Stroke::NONE,
        ));
        ui.painter().text(
            rect.right_bottom() - Vec2::new(7.0, 6.0),
            egui::Align2::RIGHT_BOTTOM,
            "Video",
            FontId::proportional(11.0),
            Color32::WHITE,
        );
    }
    if visible && texture.is_none() && !failed {
        if let (Some(url), Some(key)) = (preview_url, key.as_ref()) {
            if loading.insert(key.clone()) {
                if let Some(tx) = WORKER_TX.get() {
                    let _ = tx.send(WorkerCmd::FetchMediaPreview {
                        key: key.clone(),
                        url: url.to_string(),
                        msg: msg.clone(),
                        target_id: target.0.to_string(),
                        is_group: target.1,
                    });
                }
            }
        }
    }
    let mut action = None;
    if response.on_hover_cursor(CursorIcon::PointingHand).clicked() {
        if media_activation(is_video, failed) == MediaActivation::RetryPreview {
            if let (Some(url), Some(key), Some(tx)) = (preview_url, key.as_ref(), WORKER_TX.get()) {
                let _ = tx.send(WorkerCmd::FetchMediaPreview {
                    key: key.clone(),
                    url: url.to_string(),
                    msg: msg.clone(),
                    target_id: target.0.to_string(),
                    is_group: target.1,
                });
            }
        } else if let Some(url) = msg.link_url.as_deref().or(preview_url) {
            action = Some(BubbleAction::OpenNativeMedia {
                url: url.to_owned(),
                mime: native_media_mime(url, is_video).to_owned(),
            });
        }
    }
    let caption = msg
        .link_desc
        .as_deref()
        .filter(|s| {
            !s.trim().is_empty() && !matches!(s.trim(), "[Photo]" | "[Hình ảnh]" | "[Video]")
        })
        .or_else(|| {
            let text = msg.text.trim();
            (!text.starts_with('[')
                && !text.starts_with("http://")
                && !text.starts_with("https://")
                && !text.is_empty())
            .then_some(text)
        });
    if let Some(caption) = caption {
        ui.add_space(5.0);
        action = render_message_text(ui, msg, caption, 14.2, text_col, max_width).or(action);
    }
    action
}

fn render_message_text(
    ui: &mut egui::Ui,
    msg: &ChatMessage,
    text: &str,
    size: f32,
    color: Color32,
    width: f32,
) -> Option<BubbleAction> {
    let mut mentions = crate::message_navigation::message_mentions(msg);
    if text != msg.text {
        let mut locations = msg.text.match_indices(text);
        let offset = locations.next().map(|(offset, _)| offset);
        if text.is_empty() || locations.next().is_some() {
            mentions.clear();
        } else if let Some(offset) = offset {
            mentions.retain(|span| {
                span.byte_range.start >= offset && span.byte_range.end <= offset + text.len()
            });
            for span in &mut mentions {
                span.byte_range.start -= offset;
                span.byte_range.end -= offset;
            }
        } else {
            mentions.clear();
        }
    }
    super::emoji_text::show_with_mentions(ui, text, size, color, width, &mentions).map(|index| {
        let mention = &mentions[index];
        BubbleAction::OpenMention {
            user_id: mention.user_id.clone(),
            display_name: text[mention.byte_range.clone()]
                .trim_start_matches('@')
                .to_owned(),
        }
    })
}

// Keep the existing renderer boundary stable; rendering state is caller-owned.

#[allow(clippy::too_many_arguments)]
pub(super) fn render_message_body(
    ui: &mut egui::Ui,
    msg: &ChatMessage,
    thread_name: &str,
    target: (&str, bool),
    pal: &ThemePalette,
    outgoing: bool,
    max_text_width: f32,
    sticker_textures: &HashMap<String, TextureHandle>,
    loading_stickers: &mut HashSet<String>,
    failed_previews: &HashSet<String>,
) -> Option<BubbleAction> {
    let mut body_action = None;
    let (text_col, meta_col) = if outgoing {
        (pal.bubble_out_text, pal.bubble_out_meta)
    } else {
        (pal.bubble_in_text, pal.bubble_in_meta)
    };

    let trimmed_text = msg.text.trim();
    if trimmed_text == "[Tin nhắn đã thu hồi]" {
        ui.label(
            RichText::new(format!("{} Tin nhắn đã được thu hồi", lucide::BAN))
                .color(meta_col)
                .italics()
                .size(13.8),
        );
        return None;
    }

    let sender_label = if msg.is_outgoing {
        "Bạn".to_string()
    } else if !msg.sender.is_empty() && msg.sender != "0" {
        msg.sender.clone()
    } else {
        thread_name.to_string()
    };

    let is_photo = msg.link_src.as_deref() == Some("Hình ảnh Zalo")
        || trimmed_text == "[Hình ảnh]"
        || trimmed_text.starts_with("[Hình ảnh] ");
    let is_video =
        msg.link_src.as_deref() == Some("Video Zalo") || trimmed_text.starts_with("[Video]");
    if is_photo || is_video {
        return render_inline_media(
            ui,
            msg,
            max_text_width,
            text_col,
            sticker_textures,
            loading_stickers,
            failed_previews,
            is_video,
            sender_label,
            target,
        );
    }
    // Dedicated File / Video Attachment Card
    let is_zalo_file_or_video = matches!(
        msg.link_src.as_deref(),
        Some("Tập tin Zalo") | Some("Video Zalo")
    ) || trimmed_text.starts_with("[Tập tin]")
        || trimmed_text.starts_with("[Video]")
        || trimmed_text.starts_with("[Tin nhắn thoại]");
    if is_zalo_file_or_video {
        let is_vid =
            msg.link_src.as_deref() == Some("Video Zalo") || trimmed_text.starts_with("[Video]");
        let title_str = msg
            .link_title
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(trimmed_text);
        let sub_str = msg.link_src.as_deref().unwrap_or(if is_vid {
            "Video đính kèm"
        } else {
            "Tập tin đính kèm"
        });
        let link_col = if pal.is_dark {
            Color32::from_rgb(97, 175, 239)
        } else {
            pal.primary
        };
        let resp = ui
            .add(
                egui::Label::new(RichText::new(title_str).size(13.8).color(text_col).strong())
                    .wrap()
                    .sense(Sense::click()),
            )
            .on_hover_cursor(CursorIcon::PointingHand);
        if resp.clicked() {
            if let Some(url) = msg.link_url.as_deref().filter(|u| !u.is_empty()) {
                open_external_url(url);
            }
        }
        ui.add_space(2.0);
        let dom_resp = ui
            .add(
                egui::Label::new(RichText::new(sub_str).size(12.5).color(link_col))
                    .sense(Sense::click()),
            )
            .on_hover_cursor(CursorIcon::PointingHand);
        if dom_resp.clicked() {
            if let Some(url) = msg.link_url.as_deref().filter(|u| !u.is_empty()) {
                open_external_url(url);
            }
        }
        return None;
    }

    let Some((caption, url, domain)) = extract_message_link_view(msg) else {
        return render_message_text(ui, msg, &msg.text, 14.0, text_col, max_text_width);
    };

    let link_col = if pal.is_dark {
        Color32::from_rgb(97, 175, 239) // #61AFEF Zalo/Atom clean link blue
    } else {
        pal.primary
    };

    // Auto-enrich link metadata if title or thumbnail is missing/expired
    let needs_meta_enrich = msg.link_title.is_none()
        || msg.link_thumb.is_none()
        || msg
            .link_thumb
            .as_deref()
            .map(|th| {
                th.contains("zpcloud.zdn.vn")
                    && !sticker_textures
                        .contains_key(&format!("thumb_{}", crate::crypto::md5_hex(th)))
            })
            .unwrap_or(false);
    let preview_visible = ui.is_rect_visible(Rect::from_min_size(
        ui.cursor().min,
        Vec2::new(max_text_width, 150.0),
    ));
    if needs_meta_enrich && preview_visible {
        let meta_key = format!("linkmeta_{}", crate::crypto::md5_hex(&url));
        if loading_stickers.insert(meta_key) {
            if let Some(tx) = WORKER_TX.get() {
                let _ = tx.send(WorkerCmd::ParseLinkPreview { url: url.clone() });
            }
        }
    }

    // 1. Optional user comment above the link
    if !caption.is_empty() && caption != url {
        body_action = render_message_text(ui, msg, &caption, 13.8, text_col, max_text_width);
        ui.add_space(4.0);
    }

    // 2. Optional compact thumbnail directly inside the bubble (no nested card box)
    if let Some(thumb_url) = msg.link_thumb.as_deref().filter(|u| !u.is_empty()) {
        let key = format!("thumb_{}", crate::crypto::md5_hex(thumb_url));
        if preview_visible
            && !sticker_textures.contains_key(&key)
            && (thumb_url.starts_with("http://") || thumb_url.starts_with("https://"))
            && loading_stickers.insert(key.clone())
        {
            if let Some(tx) = WORKER_TX.get() {
                let _ = tx.send(WorkerCmd::FetchStickerImage {
                    key: key.clone(),
                    url: thumb_url.to_string(),
                });
            }
        }
        if let Some(tex) = sticker_textures.get(&key) {
            let [tw, th] = tex.size();
            let aspect = if tw > 0 && th > 0 {
                (th as f32 / tw as f32).clamp(0.45, 0.85)
            } else {
                0.56
            };
            let thumb_w = max_text_width.min(260.0);
            let img_h = (thumb_w * aspect).min(132.0);
            let img = egui::Image::new(tex)
                .fit_to_exact_size(Vec2::new(thumb_w, img_h))
                .rounding(Rounding::same(6.0))
                .sense(Sense::click());
            if ui
                .add(img)
                .on_hover_cursor(CursorIcon::PointingHand)
                .on_hover_text(&url)
                .clicked()
            {
                open_external_url(&url);
            }
            ui.add_space(5.0);
        }
    }

    // 3. Bold Title (exact Zalo PC style: no globe icon, no vertical accent bar, no nested border)
    let header_title = msg
        .link_title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty() && *t != domain)
        .unwrap_or_else(|| {
            url.strip_prefix("https://")
                .or_else(|| url.strip_prefix("http://"))
                .unwrap_or(&url)
        });
    let title_resp = ui
        .add(
            egui::Label::new(
                RichText::new(truncate_chars(header_title, 95))
                    .size(13.8)
                    .color(text_col)
                    .strong(),
            )
            .wrap()
            .sense(Sense::click()),
        )
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(&url);
    if title_resp.clicked() {
        open_external_url(&url);
    }

    // 4. Optional short description (only if concise and distinct)
    let desc_opt = msg
        .link_desc
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty() && *d != header_title && *d != url && caption.is_empty());
    if let Some(desc) = desc_opt {
        ui.add_space(2.0);
        let d_resp = ui
            .add(
                egui::Label::new(
                    RichText::new(truncate_chars(desc, 110))
                        .size(12.3)
                        .color(meta_col),
                )
                .wrap()
                .sense(Sense::click()),
            )
            .on_hover_cursor(CursorIcon::PointingHand)
            .on_hover_text(&url);
        if d_resp.clicked() {
            open_external_url(&url);
        }
    }

    // 5. Clean Domain line (`docs.google.com`, `drive.google.com`, `tiktok.com`)
    ui.add_space(2.5);
    let dom_resp = ui
        .add(
            egui::Label::new(RichText::new(&domain).size(12.6).color(link_col))
                .sense(Sense::click()),
        )
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(&url);
    if dom_resp.clicked() {
        open_external_url(&url);
    }

    body_action
}

// The preferred minimum must yield to the actual narrow-pane content limit.
fn card_content_width(measured: f32, available: f32) -> f32 {
    let upper = if available.is_nan() {
        0.0
    } else {
        available.clamp(0.0, 340.0)
    };
    measured
        .clamp(135.0_f32.min(upper), upper)
        .ceil()
        .min(upper)
}

// The bubble renderer keeps caller-owned layout, interaction and media state explicit.
#[allow(clippy::too_many_arguments)]
pub(super) fn render_chat_bubble(
    ui: &mut egui::Ui,
    msg: &ChatMessage,
    prev_same_sender: bool,
    next_same_sender: bool,
    is_latest_outgoing: bool,
    is_group: bool,
    thread_name: &str,
    target: (&str, bool),
    sender_avatar: Option<&str>,
    pal: &ThemePalette,
    sticker_textures: &HashMap<String, TextureHandle>,
    loading_stickers: &mut HashSet<String>,
    failed_previews: &HashSet<String>,
    emoji_textures: &HashMap<String, TextureHandle>,
    idx: usize,
) -> Option<BubbleAction> {
    let avail_w = ui.available_width();
    let max_bubble_width = (avail_w * 0.72).clamp(140.0, 520.0);
    let max_text_width = (max_bubble_width - 24.0).max(90.0);
    let has_sticker = is_sticker_message(msg).is_some();
    let call_card_opt = extract_call_card_view(msg);
    let is_call_card = call_card_opt.is_some();
    let is_photo_card = matches!(
        msg.link_src.as_deref(),
        Some("Hình ảnh Zalo") | Some("Video Zalo")
    ) || msg.text.trim() == "[Hình ảnh]"
        || msg.text.trim().starts_with("[Hình ảnh] ");
    let is_file_card = matches!(
        msg.link_src.as_deref(),
        Some("Tập tin Zalo") | Some("Video Zalo")
    ) || msg.text.trim().starts_with("[Tập tin]")
        || msg.text.trim().starts_with("[Video]")
        || msg.text.trim().starts_with("[Tin nhắn thoại]");
    let has_link_card = extract_message_link_view(msg).is_some();
    let has_interactive_card = is_call_card || has_link_card || is_photo_card || is_file_card;

    let show_timestamp = !msg.time.is_empty()
        && (!next_same_sender
            || is_latest_outgoing
            || has_interactive_card
            || !msg.reactions.is_empty()
            || msg.is_pinned
            || msg.is_starred);

    let has_photo_caption = is_photo_card
        && (msg
            .link_desc
            .as_deref()
            .map(|c| !c.trim().is_empty())
            .unwrap_or(false)
            || (!msg.text.trim().starts_with("[Hình ảnh]") && !msg.text.trim().is_empty()));
    let is_frameless_photo = is_photo_card
        && !has_photo_caption
        && msg.reply_to_text.is_none()
        && (msg.is_outgoing || !is_group);

    let sender_is_name = !msg.sender.is_empty()
        && msg.sender != "0"
        && !msg.sender.chars().all(|c| c.is_ascii_digit());
    let show_group_sender = !msg.is_outgoing && !prev_same_sender && is_group && sender_is_name;

    // Pre-measure exact content width for both outgoing and incoming bubbles so every bubble,
    // photo, and call card hugs its content tightly without horizontal overflow or inner/outer frame mismatch.
    let content_width = if has_sticker {
        116.0_f32
    } else if is_call_card {
        max_text_width.min(256.0)
    } else if is_photo_card {
        let thumb_u = msg
            .link_thumb
            .as_deref()
            .or(msg.link_url.as_deref())
            .unwrap_or("");
        let tkey = format!("thumb_{}", crate::crypto::md5_hex(thumb_u));
        let photo_w = if let Some(tex) = sticker_textures.get(&tkey) {
            let [tw, th] = tex.size();
            let tw_f = (tw.max(1)) as f32;
            let th_f = (th.max(1)) as f32;
            let max_w = max_text_width.clamp(160.0, 270.0);
            let max_h = 260.0_f32;
            let scale = (max_w / tw_f).min(max_h / th_f).min(1.0);
            (tw_f * scale).clamp(135.0, max_w).ceil()
        } else {
            220.0_f32
        };
        if has_photo_caption {
            let cap_str = msg
                .link_desc
                .as_deref()
                .filter(|c| !c.trim().is_empty())
                .unwrap_or_else(|| msg.text.trim());
            let cap_w = super::emoji_text::measure(ui, cap_str, 14.2, max_text_width).x;
            photo_w.max(cap_w + 4.0).min(max_text_width).ceil()
        } else {
            photo_w
        }
    } else if is_file_card || has_link_card {
        let has_loaded_thumb = msg
            .link_thumb
            .as_deref()
            .filter(|u| !u.is_empty())
            .map(|u| sticker_textures.contains_key(&format!("thumb_{}", crate::crypto::md5_hex(u))))
            .unwrap_or(false);
        if has_loaded_thumb {
            max_text_width.min(260.0)
        } else {
            let (cap, url, dom) = extract_message_link_view(msg).unwrap_or_default();
            let title = msg
                .link_title
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| {
                    if !url.is_empty() {
                        &url
                    } else {
                        msg.text.trim()
                    }
                });
            let desc = msg.link_desc.as_deref().unwrap_or("");
            let measure = |s: &str, sz: f32| -> f32 {
                if s.is_empty() {
                    0.0
                } else {
                    ui.fonts(|f| {
                        f.layout(
                            truncate_chars(s, 110).into_owned(),
                            FontId::proportional(sz),
                            Color32::WHITE,
                            max_text_width.min(340.0),
                        )
                        .size()
                        .x
                    })
                }
            };
            let w = measure(title, 13.8)
                .max(measure(&cap, 13.8))
                .max(measure(desc, 12.3))
                .max(measure(&dom, 12.6))
                + 8.0;
            card_content_width(w, max_text_width)
        }
    } else {
        let display_str = if msg.text.trim() == "[Tin nhắn đã thu hồi]" {
            "  Tin nhắn đã được thu hồi"
        } else {
            msg.text.as_str()
        };
        let text_w = super::emoji_text::measure(ui, display_str, 14.0, max_text_width).x;
        let quote_w = match msg.reply_to_text.as_ref() {
            Some(qt) => {
                let quote = super::emoji_text::truncate_graphemes(qt, 84);
                let q_body_w =
                    super::emoji_text::measure(ui, &quote, 12.0, (max_text_width - 28.0).max(80.0))
                        .x;
                let who = msg
                    .reply_to_sender
                    .as_deref()
                    .filter(|sn| !sn.is_empty() && !sn.chars().all(|c| c.is_ascii_digit()))
                    .unwrap_or("Tin nhắn trích dẫn");
                let q_who_w = ui.fonts(|f| {
                    f.layout_no_wrap(who.to_string(), FontId::proportional(11.5), Color32::WHITE)
                        .size()
                        .x
                });
                q_body_w.max(q_who_w) + 30.0
            }
            None => 0.0_f32,
        };
        let sender_w = if show_group_sender {
            ui.fonts(|f| {
                f.layout_no_wrap(
                    truncate_chars(&msg.sender, 22).into_owned(),
                    FontId::proportional(12.0),
                    Color32::WHITE,
                )
                .size()
                .x
            }) + 8.0
        } else {
            0.0_f32
        };
        let marks_w = if !msg.reactions.is_empty() || msg.is_pinned || msg.is_starred {
            78.0_f32
        } else {
            0.0_f32
        };
        let ts_min_w = if show_timestamp && is_latest_outgoing && msg.is_outgoing {
            96.0_f32
        } else {
            44.0_f32
        };
        text_w
            .max(quote_w)
            .max(sender_w)
            .max(marks_w)
            .max(ts_min_w)
            .min(max_text_width)
            .ceil()
            + 4.0
    };

    let h_margin = if has_sticker || is_frameless_photo {
        2.0_f32
    } else {
        12.0_f32
    };
    let v_margin = if has_sticker || is_frameless_photo {
        2.0_f32
    } else {
        8.5_f32
    };
    let total_bubble_w = content_width + h_margin * 2.0;

    if msg.is_outgoing {
        let out_resp = ui.vertical(|ui| {
            let row_resp = ui.horizontal(|ui| {
                let left_pad = (ui.available_width() - total_bubble_w - 6.0).max(0.0);
                if left_pad > 0.0 {
                    ui.add_space(left_pad);
                }

                let fill = if has_sticker || is_frameless_photo {
                    Color32::TRANSPARENT
                } else if let Some(card) = &call_card_opt {
                    if card.is_missed_or_failed {
                        if pal.is_dark {
                            Color32::from_rgb(38, 23, 30)
                        } else {
                            Color32::from_rgb(254, 242, 242)
                        }
                    } else {
                        pal.bg_elevated
                    }
                } else {
                    pal.bubble_out_bg
                };
                let stroke = if has_sticker || is_frameless_photo {
                    Stroke::NONE
                } else if let Some(card) = &call_card_opt {
                    let border_col = if card.is_missed_or_failed {
                        Color32::from_rgb(239, 68, 68).linear_multiply(0.55)
                    } else {
                        Color32::from_rgb(34, 197, 94).linear_multiply(0.55)
                    };
                    Stroke::new(1.2_f32, border_col)
                } else if !pal.is_dark {
                    Stroke::new(1.0_f32, Color32::from_rgb(186, 214, 255))
                } else {
                    Stroke::new(1.0_f32, Color32::from_rgb(53, 76, 104))
                };
                let rounding = Rounding::same(8.0);

                let mut card_action = None;
                let fr = egui::Frame::none()
                    .fill(fill)
                    .stroke(stroke)
                    .rounding(rounding)
                    .inner_margin(Margin::symmetric(h_margin, v_margin))
                    .show(ui, |ui| {
                        ui.set_min_width(content_width);
                        ui.set_max_width(content_width);
                        ui.with_layout(Layout::top_down(Align::Min), |ui| {
                            let quote_action =
                                render_quote_block(ui, msg, pal, true, content_width);
                            if let Some(card) = &call_card_opt {
                                card_action = render_call_card_widget(ui, card, pal, content_width);
                            } else if !render_sticker_widget(
                                ui,
                                msg,
                                sticker_textures,
                                loading_stickers,
                            ) {
                                card_action = render_message_body(
                                    ui,
                                    msg,
                                    thread_name,
                                    target,
                                    pal,
                                    true,
                                    content_width,
                                    sticker_textures,
                                    loading_stickers,
                                    failed_previews,
                                );
                            }
                            card_action = quote_action.or(card_action.take());
                            render_reactions(ui, msg, pal, emoji_textures);
                            if show_timestamp && !has_sticker && !is_frameless_photo {
                                ui.add_space(3.5);
                                let ts_txt = if is_latest_outgoing {
                                    match msg.delivery {
                                        crate::message_model::DeliveryState::Read => {
                                            format!("{} · Đã xem", msg.time)
                                        }
                                        crate::message_model::DeliveryState::Delivered => {
                                            format!("{} · Đã nhận", msg.time)
                                        }
                                        crate::message_model::DeliveryState::ServerAccepted => {
                                            format!("{} · Đã gửi", msg.time)
                                        }
                                        crate::message_model::DeliveryState::Pending => {
                                            format!("{} · Đang gửi", msg.time)
                                        }
                                        crate::message_model::DeliveryState::Failed => {
                                            format!("{} · Gửi lỗi", msg.time)
                                        }
                                        crate::message_model::DeliveryState::DeliveryUnknown => {
                                            format!("{} · Chưa xác nhận", msg.time)
                                        }
                                        _ => msg.time.clone(),
                                    }
                                } else {
                                    msg.time.clone()
                                };
                                ui.label(
                                    RichText::new(ts_txt).size(11.0).color(pal.bubble_out_meta),
                                );
                            }
                        });
                    });
                if fr.response.double_clicked() {
                    return Some(BubbleAction::Reply);
                }
                card_action.or_else(|| {
                    bubble_context_menu(
                        ui,
                        &fr.response,
                        msg,
                        idx,
                        pal,
                        emoji_textures,
                        has_interactive_card,
                    )
                })
            });

            row_resp.inner
        });
        out_resp.inner
    } else {
        ui.horizontal_top(|ui| {
            let display_sender = if sender_is_name {
                &msg.sender
            } else {
                thread_name
            };
            let sender_accent = avatar_color_for(display_sender, false);

            // Sender Avatar (only on first message of a consecutive group)
            if !prev_same_sender {
                let (av_rect, _) = ui.allocate_exact_size(Vec2::new(30.0, 30.0), Sense::hover());
                paint_user_avatar(
                    ui.painter(),
                    av_rect.center(),
                    15.0,
                    display_sender,
                    false,
                    sender_avatar,
                    sticker_textures,
                    loading_stickers,
                );
            } else {
                ui.add_space(30.0);
            }

            let fill = if has_sticker || is_frameless_photo {
                Color32::TRANSPARENT
            } else if let Some(card) = &call_card_opt {
                if card.is_missed_or_failed {
                    if pal.is_dark {
                        Color32::from_rgb(38, 23, 30)
                    } else {
                        Color32::from_rgb(254, 242, 242)
                    }
                } else {
                    pal.bg_elevated
                }
            } else {
                pal.bubble_in_bg
            };
            let stroke = if has_sticker || is_frameless_photo {
                Stroke::NONE
            } else if let Some(card) = &call_card_opt {
                let border_col = if card.is_missed_or_failed {
                    Color32::from_rgb(239, 68, 68).linear_multiply(0.55)
                } else {
                    Color32::from_rgb(34, 197, 94).linear_multiply(0.55)
                };
                Stroke::new(1.2_f32, border_col)
            } else {
                Stroke::new(1.0_f32, pal.bubble_in_border)
            };
            let rounding = Rounding::same(8.0);

            ui.vertical(|ui| {
                ui.set_max_width(total_bubble_w);
                let mut card_action = None;
                let fr = egui::Frame::none()
                    .fill(fill)
                    .stroke(stroke)
                    .rounding(rounding)
                    .inner_margin(Margin::symmetric(h_margin, v_margin))
                    .show(ui, |ui| {
                        ui.set_min_width(content_width);
                        ui.set_max_width(content_width);
                        ui.with_layout(Layout::top_down(Align::Min), |ui| {
                            if show_group_sender {
                                ui.label(
                                    RichText::new(truncate_chars(&msg.sender, 22))
                                        .color(sender_accent)
                                        .size(12.0)
                                        .strong(),
                                );
                                ui.add_space(2.0);
                            }
                            let quote_action =
                                render_quote_block(ui, msg, pal, false, content_width);
                            if let Some(card) = &call_card_opt {
                                card_action = render_call_card_widget(ui, card, pal, content_width);
                            } else if !render_sticker_widget(
                                ui,
                                msg,
                                sticker_textures,
                                loading_stickers,
                            ) {
                                card_action = render_message_body(
                                    ui,
                                    msg,
                                    thread_name,
                                    target,
                                    pal,
                                    false,
                                    content_width,
                                    sticker_textures,
                                    loading_stickers,
                                    failed_previews,
                                );
                            }
                            card_action = quote_action.or(card_action.take());
                            render_reactions(ui, msg, pal, emoji_textures);
                            if show_timestamp && !has_sticker && !is_frameless_photo {
                                ui.add_space(3.5);
                                ui.label(
                                    RichText::new(&msg.time)
                                        .color(pal.bubble_in_meta)
                                        .size(11.0),
                                );
                            }
                        });
                    });
                if fr.response.double_clicked() {
                    return Some(BubbleAction::Reply);
                }
                card_action.or_else(|| {
                    bubble_context_menu(
                        ui,
                        &fr.response,
                        msg,
                        idx,
                        pal,
                        emoji_textures,
                        has_interactive_card,
                    )
                })
            })
            .inner
        })
        .inner
    }
}

#[derive(Clone, PartialEq)]
pub(super) enum BubbleAction {
    Reply,
    JumpToReply {
        reference: String,
    },
    OpenMention {
        user_id: String,
        display_name: String,
    },
    Forward,
    React(&'static str),
    TogglePin,
    ToggleStar,
    DeleteForMe,
    RecallForEveryone,
    CallAgain {
        is_video: bool,
    },
    OpenNativeMedia {
        url: String,
        mime: String,
    },
}

/// Right-click menu on a chat bubble: reactions, reply, forward, copy, delete-for-me.
pub(super) fn bubble_context_menu(
    ui: &mut egui::Ui,
    bubble: &egui::Response,
    msg: &ChatMessage,
    idx: usize,
    _pal: &ThemePalette,
    emoji_textures: &HashMap<String, TextureHandle>,
    _has_interactive_card: bool,
) -> Option<BubbleAction> {
    let secondary_or_popup = ui.input(|i| {
        i.pointer.button_down(egui::PointerButton::Secondary)
            || i.pointer.button_clicked(egui::PointerButton::Secondary)
            || i.pointer.button_released(egui::PointerButton::Secondary)
    }) || ui.memory(|m| m.any_popup_open());
    if !secondary_or_popup {
        return None;
    }
    let resp = ui.interact(
        bubble.rect,
        Id::new(("bubble_ctx", idx, msg.ts, &msg.id)),
        Sense::click(),
    );
    let mut action = None;

    resp.context_menu(|ui| {
        ui.set_min_width(210.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(4.0, 0.0);
            for &(code, key, tooltip) in &REACTION_LIST {
                if let Some(tex) = emoji_textures.get(key) {
                    let img = egui::Image::new(tex).fit_to_exact_size(Vec2::splat(24.0));
                    let btn = egui::ImageButton::new(img).rounding(Rounding::same(6.0));
                    if ui.add(btn).on_hover_text(tooltip).clicked() {
                        action = Some(BubbleAction::React(code));
                        ui.close_menu();
                    }
                } else if ui.button(RichText::new(code).size(18.0)).clicked() {
                    action = Some(BubbleAction::React(code));
                    ui.close_menu();
                }
            }
        });
        ui.separator();
        if (msg.link_src.as_deref() == Some("Hình ảnh Zalo")
            || msg.text.trim() == "[Hình ảnh]"
            || msg.text.trim().starts_with("[Hình ảnh] "))
            && msg
                .link_thumb
                .as_deref()
                .or(msg.link_url.as_deref())
                .map(|u| !u.is_empty())
                .unwrap_or(false)
        {
            let thumb_url = msg
                .link_thumb
                .as_deref()
                .or(msg.link_url.as_deref())
                .unwrap_or_default();
            if ui
                .button(format!("{} Xem ảnh phóng to", lucide::SEARCH))
                .clicked()
            {
                let url = msg.link_url.as_deref().unwrap_or(thumb_url);
                action = Some(BubbleAction::OpenNativeMedia {
                    url: url.to_owned(),
                    mime: native_media_mime(url, false).to_owned(),
                });
                ui.close_menu();
            }
        }
        if ui.button("Trả lời").clicked() {
            action = Some(BubbleAction::Reply);
            ui.close_menu();
        }
        if ui.button("Chuyển tiếp").clicked() {
            action = Some(BubbleAction::Forward);
            ui.close_menu();
        }
        if ui.button("Copy tin nhắn").clicked() {
            ui.ctx().copy_text(msg.text.clone());
            ui.close_menu();
        }
        if ui
            .button(if msg.is_pinned {
                "Bỏ ghim tin nhắn"
            } else {
                "Ghim tin nhắn"
            })
            .clicked()
        {
            action = Some(BubbleAction::TogglePin);
            ui.close_menu();
        }
        if ui
            .button(if msg.is_starred {
                "Bỏ đánh dấu tin nhắn"
            } else {
                "Đánh dấu tin nhắn"
            })
            .clicked()
        {
            action = Some(BubbleAction::ToggleStar);
            ui.close_menu();
        }
        ui.separator();
        if msg.is_outgoing
            && msg.text.trim() != "[Tin nhắn đã thu hồi]"
            && ui
                .button(
                    RichText::new("Thu hồi với mọi người").color(Color32::from_rgb(249, 115, 22)),
                )
                .clicked()
        {
            action = Some(BubbleAction::RecallForEveryone);
            ui.close_menu();
        }
        if ui
            .button(RichText::new("Xóa chỉ ở phía tôi").color(Color32::from_rgb(220, 60, 60)))
            .clicked()
        {
            action = Some(BubbleAction::DeleteForMe);
            ui.close_menu();
        }
    });
    action
}

/// Quoted-message block shown at the top of a reply bubble.
pub(super) fn render_quote_block(
    ui: &mut egui::Ui,
    msg: &ChatMessage,
    pal: &ThemePalette,
    outgoing: bool,
    max_w: f32,
) -> Option<BubbleAction> {
    let qt = msg.reply_to_text.as_ref()?;
    let (text_col, meta_col) = if outgoing {
        (pal.bubble_out_text, pal.bubble_out_meta)
    } else {
        (pal.bubble_in_text, pal.bubble_in_meta)
    };
    let bar_col = if outgoing && pal.is_dark {
        Color32::from_rgb(147, 197, 253)
    } else {
        pal.primary
    };
    let bg_col = if outgoing && pal.is_dark {
        Color32::from_rgba_unmultiplied(0, 0, 0, 42)
    } else if outgoing {
        Color32::from_rgba_unmultiplied(255, 255, 255, 150)
    } else {
        text_col.linear_multiply(0.06)
    };
    let inner_col_w = (max_w - 28.0).max(72.0);
    let quote_frame = egui::Frame::none()
        .fill(bg_col)
        .rounding(Rounding::same(7.0))
        .inner_margin(Margin::symmetric(8.0, 5.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (bar_rect, _) = ui.allocate_exact_size(Vec2::new(3.0, 28.0), Sense::hover());
                ui.painter()
                    .rect_filled(bar_rect, Rounding::same(1.5), bar_col);
                ui.vertical(|ui| {
                    ui.set_width(inner_col_w);
                    let who = msg
                        .reply_to_sender
                        .as_deref()
                        .filter(|sn| !sn.is_empty() && !sn.chars().all(|c| c.is_ascii_digit()))
                        .unwrap_or("Tin nhắn trích dẫn");
                    ui.label(RichText::new(who).color(bar_col).size(11.5).strong());
                    let quote = super::emoji_text::truncate_graphemes(qt, 84);
                    super::emoji_text::show(ui, &quote, 12.0, meta_col, inner_col_w);
                });
            });
        });
    let action = crate::message_navigation::message_reply_reference(msg).and_then(|reference| {
        let response = ui
            .interact(
                quote_frame.response.rect,
                quote_frame.response.id.with("jump_to_reply"),
                Sense::click(),
            )
            .on_hover_cursor(CursorIcon::PointingHand)
            .on_hover_text("Đi tới tin nhắn gốc");
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Button,
                ui.is_enabled(),
                format!(
                    "Đi tới tin nhắn gốc: {}",
                    super::emoji_text::truncate_graphemes(qt, 60)
                ),
            )
        });
        response.clicked().then(|| BubbleAction::JumpToReply {
            reference: reference.clone(),
        })
    });
    ui.add_space(5.0);
    action
}

pub(super) fn render_reactions(
    ui: &mut egui::Ui,
    msg: &ChatMessage,
    pal: &ThemePalette,
    _emoji_textures: &HashMap<String, TextureHandle>,
) {
    let has_pin = msg.is_pinned;
    let has_star = msg.is_starred;
    let summaries = if msg.reaction_summaries.is_empty() {
        crate::reactions::parse_reactions(&serde_json::json!({"reactions": msg.reactions}))
    } else {
        msg.reaction_summaries.clone()
    };
    let has_react = summaries.iter().any(|s| s.count > 0);

    if !has_pin && !has_star && !has_react {
        return;
    }

    ui.add_space(3.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(4.0, 2.0);

        if has_pin {
            egui::Frame::none()
                .fill(pal.primary.linear_multiply(0.18))
                .stroke(Stroke::new(1.0_f32, pal.primary))
                .rounding(Rounding::same(8.0))
                .inner_margin(Margin::symmetric(5.0, 2.0))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(format!("{} Đã ghim", lucide::PIN))
                            .size(11.0)
                            .color(pal.primary)
                            .strong(),
                    );
                });
        }

        if has_star {
            egui::Frame::none()
                .fill(Color32::from_rgb(234, 179, 8).linear_multiply(0.2))
                .stroke(Stroke::new(1.0_f32, Color32::from_rgb(234, 179, 8)))
                .rounding(Rounding::same(8.0))
                .inner_margin(Margin::symmetric(5.0, 2.0))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(format!("{} Đã lưu", lucide::STAR))
                            .size(11.0)
                            .color(Color32::from_rgb(234, 179, 8))
                            .strong(),
                    );
                });
        }

        if has_react {
            egui::Frame::none()
                .fill(pal.bg_elevated)
                .stroke(Stroke::new(1.0_f32, pal.border))
                .rounding(Rounding::same(12.0))
                .inner_margin(Margin::symmetric(6.0, 2.0))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(3.0, 0.0);
                        for summary in summaries.iter().filter(|r| r.count > 0) {
                            emoji_text::show(ui, &summary.emoji, 12.0, pal.text_primary, 22.0);
                            let count = if summary.count_is_lower_bound {
                                "9k+".to_owned()
                            } else {
                                summary.count.to_string()
                            };
                            ui.label(RichText::new(count).size(12.0).color(pal.text_primary));
                        }
                    });
                });
        }
    });
}

#[cfg(test)]
mod narrow_card_render_tests {
    use super::*;

    #[test]
    fn headless_render_link_and_file_cards_at_narrow_chat_widths() {
        let ctx = egui::Context::default();
        let palette = ThemePalette::light();
        let textures = HashMap::new();
        let mut loading = HashSet::new();
        for width in [140.0_f32, 180.0, 218.7892, 220.0, 240.0, 320.0, 640.0] {
            for outgoing in [false, true] {
                for file_card in [false, true] {
                    let message = ChatMessage {
                        id: "narrow-card-fixture".into(),
                        sender: "Fixture sender".into(),
                        text: if file_card {
                            "[Tập tin] fixture.pdf"
                        } else {
                            "Fixture link caption"
                        }
                        .into(),
                        is_outgoing: outgoing,
                        link_url: Some("https://example.invalid/fixture".into()),
                        link_title: Some(
                            "Fixture title with enough text to exercise wrapping".into(),
                        ),
                        link_desc: Some("Fixture description".into()),
                        link_src: file_card.then(|| "Tập tin Zalo".into()),
                        ..Default::default()
                    };
                    let output = ctx.run(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                Pos2::ZERO,
                                Vec2::new(900.0, 800.0),
                            )),
                            ..Default::default()
                        },
                        |ctx| {
                            egui::CentralPanel::default().show(ctx, |ui| {
                                ui.allocate_ui_with_layout(
                                    Vec2::new(width, 600.0),
                                    Layout::top_down(Align::Min),
                                    |ui| {
                                        ui.set_max_width(width);
                                        assert!((ui.available_width() - width).abs() < 0.01);
                                        let _ = render_chat_bubble(
                                            ui,
                                            &message,
                                            false,
                                            false,
                                            outgoing,
                                            false,
                                            "Fixture thread",
                                            ("123", false),
                                            None,
                                            &palette,
                                            &textures,
                                            &mut loading,
                                            &HashSet::new(),
                                            &textures,
                                            0,
                                        );
                                    },
                                );
                            });
                        },
                    );
                    assert!(!output.shapes.is_empty());
                }
            }
        }
    }
}

#[cfg(test)]
mod narrow_card_width_tests {
    use super::card_content_width;

    #[test]
    fn fractional_upper_bound_never_falls_below_clamp_minimum() {
        for available in [0.0_f32, 90.0, 116.0, 133.5282, 135.0, 136.2, 340.0, 520.0] {
            for measured in [0.0_f32, 80.0, 135.0, 220.0, 1000.0] {
                let width = card_content_width(measured, available);
                assert!(width.is_finite());
                assert!(width >= 0.0 && width <= available.min(340.0));
            }
        }
    }
}

#[cfg(test)]
mod visual_media_tests {
    use super::*;
    #[test]
    fn native_media_mime_ignores_query_and_does_not_guess_extensionless_image() {
        assert_eq!(
            native_media_mime("https://fixture.invalid/a.PNG?token=synthetic", false),
            "image/png"
        );
        assert_eq!(
            native_media_mime("https://fixture.invalid/opaque", false),
            "image/*"
        );
        assert_eq!(
            native_media_mime("https://fixture.invalid/a.mov#part", true),
            "video/quicktime"
        );
    }
    #[test]
    fn failed_poster_never_blocks_video_activation() {
        assert_eq!(media_activation(true, true), MediaActivation::OpenVideo);
        assert_eq!(media_activation(true, false), MediaActivation::OpenVideo);
        assert_eq!(media_activation(false, true), MediaActivation::RetryPreview);
    }
    #[test]
    fn photo_and_video_use_loaded_texture_inside_narrow_pane() {
        let ctx = egui::Context::default();
        let mut textures = HashMap::new();
        let key = format!(
            "thumb_{}",
            crate::crypto::md5_hex("https://example.invalid/poster.jxl")
        );
        let texture = ctx.load_texture(
            "fixture",
            egui::ColorImage::new([40, 80], Color32::RED),
            TextureOptions::LINEAR,
        );
        let id = texture.id();
        textures.insert(key, texture);
        for video in [false, true] {
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(300.0, 500.0))),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let msg = ChatMessage {
                            link_url: Some("https://example.invalid/original".into()),
                            link_thumb: Some("https://example.invalid/poster.jxl".into()),
                            ..Default::default()
                        };
                        render_inline_media(
                            ui,
                            &msg,
                            90.0,
                            Color32::WHITE,
                            &textures,
                            &mut HashSet::new(),
                            &HashSet::new(),
                            video,
                            "Fixture".into(),
                            ("123", false),
                        );
                    });
                },
            );
            assert!(output.shapes.iter().any(
                |shape| matches!(&shape.shape,egui::Shape::Mesh(mesh) if mesh.texture_id==id)
            ));
        }
    }
}

#[cfg(test)]
mod emoji_message_tests {
    use super::*;

    #[test]
    fn emoji_message_uses_color_image_mesh() {
        let ctx = egui::Context::default();
        setup_custom_fonts(&ctx);
        let msg = ChatMessage {
            text: "Xin chào :) 😀 ❤️".into(),
            ..Default::default()
        };
        let original = msg.text.clone();
        super::super::emoji_text::seed_synthetic_images(&ctx, &msg.text);
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                render_message_body(
                    ui,
                    &msg,
                    "Fixture",
                    ("fixture", false),
                    &ThemePalette::dark(),
                    false,
                    320.0,
                    &HashMap::new(),
                    &mut HashSet::new(),
                    &HashSet::new(),
                );
            });
        });
        assert!(
            output.shapes.iter().any(|shape| matches!(&shape.shape,
            egui::Shape::Mesh(mesh) if mesh.texture_id != egui::TextureId::default())),
            "Message emoji must use a color image rather than outline font glyphs"
        );
        assert_eq!(
            msg.text, original,
            "Rendering must preserve raw message content"
        );
    }
}

#[cfg(test)]
mod message_navigation_tests {
    use super::*;

    #[test]
    fn mention_click_returns_exact_profile_action_for_duplicate_alias() {
        let ctx = egui::Context::default();
        setup_custom_fonts(&ctx);
        let msg = ChatMessage {
            text: "@An @An".into(),
            wire_ref: crate::message_model::WireMessageRef::from_raw(&serde_json::json!({
                "mentions": [{"uid":"11","pos":0,"len":3},{"uid":"22","pos":4,"len":3}]
            })),
            ..Default::default()
        };
        let mut target = Pos2::ZERO;
        let mut clicked = None;
        for frame in 0..3 {
            let events = if frame == 0 {
                vec![]
            } else {
                vec![
                    egui::Event::PointerMoved(target),
                    egui::Event::PointerButton {
                        pos: target,
                        button: egui::PointerButton::Primary,
                        pressed: frame == 1,
                        modifiers: Default::default(),
                    },
                ]
            };
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 300.0))),
                    events,
                    time: Some(frame as f64 * 0.016),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let prefix = super::super::emoji_text::measure(ui, "@An ", 14.0, 300.0).x;
                        let name_width =
                            super::super::emoji_text::measure(ui, "@An", 14.0, 300.0).x;
                        target =
                            ui.next_widget_position() + Vec2::new(prefix + name_width * 0.5, 8.0);
                        clicked = render_message_body(
                            ui,
                            &msg,
                            "Fixture",
                            ("fixture", false),
                            &ThemePalette::dark(),
                            false,
                            300.0,
                            &HashMap::new(),
                            &mut HashSet::new(),
                            &HashSet::new(),
                        )
                        .or(clicked.take());
                    });
                },
            );
        }
        assert!(
            matches!(clicked, Some(BubbleAction::OpenMention { user_id, display_name })
            if user_id == "22" && display_name == "An")
        );
    }

    #[test]
    fn quote_click_returns_exact_reference_without_touching_message() {
        let ctx = egui::Context::default();
        setup_custom_fonts(&ctx);
        let msg = ChatMessage {
            reply_to_id: Some("00012345678901234567890".into()),
            reply_to_text: Some("Tin nhắn gốc 😀".into()),
            reply_to_sender: Some("Người gửi".into()),
            ..Default::default()
        };
        let mut target = Pos2::ZERO;
        let mut clicked = None;
        for frame in 0..3 {
            let events = if frame == 0 {
                vec![]
            } else {
                vec![
                    egui::Event::PointerMoved(target),
                    egui::Event::PointerButton {
                        pos: target,
                        button: egui::PointerButton::Primary,
                        pressed: frame == 1,
                        modifiers: Default::default(),
                    },
                ]
            };
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 300.0))),
                    events,
                    time: Some(frame as f64 * 0.016),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        target = ui.next_widget_position() + Vec2::new(35.0, 15.0);
                        clicked = render_quote_block(ui, &msg, &ThemePalette::dark(), false, 250.0)
                            .or(clicked.take());
                    });
                },
            );
        }
        assert!(
            matches!(clicked, Some(BubbleAction::JumpToReply { reference }) if reference == "00012345678901234567890")
        );
        assert_eq!(msg.reply_to_text.as_deref(), Some("Tin nhắn gốc 😀"));
    }
}
