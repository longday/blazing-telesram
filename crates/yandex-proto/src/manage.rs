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

/// Every method name `ManageRequest` can put on the wire, sorted.
pub const MANAGE_METHODS: [&str; 2] = ["create_chat", "create_private_chat"];

/// The longest group title the web client lets a user type (`maxChatTitle` of the page's server
/// config); `create-chat` of the probe refuses a longer one instead of sending it.
pub const MAX_CHAT_TITLE: usize = 250;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ManageRequest {
    /// Opens (creating it when missing) the private chat with the user `guid`.
    CreatePrivateChat { guid: String },
    /// Creates a group chat named `title` with the participants `users` (guids). The payload is
    /// the one the web client's create-chat wizard sends (`design.md`, D4 outcome): it was read
    /// from the client's code and is not confirmed against the server yet. The caller's own guid
    /// is not listed, the web client does not add it either.
    CreateChat { title: String, users: Vec<String> },
}

impl ManageRequest {
    pub fn method(&self) -> &'static str {
        match self {
            Self::CreatePrivateChat { .. } => "create_private_chat",
            Self::CreateChat { .. } => "create_chat",
        }
    }

    /// Value of the multipart field `request`.
    pub(crate) fn registry_body(&self) -> String {
        let params = match self {
            // `onetime_user_token` is omitted: it comes from `create_token`, a different mutation
            // this client does not express.
            Self::CreatePrivateChat { guid } => json!({ "guid": guid, "wait_for_update": true }),
            // The flag-on form of the web client (`group_refs`/`department_refs`; the flag-off
            // form carries `groups`/`departments` inside `permissions` instead). No avatar and no
            // alias: they only exist after separate calls. `is_public` is what the wizard sends.
            Self::CreateChat { title, users } => json!({
                "name": title,
                "description": "",
                "permissions": { "users": users },
                "group_refs": [],
                "department_refs": [],
                "channel": false,
                "is_public": true,
            }),
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
/// Live shapes (tasks 5.1 and 6.2): `create_private_chat` answers `data.chat.chat_id`,
/// `create_chat` answers the chat itself, `data.chat_id`. The other spots are tolerated and
/// anything else is "no id": a success without an id is still a success.
pub fn created_chat_id(data: &Json) -> Option<String> {
    ["/chat_id", "/chat/chat_id", "/chat/id", "/id"]
        .iter()
        .find_map(|pointer| data.pointer(pointer)?.as_str().filter(|id| !id.is_empty()))
        .map(str::to_owned)
}

/// What a successful `create_chat` reply says: the id of the new group when it names one, and
/// the codes of the participants the server could not add (`data.errors[].code`). A non-empty
/// `refused` is not a failure: the group exists, the web client only lists the people it
/// could not add in a toast.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CreatedChat {
    pub chat: Option<String>,
    pub refused: Vec<String>,
}

/// Tolerant reader of the `data` of a successful `create_chat`: live, `data.chat_id` is the id
/// (the web bundle suggested `data.chat.chat_id`, which `created_chat_id` also accepts);
/// `data.errors` is optional and not seen live, an element without a string `code` counts as
/// `unknown`, and a shape that is not an array is ignored.
pub fn parse_created_chat(data: &Json) -> CreatedChat {
    let refused = data
        .get("errors")
        .and_then(Json::as_array)
        .map(|errors| {
            errors
                .iter()
                .map(|e| {
                    e.get("code")
                        .and_then(Json::as_str)
                        .unwrap_or("unknown")
                        .to_owned()
                })
                .collect()
        })
        .unwrap_or_default();
    CreatedChat {
        chat: created_chat_id(data),
        refused,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::ALLOWED_METHODS;

    /// One value per variant. The exhaustive `match` makes adding a variant a compile error
    /// here until it is listed and therefore checked against the method lists below.
    fn every_request() -> Vec<ManageRequest> {
        let all = vec![
            ManageRequest::CreatePrivateChat { guid: "g-1".into() },
            ManageRequest::CreateChat {
                title: "T".into(),
                users: vec!["g-1".into()],
            },
        ];
        for request in &all {
            match request {
                ManageRequest::CreatePrivateChat { .. } | ManageRequest::CreateChat { .. } => {}
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
            assert!(
                !ALLOWED_METHODS.contains(&method),
                "{method} is also a read"
            );
        }
    }

    #[test]
    fn deferred_management_methods_are_not_representable() {
        for deferred in [
            "change_chat_members",
            "change_chat_role",
            "change_chat_admins",
            "leave",
            "invite",
            "invite_renew",
            "set_chat_info",
            "create_token",
        ] {
            assert!(
                !MANAGE_METHODS.contains(&deferred),
                "{deferred} is in MANAGE_METHODS"
            );
            for request in every_request() {
                assert!(
                    !request.registry_body().contains(&format!("\"{deferred}\"")),
                    "{deferred} is encodable"
                );
            }
        }
    }

    #[test]
    fn create_private_chat_has_the_documented_body() {
        let request = ManageRequest::CreatePrivateChat { guid: "g-1".into() };
        let body: Json = serde_json::from_str(&request.registry_body()).unwrap();
        assert_eq!(
            body,
            json!({ "method": "create_private_chat", "params": { "guid": "g-1", "wait_for_update": true } })
        );
        assert!(!request.registry_body().contains("onetime_user_token"));
    }

    #[test]
    fn the_chat_id_is_found_in_the_usual_places_and_never_invented() {
        assert_eq!(
            created_chat_id(&json!({ "chat_id": "0/0/c" })).as_deref(),
            Some("0/0/c")
        );
        assert_eq!(
            created_chat_id(&json!({ "chat": { "chat_id": "a_b" } })).as_deref(),
            Some("a_b")
        );
        assert_eq!(
            created_chat_id(&json!({ "chat": { "id": "a_b" } })).as_deref(),
            Some("a_b")
        );
        assert_eq!(created_chat_id(&json!({ "id": "x" })).as_deref(), Some("x"));
        for none in [
            json!(null),
            json!({}),
            json!({ "chat_id": "" }),
            json!({ "chat_id": 7 }),
            json!({ "chat": "x" }),
            json!([]),
        ] {
            assert_eq!(created_chat_id(&none), None, "{none}");
        }
    }

    /// The body recorded by task 0.1 (`design.md`, D4 outcome, flag-on form), with a real title
    /// and a second participant: the exact bytes the web client's wizard would send.
    #[test]
    fn create_chat_has_the_recorded_body() {
        let request = ManageRequest::CreateChat {
            title: "Test group".into(),
            users: vec!["g-1".into(), "g-2".into()],
        };
        let body: Json = serde_json::from_str(&request.registry_body()).unwrap();
        let recorded: Json = serde_json::from_str(
            r#"{"method":"create_chat","params":{"name":"Test group","description":"","permissions":{"users":["g-1","g-2"]},"group_refs":[],"department_refs":[],"channel":false,"is_public":true}}"#,
        )
        .unwrap();
        assert_eq!(body, recorded);
        // Nothing the web client does not send: no token, no wait flag, no alias, no avatar.
        let text = request.registry_body();
        for absent in [
            "onetime_user_token",
            "wait_for_update",
            "alias",
            "avatar_id",
            "\"key\"",
            "\"groups\"",
            "\"departments\"",
        ] {
            assert!(!text.contains(absent), "{absent} in {text}");
        }
    }

    #[test]
    fn create_chat_escapes_the_title_and_keeps_the_participant_order() {
        let title = "Q \"1\" \\ \u{43f}\u{440}\u{438}\u{432}\u{435}\u{442}\n";
        let request = ManageRequest::CreateChat {
            title: title.into(),
            users: vec!["b".into(), "a".into()],
        };
        let body: Json = serde_json::from_str(&request.registry_body()).unwrap();
        assert_eq!(body["params"]["name"], title);
        assert_eq!(body["params"]["permissions"]["users"], json!(["b", "a"]));
    }

    #[test]
    fn the_method_list_is_sorted_and_holds_both_creations() {
        let mut sorted = MANAGE_METHODS;
        sorted.sort();
        assert_eq!(sorted, MANAGE_METHODS);
        assert_eq!(MANAGE_METHODS, ["create_chat", "create_private_chat"]);
    }

    #[test]
    fn a_created_group_reply_gives_the_id_and_the_refused_participants() {
        let reply = json!({
            "chat": { "chat_id": "0/0/g", "name": "Test group", "members": [] },
            "errors": [ { "code": "blocked_by_privacy_settings", "guid": "g-2" }, { "code": 7 }, {} ],
        });
        let created = parse_created_chat(&reply);
        assert_eq!(created.chat.as_deref(), Some("0/0/g"));
        assert_eq!(
            created.refused,
            ["blocked_by_privacy_settings", "unknown", "unknown"]
        );
    }

    #[test]
    fn a_created_group_reply_without_an_id_or_errors_is_still_readable() {
        assert_eq!(
            parse_created_chat(&json!({ "chat": { "chat_id": "0/0/g" } })),
            CreatedChat {
                chat: Some("0/0/g".into()),
                refused: vec![]
            }
        );
        for odd in [
            json!(null),
            json!({}),
            json!({ "chat": {} }),
            json!({ "errors": "boom" }),
            json!({ "errors": {} }),
            json!([]),
        ] {
            let created = parse_created_chat(&odd);
            assert_eq!(created, CreatedChat::default(), "{odd}");
        }
        assert_eq!(
            parse_created_chat(&json!({ "errors": [] })).refused,
            Vec::<String>::new()
        );
    }
}
