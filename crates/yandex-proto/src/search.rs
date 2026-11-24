//! Reply of the registry `search` method: one result set per entity, each with its paging
//! envelope. Items stay raw JSON here; typing them waits for a live capture (see the design).

use serde_json::Value as Json;

use crate::error::ProtoError;
use crate::request::SearchEntity;

/// One result set: `{items, total, limit, page, pages}`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SearchSet {
    /// Matches over all pages.
    pub total: u64,
    /// Page size the server applied.
    pub limit: u64,
    pub page: u64,
    pub pages: u64,
    pub items: Vec<Json>,
}

/// Result sets of one `search`. A set whose entity was not requested stays empty.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SearchReply {
    pub messages: SearchSet,
    pub users: SearchSet,
    pub chats: SearchSet,
}

/// Reads the `data` object of a `search` reply. Every requested entity must come back as a
/// well-formed set, otherwise `ProtoError::Protocol`; the error names the set only, never a
/// value (items carry message text).
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
    Ok(SearchReply {
        messages: set(SearchEntity::Messages)?,
        users: set(SearchEntity::Users)?,
        chats: set(SearchEntity::Chats)?,
    })
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
}
