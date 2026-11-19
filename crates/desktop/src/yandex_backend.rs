//! `Backend` over the Yandex web protocol: maps wire types to the domain model.

use bt_chat_core::model::*;
use bt_chat_core::{Backend, BackendError, LiveEvent};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use yandex_proto::bootstrap::bootstrap;
use yandex_proto::client::{Client, ClientConfig};
use yandex_proto::error::ProtoError;
use yandex_proto::media::{AvatarSize, MediaTarget};
use yandex_proto::session::Session;
use yandex_proto::wire::{PushEvent, Relation, WireBody, WireChat, WireChatKind, WireMessage, WireSender, WireUser};
use yandex_proto::xiva::{XivaEvent, XivaEvents};

/// How long bootstrap waits for the Xiva subscription before reporting a network error.
const CONNECT_DEADLINE: Duration = Duration::from_secs(30);

pub struct YandexBackend {
    session: Session,
    config: ClientConfig,
    inner: Mutex<Option<Connection>>,
    users: Arc<Mutex<HashMap<String, WireUser>>>,
}

struct Connection {
    client: Client,
    events: Option<XivaEvents>,
    me: String,
}

fn backend_error(error: ProtoError) -> BackendError {
    match error {
        ProtoError::SessionExpired => BackendError::SessionExpired,
        ProtoError::Network(e) => BackendError::Network(e),
        ProtoError::Protocol(e) => BackendError::Protocol(e),
    }
}

/// Requests fail with "not connected" until the subscription is up, so callers must wait here.
async fn wait_connected(events: &mut XivaEvents, deadline: Duration) -> Result<(), BackendError> {
    let wait = async {
        while let Some(event) = events.recv().await {
            match event {
                XivaEvent::Connected { .. } => return Ok(()),
                XivaEvent::SessionExpired => return Err(BackendError::SessionExpired),
                XivaEvent::Disconnected { reason, .. } => return Err(BackendError::Network(reason)),
                _ => {}
            }
        }
        Err(BackendError::Network("live connection stopped".into()))
    };
    tokio::time::timeout(deadline, wait)
        .await
        .unwrap_or_else(|_| Err(BackendError::Network("live connection timed out".into())))
}

impl YandexBackend {
    pub fn new(session: Session) -> Self {
        Self { session, config: ClientConfig::production(), inner: Mutex::new(None), users: Arc::default() }
    }

    fn client(&self) -> Result<(Client, String), BackendError> {
        let inner = self.inner.lock().unwrap();
        let connection = inner.as_ref().ok_or_else(|| BackendError::Protocol("not bootstrapped".into()))?;
        Ok((connection.client.clone(), connection.me.clone()))
    }
}

impl Backend for YandexBackend {
    async fn bootstrap(&self) -> Result<UserId, BackendError> {
        let identity = bootstrap(&self.config.registry, &self.session.cookies).await.map_err(|e| match e {
            yandex_proto::bootstrap::BootstrapError::SessionRejected => BackendError::SessionExpired,
            other => BackendError::Network(other.to_string()),
        })?;
        let mut session = self.session.clone();
        session.identity = identity.clone();
        // A fresh connection per attempt: the engine only reconnects after the old one failed.
        let (client, mut events) = Client::start(&session, self.config.clone()).map_err(backend_error)?;
        wait_connected(&mut events, CONNECT_DEADLINE).await?;
        *self.inner.lock().unwrap() = Some(Connection { client, events: Some(events), me: identity.guid.clone() });
        Ok(UserId(identity.xiva_user().to_owned()))
    }

    async fn chat_list(&self) -> Result<Vec<ChatSummary>, BackendError> {
        let (client, me) = self.client()?;
        let chats = client.chat_list().await.map_err(backend_error)?;
        client.watch_chats(chats.iter().map(|c| c.id.clone()).collect());
        let users = Users { client: &client, cache: &self.users };
        let wanted = chats.iter().flat_map(|c| {
            c.partner_guid.iter().chain(c.last_message.iter().filter_map(|m| m.from.as_ref()?.guid.as_ref()))
        });
        users.load(wanted.cloned().collect()).await?;
        Ok(chats.iter().map(|c| summary(c, &me, &self.users.lock().unwrap())).collect())
    }

