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
