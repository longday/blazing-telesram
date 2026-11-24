//! The closed set of writes this client can express.
//!
//! Every write is the socket method `push` (the web client's only mutation channel) carrying one
//! `ClientMessage` variant. [`WriteRequest`] is the only way to build such a frame, so presence
//! heartbeats, pins and every other mutating variant stay unrepresentable;
//! [`WRITE_METHODS`] and [`CLIENT_MESSAGE_VARIANTS`] state the whole surface and tests pin it.
//!
//! Wire forms: yandex-messenger-mcp `src/protocol/push.ts` and `src/protocol/mutations.ts`
//! (send, edit, delete and `SeenMarker` confirmed live there; reply and `Typing` are checked
//! live by `bt-probe`, see the change `add-sending`). A reaction is `Reaction {ChatId, Timestamp,
//! Type, Action}`: `Type` is a JSON integer, `Action` 2 (`REPLACE`, also the add) or 1 (`REMOVE`,
//! without `Type`); see the change `add-message-reactions`.

use serde_json::{Value, json};

use crate::error::ProtoError;
use crate::frame::RequestFrame;
use crate::request::{SocketContext, USER_AGENT};
use crate::wire::int_field;

/// Every method name `WriteRequest` can put on the wire.
pub const WRITE_METHODS: [&str; 1] = ["push"];

/// Every `ClientMessage` variant `WriteRequest` can build.
pub const CLIENT_MESSAGE_VARIANTS: [&str; 4] = ["Plain", "SeenMarker", "Typing", "Reaction"];

/// Longest quote of a reply, in characters (the reference's bound).
pub const QUOTE_MAX_CHARS: usize = 200;

/// `ReactionAction` of the wire: `REMOVE`.
const REACTION_REMOVE: i64 = 1;
/// `ReactionAction` of the wire: `REPLACE`.
const REACTION_REPLACE: i64 = 2;

/// The web client's `serviceId` (`X-Origin-Service-ID`), sent as `Meta.Origin`.
const ORIGIN_SERVICE_ID: u32 = 27;

/// Original of a reply: the quoted message of the same chat.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplyRef {
    pub timestamp: i64,
    /// Text of the original; cut to [`QUOTE_MAX_CHARS`] when encoded.
    pub quote: String,
}

/// What a [`WriteRequest::Reaction`] does to the account's reaction on a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReactionChange {
    /// The reaction type becomes this code; `REPLACE` also adds when there is none yet.
    Set(i64),
    /// Takes the account's reaction away; the wire form carries no type.
    Remove,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WriteRequest {
    /// New text message. `payload_id` is the server's deduplication key: resending the same id
    /// answers `DUPLICATE` instead of storing a second copy.
    SendText { chat: String, payload_id: String, text: String, reply: Option<ReplyRef> },
    /// Rewrites the whole `Plain`, so a reply must be repeated or the server drops its quote.
    Edit { chat: String, timestamp: i64, text: String, reply: Option<ReplyRef> },
    /// A `Plain` without content addresses the message for deletion.
    Delete { chat: String, timestamp: i64 },
    /// Moves the read position of `chat` up to `timestamp` inclusive (`SeqNo` is optional).
    Seen { chat: String, timestamp: i64 },
    Typing { chat: String },
    /// Sets, replaces or removes the account's reaction on a message of `chat`.
    Reaction { chat: String, timestamp: i64, change: ReactionChange },
}

fn quote(text: &str) -> &str {
    match text.char_indices().nth(QUOTE_MAX_CHARS) {
        Some((end, _)) => &text[..end],
        None => text,
    }
}

/// Reply refs and quote of a `Plain` (a reply is a forward with a quote); the original is in the
/// same chat.
fn add_reply(plain: &mut Value, chat: &str, reply: &ReplyRef) {
    plain["ForwardedMessageRefs"] = json!([{ "ChatId": chat, "Timestamp": reply.timestamp }]);
    plain["ForwardedMessageStyles"] = json!([{ "Quote": quote(&reply.quote) }]);
}

