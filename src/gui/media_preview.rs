//! Bounded media decoding. Native codecs run in a short-lived macOS helper.
use anyhow::{anyhow, bail, Context, Result};
use image::{ImageDecoder, ImageFormat, ImageReader, RgbaImage};
use std::io::Cursor;

pub const MAX_INPUT_BYTES: usize = 16 * 1024 * 1024;
const MAX_EDGE: u32 = 480;

/// Image texture quality; full images retain raster dimensions within the shared pixel budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageQuality {
    Preview,
    Full,
}

impl ImageQuality {
    /// Separate full textures from preview entries in the account-private cache.
    pub fn for_texture_key(key: &str) -> Self {
        if key.starts_with("hd_") {
            Self::Full
        } else {
            Self::Preview
        }
    }
}
const MAX_PIXELS: u64 = 64 * 1024 * 1024;

fn decode_raster(bytes: &[u8], quality: ImageQuality) -> Result<RgbaImage> {
    let format = image::guess_format(bytes)?;
    if !matches!(format, ImageFormat::Png | ImageFormat::Jpeg) {
        bail!("Native image codec required");
    }
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(32768);
    limits.max_image_height = Some(32768);
    limits.max_alloc = Some(96 * 1024 * 1024);
    reader.limits(limits);
    let mut decoder = reader.into_decoder()?;
    let (width, height) = decoder.dimensions();
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_PIXELS {
        bail!("Image dimensions exceed preview limits");
    }
    let orientation = decoder.orientation()?;
    let mut decoded = image::DynamicImage::from_decoder(decoder)?;
    decoded.apply_orientation(orientation);
    if quality == ImageQuality::Preview
        && (decoded.width() > MAX_EDGE || decoded.height() > MAX_EDGE)
    {
        decoded = decoded.thumbnail(MAX_EDGE, MAX_EDGE);
    }
    Ok(decoded.into_rgba8())
}

/// Decode at most 16 MiB into an orientation-correct preview with edge <=480.
/// CPU decoding uses the blocking pool; native decoding has a 10-second timeout.
#[cfg(test)]
pub async fn decode_preview(bytes: &[u8]) -> Result<RgbaImage> {
    decode_image(bytes, ImageQuality::Preview).await
}

/// Decode bounded image data at the requested quality. Native codecs retain the helper cap.
///
/// # Errors
/// Returns an error for oversized, malformed, unsupported or timed-out input.
pub async fn decode_image(bytes: &[u8], quality: ImageQuality) -> Result<RgbaImage> {
    if bytes.is_empty() || bytes.len() > MAX_INPUT_BYTES {
        bail!("Image input exceeds preview limits");
    }
    let owned = bytes.to_vec();
    let raster = tokio::task::spawn_blocking(move || decode_raster(&owned, quality))
        .await
        .context("Image decode task failed")?;
    match raster {
        Ok(image) => Ok(image),
        Err(_) => decode_native(bytes, quality).await,
    }
}

#[cfg(target_os = "macos")]
async fn decode_native(bytes: &[u8], quality: ImageQuality) -> Result<RgbaImage> {
    use std::process::Stdio;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let packaged = std::env::current_exe().ok().and_then(|path| {
        path.parent()?
            .parent()
            .map(|contents| contents.join("Resources/MediaPreview"))
    });
    let helper = packaged.filter(|path| path.is_file()).unwrap_or_else(|| {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("native/macos/build/MediaPreview")
    });
    let mut child = tokio::process::Command::new(helper)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("Media preview helper unavailable")?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow!("Missing helper input"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("Missing helper output"))?;
    let operation = async {
        let write = async move {
            stdin.write_all(bytes).await?;
            stdin.shutdown().await?;
            drop(stdin);
            Ok::<_, std::io::Error>(())
        };
        let read = async {
            let mut output = Vec::new();
            stdout
                .take(8 * 1024 * 1024 + 1)
                .read_to_end(&mut output)
                .await?;
            Ok::<_, std::io::Error>(output)
        };
        let (_, output) = tokio::try_join!(write, read)?;
        if output.len() > 8 * 1024 * 1024 {
            bail!("Helper output exceeds preview limits");
        }
        let status = child.wait().await?;
        if !status.success() {
            bail!("Native media preview failed");
        }
        Ok::<_, anyhow::Error>(output)
    };
    let output = tokio::time::timeout(std::time::Duration::from_secs(10), operation)
        .await
        .context("Media preview helper timed out")??;
    tokio::task::spawn_blocking(move || decode_raster(&output, quality))
        .await
        .context("Preview decode task failed")?
}

