//! Domain model shared by the sync core and the UI. Keys are server identifiers.

use crate::reactions::{ReactionCode, Reactions};
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
    /// The other person of a private chat; `None` for groups and channels. Absent in rows
    /// cached before the field existed, which load as `None` until the next list refresh.
    #[serde(default)]
    pub partner: Option<UserId>,
}

/// What a member may do in a chat. The server's `role` `"admin"` is `Admin`; every other value,
/// or none, is `Member`. The order of a member list is not relied on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemberRole {
    Member,
    Admin,
}

/// One person of a chat, as the server lists them. `me` is set by the backend, which knows the
/// account's own guid.
#[derive(Clone, Debug, PartialEq)]
pub struct Member {
    pub user: UserId,
    pub name: Option<String>,
    pub avatar: Option<MediaRef>,
    pub role: MemberRole,
    pub me: bool,
}

/// The members of one chat. The server sends no total and no "more" flag, so `partial` is set
/// when the paging bound was reached and more members may exist. Held in memory only.
#[derive(Clone, Debug, PartialEq)]
pub struct ChatMembers {
    pub chat: ChatId,
    pub users: Vec<Member>,
    pub partial: bool,
}

/// The signed-in account as `whoami` reports it. `status_text` is set only when the account has
/// a non-blank status text; no availability is inferred.
#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    pub user: UserId,
    pub name: Option<String>,
    pub avatar: Option<MediaRef>,
    pub status_text: Option<String>,
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
    /// Server-confirmed reactions; rows cached before reactions existed have none.
    #[serde(default)]
    pub reactions: Reactions,
}

impl Message {
    /// Keeps the reactions already known for this message when the incoming copy carries no
    /// reaction state at all (an edit push, a history row without reactions).
    pub fn inherit_reactions(&mut self, cached: &Message) {
        if self.reactions.counts.is_empty() && self.reactions.version.is_none() {
            self.reactions.clone_from(&cached.reactions);
        }
    }
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

/// Identifies one search request. Chosen by the store, increasing; a reply whose id is not
/// the latest issued one is stale.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SearchId(pub u64);

/// A chat found by name.
#[derive(Clone, Debug, PartialEq)]
pub struct ChatHit { pub id: ChatId, pub kind: ChatKind, pub title: String, pub avatar: Option<MediaRef> }

/// A person found by name. `chat` is the existing private chat with them, when known.
#[derive(Clone, Debug, PartialEq)]
pub struct PersonHit { pub user: UserId, pub name: String, pub avatar: Option<MediaRef>, pub chat: Option<ChatId> }

/// A message found by text.
#[derive(Clone, Debug, PartialEq)]
pub struct MessageHit {
    pub chat: ChatId,
    pub chat_title: String,
    pub timestamp: MessageTs,
    pub author_name: Option<String>,
    pub text: String,
}

/// The first page of server results per kind. Held in memory only, never cached.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SearchHits { pub chats: Vec<ChatHit>, pub people: Vec<PersonHit>, pub messages: Vec<MessageHit> }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchFailure {
    /// No connection; the request was not made.
    Offline,
    /// The request failed; local filtering is unaffected.
    Failed,
}

/// Core to UI.
#[derive(Clone, Debug, PartialEq)]
pub enum ClientEvent {
    ChatsReplaced(Vec<ChatSummary>),
    ChatUpdated(ChatSummary),
    History(HistoryChunk),
    MessageUpserted(Message),
    /// The reactions of a loaded message changed (a server update, or the optimistic overlay
    /// of the user's own choice and its rollback). Carries no message body on purpose: it
    /// must not look like a new or edited message.
    ReactionsChanged { chat: ChatId, timestamp: MessageTs, reactions: Reactions },
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
    /// A reaction, a deletion or an edit did not happen; the message keeps its state.
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
    /// Server results for the query `id` asked for.
    SearchResults { id: SearchId, hits: SearchHits },
    /// The search `id` asked for produced no results.
    SearchFailed { id: SearchId, reason: SearchFailure },
    /// The people of `ChatMembers.chat`, as loaded for a `LoadMembers`. Never cached.
    Members(ChatMembers),
    /// `LoadMembers` for `chat` produced no list. `offline` is set when the client could not ask
    /// at all (nothing was sent); otherwise `reason` is the server's or the network's, to show
    /// with a retry.
    MembersFailed { chat: ChatId, reason: String, offline: bool },
    /// The signed-in account's own profile, as loaded for a `LoadProfile`. Never cached.
    Profile(Profile),
    /// `LoadProfile` produced no profile; `offline` as for `MembersFailed`.
    ProfileFailed { reason: String, offline: bool },
    /// The chat the user asked for with `CreatePrivateChat`. `existing` is set when no request was
    /// needed (the account already had the chat, or the refreshed list showed it after an unknown
    /// result); otherwise the server created it. The chat list was refreshed before this event, so
    /// `chat` is normally in the list already. `chat` is `None` when neither the reply nor the
    /// refreshed list names it: the chat may exist, the UI cannot open it.
    ChatCreated { chat: Option<ChatId>, existing: bool },
    /// A `CreatePrivateChat` produced no chat. `unknown` is set when the request was sent and its
    /// outcome is not known (the chat may exist; the list was refreshed, did not hold it, and nothing
    /// is retried); otherwise nothing was created and `reason` is the server's, or the offline text
    /// when nothing was sent.
    CreationFailed { reason: String, unknown: bool },
    /// A `CreatePrivateChat` produced no chat and the chat list could not be read to check: either the
    /// request was sent, its outcome is unknown and the refresh that should have shown whether the
    /// chat exists failed, or the list check that precedes a new attempt after such a result failed
    /// (then no request was sent). A chat may exist unseen. Until a read of the list succeeds, a new
    /// `CreatePrivateChat` reads it first and sends nothing when that fails again.
    CreationUnverified { reason: String },
}

