use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use zalo_rust::updater::{
    download_release, install_release, stage_portable_release, validate_asset_name, DownloadLimits,
    DownloadRequest, InstallTarget,
};

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

struct Sandbox(PathBuf);

impl Sandbox {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "ctdo-updater-test-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn destination(&self) -> PathBuf {
        self.0.join("application")
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn serve(response: Vec<u8>, delay: Duration) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            let mut byte = [0; 1];
            socket.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
            assert!(
                request.len() <= 4096,
                "fixture request headers exceeded bound"
            );
        }
        std::thread::sleep(delay);
        // Timeout cases intentionally close the client before the fixture responds.
        let _ = socket.write_all(&response);
    });
    format!("http://{address}/release")
}

fn response(body: &[u8], length: usize, status: &str) -> Vec<u8> {
    let mut response =
        format!("HTTP/1.1 {status}\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n")
            .into_bytes();
    response.extend_from_slice(body);
    response
}

async fn fetch(
    url: &str,
    checksum: Option<&str>,
    expected_size: Option<u64>,
    limits: DownloadLimits,
) -> Result<Vec<u8>, String> {
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    download_release(
        &client,
        &DownloadRequest {
            url,
            asset_name: "release.dmg",
            expected_sha256: checksum,
            expected_size,
            limits,
        },
        |_, _| {},
    )
    .await
}

fn limits() -> DownloadLimits {
    DownloadLimits {
        min_bytes: 1,
        max_bytes: 64,
        timeout: Duration::from_secs(2),
    }
}

#[tokio::test]
async fn download_accepts_exact_bytes_and_uppercase_sha256() {
    let body = b"validated release";
    let checksum = format!("{:X}", Sha256::digest(body));
    let url = serve(response(body, body.len(), "200 OK"), Duration::ZERO);
    assert_eq!(
        fetch(&url, Some(&checksum), Some(body.len() as u64), limits())
            .await
            .unwrap(),
        body
    );
}

#[tokio::test]
async fn invalid_transfers_preserve_existing_destination_and_unrelated_file() {
    let sandbox = Sandbox::new();
    let destination = sandbox.destination();
    let unrelated = sandbox.0.join("user-file.txt");
    fs::write(&destination, b"old executable").unwrap();
    fs::write(&unrelated, b"user data").unwrap();
    let body = b"release";
    let cases = [
        (
            response(body, body.len(), "200 OK"),
            Some("0".repeat(64)),
            None,
        ),
        (response(body, body.len() + 10, "200 OK"), None, None),
        (response(body, body.len(), "500 Server Error"), None, None),
        (response(body, 1000, "200 OK"), None, None),
        (response(body, body.len(), "200 OK"), None, Some(9)),
    ];
    for (response, checksum, size) in cases {
        let url = serve(response, Duration::ZERO);
        let downloaded = fetch(&url, checksum.as_deref(), size, limits()).await;
        if let Ok(bytes) = &downloaded {
            install_release(bytes, &InstallTarget::Portable(destination.clone()), |_| {
                Ok(())
            })
            .unwrap();
        }
        assert!(downloaded.is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"old executable");
        assert_eq!(fs::read(&unrelated).unwrap(), b"user data");
    }
}

#[tokio::test]
async fn streaming_limit_rejects_body_without_content_length() {
    let mut raw = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".to_vec();
    raw.extend_from_slice(&[b'x'; 65]);
    let url = serve(raw, Duration::ZERO);
    assert!(fetch(&url, None, None, limits()).await.is_err());
}

#[tokio::test]
async fn transfer_stops_at_configured_deadline() {
    let url = serve(response(b"x", 1, "200 OK"), Duration::from_millis(200));
    let mut bounded = limits();
    bounded.timeout = Duration::from_millis(30);
    assert!(fetch(&url, None, None, bounded).await.is_err());
}

#[tokio::test]
async fn invalid_provided_checksum_is_rejected_before_connecting() {
    for checksum in ["short".to_string(), "z".repeat(64)] {
        assert!(fetch("http://127.0.0.1:1", Some(&checksum), None, limits())
            .await
            .unwrap_err()
            .contains("SHA-256"));
    }
}