    async fn history(&self, chat: &ChatId, before: Option<MessageTs>, _limit: usize) -> Result<HistoryChunk, BackendError> {
        let (client, me) = self.client()?;
        let page = client.history(&chat.0, before).await.map_err(backend_error)?;
        Users { client: &client, cache: &self.users }.load(authors(&page.messages)).await?;
        let users = self.users.lock().unwrap();
        Ok(HistoryChunk {
            chat: chat.clone(),
            messages: page.messages.iter().map(|m| message(m, &me, &users)).collect(),
            reached_start: page.reached_start,
            replaces: None,
        })
    }

    async fn fetch_media(&self, media: &MediaRef) -> Result<Vec<u8>, BackendError> {
        let (client, _) = self.client()?;
        let target = match media.kind {
            MediaKind::Avatar => MediaTarget::Avatar { id: media.id.clone(), size: AvatarSize::Small },
            MediaKind::Thumbnail => MediaTarget::Thumbnail { file_id: media.id.clone() },
            MediaKind::File => return Err(BackendError::Protocol("file download is not supported yet".into())),
        };
        client.fetch_media(&target).await.map_err(backend_error)
    }

    async fn subscribe(&self) -> Result<async_channel::Receiver<LiveEvent>, BackendError> {
        let (client, me, mut events) = {
            let mut inner = self.inner.lock().unwrap();
            let connection = inner.as_mut().ok_or_else(|| BackendError::Protocol("not bootstrapped".into()))?;
            let events = connection.events.take().ok_or_else(|| BackendError::Protocol("already subscribed".into()))?;
            (connection.client.clone(), connection.me.clone(), events)
        };
        let (tx, rx) = async_channel::unbounded();
        let users = self.users.clone();
        tokio::spawn(async move {
            // Bootstrap consumed the first `Connected`; every later one is a reconnect.
            while let Some(event) = events.recv().await {
                let live = match event {
                    XivaEvent::Connected { .. } => vec![LiveEvent::Reconnected],
                    XivaEvent::Disconnected { .. } => vec![LiveEvent::Disconnected],
                    XivaEvent::SessionExpired => vec![LiveEvent::Failed(BackendError::SessionExpired)],
                    XivaEvent::Push(PushEvent::MessageUpserted(wire)) => {
                        match (Users { client: &client, cache: &users }).load(authors(std::slice::from_ref(&wire))).await {
                            Ok(()) => vec![LiveEvent::MessageUpserted(message(&wire, &me, &users.lock().unwrap()))],
                            Err(error) => vec![LiveEvent::Failed(error)],
                        }
                    }
                    XivaEvent::Push(PushEvent::MessageDeleted { chat, timestamp }) => {
                        vec![LiveEvent::MessageDeleted { chat: ChatId(chat), timestamp }]
                    }
                    XivaEvent::Push(PushEvent::ChatChanged { chat }) => match client.chat_list().await {
                        Ok(chats) => {
                            let users = users.lock().unwrap();
                            chats.iter().filter(|c| c.id == chat).map(|c| LiveEvent::ChatChanged(summary(c, &me, &users))).collect()
                        }
                        Err(ProtoError::SessionExpired) => vec![LiveEvent::Failed(BackendError::SessionExpired)],
                        // A missed row update is repaired by the next chat-list fetch.
                        Err(_) => vec![],
                    },
                    XivaEvent::Push(PushEvent::Ignored { .. })
                    | XivaEvent::Connecting
                    | XivaEvent::Warning(_)
                    | XivaEvent::Wire(_) => vec![],
                };
                for event in live {
                    if tx.send(event).await.is_err() {
                        return;
                    }
                }
            }
        });
        Ok(rx)
    }
}

