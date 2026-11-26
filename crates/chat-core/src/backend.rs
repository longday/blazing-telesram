//! The seam between the sync engine and the network. Domain types only: the protocol crate
//! implements [`Backend`], tests implement it with a fake. The only writes are the user's own
//! actions listed on the trait: send, edit, delete, react, read position, typing and creating
//! a private chat.

use crate::model::{
    ChatId, ChatMembers, ChatSummary, HistoryChunk, MediaRef, Message, MessagePreview, MessageTs,
    PayloadId, Profile, ReactorsPage, ReplyTarget, SearchHits, UserId,
};
use crate::reactions::{ReactionCode, ReactionsUpdate};
use std::io::Write;
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
    /// A local write failed while saving a download (disk full, folder gone).
    #[error("storage error: {0}")]
    Storage(String),
}

/// Why creating a chat did not give a chat. A repeated creation can duplicate a chat, so the
/// two cases say whether the server may have acted. Strings never hold cookie or token values.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CreateError {
    /// Nothing was created: the request never left the client, or the server answered and
    /// refused. Repeating it is safe.
    #[error("{0}")]
    Failed(BackendError),
    /// The request was sent and its outcome is not known (timeout, dropped connection, HTTP
    /// 5xx, unreadable reply). The chat may exist; the caller must not retry by itself.
    #[error("outcome unknown: {0}")]
    Unknown(String),
}

/// What a subscription delivers. The backend owns reconnection of its live connection and
/// reports it with `Disconnected`/`Reconnected`; the engine reloads state on `Reconnected`.
#[derive(Clone, Debug, PartialEq)]
pub enum LiveEvent {
    /// New or edited message.
    MessageUpserted(Message),
    /// An own message that carries the send's `PayloadId`, from this or another client.
    Echo {
        payload: PayloadId,
        message: Message,
    },
    MessageDeleted {
        chat: ChatId,
        timestamp: MessageTs,
    },
    /// The server's reaction state of one message changed (a reaction push). Applied by the
    /// engine only when it is newer than what the cache holds.
    ReactionsChanged {
        chat: ChatId,
        timestamp: MessageTs,
        update: ReactionsUpdate,
    },
    /// `user` is typing in `chat`; the indicator lives `timeout` unless renewed.
    Typing {
        chat: ChatId,
        user: UserId,
        name: Option<String>,
        timeout: Duration,
    },
    /// Authoritative summary (unread counter, preview, pin, mute) of one chat.
    ChatChanged(ChatSummary),
    /// The pinned message of `chat` changed. `pinned` is `None` when nothing is pinned any more;
    /// `at` is the server's timestamp of the action when it carries one, for ordering against
    /// chat-list entries (`ChatSummary.pin_action_ts`).
    PinChanged {
        chat: ChatId,
        pinned: Option<MessageTs>,
        at: Option<i64>,
    },
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

    fn fetch_media(
        &self,
        media: &MediaRef,
    ) -> impl Future<Output = Result<Vec<u8>, BackendError>> + Send;

    /// Streams the original file `media` (kind `File`) into `out` without buffering it and
    /// returns the byte count. `progress(received, total)` is called as bytes arrive; `total`
    /// is the announced length when there is one. A failed write on `out` is `Storage`.
    fn download(
        &self,
        media: &MediaRef,
        out: &mut (dyn Write + Send),
        progress: &(dyn Fn(u64, Option<u64>) + Send + Sync),
    ) -> impl Future<Output = Result<u64, BackendError>> + Send;

    /// Opens the live stream. The channel closing means the connection is gone for good.
    fn subscribe(
        &self,
    ) -> impl Future<Output = Result<async_channel::Receiver<LiveEvent>, BackendError>> + Send;

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

    fn delete(
        &self,
        chat: &ChatId,
        timestamp: MessageTs,
    ) -> impl Future<Output = Result<(), BackendError>> + Send;

    /// Sets the account's reaction on a message to `target`, or removes it when `None`. One
    /// reaction per message: a `Some` replaces the previous one. Offline fails fast with
    /// `Network`.
    fn react(
        &self,
        chat: &ChatId,
        timestamp: MessageTs,
        target: Option<ReactionCode>,
    ) -> impl Future<Output = Result<(), BackendError>> + Send;

    /// Moves the read position of `chat` up to `timestamp` inclusive.
    fn mark_seen(
        &self,
        chat: &ChatId,
        timestamp: MessageTs,
    ) -> impl Future<Output = Result<(), BackendError>> + Send;

    fn typing(&self, chat: &ChatId) -> impl Future<Output = Result<(), BackendError>> + Send;

    /// Server search by name and text: at most `limit` hits per kind (chats, people,
    /// messages), first page only. Read-only; the query is typed text and must not be logged
    /// above `trace`.
    fn search(
        &self,
        query: &str,
        limit: usize,
    ) -> impl Future<Output = Result<SearchHits, BackendError>> + Send;

    /// The people of `chat`. A group or channel is read from the server (paged, bounded; the
    /// server refusing the list is `Rejected`, `permanent`); a private chat is answered from
    /// the row itself without a request. Read-only; nothing is persisted.
    fn members(
        &self,
        chat: &ChatSummary,
    ) -> impl Future<Output = Result<ChatMembers, BackendError>> + Send;

    /// The signed-in account's own profile. Read-only.
    fn profile(&self) -> impl Future<Output = Result<Profile, BackendError>> + Send;

    /// One page of the people who reacted to the message `(chat, timestamp)`: up to `limit`
    /// entries, those older than `before` when set, otherwise the first page, in server order.
    /// One call is one request; the backend never loops. `raw`, `oldest` and `skipped` pass
    /// through from the server's reply and the backend does not decide whether another page
    /// exists. `request` of the page is left 0: the engine copies the caller's token in.
    /// Read-only; nothing is persisted.
    fn reactors(
        &self,
        chat: &ChatId,
        timestamp: MessageTs,
        before: Option<MessageTs>,
        limit: usize,
    ) -> impl Future<Output = Result<ReactorsPage, BackendError>> + Send;

    /// Opens the private chat with `user`, creating it when missing: one request, never
    /// retried. Returns the chat id when the reply names one. A guard refusal (developer
    /// limit on writes) is `Failed(Rejected { permanent: true })` and sends nothing.
    fn create_private_chat(
        &self,
        user: &UserId,
    ) -> impl Future<Output = Result<Option<ChatId>, CreateError>> + Send;

    /// The one-line preview of the message `(chat, timestamp)`, or `Ok(None)` when the server
    /// no longer has it. One read request, never a push; offline fails fast with `Network`.
    /// Nothing is persisted.
    fn message_preview(
        &self,
        chat: &ChatId,
        timestamp: MessageTs,
    ) -> impl Future<Output = Result<Option<MessagePreview>, BackendError>> + Send;
}
