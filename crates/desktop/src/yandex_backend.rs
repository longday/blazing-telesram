//! `Backend` over the Yandex web protocol: maps wire types to the domain model.

use bt_chat_core::model::*;
use bt_chat_core::{Backend, BackendError, LiveEvent};
use std::collections::HashMap;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use yandex_proto::bootstrap::bootstrap;
use yandex_proto::client::{Client, ClientConfig};
use yandex_proto::error::ProtoError;
use yandex_proto::media::{AvatarSize, MediaTarget};
use yandex_proto::session::Session;
use yandex_proto::wire::{PushEvent, Relation, WireBody, WireChat, WireChatKind, WireMessage, WireSender, WireUser};
use yandex_proto::write::{Committed, ReplyRef, WriteRequest};
use yandex_proto::xiva::{XivaEvent, XivaEvents};

/// How long bootstrap waits for the Xiva subscription before reporting a network error.
const CONNECT_DEADLINE: Duration = Duration::from_secs(30);

/// How long a typing indicator lives when the push does not say. Peers repeat their typing
/// push every 3-5 s while they type (observed in live logs), so a bit more than that.
const TYPING_LIFETIME: Duration = Duration::from_secs(7);

pub struct YandexBackend {
    session: Session,
    config: ClientConfig,
    inner: Mutex<Option<Connection>>,
    users: Arc<Mutex<HashMap<String, WireUser>>>,
    /// Test runs on the real account set `BT_WRITE_ONLY_CHAT`: no write leaves for any other chat.
    write_only: Option<ChatId>,
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
        ProtoError::NotFound => BackendError::NotFound,
        ProtoError::Storage(e) => BackendError::Storage(e),
        ProtoError::Commit { status, name, permanent } => {
            BackendError::Rejected { reason: format!("{name} ({status})"), permanent }
        }
    }
}

/// What `fetch_media` loads into memory: avatars and previews. Originals are streamed by `download`.
fn fetch_target(media: &MediaRef) -> Result<MediaTarget, BackendError> {
    match media.kind {
        MediaKind::Avatar => Ok(MediaTarget::Avatar { id: media.id.clone(), size: AvatarSize::Small }),
        MediaKind::Thumbnail => Ok(MediaTarget::Thumbnail { file_id: media.id.clone() }),
        MediaKind::File => Err(BackendError::Protocol("originals are not fetched into memory; they go through download".into())),
    }
}