#[test]
fn asset_names_reject_cross_platform_traversal() {
    for name in [
        "",
        ".",
        "..",
        "../release.dmg",
        "a/b",
        "a\\b",
        "C:evil.exe",
        "/tmp/update",
        "a\0b",
    ] {
        assert!(validate_asset_name(name).is_err(), "accepted {name:?}");
    }
    validate_asset_name("ctdozlite-1.2.3-arm64.dmg").unwrap();
}

#[test]
fn portable_replacement_launches_new_bytes_and_preserves_user_file() {
    let sandbox = Sandbox::new();
    let destination = sandbox.destination();
    let unrelated = sandbox.0.join("session-fixture.json");
    fs::write(&destination, b"old executable").unwrap();
    fs::write(&unrelated, b"fake user file").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&destination, fs::Permissions::from_mode(0o755)).unwrap();
    }
    install_release(
        b"new executable",
        &InstallTarget::Portable(destination.clone()),
        |path| {
            assert_eq!(fs::read(path).unwrap(), b"new executable");
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(fs::read(&destination).unwrap(), b"new executable");
    assert_eq!(fs::read(&unrelated).unwrap(), b"fake user file");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&destination).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }
    assert_eq!(fs::read_dir(&sandbox.0).unwrap().count(), 2);
}

#[test]
fn portable_launch_failure_restores_original_bytes() {
    let sandbox = Sandbox::new();
    let destination = sandbox.destination();
    fs::write(&destination, b"original bytes").unwrap();
    let result = install_release(
        b"replacement",
        &InstallTarget::Portable(destination.clone()),
        |_| Err("injected launch failure".into()),
    );
    assert!(result.unwrap_err().contains("đã hoàn tác"));
    assert_eq!(fs::read(&destination).unwrap(), b"original bytes");
    assert_eq!(fs::read_dir(&sandbox.0).unwrap().count(), 1);
}

