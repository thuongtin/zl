use serde_json::json;
use std::{
    io::{Read, Write},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use zalo_rust::{
    chat::{self, ChatMessage, Thread},
    session::{LoginInfo, Session},
    store,
};

// This integration binary has one test so its disposable working directory is isolated.
#[tokio::test]
async fn gui_remote_refresh_never_hydrates_or_writes_legacy_but_tui_wrapper_still_does() {
    let original = std::env::current_dir().unwrap();
    let directory = std::env::temp_dir().join(format!("zl-remote-storage-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    struct Cleanup(std::path::PathBuf, std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.0);
            let _ = std::fs::remove_dir_all(&self.1);
        }
    }
    let _cleanup = Cleanup(original, directory.clone());
    std::env::set_current_dir(&directory).unwrap();
    let legacy = Thread {
        presentation: Default::default(),
        id: "7001".into(),
        name: "Known contact".into(),
        preview: String::new(),
        time: String::new(),
        avatar: None,
        is_group: false,
        messages: vec![ChatMessage {
            id: "legacy-other-account".into(),
            text: "Legacy fixture".into(),
            ts: 1000,
            ..Default::default()
        }],
        unread: Default::default(),
    };
    let mut database = store::MessageStore::open("zalo_history.db").unwrap();
    store::persist_threads(&mut database, std::slice::from_ref(&legacy)).unwrap();
    drop(database);
    chat::try_save_threads_cache(&[legacy]).unwrap();
    let before_db = std::fs::read("zalo_history.db").unwrap();
    let before_json = std::fs::read("threads_cache.json").unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let base = format!("http://fixture.zalo.me:{}", address.port());
    listener.set_nonblocking(true).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let stopping = stop.clone();
    let profile_requests = Arc::new(AtomicUsize::new(0));
    let server_profile_requests = profile_requests.clone();
    let server = std::thread::spawn(move || {
        while !stopping.load(Ordering::Relaxed) {
            let Ok((mut stream, _)) = listener.accept() else {
                std::thread::sleep(Duration::from_millis(5));
                continue;
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut bytes = Vec::new();
            let mut chunk = [0; 4096];
            while bytes.len() < 32768 && !bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                let size = stream.read(&mut chunk).unwrap_or(0);
                if size == 0 {
                    break;
                }
                bytes.extend_from_slice(&chunk[..size]);
            }
            let request = String::from_utf8_lossy(&bytes);
            let data = if request.contains("/getfriends?") {
                json!({"friends":[{"userId":"7001","displayName":"Known contact","lastActionTime":1799999999000_i64}]})
            } else if request.contains("getChatFromPolling") || request.contains("get-last-msgs") {
                json!({"msgs":[{"msgId":"8002","cliMsgId":"8003","uidFrom":"7001","idTo":"0","ts":1793425680000_i64,"content":"Remote fixture"}]})
            } else if request.contains("/api/social/group/members?") {
                server_profile_requests.fetch_add(1, Ordering::Relaxed);
                json!({"profiles":{"4242":{"displayName":"PHƯƠNG","avatar":"https://fixture.invalid/member.png"}}})
            } else if request.contains("get-last-seen") {
                json!({"clearUnreads":[{"idTo":"7001","lastMsgId":"9000","isGroup":0}]})
            } else {
                json!({})
            };
            let body = json!({"error_code":0,"data":data}).to_string();
            let response=format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body);
            let _ = stream.write_all(response.as_bytes());
        }
    });
    let mut session = Session::new(false, false, false).unwrap();
    session.http = reqwest::Client::builder()
        .no_proxy()
        .resolve("fixture.zalo.me", address)
        .build()
        .unwrap();
    session.login = Some(LoginInfo {
        uin: "account-current".into(),
        ..Default::default()
    });
    session.secret_key = Some(vec![0; 32]);
    session.domains.profile = base.clone();
    session.domains.friend = base.clone();
    session.domains.group = base.clone();
    session.domains.chat = base.clone();
    session.domains.lp_chat = base.clone();
    session.domains.convers_api = base;
    let mut remote = chat::get_friends_remote(&session).await.unwrap();
    assert_eq!(
        remote[0].sidebar_activity_ts(),
        0,
        "contact lastActionTime must not become a conversation timestamp"
    );
    assert_eq!(remote.len(), 1);
    assert!(
        remote[0].messages.is_empty(),
        "remote metadata leaked legacy history"
    );
    chat::sync_unread_metadata(&session, &mut remote)
        .await
        .unwrap();
    assert_eq!(
        remote[0].unread.last_seen_id.as_deref(),
        Some("9000"),
        "metadata-only thread was skipped during cursor sync"
    );
    chat::poll_new_messages_remote(&session, &mut Default::default(), &mut remote)
        .await
        .unwrap();
    assert_eq!(remote[0].messages.len(), 1);
    assert_eq!(remote[0].messages[0].id, "8002");
    assert_eq!(
        std::fs::read("zalo_history.db").unwrap(),
        before_db,
        "GUI remote path wrote legacy DB"
    );
    assert_eq!(
        std::fs::read("threads_cache.json").unwrap(),
        before_json,
        "GUI remote path rewrote legacy cache"
    );
    let mut group = Thread {
        presentation: chat::SidebarPresentation {
            last_message_ts: 1793425680000,
            last_sender: "4242".into(),
            last_sender_id: "4242".into(),
            ..Default::default()
        },
        id: "g-fixture".into(),
        name: "Group fixture".into(),
        preview: "Hình ảnh".into(),
        time: String::new(),
        avatar: None,
        is_group: true,
        messages: Vec::new(),
        unread: Default::default(),
    };
    chat::resolve_thread_names(&session, std::slice::from_mut(&mut group)).await;
    assert!(
        profile_requests.load(Ordering::Relaxed) > 0,
        "must use the upstream group-member profile endpoint"
    );
    assert_eq!(
        group.sidebar_preview(),
        "PHƯƠNG: Hình ảnh",
        "metadata-only latest sender must resolve by exact UID"
    );
    assert!(
        group.messages.is_empty(),
        "profile enrichment must not hydrate message history"
    );
    assert_eq!(std::fs::read("zalo_history.db").unwrap(), before_db);
    assert_eq!(std::fs::read("threads_cache.json").unwrap(), before_json);
    let mut compat = chat::get_friends(&session).await.unwrap();
    assert!(compat[0]
        .messages
        .iter()
        .any(|m| m.id == "legacy-other-account"));
    chat::poll_new_messages(&session, &mut Default::default(), &mut compat)
        .await
        .unwrap();
    assert_ne!(
        std::fs::read("threads_cache.json").unwrap(),
        before_json,
        "legacy TUI wrapper lost persistence behavior"
    );
    stop.store(true, Ordering::Relaxed);
    server.join().unwrap();
}
