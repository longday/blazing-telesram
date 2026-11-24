//! The closed set of read requests this client can express.
//!
//! There is deliberately no "call any method" entry point: every read is a variant of
//! [`ReadRequest`], its wire method comes from [`ReadRequest::method`], and
//! [`ALLOWED_METHODS`] lists every name it can put on the wire. Writes (messages, edits,
//! deletions, read markers, typing; all the socket method `push`) are a separate closed set in
//! [`crate::write`]; reactions and presence heartbeats are not representable at all.

use serde_json::{Value, json};

use crate::frame::RequestFrame;

/// Rows per history page, from the web client's `historyChunkSize`.
pub const HISTORY_CHUNK: u32 = 50;

/// Every method name `ReadRequest` can put on the wire.
pub const ALLOWED_METHODS: [&str; 7] = [
    "request_user",
    "get_users_data",
    "history",
    "subscribe",
    "search",
    "whoami",
    "get_chat_members",
];

/// Members per `get_chat_members` page, what the web client asks for (the server default is 20).
pub const MEMBERS_PAGE: u32 = 50;

/// Rows per result set requested from `search` (the server default), the same for every entity.
pub const SEARCH_LIMIT: u32 = 10;

/// Lifetime of one app-level chat subscription (the web client's `TtlMcs`).
pub(crate) const SUBSCRIPTION_TTL_MCS: i64 = 60_000_000;

pub(crate) const USER_AGENT: &str = "chats-web/3.22.0";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadRequest {
    /// Registry HTTP; needs a CSRF token.
    RequestUser,
    /// Registry HTTP; works on the bare cookie.
    GetUsersData { guids: Vec<String> },
    /// Socket `history` without a chat id: metadata of every chat plus its newest message.
    ChatList,
    /// Socket `history` of one chat: up to `limit` messages strictly older than `before`
    /// (`MaxTimestamp` is an exclusive bound), newest page when `before` is `None`.
    History { chat: String, before: Option<i64>, limit: u32 },
    /// Socket `subscribe` for live events of one chat.
    SubscribeChat { chat: String },
    /// Registry `search`; works on the bare cookie. One result set per requested entity.
    Search { query: String, limit: u32, entities: Vec<SearchEntity> },
    /// Socket `whoami`: the profile of the signed-in account (`guid` is the session's own).
    Whoami { guid: String },
    /// Registry `get_chat_members`; works on the bare cookie. One page of up to `limit` users;
    /// the next page starts after `guid_offset` (the guid of the previous page's last user).
    /// Answers group chats only: private chats, the self chat and channels are refused with
    /// `no_permissions_for_this_chat_operation`. `get_group_members` is deliberately not
    /// representable: it answers `bad_request` for every shape and no client code calls it.
    ChatMembers { chat: String, limit: u32, guid_offset: Option<String>, roles: Option<Vec<RoleFilter>> },
}

/// Role names `get_chat_members` accepts in `roles` (any other value is `bad_request`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoleFilter {
    Admin,
    Member,
}

impl RoleFilter {
    /// Name in the `roles` request parameter.
    pub const fn wire_name(self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::Member => "member",
        }
    }
}

/// What a `search` can look for. `contacts` is deliberately not representable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchEntity {
    Messages,
    Users,
    Chats,
}

impl SearchEntity {
    /// Name in the `entities` request parameter.
    pub const fn wire_name(self) -> &'static str {
        match self {
            Self::Messages => "messages",
            Self::Users => "users",
            Self::Chats => "chats",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    Registry { csrf: bool },
    Socket,
}

/// Per-frame values the socket layer owns.
pub(crate) struct SocketContext<'a> {
    pub req_id: u64,
    /// Backend correlation id carried in the JSON body.
    pub request_id: &'a str,
    /// Xiva subscription id from the `subscribed` operation frame.
    pub subscription_id: Option<&'a str>,
}

