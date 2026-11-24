//! The closed set of Registry mutations this client can express.
//!
//! Like [`crate::request::ReadRequest`] for reads, [`ManageRequest`] is the only way to build a
//! mutation: there is no "call any method" entry point, and every wire method it can produce
//! is listed in [`MANAGE_METHODS`], which is disjoint from [`crate::request::ALLOWED_METHODS`]
//! (so a mutation cannot be built through the read path). Chat management beyond creation
//! (`change_chat_members`, `change_chat_role`, `change_chat_admins`, `leave`, `invite`,
//! `invite_renew`, `set_chat_info`, `create_token`) is deliberately not representable.
//!
//! Every mutation is CSRF-guarded and is sent at most once: the caller learns from
//! [`ManageError`] whether a repeat could duplicate the result.

use serde_json::{Value as Json, json};

use crate::error::ProtoError;

/// Every method name `ManageRequest` can put on the wire.
pub const MANAGE_METHODS: [&str; 1] = ["create_private_chat"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ManageRequest {
    /// Opens (creating it when missing) the private chat with the user `guid`.
    CreatePrivateChat { guid: String },
}

impl ManageRequest {
    pub fn method(&self) -> &'static str {
        match self {
            Self::CreatePrivateChat { .. } => "create_private_chat",
        }
    }

    /// Value of the multipart field `request`.
    pub(crate) fn registry_body(&self) -> String {
        let params = match self {
            // `onetime_user_token` is omitted: it comes from `create_token`, a different mutation
            // this client does not express.
            Self::CreatePrivateChat { guid } => json!({ "guid": guid, "wait_for_update": true }),
        };
        json!({ "method": self.method(), "params": params }).to_string()
    }
}

/// What is known about a mutation that did not succeed. A repeated creation can duplicate a
/// chat, so the classes say whether the server may have acted.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ManageError {
    /// Nothing left the client: the CSRF exchange failed or the connection could not be opened.
    #[error("not sent: {0}")]
    NotSent(ProtoError),
    /// The server answered and did not act (expired session, or a reply whose `status` is not
    /// `ok`, carrying its `data.code`).
    #[error("rejected: {0}")]
    Rejected(ProtoError),
    /// The request was sent and the outcome is not known: timeout, dropped connection,
    /// HTTP 5xx or 429, or a body that is not JSON.
    #[error("outcome unknown: {0}")]
    Unknown(ProtoError),
}

/// The id of the chat in the `data` of a successful mutation reply, if the reply names one.
/// The shape is not confirmed against a live reply yet, so the common spots are tried in turn
/// and anything else is "no id": a success without an id is still a success.
pub fn created_chat_id(data: &Json) -> Option<String> {
    ["/chat_id", "/chat/chat_id", "/chat/id", "/id"]
        .iter()
        .find_map(|pointer| data.pointer(pointer)?.as_str().filter(|id| !id.is_empty()))
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::ALLOWED_METHODS;

    /// One value per variant. The exhaustive `match` makes adding a variant a compile error
    /// here until it is listed and therefore checked against the method lists below.
    fn every_request() -> Vec<ManageRequest> {
        let all = vec![ManageRequest::CreatePrivateChat { guid: "g-1".into() }];
        for request in &all {
            match request {
                ManageRequest::CreatePrivateChat { .. } => {}
            }
        }
        all
    }

    #[test]
    fn the_method_list_is_exactly_what_the_requests_encode() {
        let mut encoded: Vec<String> = every_request()
            .iter()
            .map(|request| {
                let body: Json = serde_json::from_str(&request.registry_body()).unwrap();
                let method = body["method"].as_str().unwrap().to_owned();
                assert_eq!(method, request.method());
                method
            })
            .collect();
        encoded.sort();
        encoded.dedup();
        assert_eq!(encoded, MANAGE_METHODS);
    }

    #[test]
    fn manage_methods_are_disjoint_from_the_read_allowlist() {
        for method in MANAGE_METHODS {
            assert!(!ALLOWED_METHODS.contains(&method), "{method} is also a read");
        }
    }

    #[test]
    fn deferred_management_methods_are_not_representable() {
        for deferred in [
            "change_chat_members", "change_chat_role", "change_chat_admins", "leave", "invite", "invite_renew",
            "set_chat_info", "create_token",
        ] {
            assert!(!MANAGE_METHODS.contains(&deferred), "{deferred} is in MANAGE_METHODS");
            for request in every_request() {
                assert!(!request.registry_body().contains(&format!("\"{deferred}\"")), "{deferred} is encodable");
            }
        }
    }

    #[test]
    fn create_private_chat_has_the_documented_body() {
        let request = ManageRequest::CreatePrivateChat { guid: "g-1".into() };
        let body: Json = serde_json::from_str(&request.registry_body()).unwrap();
        assert_eq!(body, json!({ "method": "create_private_chat", "params": { "guid": "g-1", "wait_for_update": true } }));
        assert!(!request.registry_body().contains("onetime_user_token"));
    }

    #[test]
    fn the_chat_id_is_found_in_the_usual_places_and_never_invented() {
        assert_eq!(created_chat_id(&json!({ "chat_id": "0/0/c" })).as_deref(), Some("0/0/c"));
        assert_eq!(created_chat_id(&json!({ "chat": { "chat_id": "a_b" } })).as_deref(), Some("a_b"));
        assert_eq!(created_chat_id(&json!({ "chat": { "id": "a_b" } })).as_deref(), Some("a_b"));
        assert_eq!(created_chat_id(&json!({ "id": "x" })).as_deref(), Some("x"));
        for none in [json!(null), json!({}), json!({ "chat_id": "" }), json!({ "chat_id": 7 }), json!({ "chat": "x" }), json!([])] {
            assert_eq!(created_chat_id(&none), None, "{none}");
        }
    }
}
