use md5::{Digest, Md5};
use std::{
    io::{Read, Write},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
use zalo_rust::{
    chat::{
        client_id_wire_value, scrape_opengraph_metadata, AttachmentUploadSource,
        SINGLE_SHOT_UPLOAD_LIMIT, UPLOAD_CHUNK_SIZE,
    },
    media_resources::{read_bounded_response, MediaError, HTML_LIMIT},
};

static NEXT_FILE: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new(bytes: &[u8]) -> Self {
        let path = std::env::temp_dir().join(format!(
            "zl-transfer-{}-{}.bin",
            std::process::id(),
            NEXT_FILE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap()
            .write_all(bytes)
            .unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[tokio::test]
async fn upload_preserves_chunk_headers_md5_tail_and_exact_reassembly() {
    let bytes: Vec<_> = (0..UPLOAD_CHUNK_SIZE * 2 + 37)
        .map(|i| (i % 251) as u8)
        .collect();
    let fixture = Fixture::new(&bytes);
    let upload = AttachmentUploadSource::open(&fixture.0).await.unwrap();
    assert_eq!(upload.total_size, bytes.len() as u64);
    assert_eq!(upload.total_chunks(), 3);
    assert_eq!(upload.checksum, format!("{:x}", Md5::digest(&bytes)));
    let headers = upload
        .chunk_parameters(
            "fixture.bin",
            9007199254740993,
            "fixture-imei",
            "group-id",
            true,
            false,
            3,
        )
        .unwrap();
    assert_eq!(headers["totalChunk"], 3);
    assert_eq!(headers["chunkId"], 3);
    assert_eq!(headers["totalSize"], bytes.len());
    assert_eq!(headers["clientId"].as_u64(), Some(9007199254740993));
    assert_eq!(headers["grid"], "group-id");
    assert_eq!(headers["imei"], "fixture-imei");
    assert_eq!(headers["isE2EE"], 0);
    assert!(headers.get("jxl").is_none());
    let photo_headers = upload
        .chunk_parameters("image.png", 7, "imei", "contact", false, true, 1)
        .unwrap();
    assert_eq!(photo_headers["jxl"], 0);
    assert_eq!(photo_headers["toid"], "contact");
    assert!(photo_headers.get("grid").is_none());
    assert!(upload
        .chunk_parameters("x", 1, "i", "t", false, false, 0)
        .is_err());
    let mut restored = Vec::new();
    let mut lengths = Vec::new();
    while let Some(chunk) = upload.next_chunk().await.unwrap() {
        lengths.push(chunk.len());
        restored.extend(chunk);
    }
    assert_eq!(lengths, vec![UPLOAD_CHUNK_SIZE, UPLOAD_CHUNK_SIZE, 37]);
    assert_eq!(restored, bytes);
    assert!(upload.next_chunk().await.unwrap().is_none());
    assert_eq!(upload.single_shot_bytes().await.unwrap(), bytes);
}

#[tokio::test]
async fn upload_preflight_rejects_zero_and_bounds_single_shot_fallback() {
    let empty = Fixture::new(&[]);
    assert!(AttachmentUploadSource::open(&empty.0).await.is_err());
    let large = Fixture::new(&[0]);
    std::fs::OpenOptions::new()
        .write(true)
        .open(&large.0)
        .unwrap()
        .set_len(SINGLE_SHOT_UPLOAD_LIMIT + 1)
        .unwrap();
    let upload = AttachmentUploadSource::open(&large.0).await.unwrap();
    assert_eq!(upload.total_chunks(), 17);
    assert!(upload
        .single_shot_bytes()
        .await
        .unwrap_err()
        .to_string()
        .contains("16 MiB"));
    assert_eq!(
        upload.next_chunk().await.unwrap().unwrap().len(),
        UPLOAD_CHUNK_SIZE
    );
}

#[tokio::test]
async fn upload_detects_truncation_and_same_length_mutation() {
    for replacement in [b"short".as_slice(), b"different!".as_slice()] {
        let fixture = Fixture::new(b"original!!");
        let upload = AttachmentUploadSource::open(&fixture.0).await.unwrap();
        std::fs::write(&fixture.0, replacement).unwrap();
        assert!(upload.next_chunk().await.is_err());
        assert!(upload.single_shot_bytes().await.is_err());
    }
}

#[test]
fn client_id_wire_format_is_integer_and_lossless_beyond_js_integer_precision() {
    for id in [
        "0",
        "1793425680000",
        "9007199254740993",
        "18446744073709551615",
    ] {
        let value = client_id_wire_value(id).unwrap();
        assert!(value.is_u64());
        assert_eq!(serde_json::to_string(&value).unwrap(), id);
    }
    for id in ["", "-1", "1.5", "12x", "01", "18446744073709551616"] {
        assert!(client_id_wire_value(id).is_err());
    }
}

fn http_fixture(response: Vec<u8>) -> (String, std::thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/fixture", listener.local_addr().unwrap());
    let task = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(3)))
            .unwrap();
        let mut request = [0; 4096];
        let _ = stream.read(&mut request);
        let _ = stream.write_all(&response);
    });
    (url, task)
}