impl ReadRequest {
    pub fn method(&self) -> &'static str {
        match self {
            Self::RequestUser => "request_user",
            Self::GetUsersData { .. } => "get_users_data",
            Self::ChatList | Self::History { .. } => "history",
            Self::Search { .. } => "search",
            Self::Whoami { .. } => "whoami",
            Self::ChatMembers { .. } => "get_chat_members",
            Self::SubscribeChat { .. } => "subscribe",
        }
    }

    pub fn route(&self) -> Route {
        match self {
            Self::RequestUser => Route::Registry { csrf: true },
            Self::GetUsersData { .. } | Self::Search { .. } | Self::ChatMembers { .. } => Route::Registry { csrf: false },
            Self::ChatList | Self::History { .. } | Self::SubscribeChat { .. } | Self::Whoami { .. } => Route::Socket,
        }
    }

    /// Value of the multipart field `request`; `None` for socket requests.
    pub(crate) fn registry_body(&self) -> Option<String> {
        let params = match self {
            Self::RequestUser => json!({ "bind_phone_number": false }),
            Self::GetUsersData { guids } => json!({ "guids": guids }),
            Self::Search { query, limit, entities } => json!({
                "query": query,
                "limit": limit,
                "entities": entities.iter().map(|e| e.wire_name()).collect::<Vec<_>>(),
            }),
            Self::ChatMembers { chat, limit, guid_offset, roles } => {
                let mut params = json!({ "chat_id": chat, "limit": limit });
                if let Some(offset) = guid_offset {
                    params["guid_offset"] = Value::from(offset.as_str());
                }
                if let Some(roles) = roles {
                    params["roles"] = roles.iter().map(|r| r.wire_name()).collect::<Vec<_>>().into();
                }
                params
            }
            _ => return None,
        };
        Some(json!({ "method": self.method(), "params": params }).to_string())
    }

    /// `None` for registry requests, and for `SubscribeChat` before the subscription id is known.
    pub(crate) fn socket_frame(&self, ctx: &SocketContext<'_>) -> Option<RequestFrame> {
        let mut body = match self {
            Self::ChatList => json!({ "Limit": 1, "ChatDataFilter": {} }),
            Self::History { chat, before, limit } => {
                let mut body = json!({ "ChatId": chat, "Limit": limit });
                if let Some(before) = before {
                    body["MaxTimestamp"] = Value::from(*before);
                }
                body
            }
            Self::Whoami { guid } => json!({ "Guid": guid }),
            Self::SubscribeChat { chat } => json!({
                "ChatId": chat,
                "TtlMcs": SUBSCRIPTION_TTL_MCS,
                "ClientTransportId": { "XivaSubscriptionId": ctx.subscription_id? },
                "UserAgent": USER_AGENT,
                "ClientSupportedFeatures": 0,
            }),
            // Registry requests have no socket frame.
            Self::RequestUser | Self::GetUsersData { .. } | Self::Search { .. } | Self::ChatMembers { .. } => return None,
        };
        body["RequestId"] = Value::from(ctx.request_id);
        Some(RequestFrame {
            service_index: 0,
            req_id: ctx.req_id,
            method: self.method().to_owned(),
            body,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One value per variant. The exhaustive `match` makes adding a variant a compile error
    /// here until it is added to this list and therefore checked against the allowlist.
    fn every_request() -> Vec<ReadRequest> {
        let all = vec![
            ReadRequest::RequestUser,
            ReadRequest::GetUsersData { guids: vec!["g1".into(), "g2".into()] },
            ReadRequest::ChatList,
            ReadRequest::History { chat: "0/0/c".into(), before: None, limit: HISTORY_CHUNK },
            ReadRequest::History { chat: "0/0/c".into(), before: Some(1_784_117_592_261_029), limit: 50 },
            ReadRequest::SubscribeChat { chat: "0/0/c".into() },
            ReadRequest::Search {
                query: "q".into(),
                limit: SEARCH_LIMIT,
                entities: vec![SearchEntity::Messages, SearchEntity::Users, SearchEntity::Chats],
            },
            ReadRequest::Whoami { guid: "g-self".into() },
            ReadRequest::ChatMembers { chat: "0/0/c".into(), limit: MEMBERS_PAGE, guid_offset: None, roles: None },
            ReadRequest::ChatMembers {
                chat: "0/0/c".into(),
                limit: 20,
                guid_offset: Some("g-last".into()),
                roles: Some(vec![RoleFilter::Admin, RoleFilter::Member]),
            },
        ];
        for request in &all {
            match request {
                ReadRequest::RequestUser
                | ReadRequest::GetUsersData { .. }
                | ReadRequest::ChatList
                | ReadRequest::History { .. }
                | ReadRequest::SubscribeChat { .. }
                | ReadRequest::Search { .. }
                | ReadRequest::Whoami { .. }
                | ReadRequest::ChatMembers { .. } => {}
            }
        }
        all
    }

    fn encode(request: &ReadRequest) -> (String, Value) {
        match request.route() {
            Route::Registry { .. } => {
                let body: Value = serde_json::from_str(&request.registry_body().unwrap()).unwrap();
                (body["method"].as_str().unwrap().to_owned(), body["params"].clone())
            }
            Route::Socket => {
                let ctx = SocketContext { req_id: 7, request_id: "rid", subscription_id: Some("sub") };
                let frame = request.socket_frame(&ctx).unwrap();
                (frame.method, frame.body)
            }
        }
    }

    #[test]
    fn every_encoded_method_is_on_the_allowlist() {
        for request in every_request() {
            let (method, _) = encode(&request);
            assert_eq!(method, request.method());
            assert!(ALLOWED_METHODS.contains(&method.as_str()), "{method} is not allowlisted");
        }
    }

    const MUTATING_MARKERS: [&str; 22] = [
        "push", "send_message", "edit", "delete", "typing", "heartbeat", "seen", "read",
        "reaction", "vote", "pin", "set_", "create_", "update_", "leave", "invite",
        "change_", "add_", "remove_", "kick", "join", "ban",
    ];

    fn looks_mutating(method: &str) -> bool {
        MUTATING_MARKERS.iter().any(|marker| method.contains(marker))
    }

    #[test]
    fn the_marker_list_flags_mutating_names() {
        for method in [
            "create_private_chat", "create_chat", "change_chat_members", "change_chat_role", "change_chat_admins",
            "leave_chat", "invite_users", "add_members", "remove_members", "kick_user", "join_chat", "set_status",
            "update_chat", "push", "heartbeat",
        ] {
            assert!(looks_mutating(method), "{method} must be flagged");
        }
        for method in ["whoami", "get_chat_members", "search"] {
            assert!(!looks_mutating(method), "{method} must not be flagged");
        }
    }

    #[test]
    fn allowlist_has_no_mutating_or_presence_methods() {
        for method in ALLOWED_METHODS {
            assert!(!looks_mutating(method), "{method} looks mutating");
        }
        assert_eq!(
            ALLOWED_METHODS,
            ["request_user", "get_users_data", "history", "subscribe", "search", "whoami", "get_chat_members"]
        );
        assert_eq!(ALLOWED_METHODS.len(), 7);
        assert!(!ALLOWED_METHODS.contains(&"get_group_members"), "answers bad_request for every shape");
    }

    #[test]
    fn whoami_is_a_socket_request_with_the_session_guid() {
        let request = ReadRequest::Whoami { guid: "g-self".into() };
        assert_eq!(request.method(), "whoami");
        assert_eq!(request.route(), Route::Socket);
        assert!(request.registry_body().is_none());
        let ctx = SocketContext { req_id: 4, request_id: "rid", subscription_id: None };
        let frame = request.socket_frame(&ctx).unwrap();
        assert_eq!((frame.service_index, frame.req_id, frame.method.as_str()), (0, 4, "whoami"));
        assert_eq!(frame.body, json!({ "Guid": "g-self", "RequestId": "rid" }));
    }

    #[test]
    fn member_read_is_a_cookie_only_registry_request_with_the_documented_body() {
        let ctx = SocketContext { req_id: 1, request_id: "rid", subscription_id: Some("sub") };
        let first = ReadRequest::ChatMembers { chat: "0/0/c".into(), limit: MEMBERS_PAGE, guid_offset: None, roles: None };
        assert_eq!(MEMBERS_PAGE, 50);
        assert_eq!(first.method(), "get_chat_members");
        assert_eq!(first.route(), Route::Registry { csrf: false });
        assert!(first.socket_frame(&ctx).is_none());
        let body: Value = serde_json::from_str(&first.registry_body().unwrap()).unwrap();
        assert_eq!(body, json!({ "method": "get_chat_members", "params": { "chat_id": "0/0/c", "limit": 50 } }));
    }

    #[test]
    fn a_paged_member_request_carries_the_offset_and_roles_only_when_given() {
        let paged = ReadRequest::ChatMembers {
            chat: "0/0/c".into(),
            limit: 50,
            guid_offset: Some("00000000-0000-4000-8000-000000000050".into()),
            roles: Some(vec![RoleFilter::Admin]),
        };
        let body: Value = serde_json::from_str(&paged.registry_body().unwrap()).unwrap();
        assert_eq!(
            body,
            json!({ "method": "get_chat_members", "params": {
                "chat_id": "0/0/c",
                "limit": 50,
                "guid_offset": "00000000-0000-4000-8000-000000000050",
                "roles": ["admin"],
            } })
        );
        let both = ReadRequest::ChatMembers {
            chat: "c".into(),
            limit: 1,
            guid_offset: None,
            roles: Some(vec![RoleFilter::Admin, RoleFilter::Member]),
        };
        let body: Value = serde_json::from_str(&both.registry_body().unwrap()).unwrap();
        assert_eq!(body["params"], json!({ "chat_id": "c", "limit": 1, "roles": ["admin", "member"] }));
    }

    #[test]
    fn search_is_a_cookie_only_registry_request_with_the_documented_body() {
        let request = ReadRequest::Search {
            query: "q".into(),
            limit: SEARCH_LIMIT,
            entities: vec![SearchEntity::Messages, SearchEntity::Users, SearchEntity::Chats],
        };
        assert_eq!(SEARCH_LIMIT, 10);
        assert_eq!(request.method(), "search");
        assert!(ALLOWED_METHODS.contains(&"search"));
        assert_eq!(request.route(), Route::Registry { csrf: false });
        // serde_json orders object keys alphabetically; compare as JSON, not as text.
        let body: Value = serde_json::from_str(&request.registry_body().unwrap()).unwrap();
        let expected: Value = serde_json::from_str(
            r#"{"method":"search","params":{"query":"q","limit":10,"entities":["messages","users","chats"]}}"#,
        )
        .unwrap();
        assert_eq!(body, expected);
        let ctx = SocketContext { req_id: 1, request_id: "rid", subscription_id: Some("sub") };
        assert!(request.socket_frame(&ctx).is_none());
    }

    #[test]
    fn route_and_encoding_agree() {
        for request in every_request() {
            assert_eq!(
                request.registry_body().is_some(),
                matches!(request.route(), Route::Registry { .. })
            );
        }
    }

    #[test]
    fn request_user_body_matches_the_bootstrap_literal() {
        assert_eq!(
            ReadRequest::RequestUser.registry_body().unwrap(),
            r#"{"method":"request_user","params":{"bind_phone_number":false}}"#
        );
        assert_eq!(ReadRequest::RequestUser.route(), Route::Registry { csrf: true });
        assert_eq!(
            ReadRequest::GetUsersData { guids: vec![] }.route(),
            Route::Registry { csrf: false }
        );
    }

    #[test]
    fn chat_list_is_history_without_a_chat_id() {
        let (method, body) = encode(&ReadRequest::ChatList);
        assert_eq!(method, "history");
        assert_eq!(body["Limit"], 1);
        assert_eq!(body["ChatDataFilter"], json!({}));
        assert_eq!(body["RequestId"], "rid");
        assert!(body.get("ChatId").is_none());
    }

    #[test]
    fn history_pages_with_an_exclusive_max_timestamp() {
        let (_, first) = encode(&ReadRequest::History { chat: "c".into(), before: None, limit: 50 });
        assert_eq!(first, json!({ "ChatId": "c", "Limit": 50, "RequestId": "rid" }));
        let (_, older) =
            encode(&ReadRequest::History { chat: "c".into(), before: Some(1_784_117_592_261_029), limit: 50 });
        assert_eq!(older["MaxTimestamp"], 1_784_117_592_261_029_i64);
    }

    #[test]
    fn subscribe_needs_the_subscription_id() {
        let request = ReadRequest::SubscribeChat { chat: "c".into() };
        let ctx = SocketContext { req_id: 3, request_id: "rid", subscription_id: None };
        assert!(request.socket_frame(&ctx).is_none());
        let ctx = SocketContext { req_id: 3, request_id: "rid", subscription_id: Some("abc") };
        let frame = request.socket_frame(&ctx).unwrap();
        assert_eq!(frame.req_id, 3);
        assert_eq!(frame.body["ClientTransportId"]["XivaSubscriptionId"], "abc");
        assert_eq!(frame.body["ChatId"], "c");
    }

    #[test]
    fn frames_use_service_index_zero_and_the_header_method() {
        let ctx = SocketContext { req_id: 2, request_id: "rid", subscription_id: None };
        let frame = ReadRequest::ChatList.socket_frame(&ctx).unwrap();
        assert_eq!((frame.service_index, frame.req_id, frame.method.as_str()), (0, 2, "history"));
    }
}
