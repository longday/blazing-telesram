//! Reply of the registry `search` method: one result set per entity, each with its paging
//! envelope. The raw items stay on [`SearchSet`]; [`SearchItems`] holds them decoded with the
//! shape captured from a live account (`tests/fixtures/live-search.json`). Decoding is tolerant,
//! as in `wire.rs`: an item without an address is skipped and counted, never an error, and
//! unknown fields are ignored.

use serde_json::Value as Json;

use crate::error::ProtoError;
use crate::request::SearchEntity;
use crate::wire::{Micros, WireBody, WireChatKind, WireSender, namespace_of, parse_server_message};

/// One result set: `{items, total, limit, page, pages}`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SearchSet {
    /// Matches over all pages.
    pub total: u64,
    /// Page size the server applied.
    pub limit: u64,
    pub page: u64,
    pub pages: u64,
    /// The items as received, for the probe and for the fixture.
    pub items: Vec<Json>,
}

/// A message hit: `data` is a `ServerMessage` (`{ClientMessage, ServerMessageInfo}`), so it is
/// decoded by the same parser as history rows. The hit carries no chat title.
#[derive(Clone, Debug, PartialEq)]
pub struct WireSearchMessage {
    /// `ClientMessage.Plain.ChatId`.
    pub chat: String,
    /// `ServerMessageInfo.Timestamp`; with `chat` it addresses the message.
    pub timestamp: Micros,
    pub from: Option<WireSender>,
    /// Any `ClientMessage` shape: an unknown one is `WireBody::Other`.
    pub body: WireBody,
}

/// A user hit (`type: users_pvp`). These are not the chat partners: the set is usually empty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WireSearchUser {
    pub guid: String,
    /// `display_name`, else `public_name`.
    pub display_name: Option<String>,
    pub avatar_id: Option<String>,
}

/// A chat hit. The item has no private-chat marker (private chats have no name), so the kind is
/// `Channel` for namespace 1 and `Group` otherwise, as `wire.rs` classifies a named chat.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WireSearchChat {
    /// `chat_id`, the id format of the chat list (`0/22/<guid>`). A hit may be absent from the
    /// loaded chat list.
    pub id: String,
    pub kind: WireChatKind,
    pub title: Option<String>,
    pub avatar_id: Option<String>,
    pub members_count: Option<u64>,
}

/// The items of one reply, decoded.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SearchItems {
    pub messages: Vec<WireSearchMessage>,
    pub users: Vec<WireSearchUser>,
    pub chats: Vec<WireSearchChat>,
    /// Items without an address (chat id / guid / chat id plus timestamp), left out above.
    pub skipped: usize,
}

/// Result sets of one `search`. A set whose entity was not requested stays empty.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SearchReply {
    pub messages: SearchSet,
    pub users: SearchSet,
    pub chats: SearchSet,
    /// The items of the three sets, decoded.
    pub decoded: SearchItems,
}

fn text(value: &Json, key: &str) -> Option<String> {
    value.get(key)?.as_str().filter(|s| !s.is_empty()).map(str::to_owned)
}

fn message_hit(item: &Json) -> Option<WireSearchMessage> {
    let message = parse_server_message(item.get("data")?, None).ok()?;
    Some(WireSearchMessage { chat: message.chat, timestamp: message.timestamp, from: message.from, body: message.body })
}

fn user_hit(item: &Json) -> Option<WireSearchUser> {
    let data = item.get("data")?;
    Some(WireSearchUser {
        guid: text(data, "guid")?,
        display_name: text(data, "display_name").or_else(|| text(data, "public_name")),
        avatar_id: text(data, "avatar_id"),
    })
}

fn chat_hit(item: &Json) -> Option<WireSearchChat> {
    let data = item.get("data")?;
    let id = text(data, "chat_id")?;
    let kind = if namespace_of(&id) == Some(1) { WireChatKind::Channel } else { WireChatKind::Group };
    Some(WireSearchChat {
        kind,
        title: text(data, "name"),
        avatar_id: text(data, "avatar_id"),
        members_count: data.get("members_count").or_else(|| item.get("member_count")).and_then(Json::as_u64),
        id,
    })
}

