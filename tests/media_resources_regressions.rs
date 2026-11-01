use std::{
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
};
use zalo_rust::media_resources::*;
fn fixture(status: &str, body: &'static [u8]) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let status = status.to_owned();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0; 4096];
        let _ = stream.read(&mut request);
        let _ = write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(body);
    });
    format!("http://{address}/fixture")
}
fn temp() -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "zl-media-test-{}-{}-{}",
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}
#[test]
fn identity_and_account_isolation() {
    let root = temp();
    let cache = PrivateMediaCache::new(&root, "account-a").unwrap();
    let mut key = MediaKey {
        account: "account-a".into(),
        thread: "thread".into(),
        message: "message".into(),
        quality: MediaQuality::Preview,
    };
    let first = cache.path_for(&key).unwrap();
    key.quality = MediaQuality::Original;
    assert_ne!(first, cache.path_for(&key).unwrap());
    key.account = "account-b".into();
    assert_eq!(cache.path_for(&key), Err(MediaError::AccountMismatch));
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn lru_accounting_evicts_and_touches() {
    let mut lru = ResourceLru::new(10);
    assert!(lru.insert("a".into(), 6).is_empty());
    assert_eq!(lru.insert("b".into(), 6), vec!["a"]);
    assert_eq!(lru.used(), 6);
    lru.touch("b");
    assert_eq!(lru.insert("huge".into(), 20), vec!["b", "huge"]);
    assert_eq!(lru.used(), 0);
}
#[tokio::test]
async fn bounded_status_and_cancellation() {
    let client = reqwest::Client::new();
    let generation = RequestGeneration::default();
    let ticket = generation.capture();
    assert_eq!(
        fetch_bounded(&client, &fixture("200 OK", b"abcd"), 3, &generation, ticket).await,
        Err(MediaError::TooLarge)
    );
    assert_eq!(
        fetch_bounded(
            &client,
            &fixture("404 Not Found", b""),
            3,
            &generation,
            ticket
        )
        .await,
        Err(MediaError::NotFound)
    );
    generation.cancel();
    assert_eq!(
        fetch_bounded(&client, "http://127.0.0.1:1", 3, &generation, ticket).await,
        Err(MediaError::Cancelled)
    );
}
#[tokio::test]
async fn streams_atomic_private_original() {
    let root = temp();
    let destination = root.join("test.media");
    let generation = RequestGeneration::default();
    assert_eq!(
        stream_original(
            &reqwest::Client::new(),
            &fixture("200 OK", b"fixture-original"),
            &destination,
            &generation,
            0
        )
        .await
        .unwrap(),
        16
    );
    assert_eq!(std::fs::read(&destination).unwrap(), b"fixture-original");
    assert!(!destination.with_extension("0.privatepart").exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&destination)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}
#[tokio::test]
async fn queue_bounds_active_jobs() {
    let queue = MediaJobQueue::default();
    let mut permits = Vec::new();
    for _ in 0..ACTIVE_JOBS {
        permits.push(queue.acquire().await.unwrap());
    }
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(10), queue.acquire())
            .await
            .is_err()
    );
    drop(permits);
    assert!(queue.acquire().await.is_some());
}

#[tokio::test]
async fn chunked_overflow_and_offline_are_distinct() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buffer = [0; 1024];
        let _ = stream.read(&mut buffer);
        let _=stream.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n4\r\nabcd\r\n4\r\nefgh\r\n0\r\n\r\n");
    });
    let client = reqwest::Client::new();
    let generation = RequestGeneration::default();
    assert_eq!(
        fetch_bounded(&client, &format!("http://{address}"), 6, &generation, 0).await,
        Err(MediaError::TooLarge)
    );
    let closed = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = closed.local_addr().unwrap();
    drop(closed);
    assert_eq!(
        fetch_bounded(&client, &format!("http://{address}"), 6, &generation, 0).await,
        Err(MediaError::Offline)
    );
}
#[tokio::test]
async fn cancellation_removes_partial_without_publication() {
    let root = temp();
    let destination = root.join("cancel.media");
    let generation = RequestGeneration::default();
    let cancel = generation.clone();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0; 1024];
        let _ = stream.read(&mut request);
        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\n\r\nabcd");
        std::thread::sleep(std::time::Duration::from_millis(50));
        cancel.cancel();
        let _ = stream.write_all(b"efgh");
    });
    assert_eq!(
        stream_original(
            &reqwest::Client::new(),
            &format!("http://{address}"),
            &destination,
            &generation,
            0
        )
        .await,
        Err(MediaError::Cancelled)
    );
    assert!(!destination.exists());
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn prune_removes_cache_only_and_preserves_saved_originals() {
    let root = temp();
    let cache = PrivateMediaCache::new(&root, "a").unwrap();
    let key = MediaKey {
        account: "a".into(),
        thread: "t".into(),
        message: "m".into(),
        quality: MediaQuality::Original,
    };
    let path = cache.path_for(&key).unwrap();
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(DISK_BUDGET + 1).unwrap();
    drop(file);
    let saved = path.parent().unwrap().join("saved-original.png");
    std::fs::write(&saved, b"saved").unwrap();
    assert_eq!(cache.prune().unwrap(), 0);
    assert!(!path.exists());
    assert_eq!(std::fs::read(saved).unwrap(), b"saved");
    std::fs::remove_dir_all(root).unwrap();
}
#[tokio::test]
async fn existing_partial_is_not_deleted() {
    let root = temp();
    let destination = root.join("test.media");
    let partial = destination.with_extension("0.privatepart");
    std::fs::write(&partial, b"other-job").unwrap();
    let generation = RequestGeneration::default();
    assert_eq!(
        stream_original(
            &reqwest::Client::new(),
            &fixture("200 OK", b"data"),
            &destination,
            &generation,
            0
        )
        .await,
        Err(MediaError::Io)
    );
    assert_eq!(std::fs::read(partial).unwrap(), b"other-job");
    std::fs::remove_dir_all(root).unwrap();
}
