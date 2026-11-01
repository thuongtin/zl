//! Global request/response logger and dev inspection scanner.
//!
//! In Dev mode (`--dev`), logs raw decrypted payloads, endpoint metrics, and provides
//! a diagnostic scanner. In Production mode, logs and dev panels are hidden and sanitized.

use std::sync::{Arc, LazyLock, Mutex};

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub id: usize,
    pub timestamp: String,
    pub method: String,
    pub url: String,
    pub status: Option<u16>,
    pub latency_ms: u128,
    pub payload_preview: String,
    pub decrypted_preview: Option<String>,
    pub is_error: bool,
    pub error_msg: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ScanFinding {
    pub level: FindingLevel,
    pub title: String,
    pub detail: String,
    pub suggestion: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindingLevel {
    Info,
    Warning,
    Critical,
}

pub struct LoggerState {
    pub dev_mode: bool,
    pub entries: Vec<LogEntry>,
    pub max_entries: usize,
    counter: usize,
}

impl Default for LoggerState {
    fn default() -> Self {
        Self {
            dev_mode: false,
            entries: Vec::new(),
            max_entries: 500,
            counter: 0,
        }
    }
}

pub static LOGGER: LazyLock<Arc<Mutex<LoggerState>>> =
    LazyLock::new(|| Arc::new(Mutex::new(LoggerState::default())));

/// Initialize logging configuration.
pub fn init(dev_mode: bool) {
    if let Ok(mut state) = LOGGER.lock() {
        state.dev_mode = dev_mode;
    }
}

pub fn is_dev_mode() -> bool {
    LOGGER.lock().map(|s| s.dev_mode).unwrap_or(false)
}

pub fn log_request(
    method: &str,
    url: &str,
    status: Option<u16>,
    latency_ms: u128,
    payload_preview: &str,
    decrypted_preview: Option<&str>,
    error_msg: Option<&str>,
) {
    if !is_dev_mode() {
        return;
    }
    let now = crate::login::unix_now();
    let time_str = format!(
        "{:02}:{:02}:{:02}",
        (now / 3600) % 24,
        (now / 60) % 60,
        now % 60
    );

    let is_error = status.map(|s| s >= 400).unwrap_or(false) || error_msg.is_some();

    if let Ok(mut state) = LOGGER.lock() {
        state.counter += 1;
        let entry = LogEntry {
            id: state.counter,
            timestamp: time_str,
            method: method.to_string(),
            url: url.to_string(),
            status,
            latency_ms,
            payload_preview: payload_preview.to_string(),
            decrypted_preview: decrypted_preview.map(str::to_string),
            is_error,
            error_msg: error_msg.map(str::to_string),
        };

        state.entries.push(entry);
        if state.entries.len() > state.max_entries {
            state.entries.remove(0);
        }
    }
}

pub fn log_error(context: &str, err: &str) {
    if !is_dev_mode() {
        return;
    }
    let now = crate::login::unix_now();
    let time_str = format!(
        "{:02}:{:02}:{:02}",
        (now / 3600) % 24,
        (now / 60) % 60,
        now % 60
    );

    if let Ok(mut state) = LOGGER.lock() {
        state.counter += 1;
        let entry = LogEntry {
            id: state.counter,
            timestamp: time_str,
            method: "ERR".to_string(),
            url: context.to_string(),
            status: None,
            latency_ms: 0,
            payload_preview: String::new(),
            decrypted_preview: None,
            is_error: true,
            error_msg: Some(err.to_string()),
        };

        state.entries.push(entry);
        if state.entries.len() > state.max_entries {
            state.entries.remove(0);
        }
    }
}

pub fn get_entries() -> Vec<LogEntry> {
    LOGGER.lock().map(|s| s.entries.clone()).unwrap_or_default()
}

pub fn clear_entries() {
    if let Ok(mut state) = LOGGER.lock() {
        state.entries.clear();
    }
}

/// Diagnostic scanner: runs automated heuristics across captured logs and identifies
/// authentication, cryptographic, or network protocol anomalies.
pub fn scan_logs() -> Vec<ScanFinding> {
    let entries = get_entries();
    let mut findings = Vec::new();

    if entries.is_empty() {
        findings.push(ScanFinding {
            level: FindingLevel::Info,
            title: "Chưa có lưu lượng mạng".into(),
            detail: "Hệ thống chưa ghi nhận request/response nào để quét.".into(),
            suggestion: "Thực hiện thao tác đăng nhập hoặc tải lại danh sách bạn bè.".into(),
        });
        return findings;
    }

    let mut err_count = 0;
    let mut has_login = false;
    let mut has_friend_list = false;
    let mut has_crypto_err = false;

    for e in &entries {
        if e.is_error {
            err_count += 1;
        }
        if e.url.contains("login") || e.url.contains("authen") {
            has_login = true;
        }
        if e.url.contains("getfriends") || e.url.contains("preloadconvers") {
            has_friend_list = true;
        }

        if let Some(err) = &e.error_msg {
            if err.contains("18022") || err.contains("Signature") {
                has_crypto_err = true;
                findings.push(ScanFinding {
                    level: FindingLevel::Critical,
                    title: "Lỗi chữ ký (18022 Invalid Signature)".into(),
                    detail: format!("Request tới {} bị từ chối chữ ký.", e.url),
                    suggestion: "Kiểm tra tập trường ký trong signkey và thứ tự tham số.".into(),
                });
            } else if err.contains("18060") || err.contains("encryption protocol") {
                has_crypto_err = true;
                findings.push(ScanFinding {
                    level: FindingLevel::Critical,
                    title: "Lỗi giao thức mã hóa (18060)".into(),
                    detail: format!("Server từ chối tham số chưa mã hoá tại {}.", e.url),
                    suggestion: "Bắt buộc sử dụng ParamCipher v2 cho endpoint này.".into(),
                });
            } else if err.contains("600") || err.contains("zpw_sek") {
                findings.push(ScanFinding {
                    level: FindingLevel::Warning,
                    title: "Thiếu cookie phiên (600 zpw_sek not found)".into(),
                    detail: format!("Endpoint {} yêu cầu cookie phiên xác thực.", e.url),
                    suggestion:
                        "Đảm bảo quét QR hoặc đăng nhập mật khẩu đã hoàn tất trước khi gọi.".into(),
                });
            }
        }
    }

    if err_count == 0 && !has_crypto_err {
        findings.push(ScanFinding {
            level: FindingLevel::Info,
            title: "Trạng thái giao thức hoạt động tốt".into(),
            detail: format!(
                "Đã quét {} requests, toàn bộ phản hồi mã thành công (200 OK / error_code 0).",
                entries.len()
            ),
            suggestion: "Không phát hiện lỗi mã hoá hay rò rỉ giao thức.".into(),
        });
    }

    if has_login && !has_friend_list {
        findings.push(ScanFinding {
            level: FindingLevel::Warning,
            title: "Đã đăng nhập nhưng chưa đồng bộ tin nhắn".into(),
            detail: "Đã có gói xác thực nhưng chưa thấy request getfriends/preloadconvers.".into(),
            suggestion: "Bấm nút làm mới (Refresh) để tải hội thoại.".into(),
        });
    }

    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_error_records_entry() {
        init(true);
        clear_entries();
        log_error("test_context", "test_error_message");
        let entries = get_entries();
        assert!(!entries.is_empty());
        let last = entries.last().unwrap();
        assert_eq!(last.url, "test_context");
        assert_eq!(last.error_msg.as_deref(), Some("test_error_message"));
        assert!(last.is_error);
    }
}
