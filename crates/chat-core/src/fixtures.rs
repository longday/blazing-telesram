//! Shared builders for unit tests.

use crate::model::*;

pub fn chat(id: &str, last: Option<MessageTs>, pinned: Option<u32>) -> ChatSummary {
    ChatSummary {
        id: ChatId(id.into()),
        kind: ChatKind::Group,
        title: format!("chat {id}"),
        avatar: None,
        last_message: last.map(|timestamp| MessagePreview {
            author: None,
            text: "x".into(),
            timestamp,
        }),
        unread: 0,
        muted: false,
        pinned_rank: pinned,
        partner: None,
    }
}

pub fn msg(chat: &str, ts: MessageTs, text: &str) -> Message {
    Message {
        chat: ChatId(chat.into()),
        timestamp: ts,
        author: UserId("u".into()),
        author_name: None,
        author_avatar: None,
        outgoing: false,
        body: MessageBody::Text(text.into()),
        edited: false,
        reply_to: None,
        forwarded_from: None,
        delivery: Delivery::Sent,
        reactions: Default::default(),
    }
}

pub fn id(chat: &str) -> ChatId {
    ChatId(chat.into())
}

pub fn reactor(guid: &str, code: i64, at: Option<MessageTs>) -> Reactor {
    Reactor {
        user: UserId(guid.into()),
        name: Some(format!("name {guid}")),
        avatar: None,
        code: crate::reactions::ReactionCode(code),
        at,
        me: false,
    }
}
