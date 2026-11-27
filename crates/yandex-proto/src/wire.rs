//! Wire types and parsers for the messenger's JSON payloads.
//!
//! Field names follow the web client's wire format (research: yandex-messenger-mcp
//! `docs/protocol-research.md` §9, §11, §14 and its live captures). Parsers are tolerant: an
//! unknown message type becomes [`WireBody::Other`], never an error, so the timeline can show a
//! placeholder. Only a missing address (timestamp, chat id) rejects a message.

use serde_json::Value as Json;

use crate::error::ProtoError;

/// Server timestamp in microseconds (16 digits, fits `i64` and JSON numbers exactly).
pub type Micros = i64;

/// Namespaces the web client hides from the chat list: `hidden_namespaces` `[5, 15]` and
/// `chat_list_excluded_namespaces` `[95]` from the `telemost.yandex.ru` `globalParams`.
pub const DEFAULT_EXCLUDED_NAMESPACES: [u32; 3] = [5, 15, 95];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WireError {
    #[error("payload has no {0}")]
    Missing(&'static str),
    #[error("{0}")]
    Invalid(String),
}

impl From<WireError> for ProtoError {
    fn from(e: WireError) -> Self {
        Self::Protocol(e.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireChatKind {
    Private,
    Group,
    Channel,
}

/// A user from `get_users_data`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireUser {
    pub guid: String,
    pub display_name: Option<String>,
    pub avatar_id: Option<String>,
    pub uid: Option<String>,
}

/// Role of a chat member as `get_chat_members` names it (`admin` and `member` observed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireMemberRole {
    Admin,
    /// Also the role of a user that carries none.
    Member,
    /// A role this client does not know, as the server spelled it.
    Other(String),
}

/// A user of `get_chat_members`; the reply carries the display name, so no `get_users_data`
/// round trip is needed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireMember {
    pub guid: String,
    pub display_name: Option<String>,
    pub avatar_id: Option<String>,
    pub role: WireMemberRole,
    /// `account_deleted: true`.
    pub deleted: bool,
}

/// One page of `get_chat_members`. The reply's `groups` and `departments` are not modelled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireMembers {
    pub users: Vec<WireMember>,
    /// Users without a guid, dropped from `users`.
    pub skipped: usize,
}

/// One entry of `list_reactions`: who reacted, with what, and when. The identity comes from the
/// entry's own `UserInfo`, so a name usually needs no second request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireReactorEntry {
    pub guid: String,
    pub display_name: Option<String>,
    pub avatar_id: Option<String>,
    pub code: i64,
    /// When the user reacted; `None` when the entry carries no timestamp.
    pub timestamp: Option<Micros>,
}

/// One page of `list_reactions`, in server order. The reply carries no total and no paging
/// marker; `raw` and `oldest` describe the page as the server sent it, so the caller's cursor and
/// end-of-list test do not depend on which entries could be shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireReactors {
    pub entries: Vec<WireReactorEntry>,
    /// Entries without a `Type` or without `UserInfo.Guid`, dropped from `entries`.
    pub skipped: usize,
    /// Entries on the wire page before any was dropped: `entries.len() + skipped`.
    pub raw: usize,
    /// Smallest `Timestamp` over all raw entries, malformed ones included.
    pub oldest: Option<Micros>,
}

/// What `whoami` says about the signed-in account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireProfile {
    pub guid: Option<String>,
    pub name: Option<String>,
    pub avatar_id: Option<String>,
    /// `UserStatusInfo.UserStatus.CustomStatus.Text`; `None` while the status is unset (`{}`).
    pub status_text: Option<String>,
}

/// Author of a message (`ServerMessageInfo.From`, else `CustomFrom`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireSender {
    pub guid: Option<String>,
    pub display_name: Option<String>,
    pub avatar_id: Option<String>,
}

/// `FileInfo`: `Id2` is `<bucket>/<uuid>` and is the only handle for downloading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireFile {
    pub id: String,
    pub name: Option<String>,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireImage {
    pub file: WireFile,
    pub width: u32,
    pub height: u32,
    pub animated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireBody {
    Text(String),
    Image(WireImage),
    File(WireFile),
    Voice {
        file: WireFile,
        duration: Option<u64>,
    },
    Gallery {
        items: Vec<WireImage>,
        text: Option<String>,
    },
    Sticker {
        id: String,
        set_id: Option<String>,
    },
    Poll {
        title: String,
    },
    /// Chat event; `kind` is the variant key (`ChatCreatedInfo`, `ParticipantsChangedDiffV2`, ...).
    System {
        kind: String,
        text: Option<String>,
    },
    Deleted,
    /// No content of its own, e.g. a forward without a comment.
    Empty,
    /// A content type this client does not know; holds the server's key.
    Other(String),
}

/// Original of a forward or reply (`ForwardedMessages[]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireForwarded {
    pub chat: Option<String>,
    pub timestamp: Option<Micros>,
    pub from: Option<WireSender>,
    pub body: WireBody,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireRef {
    pub chat: Option<String>,
    pub timestamp: Option<Micros>,
}

/// The wire models a reply as a forward with a quote, so the two are told apart by heuristic:
/// a quote style means reply; originals without own content mean forward; originals next to own
/// content mean reply. A forward with a comment is therefore shown as a reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Relation {
    None,
    Reply {
        quote: Option<String>,
        reference: Option<WireRef>,
        original: Option<WireForwarded>,
    },
    Forward {
        originals: Vec<WireForwarded>,
    },
}

/// One entry of `Reactions`: how many users chose the type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WireReactionCount {
    pub code: i64,
    pub count: u32,
}

/// One entry of `RecentUserReactions`: a user who reacted lately (the server truncates the list).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireReactor {
    /// `UserInfo.Guid`.
    pub user: Option<String>,
    pub code: i64,
    pub timestamp: Option<Micros>,
}

/// Reaction state of a message, read from the siblings of `ClientMessage` on a `ServerMessage`
/// (`Reactions`, `ReactionsVersion`, `RecentUserReactions`). Entries without `Type` and counts
/// of 0 are dropped; server order is kept. A missing `Reactions` key means no reactions.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WireReactions {
    pub counts: Vec<WireReactionCount>,
    pub version: Option<i64>,
    pub recent: Vec<WireReactor>,
}

/// `Reaction.Action`: 1 removes, 2 replaces (also the first reaction); anything else is kept as is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReactionAction {
    Remove,
    Replace,
    Other(i64),
}

#[derive(Debug, Clone, PartialEq)]
pub struct WireMessage {
    pub chat: String,
    pub timestamp: Micros,
    pub seq_no: Option<i64>,
    /// Grows on every edit or deletion of the same message.
    pub version: Option<i64>,
    /// `Plain.PayloadId`: the id the sending client chose; the echo of an own send carries it.
    pub payload_id: Option<String>,
    pub from: Option<WireSender>,
    pub body: WireBody,
    pub edited: bool,
    /// `Plain`/`Ephemeral.IsImportant` is `true`. Missing, `false`, non-bool, a deleted message
    /// and a non-`Plain` kind all give `false`.
    pub important: bool,
    pub relation: Relation,
    /// `Some` when any of `Reactions`, `ReactionsVersion`, `RecentUserReactions` is present.
    pub reactions: Option<WireReactions>,
    /// The `ServerMessage` object as received, for the cache's raw column.
    pub raw: Json,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WireChat {
    pub id: String,
    pub kind: WireChatKind,
    pub title: Option<String>,
    pub avatar_id: Option<String>,
    /// Interlocutor of a private chat.
    pub partner_guid: Option<String>,
    /// `LastTsMcs`: ordering key of the chat list.
    pub last_activity: Option<Micros>,
    /// `LastSeqNo - LastSeenByMeSeqNo`, clamped at zero.
    pub unread: u32,
    pub muted: bool,
    pub last_message: Option<WireMessage>,
    /// The chat entry without its `Messages`.
    pub raw: Json,
}

/// One history page, oldest first.
#[derive(Debug, Clone, PartialEq)]
pub struct WireHistory {
    pub chat: String,
    pub messages: Vec<WireMessage>,
    /// The server returned fewer rows than requested: nothing older exists.
    pub reached_start: bool,
    /// Rows without a usable address (timestamp), dropped from `messages`.
    pub skipped: usize,
}

/// Decoded server push.
#[derive(Debug, Clone, PartialEq)]
pub enum PushEvent {
    /// New message or an edit (same chat and timestamp, higher version).
    MessageUpserted(WireMessage),
    MessageDeleted {
        chat: String,
        timestamp: Micros,
    },
    /// Something that changes the chat row or its history without carrying a message:
    /// read state moved on another device, pin, history cleared, state sync.
    ChatChanged {
        chat: String,
    },
    /// Someone is typing: `user` is the typist's guid. `timeout_secs` is the lifetime of the
    /// indicator when the push states one.
    Typing {
        chat: String,
        user: String,
        name: Option<String>,
        timeout_secs: Option<u32>,
    },
    /// A reaction was added, replaced or removed on a message. `actor` is who did it
    /// (`ServerMessageInfo.From.Guid`); `action` and `code` come from the `Reaction` body;
    /// `reactions` is the message's state after the change.
    Reaction {
        chat: String,
        timestamp: Micros,
        actor: Option<String>,
        action: Option<ReactionAction>,
        code: Option<i64>,
        reactions: WireReactions,
    },
    /// Presence and other events this client does not act on.
    Ignored {
        kind: String,
    },
}

fn present<'a>(value: &'a Json, key: &str) -> Option<&'a Json> {
    value.get(key).filter(|v| !v.is_null())
}