#[test]
fn installer_saves_to_explicit_sandbox_and_launches_callback() {
    let sandbox = Sandbox::new();
    let destination = sandbox.0.join("release.dmg");
    install_release(
        b"installer",
        &InstallTarget::Installer(destination.clone()),
        |path| {
            assert_eq!(path, destination);
            assert_eq!(fs::read(path).unwrap(), b"installer");
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(fs::read(&destination).unwrap(), b"installer");
}

#[test]
fn new_installer_launch_failure_removes_unlaunched_artifact() {
    let sandbox = Sandbox::new();
    let destination = sandbox.0.join("release.dmg");
    assert!(install_release(
        b"installer",
        &InstallTarget::Installer(destination.clone()),
        |_| { Err("injected launch failure".into()) }
    )
    .is_err());
    assert!(!destination.exists());
    assert_eq!(fs::read_dir(&sandbox.0).unwrap().count(), 0);
}

#[test]
fn missing_parent_and_directory_targets_fail_without_launch() {
    let sandbox = Sandbox::new();
    for destination in [sandbox.0.join("missing/release.dmg"), sandbox.0.clone()] {
        assert!(
            install_release(b"installer", &InstallTarget::Installer(destination), |_| {
                panic!("invalid targets must not launch");
            })
            .is_err()
        );
    }
    assert_eq!(fs::read_dir(&sandbox.0).unwrap().count(), 0);
}

#[cfg(unix)]
#[test]
fn symlink_destination_never_changes_user_selected_file() {
    let sandbox = Sandbox::new();
    let user_file = sandbox.0.join("user-file");
    let destination = sandbox.destination();
    fs::write(&user_file, b"user content").unwrap();
    std::os::unix::fs::symlink(&user_file, &destination).unwrap();
    assert!(install_release(b"new", &InstallTarget::Installer(destination), |_| Ok(())).is_err());
    assert_eq!(fs::read(&user_file).unwrap(), b"user content");
}

#[tokio::test]
async fn minimum_size_rejects_empty_artifacts() {
    let url = serve(response(b"", 0, "200 OK"), Duration::ZERO);
    assert!(fetch(&url, None, None, limits()).await.is_err());
}

#[tokio::test]
async fn executable_signature_is_checked_before_installation() {
    let url = serve(response(b"not a PE", 8, "200 OK"), Duration::ZERO);
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    assert!(download_release(
        &client,
        &DownloadRequest {
            url: &url,
            asset_name: "release.exe",
            expected_sha256: None,
            expected_size: None,
            limits: limits(),
        },
        |_, _| {}
    )
    .await
    .unwrap_err()
    .contains("PE"));
}

#[test]
fn existing_installer_launch_failure_restores_previous_download() {
    let sandbox = Sandbox::new();
    let destination = sandbox.0.join("release.dmg");
    fs::write(&destination, b"previous installer").unwrap();
    assert!(install_release(
        b"new installer",
        &InstallTarget::Installer(destination.clone()),
        |_| { Err("injected launch failure".into()) }
    )
    .is_err());
    assert_eq!(fs::read(&destination).unwrap(), b"previous installer");
    assert_eq!(fs::read_dir(&sandbox.0).unwrap().count(), 1);
}

#[test]
fn deferred_portable_stage_preserves_original_until_explicit_launch() {
    let sandbox = Sandbox::new();
    let destination = sandbox.destination();
    fs::write(&destination, b"old executable").unwrap();
    let pending = stage_portable_release(b"new executable", &destination).unwrap();
    assert_eq!(fs::read(&destination).unwrap(), b"old executable");
    let mut launched = false;
    pending
        .launch_and_finish(|path| {
            launched = true;
            assert_eq!(fs::read(path).unwrap(), b"new executable");
            Ok(())
        })
        .unwrap();
    assert!(launched);
    assert_eq!(fs::read(&destination).unwrap(), b"new executable");
    assert_eq!(fs::read_dir(&sandbox.0).unwrap().count(), 1);
}

#[test]
fn deferred_portable_drop_discards_staging_without_touching_original() {
    let sandbox = Sandbox::new();
    let destination = sandbox.destination();
    fs::write(&destination, b"old executable").unwrap();
    let pending = stage_portable_release(b"new executable", &destination).unwrap();
    drop(pending);
    assert_eq!(fs::read(&destination).unwrap(), b"old executable");
    assert_eq!(fs::read_dir(&sandbox.0).unwrap().count(), 1);
}

#[test]
fn deferred_portable_abort_discards_staging_without_launch() {
    let sandbox = Sandbox::new();
    let destination = sandbox.destination();
    fs::write(&destination, b"old executable").unwrap();
    stage_portable_release(b"new executable", &destination)
        .unwrap()
        .abort()
        .unwrap();
    assert_eq!(fs::read(&destination).unwrap(), b"old executable");
    assert_eq!(fs::read_dir(&sandbox.0).unwrap().count(), 1);
}

#[test]
fn deferred_portable_launch_failure_restores_original() {
    let sandbox = Sandbox::new();
    let destination = sandbox.destination();
    fs::write(&destination, b"old executable").unwrap();
    let pending = stage_portable_release(b"new executable", &destination).unwrap();
    assert!(pending
        .launch_and_finish(|_| Err("injected launch failure".into()))
        .is_err());
    assert_eq!(fs::read(&destination).unwrap(), b"old executable");
    assert_eq!(fs::read_dir(&sandbox.0).unwrap().count(), 1);
}

#[test]
fn deferred_portable_unwind_restores_original() {
    let sandbox = Sandbox::new();
    let destination = sandbox.destination();
    fs::write(&destination, b"old executable").unwrap();
    let pending = stage_portable_release(b"new executable", &destination).unwrap();
    let panic = std::panic::catch_unwind(|| {
        pending
            .launch_and_finish(|_| panic!("injected launcher panic"))
            .unwrap();
    });
    assert!(panic.is_err());
    assert_eq!(fs::read(&destination).unwrap(), b"old executable");
    assert_eq!(fs::read_dir(&sandbox.0).unwrap().count(), 1);
}
