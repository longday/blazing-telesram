//! Domain model shared by the sync core and the UI. Keys are server identifiers.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
    Connection(ConnectionState),
    SessionExpired,
    SignedOut,
}

/// UI to core. Read-only by design: nothing here mutates the account.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    OpenChat(ChatId),
    LoadOlder { chat: ChatId, before: MessageTs },
    FetchMedia(MediaRef),
    SignOut,
}