fn string(value: &Json, key: &str) -> Option<String> {
    present(value, key)?
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// Accepts a JSON integer, an integral float and a digit string.
fn micros(value: &Json) -> Option<i64> {
    match value {
        Json::Number(n) => n.as_i64().or_else(|| {
            n.as_f64()
                .filter(|f| f.fract() == 0.0 && f.abs() < 9.0e15)
                .map(|f| f as i64)
        }),
        Json::String(s) => s.parse().ok(),
        _ => None,
    }
}

pub(crate) fn int_field(value: &Json, key: &str) -> Option<i64> {
    micros(present(value, key)?)
}

fn id_string(value: &Json, key: &str) -> Option<String> {
    match present(value, key)? {
        Json::String(s) if !s.is_empty() => Some(s.clone()),
        Json::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// `UserInfo` has `DisplayName` and `PublicName`; the former is what the web client shows.
fn display_name(value: &Json) -> Option<String> {
    string(value, "DisplayName").or_else(|| string(value, "PublicName"))
}

/// Chat namespace: the leading numeric segment of `0/0/<guid>`; private chat ids
/// (`<guid>_<guid>`) have none.
pub fn namespace_of(chat_id: &str) -> Option<u32> {
    let (head, _) = chat_id.split_once('/')?;
    head.parse().ok()
}

/// Application-layer status of a reply (`ResponseStatus`): absent or 0 is success.
///
/// A refusal carries its reason as `Details` (older replies) or as `ErrorInfo.Message` (what the
/// server answers for `list_reactions`, observed live 2026-11-27); either one ends the error text
/// as `status 4 ENTITY_NOT_FOUND: <reason>`, which [`refusal_reason`] reads back. The reason is
/// the user's to see only where a caller maps it on purpose (the reactor list); every other
/// consumer drops it with [`without_refusal_reason`] before it is shown or logged. An overload
/// keeps only `Details`, as before.
pub fn check_status(body: &Json) -> Result<(), ProtoError> {
    let Some(status) = body
        .get("Status")
        .and_then(Json::as_i64)
        .filter(|s| *s != 0)
    else {
        return Ok(());
    };
    if status == 6 {
        return Err(ProtoError::SessionExpired);
    }
    let name = match status {
        1 => "INTERNAL_ERROR",
        2 => "ACCESS_DENIED",
        3 => "MISCONFIGURATION",
        4 => "ENTITY_NOT_FOUND",
        5 => "OVERLOAD",
        _ => "unknown",
    };
    let details = |reason: Option<String>| reason.map(|d| format!(": {d}")).unwrap_or_default();
    if status == 5 {
        // OVERLOAD clears by itself.
        return Err(ProtoError::Network(format!(
            "server overloaded{}",
            details(string(body, "Details"))
        )));
    }
    let reason = string(body, "Details").or_else(|| {
        let message = present(body, "ErrorInfo").and_then(|i| string(i, "Message"))?;
        Some(message.trim().to_owned()).filter(|m| !m.is_empty())
    });
    // Everything else is a request the server will keep refusing.
    Err(ProtoError::Protocol(format!(
        "status {status} {name}{}",
        details(reason)
    )))
}

/// The server's reason in the text of a refused status (`status 4 ENTITY_NOT_FOUND: <reason>`),
/// `None` for any other text and for a refusal that came without a reason.
pub fn refusal_reason(text: &str) -> Option<&str> {
    let rest = text.strip_prefix("status ")?;
    let (status, rest) = rest.split_once(' ')?;
    status.parse::<i64>().ok()?;
    let (_name, reason) = rest.split_once(": ")?;
    Some(reason).filter(|r| !r.is_empty())
}

/// A refused status's text without the server's reason (`status 4 ENTITY_NOT_FOUND`), for logs;
/// any other text is returned unchanged.
pub fn without_refusal_reason(text: &str) -> &str {
    match refusal_reason(text) {
        Some(reason) => &text[..text.len() - reason.len() - ": ".len()],
        None => text,
    }
}

fn sender(info: &Json) -> Option<WireSender> {
    let from = present(info, "From").filter(|f| f.is_object());
    if let Some(from) = from {
        let guid = string(from, "Guid");
        if guid.is_some() {
            return Some(WireSender {
                guid,
                display_name: display_name(from),
                avatar_id: string(from, "AvatarId"),
            });
        }
    }
    let custom = present(info, "CustomFrom").filter(|f| f.is_object())?;
    Some(WireSender {
        guid: None,
        display_name: display_name(custom),
        avatar_id: string(custom, "AvatarId"),
    })
}

fn file_info(holder: &Json) -> Option<WireFile> {
    let info = present(holder, "FileInfo")?;
    Some(WireFile {
        id: string(info, "Id2")?,
        name: string(info, "Name"),
        size: info.get("Size").and_then(Json::as_u64),
    })
}

fn image(holder: &Json) -> Option<WireImage> {
    let dimension = |key| {
        int_field(holder, key)
            .and_then(|v| u32::try_from(v).ok())
            .unwrap_or(0)
    };
    Some(WireImage {
        file: file_info(holder)?,
        width: dimension("Width"),
        height: dimension("Height"),
        animated: holder
            .get("Animated")
            .and_then(Json::as_bool)
            .unwrap_or(false),
    })
}

/// Keys of a `Plain` body that are not content: anything else is an unknown content type.
const PLAIN_META_KEYS: [&str; 14] = [
    "ChatId",
    "PayloadId",
    "Timestamp",
    "MentionedUserIds",
    "ForwardedMessageRefs",
    "ForwardedMessageStyles",
    "UrlPreviewDisabled",
    "IsImportant",
    "CustomPayload",
    "SuggestButtonsHolder",
    "LogData",
    "NotificationBehaviour",
    "IsSilent",
    "PersistentInlineButtons",
];

/// Content of a `Plain`/`Ephemeral`/forwarded-`Payload` body.
fn plain_body(plain: &Json) -> WireBody {
    if let Some(text) = present(plain, "Text") {
        return WireBody::Text(
            text.get("MessageText")
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_owned(),
        );
    }
    if let Some(sticker) = present(plain, "Sticker") {
        return match string(sticker, "Id") {
            Some(id) => WireBody::Sticker {
                id,
                set_id: string(sticker, "SetId"),
            },
            None => WireBody::Other("Sticker".into()),
        };
    }
    if let Some(holder) = present(plain, "Image") {
        return image(holder).map_or_else(|| WireBody::Other("Image".into()), WireBody::Image);
    }
    if let Some(holder) = present(plain, "MiscFile") {
        return file_info(holder)
            .map_or_else(|| WireBody::Other("MiscFile".into()), WireBody::File);
    }
    if present(plain, "Card").is_some() {
        return WireBody::Other("Card".into());
    }
    if let Some(gallery) = present(plain, "Gallery") {
        let items = gallery
            .get("Items")
            .and_then(Json::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| image(present(item, "Image")?))
                    .collect()
            })
            .unwrap_or_default();
        return WireBody::Gallery {
            items,
            text: string(gallery, "Text"),
        };
    }
    if let Some(voice) = present(plain, "Voice") {
        return match file_info(voice) {
            Some(file) => WireBody::Voice {
                file,
                duration: voice.get("Duration").and_then(Json::as_u64),
            },
            None => WireBody::Other("Voice".into()),
        };
    }
    if let Some(poll) = present(plain, "Poll") {
        return WireBody::Poll {
            title: string(poll, "Title").unwrap_or_default(),
        };
    }
    plain
        .as_object()
        .and_then(|o| o.keys().find(|k| !PLAIN_META_KEYS.contains(&k.as_str())))
        .map_or(WireBody::Empty, |key| WireBody::Other(key.clone()))
}

fn system_body(system: &Json) -> WireBody {
    let kind = system
        .as_object()
        .and_then(|o| o.keys().find(|k| *k != "ChatId" && *k != "PayloadId"))
        .cloned()
        .unwrap_or_else(|| "SystemMessage".into());
    let text = system
        .get("GenericMessage")
        .and_then(|g| string(g, "MessageText"));
    WireBody::System { kind, text }
}

fn forwarded(items: &Json) -> Vec<WireForwarded> {
    let Some(items) = items.as_array() else {
        return Vec::new();
    };
    items
        .iter()
        .map(|item| {
            let payload = present(item, "Payload");
            let info = present(item, "ServerMessageInfo");
            WireForwarded {
                chat: payload.and_then(|p| string(p, "ChatId")),
                timestamp: info.and_then(|i| int_field(i, "Timestamp")),
                from: info.and_then(sender),
                body: payload.map_or(WireBody::Empty, plain_body),
            }
        })
        .collect()
}

fn relation(
    plain: Option<&Json>,
    originals: Vec<WireForwarded>,
    has_own_content: bool,
) -> Relation {
    let quote = plain
        .and_then(|p| p.get("ForwardedMessageStyles")?.as_array())
        .and_then(|styles| styles.iter().find_map(|s| string(s, "Quote")));
    let reference = plain
        .and_then(|p| p.get("ForwardedMessageRefs")?.as_array())
        .and_then(|refs| refs.first())
        .map(|r| WireRef {
            chat: string(r, "ChatId"),
            timestamp: int_field(r, "Timestamp").filter(|t| *t != 0),
        });

    let is_reply =
        quote.is_some() || ((!originals.is_empty() || reference.is_some()) && has_own_content);
    if is_reply {
        return Relation::Reply {
            quote,
            reference,
            original: originals.into_iter().next(),
        };
    }
    if !originals.is_empty() || reference.is_some() {
        return Relation::Forward { originals };
    }
    Relation::None
}

/// Reaction siblings of `ClientMessage`; `None` when none of the three keys is present.
fn reactions(raw: &Json) -> Option<WireReactions> {
    let counts = present(raw, "Reactions");
    let recent = present(raw, "RecentUserReactions");
    let version = int_field(raw, "ReactionsVersion");
    if counts.is_none() && recent.is_none() && version.is_none() {
        return None;
    }
    fn items(value: Option<&Json>) -> &[Json] {
        value.and_then(Json::as_array).map_or(&[], Vec::as_slice)
    }
    Some(WireReactions {
        counts: items(counts)
            .iter()
            .filter_map(|entry| {
                let code = int_field(entry, "Type")?;
                let count = u32::try_from(int_field(entry, "Count")?)
                    .ok()
                    .filter(|&c| c > 0)?;
                Some(WireReactionCount { code, count })
            })
            .collect(),
        version,
        recent: items(recent)
            .iter()
            .filter_map(|entry| {
                let code = int_field(entry, "Type")?;
                let user = present(entry, "UserInfo").and_then(|u| string(u, "Guid"));
                Some(WireReactor {
                    user,
                    code,
                    timestamp: int_field(entry, "Timestamp"),
                })
            })
            .collect(),
    })
}

/// Parses a `ServerMessage`: `{ClientMessage, ServerMessageInfo, ForwardedMessages?, ...}`.
/// History rows (`Messages[].ServerMessage`) and push bodies share this shape.
pub fn parse_server_message(raw: &Json, chat_hint: Option<&str>) -> Result<WireMessage, WireError> {
    let info = present(raw, "ServerMessageInfo").ok_or(WireError::Missing("ServerMessageInfo"))?;
    let timestamp = int_field(info, "Timestamp").ok_or(WireError::Missing("Timestamp"))?;
    let client = present(raw, "ClientMessage");
    let plain = client.and_then(|c| present(c, "Plain").or_else(|| present(c, "Ephemeral")));
    let system = client.and_then(|c| present(c, "SystemMessage"));

    let chat = plain
        .and_then(|p| string(p, "ChatId"))
        .or_else(|| system.and_then(|s| string(s, "ChatId")))
        .or_else(|| chat_hint.map(str::to_owned))
        .ok_or(WireError::Missing("ChatId"))?;

    let deleted = info.get("Deleted").and_then(Json::as_bool).unwrap_or(false);
    let edited = int_field(info, "LastEditTimestamp").is_some_and(|t| t > 0);
    let originals = raw
        .get("ForwardedMessages")
        .map(forwarded)
        .unwrap_or_default();

    let (body, relation) = if deleted {
        (WireBody::Deleted, Relation::None)
    } else if let Some(plain) = plain {
        let body = plain_body(plain);
        let own = !matches!(body, WireBody::Empty);
        (body, relation(Some(plain), originals, own))
    } else if let Some(system) = system {
        (system_body(system), Relation::None)
    } else {
        let kind = client
            .and_then(|c| c.as_object()?.keys().next().cloned())
            .unwrap_or_else(|| "ClientMessage".into());
        (WireBody::Other(kind), Relation::None)
    };

    Ok(WireMessage {
        chat,
        timestamp,
        seq_no: int_field(info, "SeqNo"),
        version: int_field(info, "Version"),
        payload_id: plain.and_then(|p| string(p, "PayloadId")),
        from: sender(info),
        body,
        edited,
        important: !deleted
            && plain.is_some_and(|p| p.get("IsImportant").and_then(Json::as_bool) == Some(true)),
        relation,
        reactions: reactions(raw),
        raw: raw.clone(),
    })
}

fn parse_chat(entry: &Json) -> Option<WireChat> {
    let id = string(entry, "ChatId")?;
    // Per-chat failure (`Status` != 0): the row has no usable metadata.
    if entry
        .get("Status")
        .and_then(Json::as_i64)
        .is_some_and(|s| s != 0)
    {
        return None;
    }
    let private = present(entry, "PrivateChatInfo").is_some();
    let kind = if private {
        WireChatKind::Private
    } else if namespace_of(&id) == Some(1) {
        WireChatKind::Channel
    } else {
        WireChatKind::Group
    };
    let partner = present(entry, "PartnerInfo");
    let info = present(entry, "ChatInfo");
    let title = partner
        .and_then(display_name)
        .or_else(|| info.and_then(|i| string(i, "Name")));
    let avatar_id = partner
        .and_then(|p| string(p, "AvatarId"))
        .or_else(|| info.and_then(|i| string(i, "AvatarId")));

    let last_seq = int_field(entry, "LastSeqNo").unwrap_or(0);
    let seen = int_field(entry, "LastSeenByMeSeqNo").unwrap_or(0);
    let last_message = entry
        .get("Messages")
        .and_then(Json::as_array)
        .and_then(|rows| {
            rows.iter()
                .filter_map(|r| present(r, "ServerMessage"))
                .next_back()
        })
        .and_then(|m| parse_server_message(m, Some(&id)).ok());

    let mut raw = entry.clone();
    if let Some(o) = raw.as_object_mut() {
        o.remove("Messages");
    }
    Some(WireChat {
        kind,
        title,
        avatar_id,
        partner_guid: if private {
            partner.and_then(|p| string(p, "Guid"))
        } else {
            None
        },
        last_activity: int_field(entry, "LastTsMcs"),
        unread: u32::try_from((last_seq - seen).max(0)).unwrap_or(u32::MAX),
        muted: entry.get("Muted").and_then(Json::as_bool).unwrap_or(false),
        last_message,
        raw,
        id,
    })
}

/// Chat list from the `history` reply to [`ReadRequest::ChatList`](crate::request::ReadRequest::ChatList),
/// without chats in `excluded` namespaces. Server order is kept.
pub fn parse_chat_list(body: &Json, excluded: &[u32]) -> Result<Vec<WireChat>, ProtoError> {
    check_status(body)?;
    // An account without chats may answer without a `Chats` key at all.
    let Some(entries) = body.get("Chats") else {
        return Ok(Vec::new());
    };
    let entries = entries
        .as_array()
        .ok_or_else(|| ProtoError::Protocol("Chats is not an array".into()))?;
    Ok(entries
        .iter()
        .filter_map(parse_chat)
        .filter(|chat| namespace_of(&chat.id).is_none_or(|ns| !excluded.contains(&ns)))
        .collect())
}

/// One history page from the reply to [`ReadRequest::History`](crate::request::ReadRequest::History).
pub fn parse_history(body: &Json, chat: &str, limit: u32) -> Result<WireHistory, ProtoError> {
    check_status(body)?;
    let entries = body
        .get("Chats")
        .and_then(Json::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let entry = entries
        .iter()
        .find(|e| string(e, "ChatId").as_deref() == Some(chat));
    let Some(entry) = entry else {
        // A chat without messages may be omitted entirely.
        return Ok(WireHistory {
            chat: chat.to_owned(),
            messages: Vec::new(),
            reached_start: true,
            skipped: 0,
        });
    };
    check_status(entry)?;

    let rows = entry
        .get("Messages")
        .and_then(Json::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut messages = Vec::with_capacity(rows.len());
    let mut skipped = 0;
    for row in rows {
        match present(row, "ServerMessage").map(|m| parse_server_message(m, Some(chat))) {
            Some(Ok(message)) => messages.push(message),
            _ => skipped += 1,
        }
    }
    messages.sort_by_key(|m| m.timestamp);
    messages.dedup_by_key(|m| m.timestamp);
    Ok(WireHistory {
        chat: chat.to_owned(),
        reached_start: rows.len() < limit as usize,
        messages,
        skipped,
    })
}

/// Users from the `data` object of a `get_users_data` reply (`{users: [...]}`, or a bare array).
/// Keys are `snake_case` over HTTP; the socket's `UserInfo` casing is accepted too.
pub fn parse_users(data: &Json) -> Result<Vec<WireUser>, ProtoError> {
    let list = data
        .get("users")
        .and_then(Json::as_array)
        .or_else(|| data.as_array())
        .ok_or_else(|| ProtoError::Protocol("get_users_data reply has no users".into()))?;
    let pick = |u: &Json, snake: &str, pascal: &str| string(u, snake).or_else(|| string(u, pascal));
    Ok(list
        .iter()
        .filter_map(|u| {
            Some(WireUser {
                guid: pick(u, "guid", "Guid")?,
                display_name: pick(u, "display_name", "DisplayName")
                    .or_else(|| pick(u, "public_name", "PublicName")),
                avatar_id: pick(u, "avatar_id", "AvatarId"),
                uid: id_string(u, "uid").or_else(|| id_string(u, "Uid")),
            })
        })
        .collect())
}

/// Members of a chat from the `data` object of a `get_chat_members` reply
/// (`{users, groups, departments}`; the server sends no paging marker and no total, so a page
/// shorter than the requested `limit` is the last one). `users` is required. `groups` and
/// `departments` are never read: their element shape was not observed (empty in every group
/// probed), so whatever they hold is ignored. A user without a guid is dropped and counted in
/// `skipped`.
pub fn parse_members(data: &Json) -> Result<WireMembers, ProtoError> {
    let users = data
        .get("users")
        .and_then(Json::as_array)
        .ok_or_else(|| ProtoError::Protocol("get_chat_members reply has no users".into()))?;
    let mut skipped = 0;
    let mut members = Vec::with_capacity(users.len());
    for user in users {
        let Some(guid) = string(user, "guid") else {
            skipped += 1;
            continue;
        };
        members.push(WireMember {
            guid,
            display_name: string(user, "display_name").or_else(|| string(user, "public_name")),
            avatar_id: string(user, "avatar_id"),
            role: match string(user, "role").as_deref() {
                Some("admin") => WireMemberRole::Admin,
                None | Some("member") => WireMemberRole::Member,
                Some(other) => WireMemberRole::Other(other.to_owned()),
            },
            deleted: user
                .get("account_deleted")
                .and_then(Json::as_bool)
                .unwrap_or(false),
        });
    }
    Ok(WireMembers {
        users: members,
        skipped,
    })
}

/// One page from the reply to [`ReadRequest::ListReactions`](crate::request::ReadRequest::ListReactions):
/// `{UserReactions: [{Type, Timestamp, UserInfo{Guid, DisplayName, AvatarId, ...}}]}`. The key is
/// absent when the list is empty (an empty page). Server order is kept. An entry without `Type`
/// or without `UserInfo.Guid` has no identity or glyph to show: it is counted in `skipped` and
/// not returned, but it still counts in `raw` and its `Timestamp` still counts in `oldest`.
pub fn parse_reactors(body: &Json) -> Result<WireReactors, ProtoError> {
    if !body.is_object() {
        return Err(ProtoError::Protocol(
            "list_reactions reply is not an object".into(),
        ));
    }
    check_status(body)?;
    let rows = match present(body, "UserReactions") {
        None => &[],
        Some(rows) => rows
            .as_array()
            .map(Vec::as_slice)
            .ok_or_else(|| ProtoError::Protocol("UserReactions is not an array".into()))?,
    };
    let mut entries = Vec::with_capacity(rows.len());
    let mut oldest: Option<Micros> = None;
    for row in rows {
        let timestamp = int_field(row, "Timestamp");
        if let Some(at) = timestamp {
            oldest = Some(oldest.map_or(at, |o| o.min(at)));
        }
        let info = present(row, "UserInfo");
        let (Some(code), Some(guid)) =
            (int_field(row, "Type"), info.and_then(|u| string(u, "Guid")))
        else {
            continue;
        };
        entries.push(WireReactorEntry {
            guid,
            display_name: info.and_then(display_name),
            avatar_id: info.and_then(|u| string(u, "AvatarId")),
            code,
            timestamp,
        });
    }
    Ok(WireReactors {
        skipped: rows.len() - entries.len(),
        raw: rows.len(),
        entries,
        oldest,
    })
}

/// The signed-in account's profile from a `whoami` reply (`{UserInfo, UserStatusInfo,
/// CurrentTime}`). `UserStatusInfo.UserStatus` is `{}` while no status is set; a status counts
/// only when `UserStatus.CustomStatus.Text` is a non-blank string (the PascalCase path the web
/// client's `whoami` normalizer reads; the populated shape was never seen live). No
/// availability is inferred from the rest of it.
pub fn parse_profile(body: &Json) -> Result<WireProfile, ProtoError> {
    check_status(body)?;
    let info = present(body, "UserInfo")
        .filter(|v| v.is_object())
        .ok_or_else(|| ProtoError::Protocol("whoami reply has no UserInfo".into()))?;
    let status_text = body
        .pointer("/UserStatusInfo/UserStatus/CustomStatus")
        .and_then(|custom| string(custom, "Text"))
        .filter(|text| !text.trim().is_empty());
    Ok(WireProfile {
        guid: string(info, "Guid"),
        name: display_name(info),
        avatar_id: string(info, "AvatarId"),
        status_text,
    })
}

/// Decodes a server push body: a flat `{ClientMessage, ServerMessageInfo, ...}`.
/// Edits and deletions are `Plain` pushes distinguished by `LastEditTimestamp` and `Deleted`.
pub fn decode_push(body: &Json) -> Result<PushEvent, WireError> {
    let client = present(body, "ClientMessage").ok_or(WireError::Missing("ClientMessage"))?;
    if present(client, "Plain").is_some()
        || present(client, "Ephemeral").is_some()
        || present(client, "SystemMessage").is_some()
    {
        let message = parse_server_message(body, None)?;
        return Ok(if matches!(message.body, WireBody::Deleted) {
            PushEvent::MessageDeleted {
                chat: message.chat,
                timestamp: message.timestamp,
            }
        } else {
            PushEvent::MessageUpserted(message)
        });
    }
    if let Some(typing) = present(client, "Typing") {
        let from = present(body, "ServerMessageInfo").and_then(sender);
        if let (
            Some(chat),
            Some(WireSender {
                guid: Some(user),
                display_name: name,
                ..
            }),
        ) = (string(typing, "ChatId"), from)
        {
            let timeout_secs =
                int_field(typing, "TimeoutSeconds").and_then(|s| u32::try_from(s).ok());
            return Ok(PushEvent::Typing {
                chat,
                user,
                name,
                timeout_secs,
            });
        }
    }
    if let Some(reaction) = present(client, "Reaction") {
        let info = present(body, "ServerMessageInfo");
        let address = string(reaction, "ChatId").zip(
            int_field(reaction, "Timestamp")
                .or_else(|| info.and_then(|i| int_field(i, "Timestamp"))),
        );
        if let Some((chat, timestamp)) = address {
            return Ok(PushEvent::Reaction {
                chat,
                timestamp,
                actor: info.and_then(sender).and_then(|s| s.guid),
                action: int_field(reaction, "Action").map(|a| match a {
                    1 => ReactionAction::Remove,
                    2 => ReactionAction::Replace,
                    other => ReactionAction::Other(other),
                }),
                code: int_field(reaction, "Type"),
                reactions: reactions(body).unwrap_or_default(),
            });
        }
    }
    // `NotificationBehaviour` rides along with real kinds (seen markers, bot requests): a
    // modifier, not the event.
    const MODIFIERS: [&str; 1] = ["NotificationBehaviour"];
    let chat_changing = [
        "SeenMarker",
        "Pin",
        "ClearUserHistory",
        "StateSync",
        "ChatApproval",
    ];
    if let Some(chat) = chat_changing
        .iter()
        .find_map(|kind| string(present(client, kind)?, "ChatId"))
    {
        return Ok(PushEvent::ChatChanged { chat });
    }
    let keys = client.as_object().into_iter().flat_map(|o| o.keys());
    let kind = keys
        .clone()
        .find(|k| !MODIFIERS.contains(&k.as_str()))
        .or_else(|| keys.clone().next())
        .cloned()
        .unwrap_or_default();
    Ok(PushEvent::Ignored { kind })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn msg(plain: Json, info: Json) -> Json {
        json!({ "ClientMessage": { "Plain": plain }, "ServerMessageInfo": info })
    }

    #[test]
    fn micros_accepts_ints_floats_and_digit_strings() {
        assert_eq!(
            micros(&json!(1784117592261029_i64)),
            Some(1_784_117_592_261_029)
        );
        assert_eq!(
            micros(&json!(1784117592261029.0)),
            Some(1_784_117_592_261_029)
        );
        assert_eq!(
            micros(&json!("1784117592261029")),
            Some(1_784_117_592_261_029)
        );
        assert_eq!(micros(&json!(1.5)), None);
        assert_eq!(micros(&json!(true)), None);
    }

    #[test]
    fn namespace_is_the_leading_numeric_segment() {
        assert_eq!(namespace_of("0/0/abc"), Some(0));
        assert_eq!(namespace_of("95/0/abc"), Some(95));
        assert_eq!(namespace_of("100/0/abc_1784"), Some(100));
        assert_eq!(namespace_of("aaa_bbb"), None);
        assert_eq!(namespace_of("x/0/abc"), None);
    }

    #[test]
    fn status_mapping() {
        assert!(check_status(&json!({})).is_ok());
        assert!(check_status(&json!({ "Status": 0 })).is_ok());
        assert_eq!(
            check_status(&json!({ "Status": 6 })),
            Err(ProtoError::SessionExpired)
        );
        assert!(
            matches!(check_status(&json!({ "Status": 4, "Details": "no chat" })), Err(ProtoError::Protocol(m)) if m.contains("ENTITY_NOT_FOUND") && m.contains("no chat"))
        );
        assert!(matches!(
            check_status(&json!({ "Status": 5 })),
            Err(ProtoError::Network(_))
        ));
    }

    #[test]
    fn a_refusal_carries_the_servers_error_info_message() {
        // The live `list_reactions` refusal for a channel: no `Details`, an `ErrorInfo` object.
        let live = json!({
            "ErrorInfo": { "Message": " reactions are anonymous ", "WorkerId": 137 },
            "Status": 4
        });
        assert_eq!(
            check_status(&live),
            Err(ProtoError::Protocol(
                "status 4 ENTITY_NOT_FOUND: reactions are anonymous".into()
            ))
        );
        // `Details` wins when both are there; a blank or missing message adds nothing.
        let both = json!({ "Status": 4, "Details": "d", "ErrorInfo": { "Message": "m" } });
        assert_eq!(
            check_status(&both),
            Err(ProtoError::Protocol("status 4 ENTITY_NOT_FOUND: d".into()))
        );
        for info in [
            json!({ "Message": "  " }),
            json!({ "WorkerId": 1 }),
            json!("x"),
        ] {
            assert_eq!(
                check_status(&json!({ "Status": 4, "ErrorInfo": info })),
                Err(ProtoError::Protocol("status 4 ENTITY_NOT_FOUND".into()))
            );
        }
        // Overload keeps its class and does not take `ErrorInfo`.
        assert_eq!(
            check_status(&json!({ "Status": 5, "ErrorInfo": { "Message": "busy" } })),
            Err(ProtoError::Network("server overloaded".into()))
        );
    }

    #[test]
    fn the_refusal_reason_is_read_back_from_the_error_text() {
        let text = |body| match check_status(&body) {
            Err(ProtoError::Protocol(t)) => t,
            other => panic!("not a protocol refusal: {other:?}"),
        };
        let live = text(json!({ "Status": 4, "ErrorInfo": { "Message": "message: not found" } }));
        assert_eq!(refusal_reason(&live), Some("message: not found"));
        assert_eq!(
            refusal_reason(&text(json!({ "Status": 9, "Details": "odd" }))),
            Some("odd")
        );
        // No reason, and texts that are not a refused status, give none.
        assert_eq!(refusal_reason(&text(json!({ "Status": 4 }))), None);
        assert_eq!(refusal_reason("get_chat_members failed: bad_request"), None);
        assert_eq!(refusal_reason("status x NAME: why"), None);
        assert_eq!(refusal_reason("status 4 NAME: "), None);
        // The log form drops the reason and nothing else.
        assert_eq!(without_refusal_reason(&live), "status 4 ENTITY_NOT_FOUND");
        assert_eq!(without_refusal_reason("status 4 NAME"), "status 4 NAME");
        assert_eq!(without_refusal_reason("bad: reply"), "bad: reply");
    }

    #[test]
    fn message_requires_an_address() {
        let no_ts = msg(json!({ "ChatId": "c" }), json!({}));
        assert_eq!(
            parse_server_message(&no_ts, None),
            Err(WireError::Missing("Timestamp"))
        );
        let no_chat = msg(json!({}), json!({ "Timestamp": 5 }));
        assert_eq!(
            parse_server_message(&no_chat, None),
            Err(WireError::Missing("ChatId"))
        );
        assert_eq!(
            parse_server_message(&no_chat, Some("hint")).unwrap().chat,
            "hint"
        );
        assert!(parse_server_message(&json!({ "ClientMessage": {} }), None).is_err());
    }

    #[test]
    fn deleted_flag_overrides_the_body() {
        let m = msg(
            json!({ "ChatId": "c", "Text": { "MessageText": "x" } }),
            json!({ "Timestamp": 5, "Deleted": true }),
        );
        let m = parse_server_message(&m, None).unwrap();
        assert_eq!(m.body, WireBody::Deleted);
        assert_eq!(m.relation, Relation::None);
    }

    #[test]
    fn reply_and_forward_heuristic() {
        let original = json!({
            "Payload": { "ChatId": "src", "Text": { "MessageText": "orig" } },
            "ServerMessageInfo": { "Timestamp": 3, "From": { "Guid": "g", "DisplayName": "Anna" } }
        });
        // Quote style: reply even without originals.
        let quoted = msg(
            json!({ "ChatId": "c", "Text": { "MessageText": "re" }, "ForwardedMessageStyles": [{ "Quote": "orig" }] }),
            json!({ "Timestamp": 9 }),
        );
        let m = parse_server_message(&quoted, None).unwrap();
        assert!(matches!(&m.relation, Relation::Reply { quote: Some(q), .. } if q == "orig"));

        // Originals and no own content: forward.
        let mut pure = msg(json!({ "ChatId": "c" }), json!({ "Timestamp": 9 }));
        pure["ForwardedMessages"] = json!([original]);
        let m = parse_server_message(&pure, None).unwrap();
        assert_eq!(m.body, WireBody::Empty);
        let Relation::Forward { originals } = m.relation else {
            panic!("not a forward")
        };
        assert_eq!(
            originals[0].from.as_ref().unwrap().display_name.as_deref(),
            Some("Anna")
        );
        assert_eq!(originals[0].timestamp, Some(3));
        assert_eq!(originals[0].chat.as_deref(), Some("src"));

        // Originals next to own text: reply.
        let mut commented = msg(
            json!({ "ChatId": "c", "Text": { "MessageText": "hm" } }),
            json!({ "Timestamp": 9 }),
        );
        commented["ForwardedMessages"] = json!([original]);
        let m = parse_server_message(&commented, None).unwrap();
        assert!(matches!(
            m.relation,
            Relation::Reply {
                original: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn unknown_content_is_a_placeholder_not_an_error() {
        let m = msg(
            json!({ "ChatId": "c", "Hologram": {} }),
            json!({ "Timestamp": 5 }),
        );
        assert_eq!(
            parse_server_message(&m, None).unwrap().body,
            WireBody::Other("Hologram".into())
        );
        let m =
            json!({ "ClientMessage": { "Reaction": {} }, "ServerMessageInfo": { "Timestamp": 5 } });
        assert_eq!(
            parse_server_message(&m, Some("c")).unwrap().body,
            WireBody::Other("Reaction".into())
        );
        // An image without a file id cannot be fetched: placeholder, not a broken thumbnail.
        let m = msg(
            json!({ "ChatId": "c", "Image": { "Width": 1, "Height": 1 } }),
            json!({ "Timestamp": 5 }),
        );
        assert_eq!(
            parse_server_message(&m, None).unwrap().body,
            WireBody::Other("Image".into())
        );
    }

    #[test]
    fn importance_flag_is_read_from_plain_and_ephemeral() {
        let important = |plain: Json, info: Json| {
            parse_server_message(&msg(plain, info), None)
                .unwrap()
                .important
        };
        let text = || json!({ "MessageText": "x" });
        let info = || json!({ "Timestamp": 5 });

        assert!(important(
            json!({ "ChatId": "c", "Text": text(), "IsImportant": true }),
            info()
        ));
        assert!(!important(
            json!({ "ChatId": "c", "Text": text(), "IsImportant": false }),
            info()
        ));
        assert!(!important(json!({ "ChatId": "c", "Text": text() }), info()));
        assert!(!important(
            json!({ "ChatId": "c", "Text": text(), "IsImportant": null }),
            info()
        ));
        for junk in [json!("true"), json!(1), json!([true]), json!({})] {
            assert!(
                !important(
                    json!({ "ChatId": "c", "Text": text(), "IsImportant": junk }),
                    info()
                ),
                "{junk}"
            );
        }

        // The flag is meta, not content: a body made of the flag alone stays empty.
        let m = parse_server_message(
            &msg(json!({ "ChatId": "c", "IsImportant": true }), info()),
            None,
        )
        .unwrap();
        assert!(m.important);
        assert_eq!(m.body, WireBody::Empty);

        // `Ephemeral` is read like `Plain`.
        let ephemeral = json!({
            "ClientMessage": { "Ephemeral": { "ChatId": "c", "Text": text(), "IsImportant": true } },
            "ServerMessageInfo": info()
        });
        assert!(parse_server_message(&ephemeral, None).unwrap().important);

        // A deleted message and a non-`Plain` kind are never important.
        assert!(!important(
            json!({ "ChatId": "c", "IsImportant": true }),
            json!({ "Timestamp": 5, "Version": 2, "Deleted": true })
        ));
        let system = json!({
            "ClientMessage": { "SystemMessage": { "ChatId": "c", "IsImportant": true } },
            "ServerMessageInfo": info()
        });
        assert!(!parse_server_message(&system, None).unwrap().important);
    }

    #[test]
    fn importance_flag_survives_a_push() {
        let pushed = |flag: Json| {
            let body = msg(
                json!({ "ChatId": "c", "Text": { "MessageText": "x" }, "IsImportant": flag }),
                json!({ "Timestamp": 5, "Version": 2 }),
            );
            let Ok(PushEvent::MessageUpserted(m)) = decode_push(&body) else {
                panic!("not an upsert")
            };
            m
        };
        assert!(pushed(json!(true)).important);
        assert!(!pushed(json!(false)).important);

        let gone = msg(
            json!({ "ChatId": "c", "IsImportant": true }),
            json!({ "Timestamp": 5, "Version": 3, "Deleted": true }),
        );
        assert!(matches!(
            decode_push(&gone),
            Ok(PushEvent::MessageDeleted { .. })
        ));
    }

    #[test]
    fn push_decoding() {
        let upsert = msg(
            json!({ "ChatId": "c", "Text": { "MessageText": "hi" } }),
            json!({ "Timestamp": 5, "Version": 1 }),
        );
        assert!(
            matches!(decode_push(&upsert), Ok(PushEvent::MessageUpserted(m)) if m.timestamp == 5)
        );

        let edit = msg(
            json!({ "ChatId": "c", "Text": { "MessageText": "hi!" } }),
            json!({ "Timestamp": 5, "Version": 2, "LastEditTimestamp": 77 }),
        );
        let Ok(PushEvent::MessageUpserted(m)) = decode_push(&edit) else {
            panic!("edit")
        };
        assert!(m.edited);
        assert_eq!(m.version, Some(2));

        let gone = msg(
            json!({ "ChatId": "c" }),
            json!({ "Timestamp": 5, "Version": 3, "Deleted": true }),
        );
        assert_eq!(
            decode_push(&gone),
            Ok(PushEvent::MessageDeleted {
                chat: "c".into(),
                timestamp: 5
            })
        );

        let seen = json!({ "ClientMessage": { "SeenMarker": { "ChatId": "c" } }, "ServerMessageInfo": {} });
        assert_eq!(
            decode_push(&seen),
            Ok(PushEvent::ChatChanged { chat: "c".into() })
        );

        let typing = json!({ "ClientMessage": { "Typing": { "ChatId": "c" } } });
        assert_eq!(
            decode_push(&typing),
            Ok(PushEvent::Ignored {
                kind: "Typing".into()
            })
        );
        let seen_without_chat = json!({ "ClientMessage": { "SeenMarker": {} } });
        assert_eq!(
            decode_push(&seen_without_chat),
            Ok(PushEvent::Ignored {
                kind: "SeenMarker".into()
            })
        );

        assert_eq!(
            decode_push(&json!({})),
            Err(WireError::Missing("ClientMessage"))
        );
    }

    #[test]
    fn history_page_is_sorted_and_flags_the_start() {
        let row = |ts: i64| json!({ "ServerMessage": msg(json!({ "ChatId": "c", "Text": { "MessageText": "m" } }), json!({ "Timestamp": ts })) });
        let body = json!({ "Chats": [{ "ChatId": "c", "Messages": [row(30), row(10), row(20), row(20), { "ServerMessage": {} }] }] });
        let page = parse_history(&body, "c", 50).unwrap();
        assert_eq!(
            page.messages
                .iter()
                .map(|m| m.timestamp)
                .collect::<Vec<_>>(),
            [10, 20, 30]
        );
        assert_eq!(page.skipped, 1);
        assert!(page.reached_start);
        // A full page does not prove the start was reached.
        assert!(!parse_history(&body, "c", 5).unwrap().reached_start);
        // Chat omitted from the reply: empty history.
        let empty = parse_history(&json!({ "Chats": [] }), "c", 50).unwrap();
        assert!(empty.messages.is_empty() && empty.reached_start);
    }

    #[test]
    fn history_errors_follow_the_status() {
        assert!(matches!(
            parse_history(&json!({ "Status": 4 }), "c", 50),
            Err(ProtoError::Protocol(_))
        ));
        let entry = json!({ "Chats": [{ "ChatId": "c", "Status": 4 }] });
        assert!(matches!(
            parse_history(&entry, "c", 50),
            Err(ProtoError::Protocol(_))
        ));
        assert_eq!(
            parse_history(&json!({ "Status": 6 }), "c", 50),
            Err(ProtoError::SessionExpired)
        );
    }

    #[test]
    fn chat_list_filters_namespaces_and_clamps_unread() {
        let body = json!({ "Chats": [
            { "ChatId": "a_b", "PrivateChatInfo": {}, "PartnerInfo": { "Guid": "b", "DisplayName": "Boris" }, "LastSeqNo": 3, "LastSeenByMeSeqNo": 5 },
            { "ChatId": "95/0/x", "ChatInfo": { "Name": "hidden" } },
            { "ChatId": "5/0/x", "ChatInfo": { "Name": "hidden too" } },
            { "ChatId": "22/0/x", "ChatInfo": { "Name": "meeting" } },
            { "ChatId": "0/0/broken", "Status": 4 },
            { "Chats": "no id" },
        ] });
        let chats = parse_chat_list(&body, &DEFAULT_EXCLUDED_NAMESPACES).unwrap();
        assert_eq!(
            chats.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            ["a_b", "22/0/x"]
        );
        assert_eq!(chats[0].unread, 0);
        assert_eq!(chats[0].partner_guid.as_deref(), Some("b"));
        assert_eq!(parse_chat_list(&json!({}), &[]).unwrap(), vec![]);
        assert!(parse_chat_list(&json!({ "Chats": 1 }), &[]).is_err());
    }

    #[test]
    fn users_accept_both_casings_and_numeric_uids() {
        let data = json!({ "users": [
            { "guid": "g1", "display_name": "Ivan", "avatar_id": "a1", "uid": 4001 },
            { "Guid": "g2", "PublicName": "Olga", "Uid": "7" },
            { "display_name": "no guid" },
        ] });
        let users = parse_users(&data).unwrap();
        assert_eq!(users.len(), 2);
        assert_eq!(
            users[0],
            WireUser {
                guid: "g1".into(),
                display_name: Some("Ivan".into()),
                avatar_id: Some("a1".into()),
                uid: Some("4001".into())
            }
        );
        assert_eq!(users[1].display_name.as_deref(), Some("Olga"));
        assert_eq!(users[1].uid.as_deref(), Some("7"));
        assert_eq!(parse_users(&json!([{ "guid": "g" }])).unwrap().len(), 1);
        assert!(parse_users(&json!({})).is_err());
    }

    #[test]
    fn members_default_to_the_member_role_and_keep_unknown_roles_and_ignore_groups_and_departments()
    {
        let data = json!({
            "users": [
                { "guid": "g1", "display_name": "Ivan", "role": "admin", "avatar_id": "a1" },
                { "guid": "g2", "public_name": "Olga" },
                { "guid": "g3", "role": "subscriber", "account_deleted": true },
                { "guid": "g4", "role": "member", "account_deleted": false },
                { "display_name": "no guid" },
                "not an object",
            ],
            "groups": [{ "anything": 1 }, "any shape"],
            "departments": { "not": "an array" },
        });
        let members = parse_members(&data).unwrap();
        assert_eq!(members.skipped, 2);
        let roles: Vec<_> = members
            .users
            .iter()
            .map(|u| (u.guid.as_str(), u.role.clone(), u.deleted))
            .collect();
        assert_eq!(
            roles,
            [
                ("g1", WireMemberRole::Admin, false),
                ("g2", WireMemberRole::Member, false),
                ("g3", WireMemberRole::Other("subscriber".into()), true),
                ("g4", WireMemberRole::Member, false),
            ]
        );
        assert_eq!(members.users[1].display_name.as_deref(), Some("Olga"));
        assert_eq!(members.users[0].avatar_id.as_deref(), Some("a1"));
        let bare = parse_members(&json!({ "users": [] })).unwrap();
        assert_eq!((bare.users.len(), bare.skipped), (0, 0));
        assert!(parse_members(&json!({})).is_err());
        assert!(parse_members(&json!({ "users": 1 })).is_err());
    }

    #[test]
    fn profile_reads_the_name_the_avatar_and_a_non_blank_status_text() {
        let reply = |status: Json| {
            json!({
                "UserInfo": { "Guid": "g", "DisplayName": "Ivan", "PublicName": "ivan", "AvatarId": "av" },
                "UserStatusInfo": { "Timestamp": 1, "UserStatus": status },
            })
        };
        let unset = parse_profile(&reply(json!({}))).unwrap();
        assert_eq!(
            unset,
            WireProfile {
                guid: Some("g".into()),
                name: Some("Ivan".into()),
                avatar_id: Some("av".into()),
                status_text: None
            }
        );
        let custom = |text: Json| json!({ "Availability": "Away", "CustomStatus": { "Emoji": "x", "Text": text } });
        let set = parse_profile(&reply(custom(json!("On leave")))).unwrap();
        assert_eq!(set.status_text.as_deref(), Some("On leave"));
        assert_eq!(
            parse_profile(&reply(custom(json!("  "))))
                .unwrap()
                .status_text,
            None
        );
        assert_eq!(
            parse_profile(&reply(custom(json!(5)))).unwrap().status_text,
            None
        );
        assert_eq!(
            parse_profile(&reply(json!({ "Availability": "Away" })))
                .unwrap()
                .status_text,
            None,
            "no CustomStatus is no status"
        );
        assert_eq!(
            parse_profile(&reply(json!({ "CustomStatus": "text" })))
                .unwrap()
                .status_text,
            None
        );
        for wrong in [
            json!({ "text": "On leave" }),
            json!({ "Text": "Pascal" }),
            json!({ "CustomStatus": { "text": "lower" } }),
        ] {
            assert_eq!(
                parse_profile(&reply(wrong)).unwrap().status_text,
                None,
                "only CustomStatus.Text is a status"
            );
        }

        let bare = parse_profile(&json!({ "UserInfo": { "PublicName": "ivan" } })).unwrap();
        assert_eq!(
            (
                bare.name.as_deref(),
                bare.guid,
                bare.avatar_id,
                bare.status_text
            ),
            (Some("ivan"), None, None, None)
        );
        assert!(parse_profile(&json!({})).is_err());
        assert!(parse_profile(&json!({ "UserInfo": null })).is_err());
        assert_eq!(
            parse_profile(&json!({ "Status": 6 })),
            Err(ProtoError::SessionExpired)
        );
    }

    #[test]
    fn sender_falls_back_to_custom_from() {
        let m = msg(
            json!({ "ChatId": "c" }),
            json!({ "Timestamp": 1, "CustomFrom": { "DisplayName": "Channel", "AvatarId": "av" } }),
        );
        let from = parse_server_message(&m, None).unwrap().from.unwrap();
        assert_eq!(
            (
                from.guid,
                from.display_name.as_deref(),
                from.avatar_id.as_deref()
            ),
            (None, Some("Channel"), Some("av"))
        );
    }

    #[test]
    fn payload_id_of_an_own_send_survives_decoding() {
        let echo = msg(
            json!({ "ChatId": "c", "PayloadId": "f2e6b568-c3ce-489d-e28c8f1d", "Text": { "MessageText": "hi" } }),
            json!({ "Timestamp": 5, "From": { "Guid": "me" } }),
        );
        let Ok(PushEvent::MessageUpserted(m)) = decode_push(&echo) else {
            panic!("echo")
        };
        assert_eq!(m.payload_id.as_deref(), Some("f2e6b568-c3ce-489d-e28c8f1d"));
        let plain = msg(json!({ "ChatId": "c" }), json!({ "Timestamp": 5 }));
        assert_eq!(parse_server_message(&plain, None).unwrap().payload_id, None);
        // A system message has no `Plain` and therefore no payload id.
        let system = json!({ "ClientMessage": { "SystemMessage": { "ChatId": "c" } }, "ServerMessageInfo": { "Timestamp": 5 } });
        assert_eq!(
            parse_server_message(&system, None).unwrap().payload_id,
            None
        );
    }

    #[test]
    fn typing_push_names_the_typist_and_may_carry_a_timeout() {
        let info = json!({ "From": { "Guid": "g1", "DisplayName": "Anna" }, "Timestamp": 7 });
        let typing = |inner: Json| json!({ "ClientMessage": { "Typing": inner }, "ServerMessageInfo": info });
        assert_eq!(
            decode_push(&typing(json!({ "ChatId": "c" }))),
            Ok(PushEvent::Typing {
                chat: "c".into(),
                user: "g1".into(),
                name: Some("Anna".into()),
                timeout_secs: None
            })
        );
        assert_eq!(
            decode_push(&typing(json!({ "ChatId": "c", "TimeoutSeconds": 6 }))),
            Ok(PushEvent::Typing {
                chat: "c".into(),
                user: "g1".into(),
                name: Some("Anna".into()),
                timeout_secs: Some(6)
            })
        );
        // Without a typist there is nobody to show.
        let anonymous = json!({ "ClientMessage": { "Typing": { "ChatId": "c" } }, "ServerMessageInfo": { "Timestamp": 7 } });
        assert_eq!(
            decode_push(&anonymous),
            Ok(PushEvent::Ignored {
                kind: "Typing".into()
            })
        );
    }

    #[test]
    fn history_row_siblings_fill_the_reactions() {
        let mut row = msg(
            json!({ "ChatId": "c", "Text": { "MessageText": "hi" } }),
            json!({ "Timestamp": 5 }),
        );
        assert_eq!(
            parse_server_message(&row, None).unwrap().reactions,
            None,
            "no sibling: no reaction state"
        );

        row["Reactions"] = json!([{ "Type": 100101, "Count": 2 }, { "Type": 7, "Count": 1 }]);
        row["ReactionsVersion"] = json!(4);
        row["RecentUserReactions"] =
            json!([{ "Timestamp": 9, "Type": 100101, "UserInfo": { "Guid": "g1" } }]);
        let reactions = parse_server_message(&row, None).unwrap().reactions.unwrap();
        assert_eq!(
            reactions,
            WireReactions {
                counts: vec![
                    WireReactionCount {
                        code: 100101,
                        count: 2
                    },
                    WireReactionCount { code: 7, count: 1 }
                ],
                version: Some(4),
                recent: vec![WireReactor {
                    user: Some("g1".into()),
                    code: 100101,
                    timestamp: Some(9)
                }],
            }
        );

        // Any single sibling is enough to carry state; a version alone means "all removed".
        let mut only_version = msg(json!({ "ChatId": "c" }), json!({ "Timestamp": 5 }));
        only_version["ReactionsVersion"] = json!(6);
        let reactions = parse_server_message(&only_version, None)
            .unwrap()
            .reactions
            .unwrap();
        assert_eq!(
            (
                reactions.counts.len(),
                reactions.version,
                reactions.recent.len()
            ),
            (0, Some(6), 0)
        );

        // The same rows come out of a history page.
        let body = json!({ "Chats": [{ "ChatId": "c", "Messages": [{ "ServerMessage": row }] }] });
        assert_eq!(
            parse_history(&body, "c", 50).unwrap().messages[0]
                .reactions
                .as_ref()
                .unwrap()
                .version,
            Some(4)
        );
    }

    #[test]
    fn zero_counts_and_untyped_entries_are_dropped() {
        let mut row = msg(json!({ "ChatId": "c" }), json!({ "Timestamp": 5 }));
        row["Reactions"] = json!([
            { "Type": 1, "Count": 0 },
            { "Count": 3 },
            { "Type": 2 },
            { "Type": 3, "Count": -1 },
            { "Type": 4, "Count": 5 },
        ]);
        row["RecentUserReactions"] = json!([
            { "Timestamp": 1, "UserInfo": { "Guid": "g" } },
            { "Type": 4, "UserInfo": {} },
        ]);
        let reactions = parse_server_message(&row, None).unwrap().reactions.unwrap();
        assert_eq!(
            reactions.counts,
            vec![WireReactionCount { code: 4, count: 5 }]
        );
        assert_eq!(
            reactions.recent,
            vec![WireReactor {
                user: None,
                code: 4,
                timestamp: None
            }]
        );
    }

    #[test]
    fn reaction_push_names_the_target_actor_and_action() {
        let push = |reaction: Json, info: Json| {
            json!({
                "ClientMessage": { "Reaction": reaction },
                "Reactions": [{ "Type": 9, "Count": 1 }],
                "ReactionsVersion": 2,
                "ServerMessageInfo": info,
            })
        };
        let reactions = WireReactions {
            counts: vec![WireReactionCount { code: 9, count: 1 }],
            version: Some(2),
            recent: vec![],
        };
        let event = |action, code| PushEvent::Reaction {
            chat: "c".into(),
            timestamp: 5,
            actor: Some("g1".into()),
            action,
            code,
            reactions: reactions.clone(),
        };
        let from = json!({ "From": { "Guid": "g1" }, "Timestamp": 8 });
        assert_eq!(
            decode_push(&push(
                json!({ "ChatId": "c", "Timestamp": 5, "Type": 9, "Action": 2 }),
                from.clone()
            )),
            Ok(event(Some(ReactionAction::Replace), Some(9)))
        );
        assert_eq!(
            decode_push(&push(
                json!({ "ChatId": "c", "Timestamp": 5, "Action": 1 }),
                from.clone()
            )),
            Ok(event(Some(ReactionAction::Remove), None))
        );
        assert_eq!(
            decode_push(&push(
                json!({ "ChatId": "c", "Timestamp": 5, "Type": 9, "Action": 7 }),
                from.clone()
            )),
            Ok(event(Some(ReactionAction::Other(7)), Some(9)))
        );
        assert_eq!(
            decode_push(&push(json!({ "ChatId": "c", "Timestamp": 5 }), from)),
            Ok(event(None, None))
        );

        // The target falls back to the push's own timestamp; the actor may be unknown.
        let anonymous = push(json!({ "ChatId": "c" }), json!({ "Timestamp": 5 }));
        assert!(matches!(
            decode_push(&anonymous),
            Ok(PushEvent::Reaction {
                timestamp: 5,
                actor: None,
                ..
            })
        ));
    }

    #[test]
    fn reaction_push_without_an_address_is_ignored() {
        let info = json!({ "From": { "Guid": "g1" } });
        let ignored = || {
            Ok(PushEvent::Ignored {
                kind: "Reaction".into(),
            })
        };
        let no_chat = json!({ "ClientMessage": { "Reaction": { "Timestamp": 5, "Action": 2, "Type": 9 } }, "ServerMessageInfo": info });
        assert_eq!(decode_push(&no_chat), ignored());
        let no_timestamp = json!({ "ClientMessage": { "Reaction": { "ChatId": "c", "Action": 2, "Type": 9 } }, "ServerMessageInfo": info });
        assert_eq!(decode_push(&no_timestamp), ignored());
        assert_eq!(
            decode_push(&json!({ "ClientMessage": { "Reaction": {} } })),
            ignored()
        );
    }

    fn reactor_entry(guid: &str, code: i64, at: i64) -> Json {
        json!({
            "Type": code,
            "Timestamp": at,
            "UserInfo": { "Guid": guid, "DisplayName": format!("name-{guid}"), "AvatarId": format!("av/{guid}") },
        })
    }

    #[test]
    fn reactors_page_keeps_server_order_and_every_field() {
        let body = json!({ "UserReactions": [
            reactor_entry("g2", 100101, 30),
            reactor_entry("g1", 7, 20),
            { "Type": 9, "Timestamp": 10, "UserInfo": { "Guid": "g3", "PublicName": "pub" } },
        ] });
        let page = parse_reactors(&body).unwrap();
        assert_eq!(
            page,
            WireReactors {
                entries: vec![
                    WireReactorEntry {
                        guid: "g2".into(),
                        display_name: Some("name-g2".into()),
                        avatar_id: Some("av/g2".into()),
                        code: 100101,
                        timestamp: Some(30),
                    },
                    WireReactorEntry {
                        guid: "g1".into(),
                        display_name: Some("name-g1".into()),
                        avatar_id: Some("av/g1".into()),
                        code: 7,
                        timestamp: Some(20),
                    },
                    WireReactorEntry {
                        guid: "g3".into(),
                        display_name: Some("pub".into()),
                        avatar_id: None,
                        code: 9,
                        timestamp: Some(10),
                    },
                ],
                skipped: 0,
                raw: 3,
                oldest: Some(10),
            }
        );
    }

    #[test]
    fn reactors_page_without_the_key_is_empty() {
        for body in [
            json!({}),
            json!({ "UserReactions": null }),
            json!({ "UserReactions": [] }),
        ] {
            assert_eq!(
                parse_reactors(&body).unwrap(),
                WireReactors {
                    entries: vec![],
                    skipped: 0,
                    raw: 0,
                    oldest: None,
                }
            );
        }
    }

    #[test]
    fn reactors_without_identity_are_skipped_but_still_bound_the_page() {
        // The oldest entry is the malformed one: the cursor must still move to its timestamp.
        let body = json!({ "UserReactions": [
            reactor_entry("g1", 1, 30),
            { "Timestamp": 20, "UserInfo": { "Guid": "g2" } },
            { "Type": 1, "Timestamp": 10, "UserInfo": { "DisplayName": "x" } },
            { "Type": 1, "Timestamp": 15 },
            "garbage",
        ] });
        let page = parse_reactors(&body).unwrap();
        assert_eq!(
            page.entries
                .iter()
                .map(|e| e.guid.as_str())
                .collect::<Vec<_>>(),
            ["g1"]
        );
        assert_eq!((page.skipped, page.raw, page.oldest), (4, 5, Some(10)));
        assert_eq!(page.raw, page.entries.len() + page.skipped);
    }

    #[test]
    fn reactor_without_a_timestamp_keeps_none_and_does_not_bound_the_page() {
        let body = json!({ "UserReactions": [
            reactor_entry("g1", 1, 30),
            { "Type": 1, "UserInfo": { "Guid": "g2" } },
        ] });
        let page = parse_reactors(&body).unwrap();
        assert_eq!(page.entries[1].timestamp, None);
        assert_eq!((page.skipped, page.raw, page.oldest), (0, 2, Some(30)));

        let none = parse_reactors(&json!({ "UserReactions": [
            { "Type": 1, "UserInfo": { "Guid": "g" } },
        ] }))
        .unwrap();
        assert_eq!((none.raw, none.oldest), (1, None));
    }

    #[test]
    fn reactors_reply_that_is_not_a_page_is_a_protocol_error() {
        for body in [json!([]), json!("x"), json!(null), json!(5)] {
            assert!(
                matches!(parse_reactors(&body), Err(ProtoError::Protocol(_))),
                "{body}"
            );
        }
        assert!(matches!(
            parse_reactors(&json!({ "UserReactions": {} })),
            Err(ProtoError::Protocol(_))
        ));
        assert!(matches!(
            parse_reactors(&json!({ "Status": 4, "Details": "no message" })),
            Err(ProtoError::Protocol(m)) if m.contains("ENTITY_NOT_FOUND")
        ));
        assert_eq!(
            parse_reactors(&json!({ "Status": 6 })),
            Err(ProtoError::SessionExpired)
        );
    }
}