/// What `download` streams: only the original of a file or image (`MediaKind::File`).
fn download_target(media: &MediaRef) -> Result<MediaTarget, BackendError> {
    match media.kind {
        MediaKind::File => Ok(MediaTarget::Original { file_id: media.id.clone() }),
        MediaKind::Avatar | MediaKind::Thumbnail => {
            Err(BackendError::Protocol("only originals (MediaKind::File) can be downloaded".into()))
        }
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

/// `BT_WRITE_ONLY_CHAT`: unset means no limit; set, it must name a chat.
pub fn parse_write_only(value: Option<String>) -> Result<Option<ChatId>, String> {
    match value {
        None => Ok(None),
        Some(chat) if chat.trim().is_empty() => Err("BT_WRITE_ONLY_CHAT is set but empty".into()),
        Some(chat) => Ok(Some(ChatId(chat))),
    }
}

fn write_guard(allowed: Option<&ChatId>, chat: &ChatId) -> Result<(), BackendError> {
    match allowed {
        Some(allowed) if allowed != chat => {
            Err(BackendError::Rejected { reason: "запись в этот чат запрещена BT_WRITE_ONLY_CHAT".into(), permanent: true })
        }
        _ => Ok(()),
    }
}

impl YandexBackend {
    pub fn new(session: Session, write_only: Option<ChatId>) -> Self {
        Self { session, config: ClientConfig::production(), inner: Mutex::new(None), users: Arc::default(), write_only }
    }

    fn client(&self) -> Result<(Client, String), BackendError> {
        let inner = self.inner.lock().unwrap();
        let connection = inner.as_ref().ok_or_else(|| BackendError::Protocol("not bootstrapped".into()))?;
        Ok((connection.client.clone(), connection.me.clone()))
    }

    /// Every write goes through here: the guard, then one log line without content.
    async fn push(&self, chat: &ChatId, request: WriteRequest) -> Result<Committed, BackendError> {
        let kind = request.kind();
        if let Err(error) = write_guard(self.write_only.as_ref(), chat) {
            tracing::warn!(kind, chat = %chat.0, "write blocked by BT_WRITE_ONLY_CHAT");
            return Err(error);
        }
        tracing::info!(kind, chat = %chat.0, "write");
        let (client, _) = self.client()?;
        client.push(request).await.map_err(backend_error)
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
            payloads: own_payloads(&page.messages, &me),
        })
    }

    async fn fetch_media(&self, media: &MediaRef) -> Result<Vec<u8>, BackendError> {
        let target = fetch_target(media)?;
        let (client, _) = self.client()?;
        client.fetch_media(&target).await.map_err(backend_error)
    }

    async fn download(
        &self,
        media: &MediaRef,
        out: &mut (dyn Write + Send),
        progress: &(dyn Fn(u64, Option<u64>) + Send + Sync),
    ) -> Result<u64, BackendError> {
        let target = download_target(media)?;
        let (client, _) = self.client()?;
        client.download_media(&target, out, progress).await.map_err(backend_error)
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
                            Ok(()) => vec![upserted(&wire, &me, &users.lock().unwrap())],
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
                    XivaEvent::Push(PushEvent::Typing { chat, user, name, timeout_secs }) => {
                        if name.is_none() {
                            if let Err(error) = (Users { client: &client, cache: &users }).load(vec![user.clone()]).await {
                                if tx.send(LiveEvent::Failed(error)).await.is_err() {
                                    return;
                                }
                                continue;
                            }
                        }
                        typing_event(chat, user, name, timeout_secs, &me, &users.lock().unwrap()).into_iter().collect()
                    }
                    XivaEvent::Push(PushEvent::Ignored { .. }) | XivaEvent::Connecting | XivaEvent::Warning(_) => vec![],
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

    fn watch(&self, chat: Option<&ChatId>) {
        if let Some(connection) = self.inner.lock().unwrap().as_ref() {
            connection.client.watch_chats(chat.map(|c| c.0.clone()).into_iter().collect());
        }
    }

    async fn send_text(
        &self,
        chat: &ChatId,
        payload: &PayloadId,
        text: &str,
        reply_to: Option<&ReplyTarget>,
    ) -> Result<Option<MessageTs>, BackendError> {
        let request = WriteRequest::SendText {
            chat: chat.0.clone(),
            payload_id: payload.0.clone(),
            text: text.to_owned(),
            reply: reply_to.map(reply_ref),
        };
        Ok(self.push(chat, request).await?.message.map(|info| info.timestamp_mcs))
    }

    async fn edit(
        &self,
        chat: &ChatId,
        timestamp: MessageTs,
        text: &str,
        reply_to: Option<&ReplyTarget>,
    ) -> Result<(), BackendError> {
        self.push(chat, edit_request(chat, timestamp, text, reply_to)).await.map(drop)
    }

    async fn delete(&self, chat: &ChatId, timestamp: MessageTs) -> Result<(), BackendError> {
        self.push(chat, WriteRequest::Delete { chat: chat.0.clone(), timestamp }).await.map(drop)
    }

    async fn mark_seen(&self, chat: &ChatId, timestamp: MessageTs) -> Result<(), BackendError> {
        self.push(chat, WriteRequest::Seen { chat: chat.0.clone(), timestamp }).await.map(drop)
    }

    async fn typing(&self, chat: &ChatId) -> Result<(), BackendError> {
        self.push(chat, WriteRequest::Typing { chat: chat.0.clone() }).await.map(drop)
    }
}

/// The quote is cut to the protocol's bound when the request is encoded.
fn reply_ref(target: &ReplyTarget) -> ReplyRef {
    ReplyRef { timestamp: target.timestamp, quote: target.text.clone() }
}

/// An edit rewrites the whole message, so a reply repeats its target.
fn edit_request(chat: &ChatId, timestamp: MessageTs, text: &str, reply_to: Option<&ReplyTarget>) -> WriteRequest {
    WriteRequest::Edit { chat: chat.0.clone(), timestamp, text: text.to_owned(), reply: reply_to.map(reply_ref) }
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
        MessageBody::Image { caption, .. } | MessageBody::Gallery { caption, .. } => {
            caption.clone().unwrap_or_else(|| "Фото".into())
        }
        MessageBody::File { name, .. } => name.clone(),
        MessageBody::Deleted => "Сообщение удалено".into(),
        MessageBody::Unsupported(kind) => format!("[{kind}]"),
    }
}

