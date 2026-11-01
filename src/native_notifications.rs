//! Native notification bridge. Account, mute, setting, and generation gates belong to the caller.
use sha2::{Digest, Sha256};
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthorizationStatus {
    NotDetermined,
    Denied,
    Authorized,
    Provisional,
    Ephemeral,
    Unavailable,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryStatus {
    Submitted,
    NotAuthorized,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotificationError {
    ExplicitUserActionRequired,
    HelperUnavailable,
    Timeout,
    InvalidResponse,
    Io,
}
impl std::fmt::Display for NotificationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for NotificationError {}
/// An immutable opaque identity. Raw account/message identifiers never leave this process.
#[derive(Clone)]
pub struct NotificationContext {
    identifier: String,
}
impl NotificationContext {
    pub fn new(opaque_context: &str) -> Self {
        Self {
            identifier: format!("{:x}", Sha256::digest(opaque_context.as_bytes())),
        }
    }
}
fn helper_path() -> PathBuf {
    let packaged = std::env::current_exe().ok().and_then(|path| {
        Some(
            path.parent()?
                .parent()?
                .join("Helpers/DailyNotifier.app/Contents/MacOS/DailyNotifier"),
        )
    });
    packaged.filter(|path| path.is_file()).unwrap_or_else(|| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("native/macos/build/DailyNotifier.app/Contents/MacOS/DailyNotifier")
    })
}
async fn invoke(
    command: &str,
    payload: Option<Vec<u8>>,
    seconds: u64,
) -> Result<String, NotificationError> {
    if !cfg!(target_os = "macos") {
        return Err(NotificationError::HelperUnavailable);
    }
    let mut child = tokio::process::Command::new(helper_path())
        .arg(command)
        .stdin(if payload.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| NotificationError::HelperUnavailable)?;
    let operation = async {
        if let Some(payload) = payload {
            let mut input = child.stdin.take().ok_or(NotificationError::Io)?;
            input
                .write_all(&payload)
                .await
                .map_err(|_| NotificationError::Io)?;
            input.shutdown().await.map_err(|_| NotificationError::Io)?;
        }
        let mut output = Vec::new();
        child
            .stdout
            .take()
            .ok_or(NotificationError::Io)?
            .take(4097)
            .read_to_end(&mut output)
            .await
            .map_err(|_| NotificationError::Io)?;
        if output.len() > 4096 {
            return Err(NotificationError::InvalidResponse);
        }
        if !child
            .wait()
            .await
            .map_err(|_| NotificationError::Io)?
            .success()
        {
            return Err(NotificationError::InvalidResponse);
        }
        String::from_utf8(output).map_err(|_| NotificationError::InvalidResponse)
    };
    tokio::time::timeout(Duration::from_secs(seconds), operation)
        .await
        .map_err(|_| NotificationError::Timeout)?
}
fn parse_status(output: &str) -> Result<AuthorizationStatus, NotificationError> {
    match output.trim() {
        "notDetermined" => Ok(AuthorizationStatus::NotDetermined),
        "denied" => Ok(AuthorizationStatus::Denied),
        "authorized" => Ok(AuthorizationStatus::Authorized),
        "provisional" => Ok(AuthorizationStatus::Provisional),
        "ephemeral" => Ok(AuthorizationStatus::Ephemeral),
        "unavailable" => Ok(AuthorizationStatus::Unavailable),
        _ => Err(NotificationError::InvalidResponse),
    }
}
/// Read authorization without asking for permission or posting a notification.
pub async fn status() -> Result<AuthorizationStatus, NotificationError> {
    parse_status(&invoke("--status", None, 10).await?)
}
/// This is the only API that can trigger the OS permission dialog.
pub async fn request_if_user_action(
    explicit_user_action: bool,
) -> Result<AuthorizationStatus, NotificationError> {
    if !explicit_user_action {
        return Err(NotificationError::ExplicitUserActionRequired);
    }
    parse_status(&invoke("--request", None, 120).await?)
}
/// Call only after the parent checks current context, enabled setting, background state, and mute.
/// The helper independently checks authorization and never requests it here.
pub async fn notify_background(
    context: &NotificationContext,
    sound: bool,
) -> Result<DeliveryStatus, NotificationError> {
    let payload=serde_json::to_vec(&serde_json::json!({"title":"Zalo","body":"You have a new message.","id":context.identifier,"sound":sound})).map_err(|_|NotificationError::InvalidResponse)?;
    match invoke("--notify", Some(payload), 10).await?.trim() {
        "submitted" => Ok(DeliveryStatus::Submitted),
        "notAuthorized" => Ok(DeliveryStatus::NotAuthorized),
        _ => Err(NotificationError::InvalidResponse),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn context_is_stable_and_opaque() {
        let a = NotificationContext::new("synthetic-account/thread/message");
        assert_eq!(a.identifier.len(), 64);
        assert_eq!(
            a.identifier,
            NotificationContext::new("synthetic-account/thread/message").identifier
        );
        assert_ne!(a.identifier, NotificationContext::new("other").identifier);
    }
    #[test]
    fn strict_status_parser() {
        assert_eq!(
            parse_status("authorized\n"),
            Ok(AuthorizationStatus::Authorized)
        );
        assert_eq!(
            parse_status("anything"),
            Err(NotificationError::InvalidResponse)
        );
    }
    #[tokio::test]
    async fn permission_requires_explicit_action() {
        assert_eq!(
            request_if_user_action(false).await,
            Err(NotificationError::ExplicitUserActionRequired)
        );
    }
}