/// UI to core. Writes are only the user's own actions: send, edit, delete, react, read, typing,
/// creating a chat.
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
    /// The user chose reaction `code` on a message. The core decides from the cached state
    /// whether that adds, replaces or removes the user's reaction.
    ToggleReaction { chat: ChatId, timestamp: MessageTs, code: ReactionCode },
    /// The newest message of `chat` the user has seen in the focused window.
    MarkRead { chat: ChatId, timestamp: MessageTs },
    /// The user changed the composer text of `chat`.
    Typing(ChatId),
    SaveDraft(Draft),
    /// Save the original `media` (kind `File`) into the Downloads folder under `name`
    /// (sanitized by the core); the user asked for it explicitly.
    Download { media: MediaRef, name: Option<String> },
    CancelDownload(MediaRef),
    /// Server search by name and text; the reply is `SearchResults` or `SearchFailed` with `id`.
    Search { id: SearchId, query: String },
    /// The user no longer waits for a search: its reply is dropped.
    CancelSearch,
    /// Read the people of `chat` (read-only). The reply is `Members` or `MembersFailed`; a
    /// second request for a chat whose load is still running is dropped.
    LoadMembers(ChatId),
    /// Read the signed-in account's own profile (read-only). The reply is `Profile` or
    /// `ProfileFailed`; a request while a load is running is dropped.
    LoadProfile,
    /// Start a private chat with `user`; the reply is `ChatCreated` or `CreationFailed`. One
    /// creation runs at a time (a command meanwhile is dropped), it is never queued or retried.
    CreatePrivateChat { user: UserId },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reactions::ReactionCount;

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

    #[test]
    fn search_contract_types_compare_by_value() {
        let hits = SearchHits {
            chats: vec![ChatHit { id: ChatId("0/0/c".into()), kind: ChatKind::Group, title: "t".into(), avatar: None }],
            people: vec![PersonHit { user: UserId("u".into()), name: "n".into(), avatar: None, chat: None }],
            messages: vec![MessageHit {
                chat: ChatId("0/0/c".into()),
                chat_title: "t".into(),
                timestamp: 1,
                author_name: None,
                text: "x".into(),
            }],
        };
        assert_eq!(hits.clone(), hits);
        assert_ne!(hits, SearchHits::default());
        let event = ClientEvent::SearchResults { id: SearchId(1), hits };
        assert_eq!(event.clone(), event);
        let failed = |reason| ClientEvent::SearchFailed { id: SearchId(1), reason };
        assert_ne!(failed(SearchFailure::Offline), failed(SearchFailure::Failed));
        let search = Command::Search { id: SearchId(2), query: "q".into() };
        assert_eq!(search.clone(), search);
        assert_ne!(Command::CancelSearch, search);
    }

    #[test]
    fn a_message_cached_without_reactions_still_decodes() {
        let mut json = serde_json::to_value(crate::fixtures::msg("0/0/c", 1, "hi")).unwrap();
        assert!(json.as_object_mut().unwrap().remove("reactions").is_some());
        let message: Message = serde_json::from_value(json).unwrap();
        assert_eq!(message.reactions, Reactions::default());
        assert_eq!(message.body, MessageBody::Text("hi".into()));
    }

    #[test]
    fn reactions_survive_a_json_round_trip() {
        let mut message = crate::fixtures::msg("0/0/c", 1, "hi");
        message.reactions = Reactions {
            counts: vec![ReactionCount { code: ReactionCode(100101), count: 2 }],
            mine: Some(ReactionCode(100101)),
            version: Some(3),
        };
        let json = serde_json::to_string(&message).unwrap();
        assert_eq!(serde_json::from_str::<Message>(&json).unwrap(), message);
    }

    fn reacted(counts: &[(i64, u32)], mine: Option<i64>, version: Option<i64>) -> Message {
        let mut message = crate::fixtures::msg("0/0/c", 1, "hi");
        message.reactions = Reactions {
            counts: counts.iter().map(|&(c, count)| ReactionCount { code: ReactionCode(c), count }).collect(),
            mine: mine.map(ReactionCode),
            version,
        };
        message
    }

    #[test]
    fn inherit_reactions_copies_only_when_the_incoming_copy_has_no_reaction_state() {
        let cached = reacted(&[(1, 2)], Some(1), Some(4));
        let mut bare = crate::fixtures::msg("0/0/c", 1, "edited");
        bare.inherit_reactions(&cached);
        assert_eq!(bare.reactions, cached.reactions);

        let mut with_counts = reacted(&[(2, 1)], None, None);
        with_counts.inherit_reactions(&cached);
        assert_eq!(with_counts.reactions, reacted(&[(2, 1)], None, None).reactions);

        let mut with_version = reacted(&[], None, Some(5));
        with_version.inherit_reactions(&cached);
        assert_eq!(with_version.reactions, reacted(&[], None, Some(5)).reactions);
    }
}
