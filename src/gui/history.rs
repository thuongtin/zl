//! Bounded history pagination over the shared command-510 WebSocket stream.
//!
//! Wire contract: zca-js listen.ts at 2a7e360257dfe98e2eb48dc46bbf389834be7e0b.
//! This is a global direct-message stream, not a per-contact history endpoint.

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

const BATCH_PAGES: usize = 20;
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(25);

fn identifier(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(s) if !s.is_empty() && s != "0" => Some(s.clone()),
        Value::Number(n) if n.as_u64().is_some_and(|v| v > 0) => Some(n.to_string()),
        _ => None,
    }
}

#[derive(Debug)]
pub(super) struct DirectHistoryPage {
    pub messages: Vec<Value>,
    next_cursor: Option<String>,
}

impl DirectHistoryPage {
    pub fn parse(payload: &Value) -> Result<Self> {
        fn check_error(value: &Value) -> Result<()> {
            if let Some(code) = value.get("error_code") {
                let code = code
                    .as_i64()
                    .or_else(|| code.as_str().and_then(|s| s.parse().ok()))
                    .context("Mã lỗi lịch sử 1-1 sai định dạng")?;
                if code != 0 {
                    bail!("Máy chủ trả lỗi khi tải lịch sử 1-1");
                }
            }
            Ok(())
        }
        check_error(payload)?;
        let raw = payload.get("data").unwrap_or(payload);
        let decoded;
        let data = if let Some(text) = raw.as_str() {
            decoded = serde_json::from_str::<Value>(text)
                .context("Phản hồi lịch sử 1-1 chưa giải mã được")?;
            &decoded
        } else {
            raw
        };
        check_error(data)?;
        let data = data.get("data").unwrap_or(data);
        check_error(data)?;
        let messages = data
            .get("msgs")
            .and_then(Value::as_array)
            .context("Phản hồi lịch sử 1-1 thiếu danh sách msgs")?
            .clone();
        if messages.iter().any(|m| !m.is_object()) {
            bail!("Phản hồi lịch sử 1-1 chứa tin nhắn sai định dạng");
        }
        // The owning consumer uses the last wire-order message, not a timestamp sort.
        let next_cursor = messages
            .last()
            .and_then(|m| identifier(m.get("actionId")).or_else(|| identifier(m.get("msgId"))));
        Ok(Self {
            messages,
            next_cursor,
        })
    }

