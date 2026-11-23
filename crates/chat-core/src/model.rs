//! Domain model shared by the sync core and the UI. Keys are server identifiers.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Instant;

/// Server chat id, e.g. `0/0/<guid>` or `1/0/<uuid>`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ChatId(pub String);

/// User guid.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UserId(pub String);

/// Server message timestamp in microseconds; unique and ordered within a chat.
pub type MessageTs = i64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChatKind { Private, Group, Channel }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MediaKind { Avatar, Thumbnail, File }

/// Remote media reference; the client downloads it into the runtime cache on demand.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MediaRef { pub id: String, pub kind: MediaKind }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MessagePreview { pub author: Option<String>, pub text: String, pub timestamp: MessageTs }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChatSummary {
    pub id: ChatId,
    pub kind: ChatKind,
    pub title: String,
    pub avatar: Option<MediaRef>,
    pub last_message: Option<MessagePreview>,
    pub unread: u32,
    pub muted: bool,
    /// Lower value sorts first; `None` means not pinned.
    pub pinned_rank: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReplyQuote { pub author: Option<String>, pub text: String, pub timestamp: Option<MessageTs> }

/// The message a draft or send answers: a loaded message of the same chat.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReplyTarget { pub timestamp: MessageTs, pub author: Option<String>, pub text: String }

/// Client-generated id of one send, fixed before the first attempt; the server answers a
/// resend with the same id as a duplicate instead of storing a second copy.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PayloadId(pub String);

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Delivery {
    #[default]
    Sent,
    /// In the outbox; `Message::timestamp` is the local creation time until the server's
    /// copy replaces it.
    Pending(PayloadId),
    /// Rejected for good; waits for `Command::Retry`.
    Failed { payload: PayloadId, reason: String },
}

/// Unsent composer state of one chat. Empty text with no reply means no draft.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Draft { pub chat: ChatId, pub text: String, pub reply_to: Option<ReplyTarget> }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum MessageBody {
    Text(String),
    Image { media: MediaRef, width: u32, height: u32, caption: Option<String> },
    File { media: MediaRef, name: String, size: u64 },
    Deleted,
    /// A message type this client cannot render yet; holds the server type name.
    Unsupported(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub chat: ChatId,
    pub timestamp: MessageTs,
    pub author: UserId,
    pub author_name: Option<String>,
    pub author_avatar: Option<MediaRef>,
    pub outgoing: bool,
    pub body: MessageBody,
    pub edited: bool,
    pub reply_to: Option<ReplyQuote>,
    pub forwarded_from: Option<String>,
    #[serde(default)]
    pub delivery: Delivery,
}

/// Messages ordered oldest to newest, all older than `before` when paging. `replaces` is the
/// inclusive timestamp range (`None` bound = open) this chunk authoritatively replaced in the
/// cache; without it a chunk only vouches for the span of its own messages.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HistoryChunk {
    pub chat: ChatId,
    pub messages: Vec<Message>,
    pub reached_start: bool,
    pub replaces: Option<(Option<MessageTs>, Option<MessageTs>)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionState { Connecting, Online, Offline }

/// Core to UI.
#[derive(Clone, Debug, PartialEq)]
pub enum ClientEvent {
    ChatsReplaced(Vec<ChatSummary>),
    ChatUpdated(ChatSummary),
    History(HistoryChunk),
    MessageUpserted(Message),
    MessageDeleted { chat: ChatId, timestamp: MessageTs },
    MediaReady { media: MediaRef, path: PathBuf },
    /// Not available now (failed, or missing on the server recently); the UI may ask again later.
    MediaUnavailable(MediaRef),
    Connection(ConnectionState),
    SessionExpired,
    SignedOut,
    /// Persisted drafts, once at startup.
    Drafts(Vec<Draft>),
    /// The pending message `payload` was settled; drop it before the server copy arrives as
    /// `MessageUpserted`.
    PendingSettled { chat: ChatId, payload: PayloadId },
    /// `user` is typing in `chat` until `until` unless their message arrives first.
    Typing { chat: ChatId, user: UserId, name: Option<String>, until: Instant },
    /// An edit or deletion did not happen; the message keeps its state.
    ActionFailed { chat: ChatId, reason: String },
}

/// UI to core. Writes are only the user's own actions: send, edit, delete, read, typing.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    OpenChat(ChatId),
    LoadOlder { chat: ChatId, before: MessageTs },
    FetchMedia(MediaRef),
    SignOut,
    Send { chat: ChatId, text: String, reply_to: Option<ReplyTarget> },
    Retry(PayloadId),
    Edit { chat: ChatId, timestamp: MessageTs, text: String },
    Delete { chat: ChatId, timestamp: MessageTs },
    /// The newest message of `chat` the user has seen in the focused window.
    MarkRead { chat: ChatId, timestamp: MessageTs },
    /// The user changed the composer text of `chat`.
    Typing(ChatId),
    SaveDraft(Draft),
}