impl WriteRequest {
    pub fn method(&self) -> &'static str {
        WRITE_METHODS[0]
    }

    /// Short name for logs; never the content.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::SendText { .. } => "send",
            Self::Edit { .. } => "edit",
            Self::Delete { .. } => "delete",
            Self::Seen { .. } => "seen",
            Self::Typing { .. } => "typing",
            Self::Reaction { .. } => "reaction",
        }
    }

    /// The `ClientMessage` variant, without `LogData`.
    fn client_message(&self) -> Value {
        match self {
            Self::SendText { chat, payload_id, text, reply } => {
                let mut plain = json!({ "ChatId": chat, "PayloadId": payload_id, "Text": { "MessageText": text } });
                if let Some(reply) = reply {
                    add_reply(&mut plain, chat, reply);
                }
                json!({ "Plain": plain })
            }
            Self::Edit { chat, timestamp, text, reply } => {
                let mut plain = json!({ "ChatId": chat, "Timestamp": timestamp, "Text": { "MessageText": text } });
                if let Some(reply) = reply {
                    add_reply(&mut plain, chat, reply);
                }
                json!({ "Plain": plain })
            }
            Self::Delete { chat, timestamp } => json!({ "Plain": { "ChatId": chat, "Timestamp": timestamp } }),
            Self::Seen { chat, timestamp } => json!({ "SeenMarker": { "ChatId": chat, "Timestamp": timestamp } }),
            Self::Typing { chat } => json!({ "Typing": { "ChatId": chat } }),
            Self::Reaction { chat, timestamp, change } => {
                let mut reaction = json!({ "ChatId": chat, "Timestamp": timestamp });
                match change {
                    ReactionChange::Set(code) => {
                        reaction["Type"] = json!(code);
                        reaction["Action"] = json!(REACTION_REPLACE);
                    }
                    ReactionChange::Remove => reaction["Action"] = json!(REACTION_REMOVE),
                }
                json!({ "Reaction": reaction })
            }
        }
    }

    /// `None` before the `subscribed` frame: the server answers a push without a subscription id
    /// with a misleading status, so nothing is sent.
    pub(crate) fn socket_frame(&self, ctx: &SocketContext<'_>, yandex_uid: &str) -> Option<RequestFrame> {
        let mut client_message = self.client_message();
        client_message["LogData"] = json!({ "YandexUid": yandex_uid });
        Some(RequestFrame {
            service_index: 0,
            req_id: ctx.req_id,
            method: self.method().to_owned(),
            body: json!({
                "RequestId": ctx.request_id,
                "ClientTransportId": { "XivaSubscriptionId": ctx.subscription_id? },
                "UserAgent": USER_AGENT,
                "ClientMessage": client_message,
                "Meta": { "Origin": ORIGIN_SERVICE_ID },
                "ClientSupportedFeatures": 0,
            }),
        })
    }
}

/// `MessageInfo` of a commit reply: where the server stored the message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageInfo {
    pub timestamp_mcs: i64,
    pub prev_timestamp_mcs: Option<i64>,
    pub seq_no: Option<i64>,
    pub version: Option<i64>,
}

/// A `push` the server accepted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Committed {
    /// `FULLY_COMMITTED` (1) or `DUPLICATE` (8).
    pub status: i64,
    /// Present on a new message; a duplicate resend and the target-addressed writes may omit it.
    pub message: Option<MessageInfo>,
}

impl Committed {
    pub fn duplicate(&self) -> bool {
        self.status == STATUS_DUPLICATE
    }
}

const STATUS_FULLY_COMMITTED: i64 = 1;
const STATUS_DUPLICATE: i64 = 8;

/// Name of a commit status and whether the server will keep refusing the same request.
/// Throttling and every unknown code are transient: repeating later may succeed.
fn commit_status(code: i64) -> (&'static str, bool) {
    match code {
        0 => ("UNCOMMITTED", false),
        2 => ("UNIPROXY_COMMITTED", false),
        3 => ("FAILED", false),
        4 => ("NO_SUCH_CHAT", true),
        5 => ("NOT_LEADING", false),
        6 => ("FOREIGN_PARTITION", false),
        7 => ("SENDER_NOT_IN_CHAT", true),
        9 => ("POSTPROC_COMMITTED", false),
        10 => ("KIKIMR_WRITE_FAILED", false),
        11 => ("MESSAGE_NOT_FOUND", true),
        12 => ("DEQUEUED_AFTER_ERROR", false),
        13 => ("BAD_REQUEST", true),
        14 => ("FILESHARE_FAILED", false),
        15 => ("NO_PERMISSION", true),
        16 => ("CONFLICT", false),
        17 => ("NO_SUCH_USER", false),
        18 => ("THROTTLED", false),
        19 => ("BANNED", true),
        20 => ("BLACKLISTED", true),
        21 => ("NOT_FOUND", false),
        22 => ("SPAM_DETECTED", false),
        23 => ("RATE_LIMIT_EXCEEDED", false),
        24 => ("BLOCKED_BY_PRIVACY_SETTINGS", true),
        25 => ("PUSH_UNAUTHORIZED", false),
        _ => ("unknown", false),
    }
}

