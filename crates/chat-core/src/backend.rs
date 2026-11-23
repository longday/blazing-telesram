//! The seam between the sync engine and the network. Domain types only: the protocol crate
//! implements [`Backend`], tests implement it with a fake. The only writes are the user's own
//! actions listed on the trait.

use crate::model::{ChatId, ChatSummary, HistoryChunk, MediaRef, Message, MessageTs, PayloadId, ReplyTarget, UserId};
use std::time::Duration;

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
    /// The item does not exist on the server (a media 404); asking again soon cannot help.
    #[error("not found")]
    NotFound,
    /// The server refused a write. `permanent` means repeating it cannot succeed (no
    /// permission, unknown chat); otherwise it may succeed later (throttling, overload).
    #[error("rejected: {reason}")]
    Rejected { reason: String, permanent: bool },
}

/// What a subscription delivers. The backend owns reconnection of its live connection and
/// reports it with `Disconnected`/`Reconnected`; the engine reloads state on `Reconnected`.
#[derive(Clone, Debug, PartialEq)]
pub enum LiveEvent {
    /// New or edited message.
    MessageUpserted(Message),
    /// An own message that carries the send's `PayloadId`, from this or another client.
    Echo { payload: PayloadId, message: Message },
    MessageDeleted { chat: ChatId, timestamp: MessageTs },
    /// `user` is typing in `chat`; the indicator lives `timeout` unless renewed.
    Typing { chat: ChatId, user: UserId, name: Option<String>, timeout: Duration },
    /// Authoritative summary (unread counter, preview, pin, mute) of one chat.
    ChatChanged(ChatSummary),
    Disconnected,
    Reconnected,
    /// The subscription cannot continue; `SessionExpired` ends the session, anything else
    /// makes the engine reconnect with backoff.
    Failed(BackendError),
}

/// Access to the account. Futures must be `Send`; implement with plain `async fn`.
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

    /// The chat the user has open, or none. The general live stream already carries new
    /// messages of every chat; per-chat updates are requested for this one only, because the
    /// server throttles per-chat subscriptions. Every new connection starts with none.
    fn watch(&self, chat: Option<&ChatId>);

    /// Sends text, optionally as a reply. Returns the server timestamp when the server reports
    /// one (a duplicate resend may not). Offline fails fast with `Network`.
    fn send_text(
        &self,
        chat: &ChatId,
        payload: &PayloadId,
        text: &str,
        reply_to: Option<&ReplyTarget>,
    ) -> impl Future<Output = Result<Option<MessageTs>, BackendError>> + Send;

    /// Rewrites the text of an own message. The server replaces the whole message, so a reply
    /// must repeat its target (`reply_to`) or it loses its quote.
    fn edit(
        &self,
        chat: &ChatId,
        timestamp: MessageTs,
        text: &str,
        reply_to: Option<&ReplyTarget>,
    ) -> impl Future<Output = Result<(), BackendError>> + Send;

    fn delete(&self, chat: &ChatId, timestamp: MessageTs) -> impl Future<Output = Result<(), BackendError>> + Send;

    /// Moves the read position of `chat` up to `timestamp` inclusive.
    fn mark_seen(&self, chat: &ChatId, timestamp: MessageTs) -> impl Future<Output = Result<(), BackendError>> + Send;

    fn typing(&self, chat: &ChatId) -> impl Future<Output = Result<(), BackendError>> + Send;
}