    pub fn normalized_payload(&self, own_uid: &str) -> Value {
        let messages: Vec<Value> = self
            .messages
            .iter()
            .cloned()
            .map(|mut msg| {
                // Both Zalo sentinel IDs and explicit self IDs occur across transports.
                // Reuse the existing preview router with its documented self sentinel.
                for key in [
                    "uidFrom", "fromUid", "fromId", "senderId", "idTo", "toid", "toId",
                ] {
                    if !own_uid.is_empty() && identifier(msg.get(key)).as_deref() == Some(own_uid) {
                        msg[key] = json!("0");
                    }
                }
                msg
            })
            .collect();
        json!({"error_code": 0, "data": {"msgs": messages}})
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct DirectHistoryRequest {
    pub last_id: Option<String>,
}

pub(super) struct DirectHistoryUpdate {
    pub next_request: Option<DirectHistoryRequest>,
    pub status: String,
}

#[derive(Default)]
pub(super) struct DirectHistorySync {
    cursor: Option<String>,
    in_flight: Option<Instant>,
    remaining: usize,
    pages: usize,
    messages: usize,
    exhausted: bool,
    status_text: String,
}

impl DirectHistorySync {
    pub fn request_batch(&mut self, now: Instant) -> Option<DirectHistoryRequest> {
        if self.in_flight.is_some() || self.exhausted {
            return None;
        }
        self.remaining = BATCH_PAGES;
        self.in_flight = Some(now);
        self.status_text = format!("Đang tải lịch sử 1-1 · {} trang đã nhận", self.pages);
        Some(DirectHistoryRequest {
            last_id: self.cursor.clone(),
        })
    }

    pub fn accept_page(&mut self, page: &DirectHistoryPage, now: Instant) -> DirectHistoryUpdate {
        // Late/duplicate frames may still be merged but must not advance an idle cursor.
        if self.in_flight.take().is_none() {
            return DirectHistoryUpdate {
                next_request: None,
                status: self.status(),
            };
        }
        self.pages += 1;
        self.messages += page.messages.len();
        self.remaining = self.remaining.saturating_sub(1);
        let mut next_request = None;
        if page.messages.is_empty() {
            self.exhausted = true;
            self.status_text = format!(
                "Đã tải lịch sử 1-1 hiện có trên máy chủ · {} tin qua {} trang",
                self.messages, self.pages
            );
        } else if page.next_cursor.is_none() || page.next_cursor == self.cursor {
            self.remaining = 0;
            self.status_text =
                "Lịch sử 1-1 tạm dừng: máy chủ thiếu hoặc lặp mốc trang; bấm tải thêm để thử lại"
                    .into();
        } else {
            self.cursor = page.next_cursor.clone();
            if self.remaining > 0 {
                self.in_flight = Some(now);
                next_request = Some(DirectHistoryRequest {
                    last_id: self.cursor.clone(),
                });
                self.status_text = format!(
                    "Đang tải lịch sử 1-1 · {} tin qua {} trang",
                    self.messages, self.pages
                );
            } else {
                self.status_text = format!(
                    "Đã nhận {} tin lịch sử 1-1 · bấm tải thêm để tiếp tục",
                    self.messages
                );
            }
        }
        DirectHistoryUpdate {
            next_request,
            status: self.status(),
        }
    }

    pub fn check_timeout(&mut self, now: Instant) -> Option<String> {
        if self
            .in_flight
            .is_some_and(|started| now.saturating_duration_since(started) >= RESPONSE_TIMEOUT)
        {
            self.fail();
            self.status_text = "Lịch sử 1-1 chưa có phản hồi; bấm tải thêm để thử lại".into();
            return Some(self.status());
        }
        None
    }

    pub fn fail(&mut self) {
        self.in_flight = None;
        self.remaining = 0;
        self.status_text = "Tải lịch sử 1-1 gặp lỗi; bấm tải thêm để thử lại".into();
    }

    pub fn status(&self) -> String {
        if self.status_text.is_empty() {
            "Lịch sử 1-1: chờ kết nối".into()
        } else {
            self.status_text.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::{merge_previews, Thread};

    fn page(id: &str) -> DirectHistoryPage {
        DirectHistoryPage::parse(&json!({"data":{"msgs":[{
            "actionId":id,"msgId":format!("m{id}"),"uidFrom":"peer-a","idTo":"0","content":"fixture","ts":1000
        }]}})).unwrap()
    }

    #[test]
    fn bounded_batch_continues_from_cursor_and_deduplicates_in_flight_requests() {
        let now = Instant::now();
        let mut sync = DirectHistorySync::default();
        assert_eq!(sync.request_batch(now).unwrap().last_id, None);
        assert!(sync.request_batch(now).is_none());
        for n in 1..=BATCH_PAGES {
            let update = sync.accept_page(&page(&n.to_string()), now);
            assert_eq!(update.next_request.is_some(), n < BATCH_PAGES);
        }
        assert!(!sync.exhausted);
        assert!(sync.status().contains("tiếp tục"));
        assert_eq!(
            sync.request_batch(now).unwrap().last_id.as_deref(),
            Some("20")
        );
    }

    #[test]
    fn empty_page_is_exhausted_but_timeout_and_cursor_stall_are_not() {
        let now = Instant::now();
        let mut sync = DirectHistorySync::default();
        sync.request_batch(now);
        sync.accept_page(&page("100"), now);
        assert!(sync.check_timeout(now + RESPONSE_TIMEOUT).is_some());
        assert!(!sync.exhausted);
        assert_eq!(
            sync.request_batch(now).unwrap().last_id.as_deref(),
            Some("100")
        );
        assert!(sync.accept_page(&page("100"), now).next_request.is_none());
        assert!(!sync.exhausted);
        sync.request_batch(now);
        let empty = DirectHistoryPage::parse(&json!({"data":{"msgs":[]}})).unwrap();
        assert!(sync.accept_page(&empty, now).next_request.is_none());
        assert!(sync.exhausted);
        assert!(sync.request_batch(now).is_none());
    }

    #[test]
    fn malformed_and_encrypted_errors_are_not_empty_history() {
        for value in [
            json!({"data":{}}),
            json!({"data":"encrypted-blob"}),
            json!({"error_code":600,"data":{"msgs":[]}}),
            json!({"data":{"error_code":600,"data":{"msgs":[]}}}),
            json!({"error_code":"600","data":{"msgs":[]}}),
            json!({"data":{"error_code":"600","msgs":[]}}),
            json!({"data":{"data":{"error_code":600,"msgs":[]}}}),
            json!({"data":{"error_code":"invalid","msgs":[]}}),
            json!({"data":{"msgs":[null]}}),
        ] {
            assert!(DirectHistoryPage::parse(&value).is_err());
        }
    }

    #[test]
    fn cursor_uses_wire_order_and_action_id_before_message_id() {
        let p = DirectHistoryPage::parse(&json!({"data":{"msgs":[
            {"msgId":"200","actionId":"900","ts":20},
            {"msgId":"100","actionId":800,"ts":10}
        ]}}))
        .unwrap();
        assert_eq!(p.next_cursor.as_deref(), Some("800"));
        let p =
            DirectHistoryPage::parse(&json!({"data":"{\"msgs\":[{\"msgId\":\"100\"}]}"})).unwrap();
        assert_eq!(p.next_cursor.as_deref(), Some("100"));
    }

    #[test]
    fn global_pages_route_each_peer_self_direction_and_replay_without_duplicates() {
        let p = DirectHistoryPage::parse(&json!({"data":{"msgs":[
            {"msgId":"1","actionId":"101","uidFrom":"peer-a","idTo":"self-user","content":"incoming a","ts":1000},
            {"msgId":"2","actionId":"102","uidFrom":"self-user","idTo":"peer-b","content":"outgoing b","ts":2000},
            {"msgId":"3","actionId":"103","uidFrom":"peer-b","idTo":"0","content":"incoming b","ts":3000}
        ]}})).unwrap();
        let mut threads: Vec<Thread> = vec![];
        let normalized = p.normalized_payload("self-user");
        merge_previews(&mut threads, &normalized);
        merge_previews(&mut threads, &normalized);
        assert_eq!(threads.len(), 2);
        let a = threads.iter().find(|t| t.id == "peer-a").unwrap();
        let b = threads.iter().find(|t| t.id == "peer-b").unwrap();
        assert_eq!(a.messages.len(), 1);
        assert!(!a.messages[0].is_outgoing);
        assert_eq!(b.messages.len(), 2);
        assert!(b.messages.iter().find(|m| m.id == "2").unwrap().is_outgoing);
        assert!(!b.messages.iter().find(|m| m.id == "3").unwrap().is_outgoing);
        assert!(!threads.iter().any(|t| t.id == "self-user"));
    }

    #[test]
    fn received_history_persists_and_reloads_after_store_reopen() {
        use crate::store::{load_history_into_threads, persist_threads, MessageStore};
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "ctdozlite-history-{}-{nonce}.db",
            std::process::id()
        ));
        let mut threads = vec![];
        merge_previews(&mut threads, &page("101").normalized_payload("self-user"));
        merge_previews(&mut threads, &page("101").normalized_payload("self-user"));
        {
            let mut store = MessageStore::open(path.to_str().unwrap()).unwrap();
            persist_threads(&mut store, &threads).unwrap();
        }
        threads[0].messages.clear();
        load_history_into_threads(path.to_str().unwrap(), &mut threads, 50);
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].messages.len(), 1);
        assert_eq!(threads[0].messages[0].text, "fixture");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn unsolicited_page_does_not_advance_sync_or_claim_completion() {
        let mut sync = DirectHistorySync::default();
        let empty = DirectHistoryPage::parse(&json!({"data":{"msgs":[]}})).unwrap();
        sync.accept_page(&empty, Instant::now());
        assert!(!sync.exhausted);
        assert_eq!(sync.pages, 0);
    }
}
