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
    Voice { file: WireFile, duration: Option<u64> },
    Gallery { items: Vec<WireImage>, text: Option<String> },
    Sticker { id: String, set_id: Option<String> },
    Poll { title: String },
    /// Chat event; `kind` is the variant key (`ChatCreatedInfo`, `ParticipantsChangedDiffV2`, ...).
    System { kind: String, text: Option<String> },
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
    Forward { originals: Vec<WireForwarded> },
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
    pub relation: Relation,
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
    MessageDeleted { chat: String, timestamp: Micros },
    /// Something that changes the chat row or its history without carrying a message:
    /// read state moved on another device, pin, history cleared, state sync.
    ChatChanged { chat: String },
    /// Someone is typing: `user` is the typist's guid. `timeout_secs` is the lifetime of the
    /// indicator when the push states one.
    Typing { chat: String, user: String, name: Option<String>, timeout_secs: Option<u32> },
    /// Presence, reactions and other events this client does not act on.
    Ignored { kind: String },
}

fn present<'a>(value: &'a Json, key: &str) -> Option<&'a Json> {
    value.get(key).filter(|v| !v.is_null())
}

fn string(value: &Json, key: &str) -> Option<String> {
    present(value, key)?.as_str().filter(|s| !s.is_empty()).map(str::to_owned)
}