/// Parses the reply of a `push`. Its `Status` is a commit status, not the read path's
/// `ResponseStatus`: 1 is success here, while `check_status` reads it as an internal error.
pub(crate) fn parse_commit(body: &Value) -> Result<Committed, ProtoError> {
    let status = int_field(body, "Status").ok_or_else(|| ProtoError::Protocol("push reply has no Status".into()))?;
    if status != STATUS_FULLY_COMMITTED && status != STATUS_DUPLICATE {
        let (name, permanent) = commit_status(status);
        return Err(ProtoError::Commit { status, name, permanent });
    }
    let message = body.get("MessageInfo").and_then(|info| {
        Some(MessageInfo {
            timestamp_mcs: int_field(info, "TimestampMcs")?,
            prev_timestamp_mcs: int_field(info, "PrevTimestampMcs"),
            seq_no: int_field(info, "SeqNo"),
            version: int_field(info, "Version"),
        })
    });
    Ok(Committed { status, message })
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHAT: &str = "0/0/c";

    fn every_request() -> Vec<WriteRequest> {
        let all = vec![
            WriteRequest::SendText { chat: CHAT.into(), payload_id: "p-1".into(), text: "hi".into(), reply: None },
            WriteRequest::SendText {
                chat: CHAT.into(),
                payload_id: "p-2".into(),
                text: "re".into(),
                reply: Some(ReplyRef { timestamp: 1_784_117_592_261_029, quote: "orig".into() }),
            },
            WriteRequest::Edit { chat: CHAT.into(), timestamp: 5, text: "new".into(), reply: None },
            WriteRequest::Edit {
                chat: CHAT.into(),
                timestamp: 6,
                text: "newer".into(),
                reply: Some(ReplyRef { timestamp: 4, quote: "orig".into() }),
            },
            WriteRequest::Delete { chat: CHAT.into(), timestamp: 5 },
            WriteRequest::Seen { chat: CHAT.into(), timestamp: 5 },
            WriteRequest::Typing { chat: CHAT.into() },
            WriteRequest::Reaction { chat: CHAT.into(), timestamp: 7, change: ReactionChange::Set(100_101) },
            WriteRequest::Reaction { chat: CHAT.into(), timestamp: 7, change: ReactionChange::Remove },
        ];
        // An exhaustive `match`: adding a variant is a compile error until it is listed above.
        for request in &all {
            match request {
                WriteRequest::SendText { .. }
                | WriteRequest::Edit { .. }
                | WriteRequest::Delete { .. }
                | WriteRequest::Seen { .. }
                | WriteRequest::Typing { .. }
                | WriteRequest::Reaction { .. } => {}
            }
        }
        all
    }

    fn frame(request: &WriteRequest) -> RequestFrame {
        let ctx = SocketContext { req_id: 4, request_id: "rid", subscription_id: Some("sub") };
        request.socket_frame(&ctx, "4242").unwrap()
    }

    /// Envelope around one `ClientMessage` variant, as the web client sends it.
    fn envelope(variant: Value) -> Value {
        let mut client_message = variant;
        client_message["LogData"] = json!({ "YandexUid": "4242" });
        json!({
            "RequestId": "rid",
            "ClientTransportId": { "XivaSubscriptionId": "sub" },
            "UserAgent": "chats-web/3.22.0",
            "ClientMessage": client_message,
            "Meta": { "Origin": 27 },
            "ClientSupportedFeatures": 0,
        })
    }

    #[test]
    fn send_is_a_plain_with_payload_id() {
        let request = WriteRequest::SendText { chat: CHAT.into(), payload_id: "p-1".into(), text: "hi".into(), reply: None };
        let frame = frame(&request);
        assert_eq!((frame.service_index, frame.req_id, frame.method.as_str()), (0, 4, "push"));
        assert_eq!(
            frame.body,
            envelope(json!({ "Plain": { "ChatId": CHAT, "PayloadId": "p-1", "Text": { "MessageText": "hi" } } }))
        );
    }

    #[test]
    fn reply_adds_a_numeric_ref_and_the_quote() {
        let request = WriteRequest::SendText {
            chat: CHAT.into(),
            payload_id: "p-2".into(),
            text: "re".into(),
            reply: Some(ReplyRef { timestamp: 1_784_117_592_261_029, quote: "orig".into() }),
        };
        assert_eq!(
            frame(&request).body,
            envelope(json!({ "Plain": {
                "ChatId": CHAT,
                "PayloadId": "p-2",
                "Text": { "MessageText": "re" },
                "ForwardedMessageRefs": [{ "ChatId": CHAT, "Timestamp": 1_784_117_592_261_029_i64 }],
                "ForwardedMessageStyles": [{ "Quote": "orig" }],
            } }))
        );
    }

    #[test]
    fn quote_is_cut_to_200_characters_not_bytes() {
        let long = "я".repeat(250);
        let request = WriteRequest::SendText {
            chat: CHAT.into(),
            payload_id: "p".into(),
            text: "t".into(),
            reply: Some(ReplyRef { timestamp: 1, quote: long.clone() }),
        };
        let body = frame(&request).body;
        let sent = body["ClientMessage"]["Plain"]["ForwardedMessageStyles"][0]["Quote"].as_str().unwrap().to_owned();
        assert_eq!(sent, long.chars().take(200).collect::<String>());
        assert_eq!(quote("short"), "short");
        assert_eq!(quote(&"a".repeat(200)).len(), 200);
    }

    #[test]
    fn edit_addresses_the_message_by_a_numeric_timestamp() {
        let request = WriteRequest::Edit { chat: CHAT.into(), timestamp: 1_784_117_592_261_029, text: "new".into(), reply: None };
        assert_eq!(
            frame(&request).body,
            envelope(json!({ "Plain": { "ChatId": CHAT, "Timestamp": 1_784_117_592_261_029_i64, "Text": { "MessageText": "new" } } }))
        );
    }

    /// An edit rewrites the whole `Plain`: without the reply refs the server drops the quote.
    #[test]
    fn editing_a_reply_repeats_its_refs_and_quote() {
        let request = WriteRequest::Edit {
            chat: CHAT.into(),
            timestamp: 20,
            text: "new".into(),
            reply: Some(ReplyRef { timestamp: 10, quote: "orig".into() }),
        };
        assert_eq!(
            frame(&request).body,
            envelope(json!({ "Plain": {
                "ChatId": CHAT,
                "Timestamp": 20,
                "Text": { "MessageText": "new" },
                "ForwardedMessageRefs": [{ "ChatId": CHAT, "Timestamp": 10 }],
                "ForwardedMessageStyles": [{ "Quote": "orig" }],
            } }))
        );
    }

    #[test]
    fn delete_is_a_plain_without_content() {
        let request = WriteRequest::Delete { chat: CHAT.into(), timestamp: 9 };
        assert_eq!(frame(&request).body, envelope(json!({ "Plain": { "ChatId": CHAT, "Timestamp": 9 } })));
    }

    #[test]
    fn seen_is_a_seen_marker_without_seq_no() {
        let request = WriteRequest::Seen { chat: CHAT.into(), timestamp: 9 };
        assert_eq!(frame(&request).body, envelope(json!({ "SeenMarker": { "ChatId": CHAT, "Timestamp": 9 } })));
    }

    #[test]
    fn typing_names_only_the_chat() {
        let request = WriteRequest::Typing { chat: CHAT.into() };
        assert_eq!(frame(&request).body, envelope(json!({ "Typing": { "ChatId": CHAT } })));
    }

    /// `REPLACE` (2) with an integer `Type`: also the add, when the user has no reaction yet.
    #[test]
    fn setting_a_reaction_is_a_replace_with_an_integer_type() {
        let request = WriteRequest::Reaction { chat: CHAT.into(), timestamp: 1_795_070_677_887_127, change: ReactionChange::Set(100_101) };
        let body = frame(&request).body;
        assert_eq!(
            body,
            envelope(json!({ "Reaction": { "ChatId": CHAT, "Timestamp": 1_795_070_677_887_127_i64, "Type": 100_101, "Action": 2 } }))
        );
        assert!(body["ClientMessage"]["Reaction"]["Type"].is_i64());
        assert_eq!(request.kind(), "reaction");
    }

    /// `REMOVE` (1) names no `Type`: the server demands it only for add and replace.
    #[test]
    fn removing_a_reaction_is_a_remove_without_a_type() {
        let request = WriteRequest::Reaction { chat: CHAT.into(), timestamp: 9, change: ReactionChange::Remove };
        let body = frame(&request).body;
        assert_eq!(body, envelope(json!({ "Reaction": { "ChatId": CHAT, "Timestamp": 9, "Action": 1 } })));
        assert!(body["ClientMessage"]["Reaction"].get("Type").is_none());
    }

    #[test]
    fn push_is_the_only_write_method_and_the_variants_are_closed() {
        assert_eq!(WRITE_METHODS, ["push"]);
        let mut seen = std::collections::BTreeSet::new();
        for request in every_request() {
            let frame = frame(&request);
            assert_eq!(frame.method, "push");
            assert_eq!(request.method(), "push");
            let client_message = frame.body["ClientMessage"].as_object().unwrap();
            for key in client_message.keys().filter(|k| *k != "LogData") {
                assert!(CLIENT_MESSAGE_VARIANTS.contains(&key.as_str()), "{key} is not an allowed variant");
                seen.insert(key.clone());
            }
            assert_eq!(client_message["LogData"], json!({ "YandexUid": "4242" }));
        }
        let mut expected = CLIENT_MESSAGE_VARIANTS.to_vec();
        expected.sort_unstable();
        assert_eq!(seen.iter().map(String::as_str).collect::<Vec<_>>(), expected);
        assert_eq!(CLIENT_MESSAGE_VARIANTS, ["Plain", "SeenMarker", "Typing", "Reaction"]);
    }

    #[test]
    fn the_read_allowlist_does_not_gain_the_write_method() {
        for method in crate::request::ALLOWED_METHODS {
            assert!(!WRITE_METHODS.contains(&method));
        }
        assert_eq!(
            crate::request::ALLOWED_METHODS,
            ["request_user", "get_users_data", "history", "subscribe", "search", "whoami", "get_chat_members"]
        );
    }

    #[test]
    fn nothing_is_sent_before_the_subscription_id_is_known() {
        let ctx = SocketContext { req_id: 1, request_id: "rid", subscription_id: None };
        assert!(WriteRequest::Typing { chat: CHAT.into() }.socket_frame(&ctx, "4242").is_none());
    }

    #[test]
    fn statuses_1_and_8_are_success() {
        let created = parse_commit(&json!({
            "Status": 1,
            "MessageInfo": { "TimestampMcs": 1_795_444_073_973_086_i64, "PrevTimestampMcs": 1_795_170_270_978_086_i64, "SeqNo": 5, "Version": 1 },
            "DebugInfo": { "Anything": true },
        }))
        .unwrap();
        assert_eq!(created.status, 1);
        assert!(!created.duplicate());
        assert_eq!(
            created.message,
            Some(MessageInfo {
                timestamp_mcs: 1_795_444_073_973_086,
                prev_timestamp_mcs: Some(1_795_170_270_978_086),
                seq_no: Some(5),
                version: Some(1),
            })
        );

        let duplicate = parse_commit(&json!({ "Status": 8 })).unwrap();
        assert!(duplicate.duplicate());
        assert_eq!(duplicate.message, None);

        // A message info without a timestamp tells nothing about where the message went.
        assert_eq!(parse_commit(&json!({ "Status": 1, "MessageInfo": { "SeqNo": 5 } })).unwrap().message, None);
    }

    #[test]
    fn refusals_split_into_permanent_and_transient() {
        let refused = |status: i64| match parse_commit(&json!({ "Status": status })) {
            Err(ProtoError::Commit { status: s, name, permanent }) => {
                assert_eq!(s, status);
                (name, permanent)
            }
            other => panic!("{other:?}"),
        };
        assert_eq!(refused(15), ("NO_PERMISSION", true));
        assert_eq!(refused(18), ("THROTTLED", false));
        for (status, name) in [
            (4, "NO_SUCH_CHAT"),
            (7, "SENDER_NOT_IN_CHAT"),
            (11, "MESSAGE_NOT_FOUND"),
            (13, "BAD_REQUEST"),
            (19, "BANNED"),
            (20, "BLACKLISTED"),
            (24, "BLOCKED_BY_PRIVACY_SETTINGS"),
        ] {
            assert_eq!(refused(status), (name, true), "{name}");
        }
        for (status, name) in [(3, "FAILED"), (23, "RATE_LIMIT_EXCEEDED"), (10, "KIKIMR_WRITE_FAILED"), (99, "unknown")] {
            assert_eq!(refused(status), (name, false), "{name}");
        }
    }

    #[test]
    fn a_reply_without_status_is_not_a_commit() {
        assert!(matches!(parse_commit(&json!({ "MessageInfo": { "TimestampMcs": 5 } })), Err(ProtoError::Protocol(m)) if m.contains("Status")));
        assert!(matches!(parse_commit(&json!({})), Err(ProtoError::Protocol(_))));
    }
}
