use crate::bootstrap::BootstrapError;

/// Failure classes the core reacts to differently: sign in again, retry later, or report a bug.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ProtoError {
    #[error("the server rejected the session")]
    SessionExpired,
    #[error("network error: {0}")]
    Network(String),
    #[error("unexpected server response: {0}")]
    Protocol(String),
    /// HTTP 404 for media: the server has no such file.
    #[error("not found")]
    NotFound,
    /// A `push` the server answered with a refusing commit status. `permanent` means the same
    /// request will keep failing (no permission, unknown chat); otherwise it may succeed later.
    #[error("push not committed: status {status} {name}")]
    Commit { status: i64, name: &'static str, permanent: bool },
}

impl From<BootstrapError> for ProtoError {
    fn from(e: BootstrapError) -> Self {
        match e {
            BootstrapError::SessionRejected => Self::SessionExpired,
            BootstrapError::BadCsrf => Self::Protocol("the server rejected the CSRF token".into()),
            BootstrapError::Protocol(m) => Self::Protocol(m),
            BootstrapError::Network(m) => Self::Network(m),
        }
    }
}

impl From<reqwest::Error> for ProtoError {
    fn from(e: reqwest::Error) -> Self {
        // The URL may carry a user id or file id; keep it out of messages.
        Self::Network(crate::net::describe(&e.without_url()))
    }
}