#[tokio::test]
async fn html_reader_bounds_content_length_and_chunked_bodies() {
    for chunked in [false, true] {
        let size = HTML_LIMIT + 1;
        let response = if chunked {
            let mut out = format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{size:x}\r\n").into_bytes();
            out.extend(vec![b'x'; size]);
            out.extend_from_slice(b"\r\n0\r\n\r\n");
            out
        } else {
            format!("HTTP/1.1 200 OK\r\nContent-Length: {size}\r\nConnection: close\r\n\r\n")
                .into_bytes()
        };
        let (url, task) = http_fixture(response);
        let response = reqwest::get(url).await.unwrap();
        assert_eq!(
            read_bounded_response(response, HTML_LIMIT)
                .await
                .unwrap_err(),
            MediaError::TooLarge
        );
        task.join().unwrap();
    }
}

#[tokio::test]
async fn opengraph_parses_bounded_html_and_rejects_oversize_before_allocation() {
    let body = b"<html><meta property='og:title' content='Fixture title'></html>";
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    let (url, task) = http_fixture(response);
    assert_eq!(
        scrape_opengraph_metadata(&url).await.unwrap().title,
        "Fixture title"
    );
    task.join().unwrap();
    let (url, task) = http_fixture(
        format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            HTML_LIMIT + 1
        )
        .into_bytes(),
    );
    assert!(scrape_opengraph_metadata(&url).await.is_none());
    task.join().unwrap();
}

#[tokio::test]
async fn exact_limits_are_accepted_and_image_headers_are_read_without_pixel_decode() {
    let fixture = Fixture::new(&[0]);
    std::fs::OpenOptions::new()
        .write(true)
        .open(&fixture.0)
        .unwrap()
        .set_len(SINGLE_SHOT_UPLOAD_LIMIT)
        .unwrap();
    let upload = AttachmentUploadSource::open(&fixture.0).await.unwrap();
    assert_eq!(
        upload.single_shot_bytes().await.unwrap().len() as u64,
        SINGLE_SHOT_UPLOAD_LIMIT
    );
    let mut body = b"<meta property='og:title' content='Exact limit'>".to_vec();
    body.resize(HTML_LIMIT, b' ');
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend(body);
    let (url, task) = http_fixture(response);
    assert_eq!(
        scrape_opengraph_metadata(&url).await.unwrap().title,
        "Exact limit"
    );
    task.join().unwrap();
    let image = Fixture::new(&[]);
    let png = image::RgbImage::new(17, 23);
    png.save_with_format(&image.0, image::ImageFormat::Png)
        .unwrap();
    assert_eq!(
        AttachmentUploadSource::open(&image.0)
            .await
            .unwrap()
            .dimensions,
        Some((17, 23))
    );
}