/// Resolves display names and avatars of users the wire left anonymous.
struct Users<'a> {
    client: &'a Client,
    cache: &'a Mutex<HashMap<String, WireUser>>,
}

impl Users<'_> {
    /// Names are cosmetic: a failed lookup leaves the fallbacks in place. Only session
    /// rejection is an error, because then nothing else will work either.
    async fn load(&self, mut guids: Vec<String>) -> Result<(), BackendError> {
        {
            let cache = self.cache.lock().unwrap();
            guids.retain(|g| !cache.contains_key(g));
        }
        guids.sort();
        guids.dedup();
        if guids.is_empty() {
            return Ok(());
        }
        match self.client.users(&guids).await {
            Ok(found) => {
                let mut cache = self.cache.lock().unwrap();
                cache.extend(found.into_iter().map(|u| (u.guid.clone(), u)));
                Ok(())
            }
            Err(ProtoError::SessionExpired) => Err(BackendError::SessionExpired),
            Err(_) => Ok(()),
        }
    }
}

fn authors(messages: &[WireMessage]) -> Vec<String> {
    messages.iter().filter_map(|m| m.from.as_ref()?.guid.clone()).collect()
}

fn sender_name(from: Option<&WireSender>, users: &HashMap<String, WireUser>) -> Option<String> {
    let from = from?;
    from.display_name.clone().or_else(|| users.get(from.guid.as_ref()?)?.display_name.clone())
}

fn sender_avatar(from: Option<&WireSender>, users: &HashMap<String, WireUser>) -> Option<MediaRef> {
    let from = from?;
    let id = from.avatar_id.clone().or_else(|| users.get(from.guid.as_ref()?)?.avatar_id.clone())?;
    Some(MediaRef { id, kind: MediaKind::Avatar })
}

fn summary(chat: &WireChat, me: &str, users: &HashMap<String, WireUser>) -> ChatSummary {
    let partner = chat.partner_guid.as_ref().and_then(|g| users.get(g));
    let title = chat
        .title
        .clone()
        .or_else(|| partner.and_then(|p| p.display_name.clone()))
        .unwrap_or_else(|| "Без названия".into());
    let avatar = chat
        .avatar_id
        .clone()
        .or_else(|| partner.and_then(|p| p.avatar_id.clone()))
        .map(|id| MediaRef { id, kind: MediaKind::Avatar });
    let last_message = chat.last_message.as_ref().map(|m| {
        let message = message(m, me, users);
        MessagePreview {
            author: if message.outgoing { Some("Вы".into()) } else { message.author_name.clone() },
            text: preview(&message.body),
            timestamp: m.timestamp,
        }
    });
    ChatSummary {
        id: ChatId(chat.id.clone()),
        kind: match chat.kind {
            WireChatKind::Private => ChatKind::Private,
            WireChatKind::Group => ChatKind::Group,
            WireChatKind::Channel => ChatKind::Channel,
        },
        title,
        avatar,
        last_message,
        unread: chat.unread,
        muted: chat.muted,
        pinned_rank: None,
    }
}

fn preview(body: &MessageBody) -> String {
    match body {
        MessageBody::Text(text) => text.clone(),
        MessageBody::Image { caption, .. } => caption.clone().unwrap_or_else(|| "Фото".into()),
        MessageBody::File { name, .. } => name.clone(),
        MessageBody::Deleted => "Сообщение удалено".into(),
        MessageBody::Unsupported(kind) => format!("[{kind}]"),
    }
}