/// Accepts a JSON integer, an integral float and a digit string.
fn micros(value: &Json) -> Option<i64> {
    match value {
        Json::Number(n) => n.as_i64().or_else(|| {
            n.as_f64().filter(|f| f.fract() == 0.0 && f.abs() < 9.0e15).map(|f| f as i64)
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
pub fn check_status(body: &Json) -> Result<(), ProtoError> {
    let Some(status) = body.get("Status").and_then(Json::as_i64).filter(|s| *s != 0) else {
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
    let details = string(body, "Details").map(|d| format!(": {d}")).unwrap_or_default();
    // OVERLOAD clears by itself; everything else is a request the server will keep refusing.
    Err(if status == 5 {
        ProtoError::Network(format!("server overloaded{details}"))
    } else {
        ProtoError::Protocol(format!("status {status} {name}{details}"))
    })
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
    Some(WireSender { guid: None, display_name: display_name(custom), avatar_id: string(custom, "AvatarId") })
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
    let dimension = |key| int_field(holder, key).and_then(|v| u32::try_from(v).ok()).unwrap_or(0);
    Some(WireImage {
        file: file_info(holder)?,
        width: dimension("Width"),
        height: dimension("Height"),
        animated: holder.get("Animated").and_then(Json::as_bool).unwrap_or(false),
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
        return WireBody::Text(text.get("MessageText").and_then(Json::as_str).unwrap_or_default().to_owned());
    }
    if let Some(sticker) = present(plain, "Sticker") {
        return match string(sticker, "Id") {
            Some(id) => WireBody::Sticker { id, set_id: string(sticker, "SetId") },
            None => WireBody::Other("Sticker".into()),
        };
    }
    if let Some(holder) = present(plain, "Image") {
        return image(holder).map_or_else(|| WireBody::Other("Image".into()), WireBody::Image);
    }
    if let Some(holder) = present(plain, "MiscFile") {
        return file_info(holder).map_or_else(|| WireBody::Other("MiscFile".into()), WireBody::File);
    }
    if present(plain, "Card").is_some() {
        return WireBody::Other("Card".into());
    }
    if let Some(gallery) = present(plain, "Gallery") {
        let items = gallery
            .get("Items")
            .and_then(Json::as_array)
            .map(|items| items.iter().filter_map(|item| image(present(item, "Image")?)).collect())
            .unwrap_or_default();
        return WireBody::Gallery { items, text: string(gallery, "Text") };
    }
    if let Some(voice) = present(plain, "Voice") {
        return match file_info(voice) {
            Some(file) => WireBody::Voice { file, duration: voice.get("Duration").and_then(Json::as_u64) },
            None => WireBody::Other("Voice".into()),
        };
    }
    if let Some(poll) = present(plain, "Poll") {
        return WireBody::Poll { title: string(poll, "Title").unwrap_or_default() };
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
    let text = system.get("GenericMessage").and_then(|g| string(g, "MessageText"));
    WireBody::System { kind, text }
}

fn forwarded(items: &Json) -> Vec<WireForwarded> {
    let Some(items) = items.as_array() else { return Vec::new() };
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

fn relation(plain: Option<&Json>, originals: Vec<WireForwarded>, has_own_content: bool) -> Relation {
    let quote = plain
        .and_then(|p| p.get("ForwardedMessageStyles")?.as_array())
        .and_then(|styles| styles.iter().find_map(|s| string(s, "Quote")));
    let reference = plain
        .and_then(|p| p.get("ForwardedMessageRefs")?.as_array())
        .and_then(|refs| refs.first())
        .map(|r| WireRef { chat: string(r, "ChatId"), timestamp: int_field(r, "Timestamp").filter(|t| *t != 0) });

    let is_reply = quote.is_some() || ((!originals.is_empty() || reference.is_some()) && has_own_content);
    if is_reply {
        return Relation::Reply { quote, reference, original: originals.into_iter().next() };
    }
    if !originals.is_empty() || reference.is_some() {
        return Relation::Forward { originals };
    }
    Relation::None
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
    let originals = raw.get("ForwardedMessages").map(forwarded).unwrap_or_default();

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
        relation,
        raw: raw.clone(),
    })
}

fn parse_chat(entry: &Json) -> Option<WireChat> {
    let id = string(entry, "ChatId")?;
    // Per-chat failure (`Status` != 0): the row has no usable metadata.
    if entry.get("Status").and_then(Json::as_i64).is_some_and(|s| s != 0) {
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
    let title = partner.and_then(display_name).or_else(|| info.and_then(|i| string(i, "Name")));
    let avatar_id = partner
        .and_then(|p| string(p, "AvatarId"))
        .or_else(|| info.and_then(|i| string(i, "AvatarId")));

    let last_seq = int_field(entry, "LastSeqNo").unwrap_or(0);
    let seen = int_field(entry, "LastSeenByMeSeqNo").unwrap_or(0);
    let last_message = entry
        .get("Messages")
        .and_then(Json::as_array)
        .and_then(|rows| rows.iter().filter_map(|r| present(r, "ServerMessage")).next_back())
        .and_then(|m| parse_server_message(m, Some(&id)).ok());

    let mut raw = entry.clone();
    if let Some(o) = raw.as_object_mut() {
        o.remove("Messages");
    }
    Some(WireChat {
        kind,
        title,
        avatar_id,
        partner_guid: if private { partner.and_then(|p| string(p, "Guid")) } else { None },
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
    let Some(entries) = body.get("Chats") else { return Ok(Vec::new()) };
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
    let entries = body.get("Chats").and_then(Json::as_array).map(Vec::as_slice).unwrap_or_default();
    let entry = entries.iter().find(|e| string(e, "ChatId").as_deref() == Some(chat));
    let Some(entry) = entry else {
        // A chat without messages may be omitted entirely.
        return Ok(WireHistory { chat: chat.to_owned(), messages: Vec::new(), reached_start: true, skipped: 0 });
    };
    check_status(entry)?;

    let rows = entry.get("Messages").and_then(Json::as_array).map(Vec::as_slice).unwrap_or_default();
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
                display_name: pick(u, "display_name", "DisplayName").or_else(|| pick(u, "public_name", "PublicName")),
                avatar_id: pick(u, "avatar_id", "AvatarId"),
                uid: id_string(u, "uid").or_else(|| id_string(u, "Uid")),
            })
        })
        .collect())
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
            PushEvent::MessageDeleted { chat: message.chat, timestamp: message.timestamp }
        } else {
            PushEvent::MessageUpserted(message)
        });
    }
    if let Some(typing) = present(client, "Typing") {
        let from = present(body, "ServerMessageInfo").and_then(sender);
        if let (Some(chat), Some(WireSender { guid: Some(user), display_name: name, .. })) = (string(typing, "ChatId"), from) {
            let timeout_secs = int_field(typing, "TimeoutSeconds").and_then(|s| u32::try_from(s).ok());
            return Ok(PushEvent::Typing { chat, user, name, timeout_secs });
        }
    }
    // `NotificationBehaviour` rides along with real kinds (seen markers, bot requests): a
    // modifier, not the event.
    const MODIFIERS: [&str; 1] = ["NotificationBehaviour"];
    let chat_changing = ["SeenMarker", "Pin", "ClearUserHistory", "StateSync", "ChatApproval"];
    if let Some(chat) = chat_changing.iter().find_map(|kind| string(present(client, kind)?, "ChatId")) {
        return Ok(PushEvent::ChatChanged { chat });
    }
    let keys = client.as_object().into_iter().flat_map(|o| o.keys());
    let kind = keys.clone().find(|k| !MODIFIERS.contains(&k.as_str())).or_else(|| keys.clone().next()).cloned().unwrap_or_default();
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
        assert_eq!(micros(&json!(1784117592261029_i64)), Some(1_784_117_592_261_029));
        assert_eq!(micros(&json!(1784117592261029.0)), Some(1_784_117_592_261_029));
        assert_eq!(micros(&json!("1784117592261029")), Some(1_784_117_592_261_029));
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
        assert_eq!(check_status(&json!({ "Status": 6 })), Err(ProtoError::SessionExpired));
        assert!(matches!(check_status(&json!({ "Status": 4, "Details": "no chat" })), Err(ProtoError::Protocol(m)) if m.contains("ENTITY_NOT_FOUND") && m.contains("no chat")));
        assert!(matches!(check_status(&json!({ "Status": 5 })), Err(ProtoError::Network(_))));
    }

    #[test]
    fn message_requires_an_address() {
        let no_ts = msg(json!({ "ChatId": "c" }), json!({}));
        assert_eq!(parse_server_message(&no_ts, None), Err(WireError::Missing("Timestamp")));
        let no_chat = msg(json!({}), json!({ "Timestamp": 5 }));
        assert_eq!(parse_server_message(&no_chat, None), Err(WireError::Missing("ChatId")));
        assert_eq!(parse_server_message(&no_chat, Some("hint")).unwrap().chat, "hint");
        assert!(parse_server_message(&json!({ "ClientMessage": {} }), None).is_err());
    }

    #[test]
    fn deleted_flag_overrides_the_body() {
        let m = msg(json!({ "ChatId": "c", "Text": { "MessageText": "x" } }), json!({ "Timestamp": 5, "Deleted": true }));
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
        let Relation::Forward { originals } = m.relation else { panic!("not a forward") };
        assert_eq!(originals[0].from.as_ref().unwrap().display_name.as_deref(), Some("Anna"));
        assert_eq!(originals[0].timestamp, Some(3));
        assert_eq!(originals[0].chat.as_deref(), Some("src"));

        // Originals next to own text: reply.
        let mut commented = msg(json!({ "ChatId": "c", "Text": { "MessageText": "hm" } }), json!({ "Timestamp": 9 }));
        commented["ForwardedMessages"] = json!([original]);
        let m = parse_server_message(&commented, None).unwrap();
        assert!(matches!(m.relation, Relation::Reply { original: Some(_), .. }));
    }

    #[test]
    fn unknown_content_is_a_placeholder_not_an_error() {
        let m = msg(json!({ "ChatId": "c", "Hologram": {} }), json!({ "Timestamp": 5 }));
        assert_eq!(parse_server_message(&m, None).unwrap().body, WireBody::Other("Hologram".into()));
        let m = json!({ "ClientMessage": { "Reaction": {} }, "ServerMessageInfo": { "Timestamp": 5 } });
        assert_eq!(parse_server_message(&m, Some("c")).unwrap().body, WireBody::Other("Reaction".into()));
        // An image without a file id cannot be fetched: placeholder, not a broken thumbnail.
        let m = msg(json!({ "ChatId": "c", "Image": { "Width": 1, "Height": 1 } }), json!({ "Timestamp": 5 }));
        assert_eq!(parse_server_message(&m, None).unwrap().body, WireBody::Other("Image".into()));
    }

    #[test]
    fn push_decoding() {
        let upsert = msg(json!({ "ChatId": "c", "Text": { "MessageText": "hi" } }), json!({ "Timestamp": 5, "Version": 1 }));
        assert!(matches!(decode_push(&upsert), Ok(PushEvent::MessageUpserted(m)) if m.timestamp == 5));

        let edit = msg(
            json!({ "ChatId": "c", "Text": { "MessageText": "hi!" } }),
            json!({ "Timestamp": 5, "Version": 2, "LastEditTimestamp": 77 }),
        );
        let Ok(PushEvent::MessageUpserted(m)) = decode_push(&edit) else { panic!("edit") };
        assert!(m.edited);
        assert_eq!(m.version, Some(2));

        let gone = msg(json!({ "ChatId": "c" }), json!({ "Timestamp": 5, "Version": 3, "Deleted": true }));
        assert_eq!(decode_push(&gone), Ok(PushEvent::MessageDeleted { chat: "c".into(), timestamp: 5 }));

        let seen = json!({ "ClientMessage": { "SeenMarker": { "ChatId": "c" } }, "ServerMessageInfo": {} });
        assert_eq!(decode_push(&seen), Ok(PushEvent::ChatChanged { chat: "c".into() }));

        let typing = json!({ "ClientMessage": { "Typing": { "ChatId": "c" } } });
        assert_eq!(decode_push(&typing), Ok(PushEvent::Ignored { kind: "Typing".into() }));
        let seen_without_chat = json!({ "ClientMessage": { "SeenMarker": {} } });
        assert_eq!(decode_push(&seen_without_chat), Ok(PushEvent::Ignored { kind: "SeenMarker".into() }));

        assert_eq!(decode_push(&json!({})), Err(WireError::Missing("ClientMessage")));
    }

    #[test]
    fn history_page_is_sorted_and_flags_the_start() {
        let row = |ts: i64| json!({ "ServerMessage": msg(json!({ "ChatId": "c", "Text": { "MessageText": "m" } }), json!({ "Timestamp": ts })) });
        let body = json!({ "Chats": [{ "ChatId": "c", "Messages": [row(30), row(10), row(20), row(20), { "ServerMessage": {} }] }] });
        let page = parse_history(&body, "c", 50).unwrap();
        assert_eq!(page.messages.iter().map(|m| m.timestamp).collect::<Vec<_>>(), [10, 20, 30]);
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
        assert!(matches!(parse_history(&json!({ "Status": 4 }), "c", 50), Err(ProtoError::Protocol(_))));
        let entry = json!({ "Chats": [{ "ChatId": "c", "Status": 4 }] });
        assert!(matches!(parse_history(&entry, "c", 50), Err(ProtoError::Protocol(_))));
        assert_eq!(parse_history(&json!({ "Status": 6 }), "c", 50), Err(ProtoError::SessionExpired));
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
        assert_eq!(chats.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["a_b", "22/0/x"]);
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
        assert_eq!(users[0], WireUser { guid: "g1".into(), display_name: Some("Ivan".into()), avatar_id: Some("a1".into()), uid: Some("4001".into()) });
        assert_eq!(users[1].display_name.as_deref(), Some("Olga"));
        assert_eq!(users[1].uid.as_deref(), Some("7"));
        assert_eq!(parse_users(&json!([{ "guid": "g" }])).unwrap().len(), 1);
        assert!(parse_users(&json!({})).is_err());
    }

    #[test]
    fn sender_falls_back_to_custom_from() {
        let m = msg(json!({ "ChatId": "c" }), json!({ "Timestamp": 1, "CustomFrom": { "DisplayName": "Channel", "AvatarId": "av" } }));
        let from = parse_server_message(&m, None).unwrap().from.unwrap();
        assert_eq!((from.guid, from.display_name.as_deref(), from.avatar_id.as_deref()), (None, Some("Channel"), Some("av")));
    }

    #[test]
    fn payload_id_of_an_own_send_survives_decoding() {
        let echo = msg(
            json!({ "ChatId": "c", "PayloadId": "f2e6b568-c3ce-489d-e28c8f1d", "Text": { "MessageText": "hi" } }),
            json!({ "Timestamp": 5, "From": { "Guid": "me" } }),
        );
        let Ok(PushEvent::MessageUpserted(m)) = decode_push(&echo) else { panic!("echo") };
        assert_eq!(m.payload_id.as_deref(), Some("f2e6b568-c3ce-489d-e28c8f1d"));
        let plain = msg(json!({ "ChatId": "c" }), json!({ "Timestamp": 5 }));
        assert_eq!(parse_server_message(&plain, None).unwrap().payload_id, None);
        // A system message has no `Plain` and therefore no payload id.
        let system = json!({ "ClientMessage": { "SystemMessage": { "ChatId": "c" } }, "ServerMessageInfo": { "Timestamp": 5 } });
        assert_eq!(parse_server_message(&system, None).unwrap().payload_id, None);
    }

    #[test]
    fn typing_push_names_the_typist_and_may_carry_a_timeout() {
        let info = json!({ "From": { "Guid": "g1", "DisplayName": "Anna" }, "Timestamp": 7 });
        let typing = |inner: Json| json!({ "ClientMessage": { "Typing": inner }, "ServerMessageInfo": info });
        assert_eq!(
            decode_push(&typing(json!({ "ChatId": "c" }))),
            Ok(PushEvent::Typing { chat: "c".into(), user: "g1".into(), name: Some("Anna".into()), timeout_secs: None })
        );
        assert_eq!(
            decode_push(&typing(json!({ "ChatId": "c", "TimeoutSeconds": 6 }))),
            Ok(PushEvent::Typing { chat: "c".into(), user: "g1".into(), name: Some("Anna".into()), timeout_secs: Some(6) })
        );
        // Without a typist there is nobody to show.
        let anonymous = json!({ "ClientMessage": { "Typing": { "ChatId": "c" } }, "ServerMessageInfo": { "Timestamp": 7 } });
        assert_eq!(decode_push(&anonymous), Ok(PushEvent::Ignored { kind: "Typing".into() }));
    }
}
