//! The seam between the sync engine and the network. Domain types only: the protocol crate
//! implements [`Backend`], tests implement it with a fake. Nothing here can mutate the account.

use crate::model::{ChatId, ChatSummary, HistoryChunk, MediaRef, Message, MessageTs, UserId};

/// Error strings must never contain cookie or token values.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum BackendError {
    /// The server rejected the stored session; retrying cannot help.
    #[error("session expired")]
    SessionExpired,
    #[error("network error: {0}")]
    Network(String),
    #[error("protocol error: {0}")]
    Protocol(String),
}

/// What a subscription delivers. The backend owns reconnection of its live connection and
/// reports it with `Disconnected`/`Reconnected`; the engine reloads state on `Reconnected`.
#[derive(Clone, Debug, PartialEq)]
pub enum LiveEvent {
    /// New or edited message.
    MessageUpserted(Message),
    MessageDeleted { chat: ChatId, timestamp: MessageTs },
    /// Authoritative summary (unread counter, preview, pin, mute) of one chat.
    ChatChanged(ChatSummary),
    Disconnected,
    Reconnected,
    /// The subscription cannot continue; `SessionExpired` ends the session, anything else
    /// makes the engine reconnect with backoff.
    Failed(BackendError),
}

/// Read-only access to the account. Futures must be `Send`; implement with plain `async fn`.
pub trait Backend: Send + Sync + 'static {
    /// Validates the session and returns the account id (the cache key).
    fn bootstrap(&self) -> impl Future<Output = Result<UserId, BackendError>> + Send;

    /// Every visible chat, hidden namespaces already excluded.
    fn chat_list(&self) -> impl Future<Output = Result<Vec<ChatSummary>, BackendError>> + Send;

    /// Up to `limit` messages strictly older than `before` (the newest when `None`), oldest
    /// first. `reached_start` is true when no older message exists.
    fn history(
        &self,
        chat: &ChatId,
        before: Option<MessageTs>,
        limit: usize,
    ) -> impl Future<Output = Result<HistoryChunk, BackendError>> + Send;

    fn fetch_media(&self, media: &MediaRef) -> impl Future<Output = Result<Vec<u8>, BackendError>> + Send;

    /// Opens the live stream. The channel closing means the connection is gone for good.
    fn subscribe(&self) -> impl Future<Output = Result<async_channel::Receiver<LiveEvent>, BackendError>> + Send;
}