#[cfg(not(target_os = "macos"))]
async fn decode_native(_: &[u8], _: ImageQuality) -> Result<RgbaImage> {
    Err(anyhow!("Unsupported media preview format"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn rejects_empty_and_oversized_input() {
        assert!(decode_preview(&[]).await.is_err());
        assert!(decode_preview(&vec![0; MAX_INPUT_BYTES + 1]).await.is_err());
    }
    #[test]
    fn hd_texture_key_selects_full_quality_only_for_explicit_viewer_requests() {
        assert_eq!(
            ImageQuality::for_texture_key("hd_fixture"),
            ImageQuality::Full
        );
        assert_eq!(
            ImageQuality::for_texture_key("preview_fixture"),
            ImageQuality::Preview
        );
        assert_eq!(
            ImageQuality::for_texture_key("shd_fixture"),
            ImageQuality::Preview
        );
    }

    #[test]
    fn raster_thumbnail_is_bounded() {
        let image = image::DynamicImage::ImageRgba8(RgbaImage::from_pixel(
            2048,
            1024,
            image::Rgba([10, 20, 30, 255]),
        ));
        let mut bytes = Cursor::new(Vec::new());
        image.write_to(&mut bytes, ImageFormat::Png).unwrap();
        let result = decode_raster(bytes.get_ref(), ImageQuality::Preview).unwrap();
        assert_eq!(result.dimensions(), (480, 240));
        let full = decode_raster(bytes.get_ref(), ImageQuality::Full).unwrap();
        assert_eq!(full.dimensions(), (2048, 1024));
    }
}

#[cfg(test)]
mod real_codec_probe {
    #[tokio::test]
    #[ignore = "Requires an explicit private image fixture path"]
    async fn real_codec_fixture() {
        let path = std::env::var("ZL_PREVIEW_FIXTURE").expect("fixture path required");
        let bytes = std::fs::read(path).unwrap();
        let image = super::decode_preview(&bytes).await.unwrap();
        assert!(image.width() <= 480 && image.height() <= 480);
        println!(
            "PREVIEW_DECODE: {}x{} RGBA, input {} bytes",
            image.width(),
            image.height(),
            bytes.len()
        );
    }
}

/// Launch only from an explicit user action after the caller checks its account/generation.
/// The native helper validates local file contents against MIME before opening a window.
#[cfg(target_os = "macos")]
pub fn open_native_viewer(path: &std::path::Path, mime: &str) -> Result<()> {
    let path = path.canonicalize().context("Media file unavailable")?;
    if !path.is_file() || !(mime.starts_with("image/") || mime.starts_with("video/")) {
        bail!("Invalid viewer input");
    }
    let packaged = std::env::current_exe()
        .ok()
        .and_then(|path| Some(path.parent()?.parent()?.join("Resources/DailyMediaViewer")));
    let helper = packaged.filter(|path| path.is_file()).unwrap_or_else(|| {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("native/macos/build/DailyMediaViewer")
    });
    let mut child = std::process::Command::new(helper)
        .arg("--file")
        .arg(path)
        .arg("--mime")
        .arg(mime)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("Media viewer unavailable")?;
    // Reap the separate helper without waiting on the GUI thread.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn open_native_viewer(_: &std::path::Path, _: &str) -> Result<()> {
    bail!("Native media viewer unavailable on this platform")
}

/// Read only the account/thread/message/quality cache, never the legacy global cache.
/// The caller must hold an admitted media-job permit and reject stale UI results.
pub async fn load_account_preview(
    client: &reqwest::Client,
    key: &crate::media_resources::MediaKey,
    url: &str,
    generation: &crate::media_resources::RequestGeneration,
    ticket: u64,
) -> std::result::Result<RgbaImage, crate::media_resources::MediaError> {
    load_account_preview_at(
        client,
        key,
        url,
        generation,
        ticket,
        std::path::Path::new("media_cache"),
    )
    .await
}

async fn load_account_preview_at(
    client: &reqwest::Client,
    key: &crate::media_resources::MediaKey,
    url: &str,
    generation: &crate::media_resources::RequestGeneration,
    ticket: u64,
    cache_root: &std::path::Path,
) -> std::result::Result<RgbaImage, crate::media_resources::MediaError> {
    use crate::media_resources::{AtomicPrivateFile, MediaError, PrivateMediaCache, PREVIEW_LIMIT};
    use std::io::Read;
    let quality = if key.quality == crate::media_resources::MediaQuality::Original {
        ImageQuality::Full
    } else {
        ImageQuality::Preview
    };
    generation.check(ticket)?;
    if key.account.is_empty() {
        return Err(MediaError::AccountMismatch);
    }
    // Disk caching is optional. A full or unavailable disk must not disable a valid preview.
    let cache = PrivateMediaCache::new(cache_root, &key.account).ok();
    let path = cache.as_ref().and_then(|cache| cache.path_for(key).ok());
    if let Some(file) = path
        .as_ref()
        .and_then(|path| std::fs::File::open(path).ok())
    {
        let mut bytes = Vec::new();
        if file
            .take(PREVIEW_LIMIT as u64 + 1)
            .read_to_end(&mut bytes)
            .is_ok()
            && bytes.len() <= PREVIEW_LIMIT
        {
            if let Ok(image) = decode_image(&bytes, quality).await {
                generation.check(ticket)?;
                if let Some(cache) = &cache {
                    let _ = cache.touch(key);
                }
                return Ok(image);
            }
        }
    }
    // Newly uploaded CDN objects and temporary transport failures can recover without
    // a manual click. Keep all attempts within one bounded, cancellable media job.
    let operation = async {
        let mut last_error = MediaError::Offline;
        for attempt in 0..3 {
            generation.check(ticket)?;
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(if attempt == 1 {
                    200
                } else {
                    600
                }))
                .await;
                generation.check(ticket)?;
            }
            let result = match crate::media_resources::fetch_bounded(
                client,
                url,
                PREVIEW_LIMIT,
                generation,
                ticket,
            )
            .await
            {
                Ok(bytes) => decode_image(&bytes, quality)
                    .await
                    .map(|image| (bytes, image))
                    .map_err(|_| MediaError::Decoder),
                Err(error) => Err(error),
            };
            match result {
                Ok(result) => return Ok(result),
                Err(error) => {
                    let retry = matches!(
                        error,
                        MediaError::Offline
                            | MediaError::Timeout
                            | MediaError::NotFound
                            | MediaError::Decoder
                            | MediaError::Http(408 | 425 | 429 | 500..=599)
                    );
                    if !retry {
                        return Err(error);
                    }
                    last_error = error;
                }
            }
        }
        Err(last_error)
    };
    let (bytes, image) = tokio::time::timeout(std::time::Duration::from_secs(30), operation)
        .await
        .map_err(|_| MediaError::Timeout)??;
    generation.check(ticket)?;
    // Persist the bounded source bytes. Native formats keep their original orientation metadata.
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let suffix = format!(
        "preview-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    if let Some(mut file) = path
        .as_ref()
        .and_then(|path| AtomicPrivateFile::create(path, &suffix).ok())
    {
        if file.write_all(&bytes).is_ok() && generation.is_current(ticket) {
            let _ = file.commit();
        }
    }
    if let Some(cache) = &cache {
        let _ = cache.prune();
    }
    generation.check(ticket)?;
    Ok(image)
}

#[cfg(test)]
mod private_download_probe {
    #[tokio::test]
    #[ignore = "Requires explicit private media fixture and disposable working directory"]
    async fn recent_photo_download_fixture() {
        let fixture = std::env::var("ZL_RECENT_MEDIA_FIXTURE").expect("fixture path required");
        let disposable =
            std::env::var("ZL_MEDIA_PROBE_CWD").expect("disposable directory required");
        let rows: serde_json::Value =
            serde_json::from_slice(&std::fs::read(fixture).unwrap()).unwrap();
        let session = crate::session::Session::new(false, false, false).unwrap();
        for (index, row) in rows.as_array().unwrap().iter().enumerate() {
            let message: crate::chat::ChatMessage =
                serde_json::from_value(row["message"].clone()).unwrap();
            let url = message.link_thumb.as_deref().unwrap();
            let key = crate::media_resources::MediaKey {
                account: "synthetic-probe-account".into(),
                thread: "synthetic-thread".into(),
                message: index.to_string(),
                quality: crate::media_resources::MediaQuality::Preview,
            };
            let generation = crate::media_resources::RequestGeneration::default();
            let image = super::load_account_preview_at(
                &session.http,
                &key,
                url,
                &generation,
                generation.capture(),
                std::path::Path::new(&disposable)
                    .join("media_cache")
                    .as_path(),
            )
            .await
            .unwrap();
            println!(
                "RECENT_PHOTO_{index}: {}x{} RGBA PASS",
                image.width(),
                image.height()
            );
        }
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    use crate::media_resources::{MediaKey, MediaQuality, RequestGeneration};

    async fn serve_photo(statuses: Vec<u16>, blocked_cache: bool) -> Result<RgbaImage> {
        serve_photo_with_quality_checks(statuses, blocked_cache, false).await
    }

    async fn serve_photo_with_quality_checks(
        statuses: Vec<u16>,
        blocked_cache: bool,
        quality_checks: bool,
    ) -> Result<RgbaImage> {
        use std::io::{Read, Write};
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "zl-preview-recovery-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root)?;
        let cache_root = root.join(if blocked_cache { "blocked" } else { "cache" });
        if blocked_cache {
            std::fs::write(&cache_root, b"not a directory")?;
        }
        let mut encoded = Cursor::new(Vec::new());
        let dimensions = if quality_checks { (2048, 1024) } else { (2, 3) };
        image::DynamicImage::ImageRgba8(RgbaImage::from_pixel(
            dimensions.0,
            dimensions.1,
            image::Rgba([1, 2, 3, 255]),
        ))
        .write_to(&mut encoded, ImageFormat::Png)?;
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        listener.set_nonblocking(true)?;
        let body = encoded.into_inner();
        let expected_requests = statuses.len();
        let server = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            let mut served = 0;
            let mut responses = statuses.into_iter();
            while std::time::Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        // Accepted sockets inherit O_NONBLOCK on macOS. The fixture uses
                        // blocking reads, so clear it before waiting for request headers.
                        stream.set_nonblocking(false).unwrap();
                        stream
                            .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                            .unwrap();
                        // TCP reads can split headers. Drain the complete bounded GET request
                        // before closing, otherwise unread input can reset a valid response.
                        let mut request = Vec::new();
                        let mut chunk = [0; 1024];
                        while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                            let count = stream
                                .read(&mut chunk)
                                .expect("Fixture request read failed");
                            assert!(count > 0, "Fixture request closed before complete headers");
                            request.extend_from_slice(&chunk[..count]);
                            assert!(
                                request.len() <= 16 * 1024,
                                "Fixture request headers exceed limit"
                            );
                        }
                        let status = responses.next().unwrap_or(500);
                        let bytes = if status == 200 { body.as_slice() } else { &[] };
                        let mut response = format!("HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", bytes.len()).into_bytes();
                        response.extend_from_slice(bytes);
                        stream
                            .write_all(&response)
                            .expect("Fixture response write failed");
                        stream.shutdown(std::net::Shutdown::Write).unwrap();
                        served += 1;
                        if responses.len() == 0 {
                            break;
                        }
                    }
                    Err(_) => std::thread::sleep(std::time::Duration::from_millis(5)),
                }
            }
            served
        });
        let key = MediaKey {
            account: "fixture-account".into(),
            thread: "fixture-thread".into(),
            message: "fixture-message".into(),
            quality: MediaQuality::Preview,
        };
        let generation = RequestGeneration::default();
        let result = load_account_preview_at(
            &reqwest::Client::new(),
            &key,
            &format!("http://{address}/fixture"),
            &generation,
            0,
            &cache_root,
        )
        .await;
        let full_key = MediaKey {
            quality: MediaQuality::Original,
            ..key.clone()
        };
        if quality_checks {
            assert_eq!(result.as_ref().unwrap().dimensions(), (480, 240));
            let full = load_account_preview_at(
                &reqwest::Client::new(),
                &full_key,
                &format!("http://{address}/fixture"),
                &generation,
                0,
                &cache_root,
            )
            .await
            .unwrap();
            assert_eq!(full.dimensions(), (2048, 1024));
        }
        let served = server.join().unwrap();
        assert_eq!(
            served, expected_requests,
            "Fixture did not serve every expected request"
        );
        if quality_checks {
            let cache = crate::media_resources::PrivateMediaCache::new(&cache_root, &key.account)?;
            let preview_path = cache.path_for(&key)?;
            let full_path = cache.path_for(&full_key)?;
            assert_ne!(preview_path, full_path);
            assert!(preview_path.is_file() && full_path.is_file());
            // Both cache entries store bounded original source bytes, not the thumbnail.
            for path in [preview_path, full_path] {
                assert_eq!(
                    image::load_from_memory(&std::fs::read(path)?)?.width(),
                    2048
                );
            }
            // The HTTP fixture has closed: success now proves the disk-hit decoder path.
            for (identity, expected) in [(&key, (480, 240)), (&full_key, (2048, 1024))] {
                let hit = load_account_preview_at(
                    &reqwest::Client::new(),
                    identity,
                    "not-a-network-url",
                    &generation,
                    0,
                    &cache_root,
                )
                .await
                .unwrap();
                assert_eq!(hit.dimensions(), expected);
            }
        }
        std::fs::remove_dir_all(root)?;
        Ok(result?)
    }

    #[tokio::test]
    async fn invalid_account_and_cancelled_request_do_not_fetch() {
        let root = std::env::temp_dir().join(format!("zl-preview-no-fetch-{}", std::process::id()));
        let mut key = MediaKey {
            account: String::new(),
            thread: "fixture".into(),
            message: "fixture".into(),
            quality: MediaQuality::Preview,
        };
        let client = reqwest::Client::new();
        let generation = RequestGeneration::default();
        assert!(matches!(
            load_account_preview_at(
                &client,
                &key,
                "http://127.0.0.1:1/fixture",
                &generation,
                0,
                &root
            )
            .await,
            Err(crate::media_resources::MediaError::AccountMismatch)
        ));
        key.account = "fixture".into();
        generation.cancel();
        assert!(matches!(
            load_account_preview_at(
                &client,
                &key,
                "http://127.0.0.1:1/fixture",
                &generation,
                0,
                &root
            )
            .await,
            Err(crate::media_resources::MediaError::Cancelled)
        ));
        assert!(!root.exists());
    }

    #[tokio::test]
    async fn account_cache_miss_and_hit_preserve_full_and_preview_quality() {
        serve_photo_with_quality_checks(vec![200, 200], false, true)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn transient_photo_fetch_recovers_without_user_click() {
        let image = serve_photo(vec![503, 200], false).await.unwrap();
        assert_eq!(image.dimensions(), (2, 3));
    }

    #[tokio::test]
    async fn unavailable_disk_cache_does_not_block_preview() {
        let image = serve_photo(vec![200], true).await.unwrap();
        assert_eq!(image.dimensions(), (2, 3));
    }
}