fn body(wire: &WireBody) -> MessageBody {
    match wire {
        WireBody::Text(text) => MessageBody::Text(text.clone()),
        WireBody::Image(image) => image_body(image, None),
        WireBody::Gallery { items, text } => match items.first() {
            Some(first) => image_body(first, text.clone()),
            None => MessageBody::Text(text.clone().unwrap_or_default()),
        },
        WireBody::File(file) => MessageBody::File {
            media: MediaRef { id: file.id.clone(), kind: MediaKind::File },
            name: file.name.clone().unwrap_or_else(|| "Файл".into()),
            size: file.size.unwrap_or(0),
        },
        WireBody::Voice { .. } => MessageBody::Unsupported("Голосовое сообщение".into()),
        WireBody::Sticker { .. } => MessageBody::Unsupported("Стикер".into()),
        WireBody::Poll { title } => MessageBody::Unsupported(format!("Опрос: {title}")),
        WireBody::System { text: Some(text), .. } => MessageBody::Text(text.clone()),
        WireBody::System { kind, text: None } => MessageBody::Unsupported(kind.clone()),
        WireBody::Deleted => MessageBody::Deleted,
        WireBody::Empty => MessageBody::Text(String::new()),
        WireBody::Other(kind) => MessageBody::Unsupported(kind.clone()),
    }
}

fn image_body(image: &yandex_proto::wire::WireImage, caption: Option<String>) -> MessageBody {
    MessageBody::Image {
        media: MediaRef { id: image.file.id.clone(), kind: MediaKind::Thumbnail },
        width: image.width,
        height: image.height,
        caption,
    }
}