fn body(wire: &WireBody) -> MessageBody {
    match wire {
        WireBody::Text(text) => MessageBody::Text(text.clone()),
        WireBody::Image(image) => image_body(image, None),
        WireBody::Gallery { items, text } if items.is_empty() => MessageBody::Text(text.clone().unwrap_or_default()),
        WireBody::Gallery { items, text } => {
            MessageBody::Gallery { items: items.iter().map(gallery_item).collect(), caption: text.clone() }
        }
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

fn thumbnail_ref(image: &yandex_proto::wire::WireImage) -> MediaRef {
    MediaRef { id: image.file.id.clone(), kind: MediaKind::Thumbnail }
}

fn gallery_item(image: &yandex_proto::wire::WireImage) -> GalleryItem {
    GalleryItem { media: thumbnail_ref(image), width: image.width, height: image.height, name: image.file.name.clone() }
}

fn image_body(image: &yandex_proto::wire::WireImage, caption: Option<String>) -> MessageBody {
    MessageBody::Image {
        media: thumbnail_ref(image),
        width: image.width,
        height: image.height,
        caption,
        name: image.file.name.clone(),
    }
}

fn is_own(wire: &WireMessage, me: &str) -> bool {
    wire.from.as_ref().and_then(|f| f.guid.as_deref()) == Some(me)
}

/// `PayloadId`s of the account's own messages: a send that reached the server shows up here
/// even when its confirmation and echo were lost.
fn own_payloads(messages: &[WireMessage], me: &str) -> Vec<PayloadId> {
    messages.iter().filter(|m| is_own(m, me)).filter_map(|m| m.payload_id.clone().map(PayloadId)).collect()
}

fn message(wire: &WireMessage, me: &str, users: &HashMap<String, WireUser>) -> Message {
    let from = wire.from.as_ref();
    let mut message = Message {
        chat: ChatId(wire.chat.clone()),
        timestamp: wire.timestamp,
        author: UserId(from.and_then(|f| f.guid.clone()).unwrap_or_default()),
        author_name: sender_name(from, users),
        author_avatar: sender_avatar(from, users),
        outgoing: is_own(wire, me),
        body: body(&wire.body),
        edited: wire.edited,
        reply_to: None,
        forwarded_from: None,
        delivery: Delivery::Sent,
    };
    match &wire.relation {
        Relation::None => {}
        Relation::Reply { quote, reference, original } => {
            let original_text = original.as_ref().map(|o| preview(&body(&o.body)));
            // A forward with a comment parses as a reply to its original, which lives in another
            // chat. An edit would rebuild the reference with this chat's id, so such a quote gets
            // no target timestamp: it stays visible, and core refuses to edit the message.
            let mut source_chats = reference.iter().filter_map(|r| r.chat.as_ref()).chain(original.iter().filter_map(|o| o.chat.as_ref()));
            let cross_chat = source_chats.any(|chat| *chat != wire.chat);
            let timestamp = reference.as_ref().and_then(|r| r.timestamp).or(original.as_ref().and_then(|o| o.timestamp));
            message.reply_to = Some(ReplyQuote {
                author: original.as_ref().and_then(|o| sender_name(o.from.as_ref(), users)),
                text: quote.clone().or(original_text).unwrap_or_default(),
                timestamp: timestamp.filter(|_| !cross_chat),
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

/// An own message that is new and carries the sender's `PayloadId` is the echo of a send, from
/// this or another client. Edits keep the `PayloadId` but are plain updates, and so is everything
/// other people write.
fn upserted(wire: &WireMessage, me: &str, users: &HashMap<String, WireUser>) -> LiveEvent {
    let message = message(wire, me, users);
    match &wire.payload_id {
        Some(payload) if message.outgoing && !wire.edited => LiveEvent::Echo { payload: PayloadId(payload.clone()), message },
        _ => LiveEvent::MessageUpserted(message),
    }
}

/// The user's own typing on another device is not shown to the user.
fn typing_event(
    chat: String,
    user: String,
    name: Option<String>,
    timeout_secs: Option<u32>,
    me: &str,
    users: &HashMap<String, WireUser>,
) -> Option<LiveEvent> {
    if user == me {
        return None;
    }
    let name = name.or_else(|| users.get(&user)?.display_name.clone());
    let timeout = timeout_secs.filter(|s| *s > 0).map_or(TYPING_LIFETIME, |s| Duration::from_secs(s.into()));
    Some(LiveEvent::Typing { chat: ChatId(chat), user: UserId(user), name, timeout })
}

#[cfg(test)]
mod tests {
    use super::*;
    use yandex_proto::wire::{WireFile, WireForwarded, WireRef};

    fn wire(body: WireBody, relation: Relation, from: &str) -> WireMessage {
        WireMessage {
            chat: "0/0/a".into(),
            timestamp: 10,
            seq_no: None,
            version: None,
            payload_id: None,
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

    fn test_session() -> Session {
        Session { cookies: Vec::new(), identity: yandex_proto::session::Identity { uid: None, guid: "me".into(), display_name: None } }
    }

    #[test]
    fn write_only_chat_blocks_writes_to_every_other_chat() {
        let allowed = ChatId("self".into());
        assert_eq!(write_guard(Some(&allowed), &allowed), Ok(()));
        assert!(matches!(
            write_guard(Some(&allowed), &ChatId("0/0/other".into())),
            Err(BackendError::Rejected { permanent: true, .. })
        ));
        assert_eq!(write_guard(None, &ChatId("0/0/other".into())), Ok(()));
    }

    #[test]
    fn write_only_chat_must_name_a_chat_when_set() {
        assert_eq!(parse_write_only(None), Ok(None));
        assert_eq!(parse_write_only(Some("self".into())), Ok(Some(ChatId("self".into()))));
        assert!(parse_write_only(Some(" ".into())).is_err());
    }

    #[tokio::test]
    async fn a_blocked_write_never_reaches_the_connection() {
        let backend = YandexBackend::new(test_session(), Some(ChatId("self".into())));
        // Unbootstrapped: anything that got past the guard would fail with "not bootstrapped".
        assert!(matches!(
            backend.mark_seen(&ChatId("0/0/other".into()), 1).await,
            Err(BackendError::Rejected { permanent: true, .. })
        ));
        assert!(matches!(backend.mark_seen(&ChatId("self".into()), 1).await, Err(BackendError::Protocol(_))));
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
    fn a_reply_keeps_its_timestamp_only_when_the_original_is_in_the_same_chat() {
        let users = HashMap::new();
        let reply = |reference_chat: Option<&str>, original_chat: Option<&str>| {
            let original = WireForwarded {
                chat: original_chat.map(Into::into),
                timestamp: Some(5),
                from: None,
                body: WireBody::Text("orig".into()),
            };
            let reference = WireRef { chat: reference_chat.map(Into::into), timestamp: Some(5) };
            let relation = Relation::Reply { quote: None, reference: Some(reference), original: Some(original) };
            message(&wire(WireBody::Text("comment".into()), relation, "me"), "me", &users).reply_to.unwrap()
        };
        let same = reply(Some("0/0/a"), Some("0/0/a"));
        assert_eq!((same.text.as_str(), same.timestamp), ("orig", Some(5)));
        // A forward with a comment parses as a reply whose original lives in another chat: the
        // quote stays visible, but an edit could not rebuild its reference, so it has no target.
        for (reference_chat, original_chat) in [(Some("0/0/other"), Some("0/0/other")), (Some("0/0/other"), None), (None, Some("0/0/other"))] {
            let cross = reply(reference_chat, original_chat);
            assert_eq!((cross.text.as_str(), cross.timestamp), ("orig", None), "{reference_chat:?} {original_chat:?}");
        }
    }

    #[test]
    fn commit_refusals_become_rejections_that_keep_their_permanence() {
        let refusal = |status, name, permanent| backend_error(ProtoError::Commit { status, name, permanent });
        assert_eq!(
            refusal(15, "NO_PERMISSION", true),
            BackendError::Rejected { reason: "NO_PERMISSION (15)".into(), permanent: true }
        );
        assert_eq!(
            refusal(18, "THROTTLED", false),
            BackendError::Rejected { reason: "THROTTLED (18)".into(), permanent: false }
        );
        assert_eq!(backend_error(ProtoError::SessionExpired), BackendError::SessionExpired);
        assert_eq!(backend_error(ProtoError::NotFound), BackendError::NotFound);
    }

    #[test]
    fn own_new_message_with_a_payload_id_is_an_echo_everything_else_an_upsert() {
        let users = HashMap::new();
        let mut own = wire(WireBody::Text("hi".into()), Relation::None, "me");
        own.payload_id = Some("p-1".into());
        let LiveEvent::Echo { payload, message } = upserted(&own, "me", &users) else { panic!("echo") };
        assert_eq!((payload, message.outgoing), (PayloadId("p-1".into()), true));

        // An edit keeps the payload id but changes an existing message.
        let mut edit = own.clone();
        edit.edited = true;
        assert!(matches!(upserted(&edit, "me", &users), LiveEvent::MessageUpserted(_)));
        // A message without a payload id, and one from somebody else, are plain upserts.
        let mut bare = own.clone();
        bare.payload_id = None;
        assert!(matches!(upserted(&bare, "me", &users), LiveEvent::MessageUpserted(_)));
        let mut theirs = wire(WireBody::Text("hi".into()), Relation::None, "u1");
        theirs.payload_id = Some("p-2".into());
        assert!(matches!(upserted(&theirs, "me", &users), LiveEvent::MessageUpserted(_)));
    }

    #[test]
    fn edit_repeats_the_reply_target_and_a_plain_edit_has_none() {
        let chat = ChatId("0/0/a".into());
        let target = ReplyTarget { timestamp: 7, author: Some("Ann".into()), text: "orig".into() };
        assert_eq!(
            edit_request(&chat, 9, "new", Some(&target)),
            WriteRequest::Edit {
                chat: "0/0/a".into(),
                timestamp: 9,
                text: "new".into(),
                reply: Some(ReplyRef { timestamp: 7, quote: "orig".into() }),
            }
        );
        assert_eq!(
            edit_request(&chat, 9, "new", None),
            WriteRequest::Edit { chat: "0/0/a".into(), timestamp: 9, text: "new".into(), reply: None }
        );
    }

    #[test]
    fn history_reports_the_payload_ids_of_own_messages_only() {
        let with_payload = |from: &str, payload: Option<&str>| {
            let mut m = wire(WireBody::Text("x".into()), Relation::None, from);
            m.payload_id = payload.map(Into::into);
            m
        };
        let messages = [
            with_payload("me", Some("p-1")),
            with_payload("u1", Some("p-2")),
            with_payload("me", None),
            with_payload("me", Some("p-3")),
        ];
        assert_eq!(own_payloads(&messages, "me"), [PayloadId("p-1".into()), PayloadId("p-3".into())]);
        assert!(own_payloads(&[], "me").is_empty());
    }

    #[test]
    fn typing_is_named_from_the_push_or_the_user_cache_and_never_shows_the_own_account() {
        let users = HashMap::from([user("u1", "Ann")]);
        let typing = |user: &str, name: Option<&str>, secs| typing_event("c".into(), user.into(), name.map(Into::into), secs, "me", &users);
        let Some(LiveEvent::Typing { chat, user, name, timeout }) = typing("u1", None, None) else { panic!("typing") };
        assert_eq!((chat, user, name.as_deref(), timeout), (ChatId("c".into()), UserId("u1".into()), Some("Ann"), TYPING_LIFETIME));
        let Some(LiveEvent::Typing { name, .. }) = typing("u1", Some("Anna"), None) else { panic!("typing") };
        assert_eq!(name.as_deref(), Some("Anna"));
        let Some(LiveEvent::Typing { name, .. }) = typing("stranger", None, None) else { panic!("typing") };
        assert_eq!(name, None);
        // A stated timeout wins; a zero one means "not stated".
        let Some(LiveEvent::Typing { timeout, .. }) = typing("u1", None, Some(3)) else { panic!("typing") };
        assert_eq!(timeout, Duration::from_secs(3));
        let Some(LiveEvent::Typing { timeout, .. }) = typing("u1", None, Some(0)) else { panic!("typing") };
        assert_eq!(timeout, TYPING_LIFETIME);
        assert!(typing("me", None, None).is_none());
    }

    #[test]
    fn backend_error_maps_a_local_write_failure_to_storage() {
        assert_eq!(backend_error(ProtoError::Storage("storage full".into())), BackendError::Storage("storage full".into()));
        assert_eq!(backend_error(ProtoError::NotFound), BackendError::NotFound);
    }

    #[test]
    fn download_targets_an_original_and_fetch_keeps_refusing_one() {
        let media = |kind| MediaRef { id: "b/1".into(), kind };
        assert_eq!(download_target(&media(MediaKind::File)), Ok(MediaTarget::Original { file_id: "b/1".into() }));
        assert!(matches!(download_target(&media(MediaKind::Thumbnail)), Err(BackendError::Protocol(_))));
        assert!(matches!(download_target(&media(MediaKind::Avatar)), Err(BackendError::Protocol(_))));

        assert_eq!(
            fetch_target(&media(MediaKind::Thumbnail)),
            Ok(MediaTarget::Thumbnail { file_id: "b/1".into() })
        );
        assert_eq!(
            fetch_target(&media(MediaKind::Avatar)),
            Ok(MediaTarget::Avatar { id: "b/1".into(), size: AvatarSize::Small })
        );
        let Err(BackendError::Protocol(message)) = fetch_target(&media(MediaKind::File)) else { panic!("file must be refused") };
        assert!(message.contains("download"), "{message}");
    }

    #[test]
    fn maps_media_and_unknown_bodies() {
        let file = WireFile { id: "b/1".into(), name: Some("a.pdf".into()), size: Some(42) };
        assert_eq!(
            body(&WireBody::File(file.clone())),
            MessageBody::File { media: MediaRef { id: "b/1".into(), kind: MediaKind::File }, name: "a.pdf".into(), size: 42 }
        );
        let image = yandex_proto::wire::WireImage { file, width: 4, height: 3, animated: false };
        assert_eq!(
            body(&WireBody::Image(image)),
            MessageBody::Image {
                media: MediaRef { id: "b/1".into(), kind: MediaKind::Thumbnail },
                width: 4,
                height: 3,
                caption: None,
                name: Some("a.pdf".into()),
            }
        );
        assert_eq!(body(&WireBody::Other("Foo".into())), MessageBody::Unsupported("Foo".into()));
        assert_eq!(body(&WireBody::Deleted), MessageBody::Deleted);
    }

    #[test]
    fn a_wire_gallery_keeps_every_item_with_its_name_and_an_empty_one_is_text() {
        let wire_image = |id: &str, name: Option<&str>, width| yandex_proto::wire::WireImage {
            file: WireFile { id: id.into(), name: name.map(Into::into), size: None },
            width,
            height: 3,
            animated: false,
        };
        let items = vec![wire_image("b/1", Some("a.jpg"), 4), wire_image("b/2", None, 5), wire_image("b/3", Some("c.png"), 6)];
        let MessageBody::Gallery { items, caption } = body(&WireBody::Gallery { items, text: Some("отпуск".into()) }) else {
            panic!("gallery")
        };
        assert_eq!(caption.as_deref(), Some("отпуск"));
        let summary: Vec<_> = items.iter().map(|i| (i.media.id.as_str(), i.media.kind, i.width, i.name.as_deref())).collect();
        assert_eq!(
            summary,
            [
                ("b/1", MediaKind::Thumbnail, 4, Some("a.jpg")),
                ("b/2", MediaKind::Thumbnail, 5, None),
                ("b/3", MediaKind::Thumbnail, 6, Some("c.png")),
            ]
        );
        assert_eq!(
            body(&WireBody::Gallery { items: Vec::new(), text: Some("t".into()) }),
            MessageBody::Text("t".into())
        );
        assert_eq!(body(&WireBody::Gallery { items: Vec::new(), text: None }), MessageBody::Text(String::new()));
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
