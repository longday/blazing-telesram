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
pub const ALLOWED_METHODS: [&str; 4] = ["request_user", "get_users_data", "history", "subscribe"];

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
            Self::SubscribeChat { .. } => "subscribe",
        }
    }

    pub fn route(&self) -> Route {
        match self {
            Self::RequestUser => Route::Registry { csrf: true },
            Self::GetUsersData { .. } => Route::Registry { csrf: false },
            Self::ChatList | Self::History { .. } | Self::SubscribeChat { .. } => Route::Socket,
        }
    }

    /// Value of the multipart field `request`; `None` for socket requests.
    pub(crate) fn registry_body(&self) -> Option<String> {
        let params = match self {
            Self::RequestUser => json!({ "bind_phone_number": false }),
            Self::GetUsersData { guids } => json!({ "guids": guids }),
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
            Self::SubscribeChat { chat } => json!({
                "ChatId": chat,
                "TtlMcs": SUBSCRIPTION_TTL_MCS,
                "ClientTransportId": { "XivaSubscriptionId": ctx.subscription_id? },
                "UserAgent": USER_AGENT,
                "ClientSupportedFeatures": 0,
            }),
            Self::RequestUser | Self::GetUsersData { .. } => return None,
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
        ];
        for request in &all {
            match request {
                ReadRequest::RequestUser
                | ReadRequest::GetUsersData { .. }
                | ReadRequest::ChatList
                | ReadRequest::History { .. }
                | ReadRequest::SubscribeChat { .. } => {}
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

    #[test]
    fn allowlist_has_no_mutating_or_presence_methods() {
        for forbidden in [
            "push", "send_message", "edit", "delete", "typing", "heartbeat", "seen", "read",
            "reaction", "vote", "pin", "set_", "create_", "update_", "leave", "invite",
        ] {
            for method in ALLOWED_METHODS {
                assert!(!method.contains(forbidden), "{method} looks like {forbidden}");
            }
        }
        assert_eq!(ALLOWED_METHODS.len(), 4);
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