fn message(wire: &WireMessage, me: &str, users: &HashMap<String, WireUser>) -> Message {
    let from = wire.from.as_ref();
    let mut message = Message {
        chat: ChatId(wire.chat.clone()),
        timestamp: wire.timestamp,
        author: UserId(from.and_then(|f| f.guid.clone()).unwrap_or_default()),
        author_name: sender_name(from, users),
        author_avatar: sender_avatar(from, users),
        outgoing: from.and_then(|f| f.guid.as_deref()) == Some(me),
        body: body(&wire.body),
        edited: wire.edited,
        reply_to: None,
        forwarded_from: None,
    };
    match &wire.relation {
        Relation::None => {}
        Relation::Reply { quote, reference, original } => {
            let original_text = original.as_ref().map(|o| preview(&body(&o.body)));
            message.reply_to = Some(ReplyQuote {
                author: original.as_ref().and_then(|o| sender_name(o.from.as_ref(), users)),
                text: quote.clone().or(original_text).unwrap_or_default(),
                timestamp: reference.as_ref().and_then(|r| r.timestamp).or(original.as_ref().and_then(|o| o.timestamp)),
            });
        }
        Relation::Forward { originals } => {
            if let Some(first) = originals.first() {
                message.forwarded_from = Some(sender_name(first.from.as_ref(), users).unwrap_or_else(|| "Неизвестный".into()));
                if wire.body == WireBody::Empty {
                    message.body = body(&first.body);
                }
            }
        }
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;
    use yandex_proto::wire::{WireFile, WireForwarded};

    fn wire(body: WireBody, relation: Relation, from: &str) -> WireMessage {
        WireMessage {
            chat: "0/0/a".into(),
            timestamp: 10,
            seq_no: None,
            version: None,
            from: Some(WireSender { guid: Some(from.into()), display_name: None, avatar_id: None }),
            body,
            edited: false,
            relation,
            raw: serde_json::Value::Null,
        }
    }

    fn user(guid: &str, name: &str) -> (String, WireUser) {
        (guid.into(), WireUser { guid: guid.into(), display_name: Some(name.into()), avatar_id: Some(format!("av-{guid}")), uid: None })
    }

    #[test]
    fn maps_author_from_users_and_marks_own_messages() {
        let users = HashMap::from([user("u1", "Ann")]);
        let theirs = message(&wire(WireBody::Text("hi".into()), Relation::None, "u1"), "me", &users);
        assert_eq!((theirs.author_name.as_deref(), theirs.outgoing), (Some("Ann"), false));
        assert_eq!(theirs.author_avatar, Some(MediaRef { id: "av-u1".into(), kind: MediaKind::Avatar }));
        assert!(message(&wire(WireBody::Text("x".into()), Relation::None, "me"), "me", &users).outgoing);
    }

    #[test]
    fn forward_without_comment_shows_the_original() {
        let users = HashMap::from([user("u2", "Bob")]);
        let original = WireForwarded {
            chat: None,
            timestamp: Some(5),
            from: Some(WireSender { guid: Some("u2".into()), display_name: None, avatar_id: None }),
            body: WireBody::Text("orig".into()),
        };
        let m = message(&wire(WireBody::Empty, Relation::Forward { originals: vec![original] }, "u1"), "me", &users);
        assert_eq!((m.forwarded_from.as_deref(), m.body), (Some("Bob"), MessageBody::Text("orig".into())));
    }

    #[test]
    fn reply_prefers_quote_and_falls_back_to_original() {
        let users = HashMap::new();
        let original = WireForwarded { chat: None, timestamp: Some(5), from: None, body: WireBody::Text("orig".into()) };
        let quoted = Relation::Reply { quote: Some("q".into()), reference: None, original: Some(original.clone()) };
        assert_eq!(message(&wire(WireBody::Text("a".into()), quoted, "u1"), "me", &users).reply_to.unwrap().text, "q");
        let plain = Relation::Reply { quote: None, reference: None, original: Some(original) };
        let reply = message(&wire(WireBody::Text("a".into()), plain, "u1"), "me", &users).reply_to.unwrap();
        assert_eq!((reply.text.as_str(), reply.timestamp), ("orig", Some(5)));
    }

    #[test]
    fn maps_media_and_unknown_bodies() {
        let file = WireFile { id: "b/1".into(), name: Some("a.pdf".into()), size: Some(42) };
        assert_eq!(
            body(&WireBody::File(file.clone())),
            MessageBody::File { media: MediaRef { id: "b/1".into(), kind: MediaKind::File }, name: "a.pdf".into(), size: 42 }
        );
        let image = yandex_proto::wire::WireImage { file, width: 4, height: 3, animated: false };
        assert!(matches!(body(&WireBody::Image(image)), MessageBody::Image { width: 4, height: 3, .. }));
        assert_eq!(body(&WireBody::Other("Foo".into())), MessageBody::Unsupported("Foo".into()));
        assert_eq!(body(&WireBody::Deleted), MessageBody::Deleted);
    }

    #[test]
    fn private_chat_takes_partner_name_and_avatar() {
        let users = HashMap::from([user("u1", "Ann")]);
        let chat = WireChat {
            id: "0/0/u1".into(),
            kind: WireChatKind::Private,
            title: None,
            avatar_id: None,
            partner_guid: Some("u1".into()),
            last_activity: Some(10),
            unread: 2,
            muted: true,
            last_message: Some(wire(WireBody::Text("hey".into()), Relation::None, "me")),
            raw: serde_json::Value::Null,
        };
        let s = summary(&chat, "me", &users);
        assert_eq!((s.title.as_str(), s.unread, s.muted), ("Ann", 2, true));
        let preview = s.last_message.unwrap();
        assert_eq!((preview.author.as_deref(), preview.text.as_str()), (Some("Вы"), "hey"));
    }

    #[tokio::test(start_paused = true)]
    async fn bootstrap_waits_for_a_delayed_handshake() {
        let (tx, mut events) = tokio::sync::mpsc::unbounded_channel();
        tx.send(XivaEvent::Connecting).unwrap();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(5)).await;
            tx.send(XivaEvent::Connected { subscription_id: "s".into() }).unwrap();
        });
        assert_eq!(wait_connected(&mut events, Duration::from_secs(30)).await, Ok(()));
    }

    #[tokio::test(start_paused = true)]
    async fn bootstrap_fails_on_rejection_and_on_silence() {
        let (tx, mut events) = tokio::sync::mpsc::unbounded_channel();
        tx.send(XivaEvent::SessionExpired).unwrap();
        assert_eq!(wait_connected(&mut events, Duration::from_secs(30)).await, Err(BackendError::SessionExpired));

        let (_tx, mut events) = tokio::sync::mpsc::unbounded_channel();
        assert!(matches!(wait_connected(&mut events, Duration::from_secs(30)).await, Err(BackendError::Network(_))));
    }
}