/// Decodes every item of `items`, counting the ones `hit` rejects.
fn decode<T>(items: &[Json], hit: fn(&Json) -> Option<T>, skipped: &mut usize) -> Vec<T> {
    let hits: Vec<T> = items.iter().filter_map(hit).collect();
    *skipped += items.len() - hits.len();
    hits
}

/// Reads the `data` object of a `search` reply. Every requested entity must come back as a
/// well-formed set, otherwise `ProtoError::Protocol`; the error names the set only, never a
/// value (items carry message text). Items inside a well-formed set never fail the parse.
pub fn parse_search(data: &Json, requested: &[SearchEntity]) -> Result<SearchReply, ProtoError> {
    if !data.is_object() {
        return Err(ProtoError::Protocol("search reply data is not an object".into()));
    }
    let set = |entity: SearchEntity| -> Result<SearchSet, ProtoError> {
        if !requested.contains(&entity) {
            return Ok(SearchSet::default());
        }
        let name = entity.wire_name();
        let malformed = || ProtoError::Protocol(format!("search reply has a malformed {name} set"));
        let set = data.get(name).and_then(Json::as_object).ok_or_else(|| {
            ProtoError::Protocol(format!("search reply has no {name} set"))
        })?;
        let number = |key: &str| set.get(key).and_then(Json::as_u64).ok_or_else(malformed);
        Ok(SearchSet {
            total: number("total")?,
            limit: number("limit")?,
            page: number("page")?,
            pages: number("pages")?,
            items: set.get("items").and_then(Json::as_array).ok_or_else(malformed)?.clone(),
        })
    };
    let messages = set(SearchEntity::Messages)?;
    let users = set(SearchEntity::Users)?;
    let chats = set(SearchEntity::Chats)?;
    let mut skipped = 0;
    let decoded = SearchItems {
        messages: decode(&messages.items, message_hit, &mut skipped),
        users: decode(&users.items, user_hit, &mut skipped),
        chats: decode(&chats.items, chat_hit, &mut skipped),
        skipped,
    };
    Ok(SearchReply { messages, users, chats, decoded })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const ALL: [SearchEntity; 3] = [SearchEntity::Messages, SearchEntity::Users, SearchEntity::Chats];

    fn set(items: Json, total: u64) -> Json {
        json!({ "items": items, "total": total, "limit": 10, "page": 1, "pages": 1 })
    }

    #[test]
    fn a_normal_reply_keeps_every_set() {
        let data = json!({
            "messages": set(json!([{"a": 1}, {"b": 2}]), 2),
            "users": set(json!([{"u": 1}]), 1),
            "chats": set(json!([]), 0),
            "warnings": { "users": [] },
        });
        let reply = parse_search(&data, &ALL).unwrap();
        assert_eq!(reply.messages, SearchSet { total: 2, limit: 10, page: 1, pages: 1, items: vec![json!({"a": 1}), json!({"b": 2})] });
        assert_eq!(reply.users.items, vec![json!({"u": 1})]);
        assert!(reply.chats.items.is_empty());
    }

    #[test]
    fn an_empty_reply_is_not_an_error() {
        let empty = json!({ "items": [], "total": 0, "limit": 0, "page": 1, "pages": 1 });
        let data = json!({ "messages": empty, "users": empty, "chats": empty });
        let reply = parse_search(&data, &ALL).unwrap();
        assert!(reply.messages.items.is_empty() && reply.users.items.is_empty() && reply.chats.items.is_empty());
        assert_eq!((reply.messages.total, reply.messages.pages), (0, 1));
    }

    #[test]
    fn a_missing_requested_set_is_a_protocol_error() {
        let data = json!({ "messages": set(json!([]), 0), "users": set(json!([]), 0) });
        let err = parse_search(&data, &ALL).unwrap_err();
        assert!(matches!(&err, ProtoError::Protocol(m) if m.contains("chats")), "{err:?}");
    }

    #[test]
    fn unrequested_sets_are_ignored_even_when_absent() {
        let data = json!({ "users": set(json!([{"u": 1}]), 1) });
        let reply = parse_search(&data, &[SearchEntity::Users]).unwrap();
        assert_eq!(reply.users.total, 1);
        assert_eq!(reply.messages, SearchSet::default());
        assert_eq!(reply.chats, SearchSet::default());
    }

    #[test]
    fn a_malformed_set_is_a_protocol_error_that_leaks_no_values() {
        let good = || set(json!([]), 0);
        for bad in [
            json!({ "items": "secret text", "total": 1, "limit": 10, "page": 1, "pages": 1 }),
            json!({ "items": [], "total": "secret text", "limit": 10, "page": 1, "pages": 1 }),
            json!({ "items": [], "total": 1, "limit": 10, "page": 1 }),
            json!(["secret text"]),
        ] {
            let data = json!({ "messages": bad, "users": good(), "chats": good() });
            let err = parse_search(&data, &ALL).unwrap_err();
            assert!(matches!(&err, ProtoError::Protocol(m) if m.contains("messages") && !m.contains("secret")), "{err:?}");
        }
        assert!(matches!(parse_search(&json!([]), &ALL), Err(ProtoError::Protocol(_))));
    }

    fn message_item(chat: Option<&str>, timestamp: Option<i64>) -> Json {
        let mut plain = json!({ "Text": { "MessageText": "hello" } });
        if let Some(chat) = chat {
            plain["ChatId"] = json!(chat);
        }
        let mut info = json!({ "From": { "Guid": "g1", "DisplayName": "Ann" }, "Unknown": [1] });
        if let Some(timestamp) = timestamp {
            info["Timestamp"] = json!(timestamp);
        }
        json!({ "data": { "ClientMessage": { "Plain": plain }, "ServerMessageInfo": info }, "entity": "message", "type": "messages", "extra": 1 })
    }

    #[test]
    fn items_without_an_address_are_skipped_and_counted() {
        let data = json!({
            "messages": set(json!([
                message_item(Some("0/22/c"), Some(5)),
                message_item(None, Some(6)),
                message_item(Some("0/22/c"), None),
                { "entity": "message" },
            ]), 4),
            "users": set(json!([
                { "data": { "guid": "u1", "public_name": "Bob", "unknown": true } },
                { "data": { "display_name": "no guid" } },
                { "data": { "guid": "" } },
            ]), 3),
            "chats": set(json!([
                { "data": { "chat_id": "1/0/ch", "name": "News" }, "member_count": 9 },
                { "data": { "name": "no id" } },
            ]), 2),
        });
        let reply = parse_search(&data, &ALL).unwrap();
        assert_eq!(reply.decoded.skipped, 3 + 2 + 1);

        let [message] = reply.decoded.messages.as_slice() else { panic!("{:?}", reply.decoded.messages) };
        assert_eq!((message.chat.as_str(), message.timestamp), ("0/22/c", 5));
        assert!(matches!(&message.body, WireBody::Text(t) if t == "hello"));
        assert_eq!(message.from.as_ref().and_then(|f| f.display_name.as_deref()), Some("Ann"));

        let [user] = reply.decoded.users.as_slice() else { panic!("{:?}", reply.decoded.users) };
        assert_eq!((user.guid.as_str(), user.display_name.as_deref(), user.avatar_id.as_deref()), ("u1", Some("Bob"), None));

        let [chat] = reply.decoded.chats.as_slice() else { panic!("{:?}", reply.decoded.chats) };
        assert_eq!((chat.id.as_str(), chat.kind.clone(), chat.title.as_deref(), chat.members_count), ("1/0/ch", WireChatKind::Channel, Some("News"), Some(9)));

        // The raw items stay untouched, skipped ones included.
        assert_eq!(reply.messages.items.len(), 4);
    }

    #[test]
    fn an_unknown_message_shape_decodes_to_a_placeholder_body() {
        let item = json!({ "data": {
            "ClientMessage": { "Plain": { "ChatId": "0/22/c", "Sticker": { "Id": "s" } } },
            "ServerMessageInfo": { "Timestamp": 7 },
        }});
        let hit = message_hit(&item).unwrap();
        assert!(hit.from.is_none());
        assert!(!matches!(hit.body, WireBody::Text(_)));
    }
}
