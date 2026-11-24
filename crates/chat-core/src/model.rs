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

impl MediaRef {
    /// The handle of the original file behind a preview: same id, kind `File`. Originals go
    /// through `Command::Download`, never through `FetchMedia`.
    pub fn original(&self) -> MediaRef {
        MediaRef { id: self.id.clone(), kind: MediaKind::File }
    }
}

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

/// One picture of a message: its preview handle (kind `Thumbnail`), pixel size and file name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GalleryItem { pub media: MediaRef, pub width: u32, pub height: u32, pub name: Option<String> }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum MessageBody {
    Text(String),
    /// `name` is absent in rows cached before downloads existed.
    Image { media: MediaRef, width: u32, height: u32, caption: Option<String>, #[serde(default)] name: Option<String> },
    /// Several pictures sent as one message.
    Gallery { items: Vec<GalleryItem>, caption: Option<String> },
    File { media: MediaRef, name: String, size: u64 },
    Deleted,
    /// A message type this client cannot render yet; holds the server type name.
    Unsupported(String),
}

impl MessageBody {
    /// The pictures of the body in order: one for an image, all for a gallery, none otherwise.
    pub fn gallery_items(&self) -> Vec<GalleryItem> {
        match self {
            MessageBody::Image { media, width, height, name, .. } => {
                vec![GalleryItem { media: media.clone(), width: *width, height: *height, name: name.clone() }]
            }
            MessageBody::Gallery { items, .. } => items.clone(),
            _ => Vec::new(),
        }
    }
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
    /// `PayloadId`s carried by own messages of this chunk; a matching outbox entry was
    /// delivered even if its confirmation and echo were lost.
    #[serde(default)]
    pub payloads: Vec<PayloadId>,
}

/// Why a download did not produce a file; closed on purpose, the UI words each case once.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DownloadFailure {
    /// Not online: downloads are refused, not queued.
    Offline,
    /// The server no longer has the file.
    NotFound,
    /// There is no usable Downloads folder.
    Destination,
    /// A local write failed.
    Disk,
    Network,
    Other,
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
    /// A send was not accepted into durable storage; nothing was queued. The UI puts the
    /// text and reply target back so the user can try again.
    SendRefused { draft: Draft, reason: String },
    /// `received` bytes of `total` (when announced) of the original `media` are on disk.
    DownloadProgress { media: MediaRef, received: u64, total: Option<u64> },
    /// The original `media` was saved at `path` (the final name, never an overwrite).
    Downloaded { media: MediaRef, path: PathBuf },
    DownloadFailed { media: MediaRef, reason: DownloadFailure },
    /// A `CancelDownload` took effect; nothing was left on disk.
    DownloadCancelled(MediaRef),
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
    /// Save the original `media` (kind `File`) into the Downloads folder under `name`
    /// (sanitized by the core); the user asked for it explicitly.
    Download { media: MediaRef, name: Option<String> },
    CancelDownload(MediaRef),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thumb(id: &str) -> MediaRef {
        MediaRef { id: id.into(), kind: MediaKind::Thumbnail }
    }

    fn item(id: &str, name: Option<&str>) -> GalleryItem {
        GalleryItem { media: thumb(id), width: 4, height: 3, name: name.map(Into::into) }
    }

    fn image() -> MessageBody {
        MessageBody::Image { media: thumb("b/1"), width: 4, height: 3, caption: Some("c".into()), name: Some("a.jpg".into()) }
    }

    #[test]
    fn original_keeps_the_id_and_switches_to_the_file_kind() {
        assert_eq!(thumb("b/1").original(), MediaRef { id: "b/1".into(), kind: MediaKind::File });
    }

    #[test]
    fn gallery_items_lists_the_pictures_of_a_body_in_order() {
        assert_eq!(image().gallery_items(), [item("b/1", Some("a.jpg"))]);
        let gallery = MessageBody::Gallery { items: vec![item("b/2", None), item("b/1", Some("a.jpg"))], caption: None };
        assert_eq!(gallery.gallery_items(), [item("b/2", None), item("b/1", Some("a.jpg"))]);
        assert!(MessageBody::Text("x".into()).gallery_items().is_empty());
        assert!(MessageBody::Deleted.gallery_items().is_empty());
        let file = MessageBody::File { media: thumb("b/3").original(), name: "f".into(), size: 1 };
        assert!(file.gallery_items().is_empty());
    }

    #[test]
    fn image_and_gallery_survive_a_json_round_trip() {
        let gallery = MessageBody::Gallery { items: vec![item("b/1", Some("a.jpg")), item("b/2", None)], caption: Some("c".into()) };
        for body in [image(), gallery] {
            let json = serde_json::to_string(&body).unwrap();
            assert_eq!(serde_json::from_str::<MessageBody>(&json).unwrap(), body);
        }
    }

    #[test]
    fn an_image_cached_before_names_existed_still_decodes() {
        let old = r#"{"Image":{"media":{"id":"b/1","kind":"Thumbnail"},"width":4,"height":3,"caption":null}}"#;
        let MessageBody::Image { name, width, .. } = serde_json::from_str(old).unwrap() else { panic!("image") };
        assert_eq!((name, width), (None, 4));
    }
}
