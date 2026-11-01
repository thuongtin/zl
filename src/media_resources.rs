//! Account-isolated media resources, bounded readers, and cancellation tickets.
use sha2::{Digest, Sha256};
use std::io::Write;
use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

pub const PREVIEW_LIMIT: usize = 16 * 1024 * 1024;
pub const HTML_LIMIT: usize = 128 * 1024;
pub const TEXTURE_BUDGET: u64 = 128 * 1024 * 1024;
pub const DISK_BUDGET: u64 = 512 * 1024 * 1024;
pub const ACTIVE_JOBS: usize = 4;
pub const PENDING_JOBS: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum MediaQuality {
    Thumbnail,
    Preview,
    Original,
}
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct MediaKey {
    pub account: String,
    pub thread: String,
    pub message: String,
    pub quality: MediaQuality,
}
impl MediaKey {
    pub fn cache_id(&self) -> String {
        let mut hash = Sha256::new();
        for value in [&self.account, &self.thread, &self.message] {
            hash.update((value.len() as u64).to_le_bytes());
            hash.update(value.as_bytes());
        }
        hash.update([match self.quality {
            MediaQuality::Thumbnail => 0,
            MediaQuality::Preview => 1,
            MediaQuality::Original => 2,
        }]);
        format!("{:x}", hash.finalize())
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaError {
    Timeout,
    Offline,
    NotFound,
    Http(u16),
    TooLarge,
    Cancelled,
    Decoder,
    Io,
    AccountMismatch,
}
impl std::fmt::Display for MediaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for MediaError {}
fn network_error(error: reqwest::Error) -> MediaError {
    if error.is_timeout() {
        MediaError::Timeout
    } else {
        MediaError::Offline
    }
}
#[derive(Clone, Default)]
pub struct RequestGeneration(Arc<AtomicU64>);
impl RequestGeneration {
    pub fn capture(&self) -> u64 {
        self.0.load(Ordering::Acquire)
    }
    pub fn cancel(&self) {
        self.0.fetch_add(1, Ordering::AcqRel);
    }
    pub fn is_current(&self, ticket: u64) -> bool {
        self.capture() == ticket
    }
    pub fn check(&self, ticket: u64) -> Result<(), MediaError> {
        if self.is_current(ticket) {
            Ok(())
        } else {
            Err(MediaError::Cancelled)
        }
    }
}
fn check_response(response: &reqwest::Response) -> Result<(), MediaError> {
    match response.status().as_u16() {
        200..=299 => Ok(()),
        404 => Err(MediaError::NotFound),
        code => Err(MediaError::Http(code)),
    }
}
pub async fn read_bounded_response(
    mut response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, MediaError> {
    check_response(&response)?;
    if response.content_length().is_some_and(|n| n > limit as u64) {
        return Err(MediaError::TooLarge);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(network_error)? {
        if chunk.len() > limit.saturating_sub(bytes.len()) {
            return Err(MediaError::TooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
pub async fn fetch_bounded(
    client: &reqwest::Client,
    url: &str,
    limit: usize,
    generation: &RequestGeneration,
    ticket: u64,
) -> Result<Vec<u8>, MediaError> {
    generation.check(ticket)?;
    let operation = async {
        read_bounded_response(
            client
                .get(url)
                .timeout(Duration::from_secs(20))
                .send()
                .await
                .map_err(network_error)?,
            limit,
        )
        .await
    };
    let bytes = tokio::time::timeout(Duration::from_secs(30), operation)
        .await
        .map_err(|_| MediaError::Timeout)??;
    generation.check(ticket)?;
    Ok(bytes)
}

/// An exclusive private file that removes its partial data on every uncommitted drop.
pub struct AtomicPrivateFile {
    partial: PathBuf,
    destination: PathBuf,
    file: Option<std::fs::File>,
    committed: bool,
}
impl AtomicPrivateFile {
    pub fn create(destination: &Path, suffix: &str) -> Result<Self, MediaError> {
        if suffix.is_empty()
            || suffix.len() > 96
            || !suffix
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err(MediaError::Io);
        }
        let partial = destination.with_extension(format!("{suffix}.privatepart"));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&partial).map_err(|_| MediaError::Io)?;
        Ok(Self {
            partial,
            destination: destination.to_owned(),
            file: Some(file),
            committed: false,
        })
    }
    pub fn write_all(&mut self, bytes: &[u8]) -> Result<(), MediaError> {
        self.file
            .as_mut()
            .ok_or(MediaError::Io)?
            .write_all(bytes)
            .map_err(|_| MediaError::Io)
    }
    pub fn commit(mut self) -> Result<(), MediaError> {
        self.file
            .as_ref()
            .ok_or(MediaError::Io)?
            .sync_all()
            .map_err(|_| MediaError::Io)?;
        drop(self.file.take());
        std::fs::rename(&self.partial, &self.destination).map_err(|_| MediaError::Io)?;
        self.committed = true;
        Ok(())
    }
}
impl Drop for AtomicPrivateFile {
    fn drop(&mut self) {
        drop(self.file.take());
        if !self.committed {
            let _ = std::fs::remove_file(&self.partial);
        }
    }
}

/// Stream an original to a private cache path, never directly over a user-saved original.
/// Dropping or aborting this future removes its owned partial file.
pub async fn stream_original(
    client: &reqwest::Client,
    url: &str,
    destination: &Path,
    generation: &RequestGeneration,
    ticket: u64,
) -> Result<u64, MediaError> {
    generation.check(ticket)?;
    let operation = async {
        let mut response = client
            .get(url)
            .timeout(Duration::from_secs(120))
            .send()
            .await
            .map_err(network_error)?;
        check_response(&response)?;
        if response
            .content_length()
            .is_some_and(|size| size > DISK_BUDGET)
        {
            return Err(MediaError::TooLarge);
        }
        let mut file = AtomicPrivateFile::create(destination, &ticket.to_string())?;
        let mut total = 0u64;
        while let Some(chunk) = response.chunk().await.map_err(network_error)? {
            generation.check(ticket)?;
            if chunk.len() as u64 > DISK_BUDGET.saturating_sub(total) {
                return Err(MediaError::TooLarge);
            }
            file.write_all(&chunk)?;
            total += chunk.len() as u64;
        }
        generation.check(ticket)?;
        file.commit()?;
        Ok(total)
    };
    tokio::time::timeout(Duration::from_secs(125), operation)
        .await
        .unwrap_or(Err(MediaError::Timeout))
}

pub struct PrivateMediaCache {
    directory: PathBuf,
    account: String,
}
impl PrivateMediaCache {
    pub fn new(root: &Path, account: &str) -> Result<Self, MediaError> {
        if account.is_empty() {
            return Err(MediaError::AccountMismatch);
        }
        let directory = root.join(format!("{:x}", Sha256::digest(account.as_bytes())));
        std::fs::create_dir_all(&directory).map_err(|_| MediaError::Io)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
                .map_err(|_| MediaError::Io)?;
        }
        Ok(Self {
            directory,
            account: account.to_owned(),
        })
    }
    pub fn path_for(&self, key: &MediaKey) -> Result<PathBuf, MediaError> {
        if key.account != self.account {
            return Err(MediaError::AccountMismatch);
        }
        Ok(self.directory.join(format!("{}.media", key.cache_id())))
    }
    pub fn touch(&self, key: &MediaKey) -> Result<(), MediaError> {
        let path = self.path_for(key)?;
        let file = std::fs::File::open(path).map_err(|_| MediaError::Io)?;
        file.set_times(std::fs::FileTimes::new().set_modified(std::time::SystemTime::now()))
            .map_err(|_| MediaError::Io)
    }
    /// Only cache-owned regular .media files are candidates; saved originals are not tracked.
    pub fn prune(&self) -> Result<u64, MediaError> {
        let mut entries = Vec::new();
        let mut total = 0u64;
        for entry in std::fs::read_dir(&self.directory).map_err(|_| MediaError::Io)? {
            let entry = entry.map_err(|_| MediaError::Io)?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.len() != 70
                || !name.ends_with(".media")
                || !name[..64].bytes().all(|b| b.is_ascii_hexdigit())
            {
                continue;
            }
            let meta = entry
                .path()
                .symlink_metadata()
                .map_err(|_| MediaError::Io)?;
            if !meta.is_file() {
                continue;
            }
            total += meta.len();
            entries.push((
                meta.modified().unwrap_or(std::time::UNIX_EPOCH),
                entry.path(),
                meta.len(),
            ));
        }
        entries.sort_by_key(|entry| entry.0);
        for (_, path, size) in entries {
            if total <= DISK_BUDGET {
                break;
            }
            std::fs::remove_file(path).map_err(|_| MediaError::Io)?;
            total -= size;
        }
        Ok(total)
    }
}
/// The caller owns actual texture handles and removes returned evictions.
pub struct ResourceLru {
    budget: u64,
    used: u64,
    sizes: HashMap<String, u64>,
    order: VecDeque<String>,
}
impl ResourceLru {
    pub fn new(budget: u64) -> Self {
        Self {
            budget,
            used: 0,
            sizes: HashMap::new(),
            order: VecDeque::new(),
        }
    }
    pub fn used(&self) -> u64 {
        self.used
    }
    pub fn touch(&mut self, key: &str) {
        if self.sizes.contains_key(key) {
            self.order.retain(|k| k != key);
            self.order.push_back(key.to_owned());
        }
    }
    pub fn remove(&mut self, key: &str) {
        if let Some(size) = self.sizes.remove(key) {
            self.used -= size;
        }
        self.order.retain(|k| k != key);
    }
    pub fn insert(&mut self, key: String, bytes: u64) -> Vec<String> {
        self.remove(&key);
        self.used = self.used.saturating_add(bytes);
        self.sizes.insert(key.clone(), bytes);
        self.order.push_back(key);
        let mut evicted = Vec::new();
        while self.used > self.budget {
            if let Some(key) = self.order.pop_front() {
                if let Some(size) = self.sizes.remove(&key) {
                    self.used -= size;
                }
                evicted.push(key);
            } else {
                break;
            }
        }
        evicted
    }
}
/// Admission control, independent of any GUI runtime. Dropping a permit releases capacity.
#[derive(Clone)]
pub struct MediaJobQueue {
    pending: Arc<tokio::sync::Semaphore>,
    active: Arc<tokio::sync::Semaphore>,
}
pub struct MediaJobPermit {
    _pending: tokio::sync::OwnedSemaphorePermit,
    _active: tokio::sync::OwnedSemaphorePermit,
}
impl Default for MediaJobQueue {
    fn default() -> Self {
        Self {
            pending: Arc::new(tokio::sync::Semaphore::new(PENDING_JOBS + ACTIVE_JOBS)),
            active: Arc::new(tokio::sync::Semaphore::new(ACTIVE_JOBS)),
        }
    }
}
pub struct MediaJobAdmission {
    pending: tokio::sync::OwnedSemaphorePermit,
    active: Arc<tokio::sync::Semaphore>,
}
impl MediaJobAdmission {
    pub async fn start(self) -> Option<MediaJobPermit> {
        let active = self.active.acquire_owned().await.ok()?;
        Some(MediaJobPermit {
            _pending: self.pending,
            _active: active,
        })
    }
}
impl MediaJobQueue {
    /// Reserve synchronously before spawning. At most 132 tasks may be admitted,
    /// of which at most four run and at most 128 wait for an active permit.
    pub fn try_admit(&self) -> Option<MediaJobAdmission> {
        Some(MediaJobAdmission {
            pending: self.pending.clone().try_acquire_owned().ok()?,
            active: self.active.clone(),
        })
    }
    pub async fn acquire(&self) -> Option<MediaJobPermit> {
        self.try_admit()?.start().await
    }
}

#[cfg(test)]
mod admission_and_drop_tests {
    use super::*;
    #[test]
    fn admission_is_bounded_before_any_task_is_spawned() {
        let queue = MediaJobQueue::default();
        let slots: Vec<_> = (0..ACTIVE_JOBS + PENDING_JOBS)
            .map(|_| queue.try_admit().unwrap())
            .collect();
        assert!(queue.try_admit().is_none());
        drop(slots);
        assert!(queue.try_admit().is_some());
    }
    #[tokio::test]
    async fn dropping_inflight_future_cleans_partial_without_destination() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "zl-private-drop-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let destination = root.join("original.media");
        let partial = destination.with_extension("drop.privatepart");
        let operation = async {
            let mut file = AtomicPrivateFile::create(&destination, "drop").unwrap();
            file.write_all(b"synthetic-partial").unwrap();
            std::future::pending::<()>().await;
        };
        assert!(tokio::time::timeout(Duration::from_millis(5), operation)
            .await
            .is_err());
        assert!(!partial.exists());
        assert!(!destination.exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
