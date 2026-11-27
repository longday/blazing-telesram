//! `Backend` over the Yandex web protocol: maps wire types to the domain model.

use bt_chat_core::model::*;
use bt_chat_core::reactions::{ReactionCode, ReactionCount, Reactions, ReactionsUpdate};
use bt_chat_core::{Backend, BackendError, CreateError, LiveEvent};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use yandex_proto::bootstrap::bootstrap;
use yandex_proto::client::{Client, ClientConfig};
use yandex_proto::error::ProtoError;
use yandex_proto::manage::ManageError;
use yandex_proto::media::{AvatarSize, MediaTarget};
use yandex_proto::search::SearchItems;
use yandex_proto::session::Session;
use yandex_proto::wire::{
    PushEvent, ReactionAction, Relation, WireBody, WireChat, WireChatKind, WireMember,
    WireMemberRole, WireMembers, WireMessage, WireProfile, WireReactions, WireReactorEntry,
    WireReactors, WireSender, WireUser, refusal_reason, without_refusal_reason,
};
use yandex_proto::write::{Committed, ReactionChange, ReplyRef, WriteRequest};
use yandex_proto::xiva::{XivaEvent, XivaEvents};

/// How long bootstrap waits for the Xiva subscription before reporting a network error.
const CONNECT_DEADLINE: Duration = Duration::from_secs(30);

/// How long a typing indicator lives when the push does not say. Peers repeat their typing
/// push every 3-5 s while they type (observed in live logs), so a bit more than that.
const TYPING_LIFETIME: Duration = Duration::from_secs(7);

/// Members asked per `get_chat_members` page (what the web client asks).
const MEMBER_PAGE: usize = 50;
/// Page requests per member load: 20 pages of 50, so at most 1000 members are listed.
const MEMBER_PAGE_BOUND: usize = 20;
/// The code `get_chat_members` answers for a chat whose list the account may not read (private
/// chats, the self chat, channels the account does not administer).
const MEMBERS_REFUSED_CODE: &str = "no_permissions_for_this_chat_operation";
/// Why a refused member list is not shown.
const MEMBERS_UNAVAILABLE: &str = "список участников недоступен для этой учётной записи";
/// Leads the server's own reason when it refuses a reactor list (the reason text is the server's).
const REACTORS_REFUSED: &str = "Сервер отказал";

pub struct YandexBackend {
    session: Session,
    config: ClientConfig,
    inner: Mutex<Option<Connection>>,
    users: Arc<Mutex<HashMap<String, WireUser>>>,
    /// What the last `chat_list()` showed; search hits resolve private chats and titles from it.
    known: Mutex<KnownChats>,
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
        ProtoError::Commit {
            status,
            name,
            permanent,
        } => BackendError::Rejected {
            reason: format!("{name} ({status})"),
            permanent,
        },
    }
}

/// What `fetch_media` loads into memory: avatars and previews. Originals are streamed by `download`.
fn fetch_target(media: &MediaRef) -> Result<MediaTarget, BackendError> {
    match media.kind {
        MediaKind::Avatar => Ok(MediaTarget::Avatar {
            id: media.id.clone(),
            size: AvatarSize::Small,
        }),
        MediaKind::Thumbnail => Ok(MediaTarget::Thumbnail {
            file_id: media.id.clone(),
        }),
        MediaKind::File => Err(BackendError::Protocol(
            "originals are not fetched into memory; they go through download".into(),
        )),
    }
}

/// What `download` streams: only the original of a file or image (`MediaKind::File`).
fn download_target(media: &MediaRef) -> Result<MediaTarget, BackendError> {
    match media.kind {
        MediaKind::File => Ok(MediaTarget::Original {
            file_id: media.id.clone(),
        }),
        MediaKind::Avatar | MediaKind::Thumbnail => Err(BackendError::Protocol(
            "only originals (MediaKind::File) can be downloaded".into(),
        )),
    }
}

/// Requests fail with "not connected" until the subscription is up, so callers must wait here.
async fn wait_connected(events: &mut XivaEvents, deadline: Duration) -> Result<(), BackendError> {
    let wait = async {
        while let Some(event) = events.recv().await {
            match event {
                XivaEvent::Connected { .. } => return Ok(()),
                XivaEvent::SessionExpired => return Err(BackendError::SessionExpired),
                XivaEvent::Disconnected { reason, .. } => {
                    return Err(BackendError::Network(reason));
                }
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
        Some(allowed) if allowed != chat => Err(BackendError::Rejected {
            reason: "запись в этот чат запрещена BT_WRITE_ONLY_CHAT".into(),
            permanent: true,
        }),
        _ => Ok(()),
    }
}

/// Creating a chat writes outside any one chat, so `BT_WRITE_ONLY_CHAT` (which limits writes to
/// one existing chat) refuses it whatever the target.
fn creation_guard(allowed: Option<&ChatId>) -> Result<(), BackendError> {
    match allowed {
        Some(_) => Err(BackendError::Rejected {
            reason: "создание чатов запрещено BT_WRITE_ONLY_CHAT".into(),
            permanent: true,
        }),
        None => Ok(()),
    }
}

impl YandexBackend {
    pub fn new(session: Session, write_only: Option<ChatId>) -> Self {
        Self {
            session,
            config: ClientConfig::production(),
            inner: Mutex::new(None),
            users: Arc::default(),
            known: Mutex::default(),
            write_only,
        }
    }

    fn client(&self) -> Result<(Client, String), BackendError> {
        let inner = self.inner.lock().unwrap();
        let connection = inner
            .as_ref()
            .ok_or_else(|| BackendError::Protocol("not bootstrapped".into()))?;
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
        let identity = bootstrap(&self.config.registry, &self.session.cookies)
            .await
            .map_err(|e| match e {
                yandex_proto::bootstrap::BootstrapError::SessionRejected => {
                    BackendError::SessionExpired
                }
                other => BackendError::Network(other.to_string()),
            })?;
        let mut session = self.session.clone();
        session.identity = identity.clone();
        // A fresh connection per attempt: the engine only reconnects after the old one failed.
        let (client, mut events) =
            Client::start(&session, self.config.clone()).map_err(backend_error)?;
        wait_connected(&mut events, CONNECT_DEADLINE).await?;
        *self.inner.lock().unwrap() = Some(Connection {
            client,
            events: Some(events),
            me: identity.guid.clone(),
        });
        Ok(UserId(identity.xiva_user().to_owned()))
    }

    async fn chat_list(&self) -> Result<Vec<ChatSummary>, BackendError> {
        let (client, me) = self.client()?;
        let chats = client.chat_list().await.map_err(backend_error)?;
        let users = Users {
            client: &client,
            cache: &self.users,
        };
        let wanted = chats.iter().flat_map(|c| {
            c.partner_guid.iter().chain(
                c.last_message
                    .iter()
                    .filter_map(|m| m.from.as_ref()?.guid.as_ref()),
            )
        });
        users.load(wanted.cloned().collect()).await?;
        let summaries: Vec<ChatSummary> = {
            let users = self.users.lock().unwrap();
            chats.iter().map(|c| summary(c, &me, &users)).collect()
        };
        *self.known.lock().unwrap() = KnownChats::of(&summaries);
        Ok(summaries)
    }

    async fn history(
        &self,
        chat: &ChatId,
        before: Option<MessageTs>,
        _limit: usize,
    ) -> Result<HistoryChunk, BackendError> {
        let (client, me) = self.client()?;
        let page = client
            .history(&chat.0, before)
            .await
            .map_err(backend_error)?;
        Users {
            client: &client,
            cache: &self.users,
        }
        .load(authors(&page.messages))
        .await?;
        let users = self.users.lock().unwrap();
        Ok(HistoryChunk {
            chat: chat.clone(),
            messages: page
                .messages
                .iter()
                .map(|m| message(m, &me, &users))
                .collect(),
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
        client
            .download_media(&target, out, progress)
            .await
            .map_err(backend_error)
    }

    async fn subscribe(&self) -> Result<async_channel::Receiver<LiveEvent>, BackendError> {
        let (client, me, mut events) = {
            let mut inner = self.inner.lock().unwrap();
            let connection = inner
                .as_mut()
                .ok_or_else(|| BackendError::Protocol("not bootstrapped".into()))?;
            let events = connection
                .events
                .take()
                .ok_or_else(|| BackendError::Protocol("already subscribed".into()))?;
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
                    XivaEvent::SessionExpired => {
                        vec![LiveEvent::Failed(BackendError::SessionExpired)]
                    }
                    XivaEvent::Push(PushEvent::MessageUpserted(wire)) => {
                        match (Users {
                            client: &client,
                            cache: &users,
                        })
                        .load(authors(std::slice::from_ref(&wire)))
                        .await
                        {
                            Ok(()) => vec![upserted(&wire, &me, &users.lock().unwrap())],
                            Err(error) => vec![LiveEvent::Failed(error)],
                        }
                    }
                    XivaEvent::Push(PushEvent::MessageDeleted { chat, timestamp }) => {
                        vec![LiveEvent::MessageDeleted {
                            chat: ChatId(chat),
                            timestamp,
                        }]
                    }
                    XivaEvent::Push(PushEvent::ChatChanged { chat }) => {
                        match client.chat_list().await {
                            Ok(chats) => {
                                let users = users.lock().unwrap();
                                chats
                                    .iter()
                                    .filter(|c| c.id == chat)
                                    .map(|c| LiveEvent::ChatChanged(summary(c, &me, &users)))
                                    .collect()
                            }
                            Err(ProtoError::SessionExpired) => {
                                vec![LiveEvent::Failed(BackendError::SessionExpired)]
                            }
                            // A missed row update is repaired by the next chat-list fetch.
                            Err(_) => vec![],
                        }
                    }
                    XivaEvent::Push(PushEvent::Typing {
                        chat,
                        user,
                        name,
                        timeout_secs,
                    }) => {
                        if name.is_none() {
                            if let Err(error) = (Users {
                                client: &client,
                                cache: &users,
                            })
                            .load(vec![user.clone()])
                            .await
                            {
                                if tx.send(LiveEvent::Failed(error)).await.is_err() {
                                    return;
                                }
                                continue;
                            }
                        }
                        typing_event(chat, user, name, timeout_secs, &me, &users.lock().unwrap())
                            .into_iter()
                            .collect()
                    }
                    XivaEvent::Push(PushEvent::Reaction {
                        chat,
                        timestamp,
                        actor,
                        action,
                        code,
                        reactions,
                    }) => {
                        vec![reactions_changed(
                            chat,
                            timestamp,
                            actor.as_deref(),
                            action,
                            code,
                            &reactions,
                            &me,
                        )]
                    }
                    XivaEvent::Push(PushEvent::Ignored { .. })
                    | XivaEvent::Connecting
                    | XivaEvent::Warning(_) => vec![],
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
            connection
                .client
                .watch_chats(chat.map(|c| c.0.clone()).into_iter().collect());
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
        Ok(self
            .push(chat, request)
            .await?
            .message
            .map(|info| info.timestamp_mcs))
    }

    async fn edit(
        &self,
        chat: &ChatId,
        timestamp: MessageTs,
        text: &str,
        reply_to: Option<&ReplyTarget>,
    ) -> Result<(), BackendError> {
        self.push(chat, edit_request(chat, timestamp, text, reply_to))
            .await
            .map(drop)
    }

    async fn delete(&self, chat: &ChatId, timestamp: MessageTs) -> Result<(), BackendError> {
        self.push(
            chat,
            WriteRequest::Delete {
                chat: chat.0.clone(),
                timestamp,
            },
        )
        .await
        .map(drop)
    }

    async fn react(
        &self,
        chat: &ChatId,
        timestamp: MessageTs,
        target: Option<ReactionCode>,
    ) -> Result<(), BackendError> {
        self.push(chat, reaction_request(chat, timestamp, target))
            .await
            .map(drop)
    }

    async fn mark_seen(&self, chat: &ChatId, timestamp: MessageTs) -> Result<(), BackendError> {
        self.push(
            chat,
            WriteRequest::Seen {
                chat: chat.0.clone(),
                timestamp,
            },
        )
        .await
        .map(drop)
    }

    async fn typing(&self, chat: &ChatId) -> Result<(), BackendError> {
        self.push(
            chat,
            WriteRequest::Typing {
                chat: chat.0.clone(),
            },
        )
        .await
        .map(drop)
    }

    async fn search(&self, query: &str, limit: usize) -> Result<SearchHits, BackendError> {
        let (client, me) = self.client()?;
        let reply = client.search(query).await.map_err(backend_error)?;
        let items = reply.decoded;
        // Search items carry names only sometimes; the user cache fills the rest.
        let nameless = items
            .users
            .iter()
            .filter(|u| u.display_name.is_none())
            .map(|u| u.guid.clone());
        let anonymous = items.messages.iter().filter_map(|m| {
            let from = m.from.as_ref()?;
            from.guid.clone().filter(|_| from.display_name.is_none())
        });
        Users {
            client: &client,
            cache: &self.users,
        }
        .load(nameless.chain(anonymous).collect())
        .await?;
        let hits = search_hits(
            &items,
            &me,
            &self.users.lock().unwrap(),
            &self.known.lock().unwrap(),
            limit,
        );
        // Counts only: the query and the hits' text stay out of the log above `trace`.
        tracing::debug!(
            chats = hits.chats.len(),
            people = hits.people.len(),
            messages = hits.messages.len(),
            skipped = items.skipped,
            "search mapped"
        );
        Ok(hits)
    }

    async fn members(&self, chat: &ChatSummary) -> Result<ChatMembers, BackendError> {
        let (client, me) = self.client()?;
        if chat.kind == ChatKind::Private {
            return Ok(private_members(chat, &me));
        }
        let id = chat.id.0.as_str();
        let listed = list_members(|offset| {
            let client = &client;
            async move {
                client
                    .members(id, MEMBER_PAGE as u32, offset.as_deref())
                    .await
                    .map_err(members_error)
            }
        })
        .await?;
        // The member reply names almost everyone; the user cache fills only the rows it left blank.
        let nameless = listed
            .users
            .iter()
            .filter(|u| blank(u.display_name.as_deref()))
            .map(|u| u.guid.clone());
        Users {
            client: &client,
            cache: &self.users,
        }
        .load(nameless.collect())
        .await?;
        let users = {
            let cache = self.users.lock().unwrap();
            listed
                .users
                .iter()
                .map(|u| member(u, &me, &cache))
                .collect::<Vec<_>>()
        };
        // Counts only: no name, guid or chat id reaches the log.
        tracing::debug!(
            count = users.len(),
            partial = listed.partial,
            skipped = listed.skipped,
            "members mapped"
        );
        Ok(ChatMembers {
            chat: chat.id.clone(),
            users,
            partial: listed.partial,
        })
    }

    async fn profile(&self) -> Result<Profile, BackendError> {
        let (client, me) = self.client()?;
        let wire = client.whoami().await.map_err(backend_error)?;
        Ok(profile_of(wire, &me))
    }

    async fn reactors(
        &self,
        chat: &ChatId,
        timestamp: MessageTs,
        before: Option<MessageTs>,
        limit: usize,
    ) -> Result<ReactorsPage, BackendError> {
        let (client, me) = self.client()?;
        // The page size is the caller's: it goes to the server as received (saturating at the
        // wire's u32), not replaced by a protocol constant.
        let limit = u32::try_from(limit).unwrap_or(u32::MAX);
        let wire = client
            .reactors(&chat.0, timestamp, before, limit)
            .await
            .map_err(reactors_error)?;
        // The entries name almost everyone; the user cache fills only the rows they left blank.
        let nameless = wire
            .entries
            .iter()
            .filter(|e| blank(e.display_name.as_deref()))
            .map(|e| e.guid.clone());
        Users {
            client: &client,
            cache: &self.users,
        }
        .load(nameless.collect())
        .await?;
        let page = reactors_page(chat, timestamp, wire, &me, &self.users.lock().unwrap());
        // Counts only: no name, guid or chat id reaches the log.
        tracing::debug!(
            shown = page.reactors.len(),
            raw = page.raw,
            skipped = page.skipped,
            "reactors mapped"
        );
        Ok(page)
    }

    async fn create_private_chat(&self, user: &UserId) -> Result<Option<ChatId>, CreateError> {
        const KIND: &str = "create_private_chat";
        if let Err(error) = creation_guard(self.write_only.as_ref()) {
            tracing::warn!(kind = KIND, "creation blocked by BT_WRITE_ONLY_CHAT");
            return Err(CreateError::Failed(error));
        }
        let (client, _) = self.client().map_err(CreateError::Failed)?;
        // The kind and the outcome only: the person stays out of the log.
        tracing::info!(kind = KIND, "create");
        match client.create_private_chat(&user.0).await {
            Ok(created) => {
                tracing::info!(kind = KIND, named = created.is_some(), "created");
                Ok(created.map(ChatId))
            }
            Err(error) => {
                let error = create_error(error);
                tracing::warn!(
                    kind = KIND,
                    unknown = matches!(error, CreateError::Unknown(_)),
                    "creation did not succeed"
                );
                Err(error)
            }
        }
    }
}

/// A refused member list is not an error of the protocol but a fact about the account: say so
/// in words the panel can show, permanent because asking again cannot help.
fn members_error(error: ProtoError) -> BackendError {
    match error {
        ProtoError::Protocol(reason) if reason.contains(MEMBERS_REFUSED_CODE) => {
            BackendError::Rejected {
                reason: MEMBERS_UNAVAILABLE.into(),
                permanent: true,
            }
        }
        other => backend_error(other),
    }
}

/// A refused reactor list carries the server's reason (`ErrorInfo.Message`, e.g. "reactions are
/// anonymous" for a channel message) and the status code and name, as a permanent rejection the
/// list shows: «Сервер отказал: <reason> (status 4 ENTITY_NOT_FOUND)». A refusal without a reason
/// and every other failure keep the ordinary mapping.
fn reactors_error(error: ProtoError) -> BackendError {
    match error {
        ProtoError::Protocol(text) => match refusal_reason(&text) {
            Some(reason) => BackendError::Rejected {
                reason: format!(
                    "{REACTORS_REFUSED}: {reason} ({})",
                    without_refusal_reason(&text)
                ),
                permanent: true,
            },
            None => BackendError::Protocol(text),
        },
        other => backend_error(other),
    }
}

/// `NotSent` and `Rejected` mean nothing was created; `Unknown` means the chat may exist.
fn create_error(error: ManageError) -> CreateError {
    match error {
        ManageError::NotSent(e) | ManageError::Rejected(e) => CreateError::Failed(backend_error(e)),
        ManageError::Unknown(e) => CreateError::Unknown(e.to_string()),
    }
}

fn blank(text: Option<&str>) -> bool {
    text.is_none_or(|t| t.trim().is_empty())
}

/// Every member of a group or channel as far as the paging bound allows.
struct Listed {
    users: Vec<WireMember>,
    /// The bound was reached, or the server stopped advancing, with a full last page.
    partial: bool,
    /// Users the pages carried without a guid.
    skipped: usize,
}

/// Pages through `page(guid_offset)` until a page holds fewer than `MEMBER_PAGE` users: the
/// server sends no total and no "more" flag. A failing page fails the whole load, so a cut
/// list is never delivered as complete. A full page that adds nobody new (the server ignored
/// the offset) cannot be continued and counts as partial rather than looping.
async fn list_members<F, Fut>(mut page: F) -> Result<Listed, BackendError>
where
    F: FnMut(Option<String>) -> Fut,
    Fut: Future<Output = Result<WireMembers, BackendError>>,
{
    let mut listed = Listed {
        users: Vec::new(),
        partial: false,
        skipped: 0,
    };
    let mut seen = HashSet::new();
    let mut offset = None;
    for _ in 0..MEMBER_PAGE_BOUND {
        let reply = page(offset.take()).await?;
        listed.skipped += reply.skipped;
        let full = reply.users.len() + reply.skipped >= MEMBER_PAGE;
        let last = reply.users.last().map(|u| u.guid.clone());
        let before = listed.users.len();
        listed.users.extend(
            reply
                .users
                .into_iter()
                .filter(|u| seen.insert(u.guid.clone())),
        );
        if !full {
            return Ok(listed);
        }
        match last {
            Some(last) if listed.users.len() > before => offset = Some(last),
            _ => break,
        }
    }
    listed.partial = true;
    Ok(listed)
}

fn member(wire: &WireMember, me: &str, users: &HashMap<String, WireUser>) -> Member {
    let cached = users.get(&wire.guid);
    let name = wire
        .display_name
        .clone()
        .filter(|n| !blank(Some(n)))
        .or_else(|| cached?.display_name.clone());
    let avatar = wire.avatar_id.clone().or_else(|| cached?.avatar_id.clone());
    Member {
        user: UserId(wire.guid.clone()),
        name,
        avatar: avatar.map(|id| MediaRef {
            id,
            kind: MediaKind::Avatar,
        }),
        role: if wire.role == WireMemberRole::Admin {
            MemberRole::Admin
        } else {
            MemberRole::Member
        },
        me: wire.guid == me,
    }
}

/// The domain page of one `list_reactions` reply. `raw`, `skipped` and `oldest` pass through
/// from the wire untouched (the cursor must not depend on which entries could be shown);
/// `request` stays 0, the engine copies the caller's token in.
fn reactors_page(
    chat: &ChatId,
    timestamp: MessageTs,
    wire: WireReactors,
    me: &str,
    users: &HashMap<String, WireUser>,
) -> ReactorsPage {
    ReactorsPage {
        chat: chat.clone(),
        timestamp,
        request: 0,
        reactors: wire
            .entries
            .iter()
            .map(|e| reactor_row(e, me, users))
            .collect(),
        skipped: wire.skipped,
        raw: wire.raw,
        oldest: wire.oldest,
    }
}

/// The entry's own name (non-blank), else the cached user's, else none; the avatar the same way.
fn reactor_row(wire: &WireReactorEntry, me: &str, users: &HashMap<String, WireUser>) -> Reactor {
    let cached = users.get(&wire.guid);
    let name = wire
        .display_name
        .clone()
        .filter(|n| !blank(Some(n)))
        .or_else(|| cached?.display_name.clone().filter(|n| !blank(Some(n))));
    let avatar = wire.avatar_id.clone().or_else(|| cached?.avatar_id.clone());
    Reactor {
        user: UserId(wire.guid.clone()),
        name,
        avatar: avatar.map(|id| MediaRef {
            id,
            kind: MediaKind::Avatar,
        }),
        code: ReactionCode(wire.code),
        at: wire.timestamp,
        me: wire.guid == me,
    }
}

/// A private chat has no member list on the server: the partner is the chat row itself, with
/// the name and avatar the row carries. A row without a partner has nobody to show.
fn private_members(chat: &ChatSummary, me: &str) -> ChatMembers {
    let users = chat
        .partner
        .iter()
        .map(|partner| Member {
            user: partner.clone(),
            name: Some(chat.title.clone()),
            avatar: chat.avatar.clone(),
            role: MemberRole::Member,
            me: partner.0 == me,
        })
        .collect();
    ChatMembers {
        chat: chat.id.clone(),
        users,
        partial: false,
    }
}

/// `whoami` leaves the guid out only in odd replies; the session's own guid stands in.
fn profile_of(wire: WireProfile, me: &str) -> Profile {
    Profile {
        user: UserId(wire.guid.unwrap_or_else(|| me.to_owned())),
        name: wire.name,
        avatar: wire.avatar_id.map(|id| MediaRef {
            id,
            kind: MediaKind::Avatar,
        }),
        status_text: wire.status_text,
    }
}

/// The quote is cut to the protocol's bound when the request is encoded.
fn reply_ref(target: &ReplyTarget) -> ReplyRef {
    ReplyRef {
        timestamp: target.timestamp,
        quote: target.text.clone(),
    }
}

/// An edit rewrites the whole message, so a reply repeats its target.
fn edit_request(
    chat: &ChatId,
    timestamp: MessageTs,
    text: &str,
    reply_to: Option<&ReplyTarget>,
) -> WriteRequest {
    WriteRequest::Edit {
        chat: chat.0.clone(),
        timestamp,
        text: text.to_owned(),
        reply: reply_to.map(reply_ref),
    }
}

/// `None` removes the account's reaction; `Some` sets it (the server replaces the previous one).
fn reaction_request(
    chat: &ChatId,
    timestamp: MessageTs,
    target: Option<ReactionCode>,
) -> WriteRequest {
    let change = target.map_or(ReactionChange::Remove, |code| ReactionChange::Set(code.0));
    WriteRequest::Reaction {
        chat: chat.0.clone(),
        timestamp,
        change,
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
    async fn load(&self, guids: Vec<String>) -> Result<(), BackendError> {
        let guids = uncached_users(self.cache, guids);
        if guids.is_empty() {
            return Ok(());
        }
        absorb_users(self.cache, self.client.users(&guids).await)
    }
}

/// The guids of `guids` the cache does not know yet, sorted and without repeats.
fn uncached_users(cache: &Mutex<HashMap<String, WireUser>>, mut guids: Vec<String>) -> Vec<String> {
    {
        let cache = cache.lock().unwrap();
        guids.retain(|g| !cache.contains_key(g));
    }
    guids.sort();
    guids.dedup();
    guids
}

/// Files a lookup answer into the cache. Any failure but a rejected session leaves it as it was.
fn absorb_users(
    cache: &Mutex<HashMap<String, WireUser>>,
    found: Result<Vec<WireUser>, ProtoError>,
) -> Result<(), BackendError> {
    match found {
        Ok(found) => {
            cache
                .lock()
                .unwrap()
                .extend(found.into_iter().map(|u| (u.guid.clone(), u)));
            Ok(())
        }
        Err(ProtoError::SessionExpired) => Err(BackendError::SessionExpired),
        Err(_) => Ok(()),
    }
}

/// The chat list as the last `chat_list()` call returned it: what a search hit needs to point at
/// an existing chat. The search reply has no private-chat marker (private chats have no name),
/// so a person's chat can only come from here.
#[derive(Default)]
struct KnownChats {
    /// Partner guid to the private chat with that person.
    private: HashMap<String, ChatId>,
    /// Chat id to the title the list shows.
    titles: HashMap<String, String>,
}

impl KnownChats {
    /// Private chats by partner come from the summaries' `partner`, so the guid of a person's
    /// chat is derived once, in `summary()`.
    fn of(summaries: &[ChatSummary]) -> Self {
        let private = summaries
            .iter()
            .filter_map(|s| Some((s.partner.as_ref()?.0.clone(), s.id.clone())))
            .collect();
        let titles = summaries
            .iter()
            .map(|s| (s.id.0.clone(), s.title.clone()))
            .collect();
        Self { private, titles }
    }
}

/// Maps decoded search items to hits, at most `limit` per kind. A message hit has no chat title
/// on the wire: it comes from the loaded chat list, else from a chat hit of the same reply, else
/// it stays empty.
fn search_hits(
    items: &SearchItems,
    me: &str,
    users: &HashMap<String, WireUser>,
    known: &KnownChats,
    limit: usize,
) -> SearchHits {
    let avatar = |id: Option<String>| {
        id.map(|id| MediaRef {
            id,
            kind: MediaKind::Avatar,
        })
    };
    let chat_title = |id: &str| {
        let listed = known.titles.get(id);
        listed
            .or_else(|| items.chats.iter().find(|c| c.id == id)?.title.as_ref())
            .cloned()
    };
    SearchHits {
        chats: items
            .chats
            .iter()
            .take(limit)
            .map(|c| ChatHit {
                id: ChatId(c.id.clone()),
                kind: match c.kind {
                    WireChatKind::Private => ChatKind::Private,
                    WireChatKind::Group => ChatKind::Group,
                    WireChatKind::Channel => ChatKind::Channel,
                },
                title: c
                    .title
                    .clone()
                    .or_else(|| known.titles.get(&c.id).cloned())
                    .unwrap_or_else(|| "Без названия".into()),
                avatar: avatar(c.avatar_id.clone()),
            })
            .collect(),
        people: items
            .users
            .iter()
            .take(limit)
            .map(|u| {
                let cached = users.get(&u.guid);
                PersonHit {
                    user: UserId(u.guid.clone()),
                    name: u
                        .display_name
                        .clone()
                        .or_else(|| cached?.display_name.clone())
                        .unwrap_or_else(|| "Без названия".into()),
                    avatar: avatar(u.avatar_id.clone().or_else(|| cached?.avatar_id.clone())),
                    chat: known.private.get(&u.guid).cloned(),
                }
            })
            .collect(),
        messages: items
            .messages
            .iter()
            .take(limit)
            .map(|m| {
                let own = m.from.as_ref().and_then(|f| f.guid.as_deref()) == Some(me);
                MessageHit {
                    chat: ChatId(m.chat.clone()),
                    chat_title: chat_title(&m.chat).unwrap_or_default(),
                    timestamp: m.timestamp,
                    author_name: if own {
                        Some("Вы".into())
                    } else {
                        sender_name(m.from.as_ref(), users)
                    },
                    text: preview(&body(&m.body)),
                }
            })
            .collect(),
    }
}

fn authors(messages: &[WireMessage]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|m| m.from.as_ref()?.guid.clone())
        .collect()
}

fn sender_name(from: Option<&WireSender>, users: &HashMap<String, WireUser>) -> Option<String> {
    let from = from?;
    from.display_name
        .clone()
        .or_else(|| users.get(from.guid.as_ref()?)?.display_name.clone())
}

fn sender_avatar(from: Option<&WireSender>, users: &HashMap<String, WireUser>) -> Option<MediaRef> {
    let from = from?;
    let id = from
        .avatar_id
        .clone()
        .or_else(|| users.get(from.guid.as_ref()?)?.avatar_id.clone())?;
    Some(MediaRef {
        id,
        kind: MediaKind::Avatar,
    })
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
        .map(|id| MediaRef {
            id,
            kind: MediaKind::Avatar,
        });
    let last_message = chat.last_message.as_ref().map(|m| {
        let message = message(m, me, users);
        MessagePreview {
            author: if message.outgoing {
                Some("Вы".into())
            } else {
                message.author_name.clone()
            },
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
        partner: chat.partner_guid.clone().map(UserId),
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
        WireBody::Gallery { items, text } if items.is_empty() => {
            MessageBody::Text(text.clone().unwrap_or_default())
        }
        WireBody::Gallery { items, text } => MessageBody::Gallery {
            items: items.iter().map(gallery_item).collect(),
            caption: text.clone(),
        },
        WireBody::File(file) => MessageBody::File {
            media: MediaRef {
                id: file.id.clone(),
                kind: MediaKind::File,
            },
            name: file.name.clone().unwrap_or_else(|| "Файл".into()),
            size: file.size.unwrap_or(0),
        },
        WireBody::Voice { .. } => MessageBody::Unsupported("Голосовое сообщение".into()),
        WireBody::Sticker { .. } => MessageBody::Unsupported("Стикер".into()),
        WireBody::Poll { title } => MessageBody::Unsupported(format!("Опрос: {title}")),
        WireBody::System {
            text: Some(text), ..
        } => MessageBody::Text(text.clone()),
        WireBody::System { kind, text: None } => MessageBody::Unsupported(kind.clone()),
        WireBody::Deleted => MessageBody::Deleted,
        WireBody::Empty => MessageBody::Text(String::new()),
        WireBody::Other(kind) => MessageBody::Unsupported(kind.clone()),
    }
}

fn thumbnail_ref(image: &yandex_proto::wire::WireImage) -> MediaRef {
    MediaRef {
        id: image.file.id.clone(),
        kind: MediaKind::Thumbnail,
    }
}

fn gallery_item(image: &yandex_proto::wire::WireImage) -> GalleryItem {
    GalleryItem {
        media: thumbnail_ref(image),
        width: image.width,
        height: image.height,
        name: image.file.name.clone(),
    }
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
    messages
        .iter()
        .filter(|m| is_own(m, me))
        .filter_map(|m| m.payload_id.clone().map(PayloadId))
        .collect()
}

/// The account's reaction according to `RecentUserReactions`: the newest entry of the account.
/// The server truncates the list, so `None` means "not seen", not "none".
fn recent_own(reactions: &WireReactions, me: &str) -> Option<ReactionCode> {
    let own = reactions
        .recent
        .iter()
        .filter(|r| r.user.as_deref() == Some(me));
    own.max_by_key(|r| r.timestamp)
        .map(|r| ReactionCode(r.code))
}

fn reaction_counts(reactions: &WireReactions) -> Vec<ReactionCount> {
    reactions
        .counts
        .iter()
        .map(|c| ReactionCount {
            code: ReactionCode(c.code),
            count: c.count,
        })
        .collect()
}

/// Reaction state of a history row or push: counts and version one to one, `mine` from the
/// newest recent entry of the account.
fn message_reactions(wire: &WireReactions, me: &str) -> Reactions {
    Reactions {
        counts: reaction_counts(wire),
        mine: recent_own(wire, me),
        version: wire.version,
    }
}

/// A `Reaction` push as a live update. The actor tells the account's own reaction when it is the
/// account (`Remove` clears it, `Replace` sets the pushed type); otherwise the recent list may
/// still name it; otherwise the event does not say (`None`).
fn reactions_changed(
    chat: String,
    timestamp: MessageTs,
    actor: Option<&str>,
    action: Option<ReactionAction>,
    code: Option<i64>,
    reactions: &WireReactions,
    me: &str,
) -> LiveEvent {
    let from_actor = match (actor == Some(me), action, code) {
        (true, Some(ReactionAction::Remove), _) => Some(None),
        (true, Some(ReactionAction::Replace), Some(code)) => Some(Some(ReactionCode(code))),
        _ => None,
    };
    let mine = from_actor.or_else(|| recent_own(reactions, me).map(Some));
    let update = ReactionsUpdate {
        counts: reaction_counts(reactions),
        version: reactions.version,
        mine,
    };
    LiveEvent::ReactionsChanged {
        chat: ChatId(chat),
        timestamp,
        update,
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
        outgoing: is_own(wire, me),
        body: body(&wire.body),
        edited: wire.edited,
        reply_to: None,
        forwarded_from: None,
        delivery: Delivery::Sent,
        reactions: wire
            .reactions
            .as_ref()
            .map(|r| message_reactions(r, me))
            .unwrap_or_default(),
    };
    match &wire.relation {
        Relation::None => {}
        Relation::Reply {
            quote,
            reference,
            original,
        } => {
            let original_text = original.as_ref().map(|o| preview(&body(&o.body)));
            // A forward with a comment parses as a reply to its original, which lives in another
            // chat. An edit would rebuild the reference with this chat's id, so such a quote gets
            // no target timestamp: it stays visible, and core refuses to edit the message.
            let mut source_chats = reference
                .iter()
                .filter_map(|r| r.chat.as_ref())
                .chain(original.iter().filter_map(|o| o.chat.as_ref()));
            let cross_chat = source_chats.any(|chat| *chat != wire.chat);
            let timestamp = reference
                .as_ref()
                .and_then(|r| r.timestamp)
                .or(original.as_ref().and_then(|o| o.timestamp));
            message.reply_to = Some(ReplyQuote {
                author: original
                    .as_ref()
                    .and_then(|o| sender_name(o.from.as_ref(), users)),
                text: quote.clone().or(original_text).unwrap_or_default(),
                timestamp: timestamp.filter(|_| !cross_chat),
            });
        }
        Relation::Forward { originals } => {
            if let Some(first) = originals.first() {
                message.forwarded_from = Some(
                    sender_name(first.from.as_ref(), users).unwrap_or_else(|| "Неизвестный".into()),
                );
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
        Some(payload) if message.outgoing && !wire.edited => LiveEvent::Echo {
            payload: PayloadId(payload.clone()),
            message,
        },
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
    let timeout = timeout_secs
        .filter(|s| *s > 0)
        .map_or(TYPING_LIFETIME, |s| Duration::from_secs(s.into()));
    Some(LiveEvent::Typing {
        chat: ChatId(chat),
        user: UserId(user),
        name,
        timeout,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use yandex_proto::frame::{IncomingFrame, RequestFrame};
    use yandex_proto::search::{WireSearchChat, WireSearchMessage, WireSearchUser};
    use yandex_proto::wire::{WireFile, WireForwarded, WireReactionCount, WireReactor, WireRef};
    use yandex_proto::xiva::{ConnectError, Connector, Socket, SocketEvent, Target};

    fn wire(body: WireBody, relation: Relation, from: &str) -> WireMessage {
        WireMessage {
            chat: "0/0/a".into(),
            timestamp: 10,
            seq_no: None,
            version: None,
            payload_id: None,
            from: Some(WireSender {
                guid: Some(from.into()),
                display_name: None,
                avatar_id: None,
            }),
            body,
            edited: false,
            important: false,
            relation,
            reactions: None,
            raw: serde_json::Value::Null,
        }
    }

    fn user(guid: &str, name: &str) -> (String, WireUser) {
        (
            guid.into(),
            WireUser {
                guid: guid.into(),
                display_name: Some(name.into()),
                avatar_id: Some(format!("av-{guid}")),
                uid: None,
            },
        )
    }

    fn test_session() -> Session {
        Session {
            cookies: Vec::new(),
            identity: yandex_proto::session::Identity {
                uid: None,
                guid: "me".into(),
                display_name: None,
            },
        }
    }

    #[test]
    fn write_only_chat_blocks_writes_to_every_other_chat() {
        let allowed = ChatId("self".into());
        assert_eq!(write_guard(Some(&allowed), &allowed), Ok(()));
        assert!(matches!(
            write_guard(Some(&allowed), &ChatId("0/0/other".into())),
            Err(BackendError::Rejected {
                permanent: true,
                ..
            })
        ));
        assert_eq!(write_guard(None, &ChatId("0/0/other".into())), Ok(()));
    }

    #[test]
    fn write_only_chat_must_name_a_chat_when_set() {
        assert_eq!(parse_write_only(None), Ok(None));
        assert_eq!(
            parse_write_only(Some("self".into())),
            Ok(Some(ChatId("self".into())))
        );
        assert!(parse_write_only(Some(" ".into())).is_err());
    }

    #[tokio::test]
    async fn a_blocked_write_never_reaches_the_connection() {
        let backend = YandexBackend::new(test_session(), Some(ChatId("self".into())));
        // Unbootstrapped: anything that got past the guard would fail with "not bootstrapped".
        assert!(matches!(
            backend.mark_seen(&ChatId("0/0/other".into()), 1).await,
            Err(BackendError::Rejected {
                permanent: true,
                ..
            })
        ));
        assert!(matches!(
            backend.mark_seen(&ChatId("self".into()), 1).await,
            Err(BackendError::Protocol(_))
        ));
    }

    #[tokio::test]
    async fn fetch_media_refuses_an_original_before_touching_the_connection() {
        let backend = YandexBackend::new(test_session(), None);
        // Unbootstrapped: only the kind check can produce a message about `download`.
        let file = MediaRef {
            id: "b/1".into(),
            kind: MediaKind::File,
        };
        let Err(BackendError::Protocol(message)) = backend.fetch_media(&file).await else {
            panic!("file must be refused")
        };
        assert!(message.contains("download"), "{message}");
        let mut sink = Vec::new();
        let thumb = MediaRef {
            id: "b/1".into(),
            kind: MediaKind::Thumbnail,
        };
        let Err(BackendError::Protocol(message)) =
            backend.download(&thumb, &mut sink, &|_, _| {}).await
        else {
            panic!("a thumbnail is not an original")
        };
        assert!(message.contains("originals"), "{message}");
        assert!(sink.is_empty());
    }

    #[test]
    fn maps_author_from_users_and_marks_own_messages() {
        let users = HashMap::from([user("u1", "Ann")]);
        let theirs = message(
            &wire(WireBody::Text("hi".into()), Relation::None, "u1"),
            "me",
            &users,
        );
        assert_eq!(
            (theirs.author_name.as_deref(), theirs.outgoing),
            (Some("Ann"), false)
        );
        assert_eq!(
            theirs.author_avatar,
            Some(MediaRef {
                id: "av-u1".into(),
                kind: MediaKind::Avatar
            })
        );
        assert!(
            message(
                &wire(WireBody::Text("x".into()), Relation::None, "me"),
                "me",
                &users
            )
            .outgoing
        );
    }

    #[test]
    fn forward_without_comment_shows_the_original() {
        let users = HashMap::from([user("u2", "Bob")]);
        let original = WireForwarded {
            chat: None,
            timestamp: Some(5),
            from: Some(WireSender {
                guid: Some("u2".into()),
                display_name: None,
                avatar_id: None,
            }),
            body: WireBody::Text("orig".into()),
        };
        let m = message(
            &wire(
                WireBody::Empty,
                Relation::Forward {
                    originals: vec![original],
                },
                "u1",
            ),
            "me",
            &users,
        );
        assert_eq!(
            (m.forwarded_from.as_deref(), m.body),
            (Some("Bob"), MessageBody::Text("orig".into()))
        );
    }

    #[test]
    fn reply_prefers_quote_and_falls_back_to_original() {
        let users = HashMap::new();
        let original = WireForwarded {
            chat: None,
            timestamp: Some(5),
            from: None,
            body: WireBody::Text("orig".into()),
        };
        let quoted = Relation::Reply {
            quote: Some("q".into()),
            reference: None,
            original: Some(original.clone()),
        };
        assert_eq!(
            message(
                &wire(WireBody::Text("a".into()), quoted, "u1"),
                "me",
                &users
            )
            .reply_to
            .unwrap()
            .text,
            "q"
        );
        let plain = Relation::Reply {
            quote: None,
            reference: None,
            original: Some(original),
        };
        let reply = message(&wire(WireBody::Text("a".into()), plain, "u1"), "me", &users)
            .reply_to
            .unwrap();
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
            let reference = WireRef {
                chat: reference_chat.map(Into::into),
                timestamp: Some(5),
            };
            let relation = Relation::Reply {
                quote: None,
                reference: Some(reference),
                original: Some(original),
            };
            message(
                &wire(WireBody::Text("comment".into()), relation, "me"),
                "me",
                &users,
            )
            .reply_to
            .unwrap()
        };
        let same = reply(Some("0/0/a"), Some("0/0/a"));
        assert_eq!((same.text.as_str(), same.timestamp), ("orig", Some(5)));
        // A forward with a comment parses as a reply whose original lives in another chat: the
        // quote stays visible, but an edit could not rebuild its reference, so it has no target.
        for (reference_chat, original_chat) in [
            (Some("0/0/other"), Some("0/0/other")),
            (Some("0/0/other"), None),
            (None, Some("0/0/other")),
        ] {
            let cross = reply(reference_chat, original_chat);
            assert_eq!(
                (cross.text.as_str(), cross.timestamp),
                ("orig", None),
                "{reference_chat:?} {original_chat:?}"
            );
        }
    }

    #[test]
    fn commit_refusals_become_rejections_that_keep_their_permanence() {
        let refusal = |status, name, permanent| {
            backend_error(ProtoError::Commit {
                status,
                name,
                permanent,
            })
        };
        assert_eq!(
            refusal(15, "NO_PERMISSION", true),
            BackendError::Rejected {
                reason: "NO_PERMISSION (15)".into(),
                permanent: true
            }
        );
        assert_eq!(
            refusal(18, "THROTTLED", false),
            BackendError::Rejected {
                reason: "THROTTLED (18)".into(),
                permanent: false
            }
        );
        assert_eq!(
            backend_error(ProtoError::SessionExpired),
            BackendError::SessionExpired
        );
        assert_eq!(backend_error(ProtoError::NotFound), BackendError::NotFound);
    }

    #[test]
    fn own_new_message_with_a_payload_id_is_an_echo_everything_else_an_upsert() {
        let users = HashMap::new();
        let mut own = wire(WireBody::Text("hi".into()), Relation::None, "me");
        own.payload_id = Some("p-1".into());
        let LiveEvent::Echo { payload, message } = upserted(&own, "me", &users) else {
            panic!("echo")
        };
        assert_eq!((payload, message.outgoing), (PayloadId("p-1".into()), true));

        // An edit keeps the payload id but changes an existing message.
        let mut edit = own.clone();
        edit.edited = true;
        assert!(matches!(
            upserted(&edit, "me", &users),
            LiveEvent::MessageUpserted(_)
        ));
        // A message without a payload id, and one from somebody else, are plain upserts.
        let mut bare = own.clone();
        bare.payload_id = None;
        assert!(matches!(
            upserted(&bare, "me", &users),
            LiveEvent::MessageUpserted(_)
        ));
        let mut theirs = wire(WireBody::Text("hi".into()), Relation::None, "u1");
        theirs.payload_id = Some("p-2".into());
        assert!(matches!(
            upserted(&theirs, "me", &users),
            LiveEvent::MessageUpserted(_)
        ));
    }

    #[test]
    fn edit_repeats_the_reply_target_and_a_plain_edit_has_none() {
        let chat = ChatId("0/0/a".into());
        let target = ReplyTarget {
            timestamp: 7,
            author: Some("Ann".into()),
            text: "orig".into(),
        };
        assert_eq!(
            edit_request(&chat, 9, "new", Some(&target)),
            WriteRequest::Edit {
                chat: "0/0/a".into(),
                timestamp: 9,
                text: "new".into(),
                reply: Some(ReplyRef {
                    timestamp: 7,
                    quote: "orig".into()
                }),
            }
        );
        assert_eq!(
            edit_request(&chat, 9, "new", None),
            WriteRequest::Edit {
                chat: "0/0/a".into(),
                timestamp: 9,
                text: "new".into(),
                reply: None
            }
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
        assert_eq!(
            own_payloads(&messages, "me"),
            [PayloadId("p-1".into()), PayloadId("p-3".into())]
        );
        assert!(own_payloads(&[], "me").is_empty());
    }

    #[test]
    fn typing_is_named_from_the_push_or_the_user_cache_and_never_shows_the_own_account() {
        let users = HashMap::from([user("u1", "Ann")]);
        let typing = |user: &str, name: Option<&str>, secs| {
            typing_event(
                "c".into(),
                user.into(),
                name.map(Into::into),
                secs,
                "me",
                &users,
            )
        };
        let Some(LiveEvent::Typing {
            chat,
            user,
            name,
            timeout,
        }) = typing("u1", None, None)
        else {
            panic!("typing")
        };
        assert_eq!(
            (chat, user, name.as_deref(), timeout),
            (
                ChatId("c".into()),
                UserId("u1".into()),
                Some("Ann"),
                TYPING_LIFETIME
            )
        );
        let Some(LiveEvent::Typing { name, .. }) = typing("u1", Some("Anna"), None) else {
            panic!("typing")
        };
        assert_eq!(name.as_deref(), Some("Anna"));
        let Some(LiveEvent::Typing { name, .. }) = typing("stranger", None, None) else {
            panic!("typing")
        };
        assert_eq!(name, None);
        // A stated timeout wins; a zero one means "not stated".
        let Some(LiveEvent::Typing { timeout, .. }) = typing("u1", None, Some(3)) else {
            panic!("typing")
        };
        assert_eq!(timeout, Duration::from_secs(3));
        let Some(LiveEvent::Typing { timeout, .. }) = typing("u1", None, Some(0)) else {
            panic!("typing")
        };
        assert_eq!(timeout, TYPING_LIFETIME);
        assert!(typing("me", None, None).is_none());
    }

    #[test]
    fn backend_error_maps_a_local_write_failure_to_storage() {
        assert_eq!(
            backend_error(ProtoError::Storage("storage full".into())),
            BackendError::Storage("storage full".into())
        );
        assert_eq!(backend_error(ProtoError::NotFound), BackendError::NotFound);
    }

    #[test]
    fn download_targets_an_original_and_fetch_keeps_refusing_one() {
        let media = |kind| MediaRef {
            id: "b/1".into(),
            kind,
        };
        assert_eq!(
            download_target(&media(MediaKind::File)),
            Ok(MediaTarget::Original {
                file_id: "b/1".into()
            })
        );
        assert!(matches!(
            download_target(&media(MediaKind::Thumbnail)),
            Err(BackendError::Protocol(_))
        ));
        assert!(matches!(
            download_target(&media(MediaKind::Avatar)),
            Err(BackendError::Protocol(_))
        ));

        assert_eq!(
            fetch_target(&media(MediaKind::Thumbnail)),
            Ok(MediaTarget::Thumbnail {
                file_id: "b/1".into()
            })
        );
        assert_eq!(
            fetch_target(&media(MediaKind::Avatar)),
            Ok(MediaTarget::Avatar {
                id: "b/1".into(),
                size: AvatarSize::Small
            })
        );
        let Err(BackendError::Protocol(message)) = fetch_target(&media(MediaKind::File)) else {
            panic!("file must be refused")
        };
        assert!(message.contains("download"), "{message}");
    }

    #[test]
    fn maps_media_and_unknown_bodies() {
        let file = WireFile {
            id: "b/1".into(),
            name: Some("a.pdf".into()),
            size: Some(42),
        };
        assert_eq!(
            body(&WireBody::File(file.clone())),
            MessageBody::File {
                media: MediaRef {
                    id: "b/1".into(),
                    kind: MediaKind::File
                },
                name: "a.pdf".into(),
                size: 42
            }
        );
        let image = yandex_proto::wire::WireImage {
            file,
            width: 4,
            height: 3,
            animated: false,
        };
        assert_eq!(
            body(&WireBody::Image(image)),
            MessageBody::Image {
                media: MediaRef {
                    id: "b/1".into(),
                    kind: MediaKind::Thumbnail
                },
                width: 4,
                height: 3,
                caption: None,
                name: Some("a.pdf".into()),
            }
        );
        assert_eq!(
            body(&WireBody::Other("Foo".into())),
            MessageBody::Unsupported("Foo".into())
        );
        assert_eq!(body(&WireBody::Deleted), MessageBody::Deleted);
    }

    #[test]
    fn a_wire_gallery_keeps_every_item_with_its_name_and_an_empty_one_is_text() {
        let wire_image = |id: &str, name: Option<&str>, width| yandex_proto::wire::WireImage {
            file: WireFile {
                id: id.into(),
                name: name.map(Into::into),
                size: None,
            },
            width,
            height: 3,
            animated: false,
        };
        let items = vec![
            wire_image("b/1", Some("a.jpg"), 4),
            wire_image("b/2", None, 5),
            wire_image("b/3", Some("c.png"), 6),
        ];
        let MessageBody::Gallery { items, caption } = body(&WireBody::Gallery {
            items,
            text: Some("отпуск".into()),
        }) else {
            panic!("gallery")
        };
        assert_eq!(caption.as_deref(), Some("отпуск"));
        let summary: Vec<_> = items
            .iter()
            .map(|i| {
                (
                    i.media.id.as_str(),
                    i.media.kind,
                    i.width,
                    i.name.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                ("b/1", MediaKind::Thumbnail, 4, Some("a.jpg")),
                ("b/2", MediaKind::Thumbnail, 5, None),
                ("b/3", MediaKind::Thumbnail, 6, Some("c.png")),
            ]
        );
        assert_eq!(
            body(&WireBody::Gallery {
                items: Vec::new(),
                text: Some("t".into())
            }),
            MessageBody::Text("t".into())
        );
        assert_eq!(
            body(&WireBody::Gallery {
                items: Vec::new(),
                text: None
            }),
            MessageBody::Text(String::new())
        );
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
        assert_eq!(
            (preview.author.as_deref(), preview.text.as_str()),
            (Some("Вы"), "hey")
        );
    }

    fn search_message(
        chat: &str,
        from: Option<(&str, Option<&str>)>,
        text: &str,
    ) -> WireSearchMessage {
        WireSearchMessage {
            chat: chat.into(),
            timestamp: 7,
            from: from.map(|(guid, name)| WireSender {
                guid: Some(guid.into()),
                display_name: name.map(Into::into),
                avatar_id: None,
            }),
            body: WireBody::Text(text.into()),
        }
    }

    fn search_user(guid: &str, name: Option<&str>) -> WireSearchUser {
        WireSearchUser {
            guid: guid.into(),
            display_name: name.map(Into::into),
            avatar_id: None,
        }
    }

    fn search_chat(id: &str, kind: WireChatKind, title: Option<&str>) -> WireSearchChat {
        WireSearchChat {
            id: id.into(),
            kind,
            title: title.map(Into::into),
            avatar_id: Some("av".into()),
            members_count: None,
        }
    }

    fn listed_chats() -> KnownChats {
        KnownChats {
            private: HashMap::from([("u1".into(), ChatId("0/0/u1".into()))]),
            titles: HashMap::from([
                ("0/0/u1".into(), "Ann".into()),
                ("0/1/g".into(), "Team".into()),
            ]),
        }
    }

    #[test]
    fn search_maps_chats_people_and_messages() {
        let users = HashMap::from([user("u2", "Bob")]);
        let items = SearchItems {
            chats: vec![
                search_chat("0/1/g", WireChatKind::Group, Some("Team")),
                search_chat("1/2/c", WireChatKind::Channel, Some("News")),
                search_chat("0/1/h", WireChatKind::Group, None),
            ],
            users: vec![
                search_user("u1", Some("Ann")),
                search_user("u9", Some("Eve")),
            ],
            messages: vec![
                // Listed chat, author named by the wire.
                search_message("0/0/u1", Some(("u1", Some("Ann"))), "привет"),
                // Chat only in the reply's chat hits, author from the user cache.
                search_message("1/2/c", Some(("u2", None)), "news"),
                // Unknown chat and author; the own message is told by the account guid.
                search_message("0/9/x", None, "lost"),
                search_message("0/1/g", Some(("me", None)), "mine"),
            ],
            skipped: 0,
        };
        let hits = search_hits(&items, "me", &users, &listed_chats(), 10);

        let chats: Vec<_> = hits
            .chats
            .iter()
            .map(|c| (c.id.0.as_str(), c.kind, c.title.as_str()))
            .collect();
        assert_eq!(
            chats,
            [
                ("0/1/g", ChatKind::Group, "Team"),
                ("1/2/c", ChatKind::Channel, "News"),
                // A hit without a title takes none from a list that lacks it either.
                ("0/1/h", ChatKind::Group, "Без названия"),
            ]
        );
        assert_eq!(
            hits.chats[0].avatar,
            Some(MediaRef {
                id: "av".into(),
                kind: MediaKind::Avatar
            })
        );

        let messages: Vec<_> = hits
            .messages
            .iter()
            .map(|m| {
                (
                    m.chat.0.as_str(),
                    m.chat_title.as_str(),
                    m.author_name.as_deref(),
                    m.text.as_str(),
                )
            })
            .collect();
        assert_eq!(
            messages,
            [
                ("0/0/u1", "Ann", Some("Ann"), "привет"),
                ("1/2/c", "News", Some("Bob"), "news"),
                ("0/9/x", "", None, "lost"),
                ("0/1/g", "Team", Some("Вы"), "mine"),
            ]
        );
        assert_eq!(hits.messages[0].timestamp, 7);
    }

    #[test]
    fn a_person_opens_a_private_chat_only_when_the_chat_list_has_one() {
        let users = HashMap::from([user("u3", "Cat")]);
        let items = SearchItems {
            users: vec![
                search_user("u1", Some("Ann")),
                search_user("u9", Some("Eve")),
                search_user("u3", None),
            ],
            ..SearchItems::default()
        };
        let hits = search_hits(&items, "me", &users, &listed_chats(), 10);
        let people: Vec<_> = hits
            .people
            .iter()
            .map(|p| (p.user.0.as_str(), p.name.as_str(), p.chat.clone()))
            .collect();
        assert_eq!(
            people,
            [
                ("u1", "Ann", Some(ChatId("0/0/u1".into()))),
                ("u9", "Eve", None),
                ("u3", "Cat", None)
            ]
        );
        // The avatar of a nameless hit comes from the user cache too.
        assert_eq!(
            hits.people[2].avatar,
            Some(MediaRef {
                id: "av-u3".into(),
                kind: MediaKind::Avatar
            })
        );
        // Nothing but the map opens a private chat: the reply has no private chat hit.
        assert!(hits.chats.is_empty());
    }

    #[test]
    fn search_returns_at_most_limit_hits_per_kind() {
        let items = SearchItems {
            chats: (0..4)
                .map(|i| search_chat(&format!("0/1/{i}"), WireChatKind::Group, Some("t")))
                .collect(),
            users: (0..4)
                .map(|i| search_user(&format!("u{i}"), Some("n")))
                .collect(),
            messages: (0..4)
                .map(|i| search_message(&format!("0/1/{i}"), None, "m"))
                .collect(),
            skipped: 0,
        };
        let hits = search_hits(&items, "me", &HashMap::new(), &KnownChats::default(), 3);
        assert_eq!(
            (hits.chats.len(), hits.people.len(), hits.messages.len()),
            (3, 3, 3)
        );
    }

    #[test]
    fn the_chat_list_gives_private_chats_by_partner_and_titles_by_chat() {
        let chat = |id: &str, kind, partner: Option<&str>| WireChat {
            id: id.into(),
            kind,
            title: None,
            avatar_id: None,
            partner_guid: partner.map(Into::into),
            last_activity: None,
            unread: 0,
            muted: false,
            last_message: None,
            raw: serde_json::Value::Null,
        };
        let users = HashMap::from([user("u1", "Ann")]);
        let chats = [
            chat("0/0/u1", WireChatKind::Private, Some("u1")),
            chat("0/1/g", WireChatKind::Group, None),
            // A private chat whose partner is unknown cannot be found by guid.
            chat("0/0/zz", WireChatKind::Private, None),
        ];
        let summaries: Vec<_> = chats.iter().map(|c| summary(c, "me", &users)).collect();
        let known = KnownChats::of(&summaries);
        assert_eq!(
            known.private,
            HashMap::from([("u1".to_owned(), ChatId("0/0/u1".into()))])
        );
        assert_eq!(known.titles.get("0/0/u1").map(String::as_str), Some("Ann"));
        assert_eq!(
            known.titles.get("0/1/g").map(String::as_str),
            Some("Без названия")
        );
    }

    #[test]
    fn search_errors_keep_their_kind() {
        assert_eq!(
            backend_error(ProtoError::Network("down".into())),
            BackendError::Network("down".into())
        );
        assert_eq!(
            backend_error(ProtoError::Protocol("search: no data".into())),
            BackendError::Protocol("search: no data".into())
        );
        assert_eq!(
            backend_error(ProtoError::SessionExpired),
            BackendError::SessionExpired
        );
    }

    #[tokio::test]
    async fn search_needs_a_connection() {
        let backend = YandexBackend::new(test_session(), None);
        assert!(matches!(
            backend.search("q", 10).await,
            Err(BackendError::Protocol(_))
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn bootstrap_waits_for_a_delayed_handshake() {
        let (tx, mut events) = tokio::sync::mpsc::unbounded_channel();
        tx.send(XivaEvent::Connecting).unwrap();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(5)).await;
            tx.send(XivaEvent::Connected {
                subscription_id: "s".into(),
            })
            .unwrap();
        });
        assert_eq!(
            wait_connected(&mut events, Duration::from_secs(30)).await,
            Ok(())
        );
    }

    #[tokio::test(start_paused = true)]
    async fn bootstrap_fails_on_rejection_and_on_silence() {
        let (tx, mut events) = tokio::sync::mpsc::unbounded_channel();
        tx.send(XivaEvent::SessionExpired).unwrap();
        assert_eq!(
            wait_connected(&mut events, Duration::from_secs(30)).await,
            Err(BackendError::SessionExpired)
        );

        let (_tx, mut events) = tokio::sync::mpsc::unbounded_channel();
        assert!(matches!(
            wait_connected(&mut events, Duration::from_secs(30)).await,
            Err(BackendError::Network(_))
        ));
    }

    fn reactor(user: &str, code: i64, timestamp: i64) -> WireReactor {
        WireReactor {
            user: Some(user.into()),
            code,
            timestamp: Some(timestamp),
        }
    }

    fn wire_reactions(recent: Vec<WireReactor>) -> WireReactions {
        WireReactions {
            counts: vec![
                WireReactionCount {
                    code: 100101,
                    count: 2,
                },
                WireReactionCount { code: 7, count: 1 },
            ],
            version: Some(4),
            recent,
        }
    }

    fn code(value: i64) -> ReactionCode {
        ReactionCode(value)
    }

    fn update_of(event: LiveEvent) -> ReactionsUpdate {
        let LiveEvent::ReactionsChanged {
            chat,
            timestamp,
            update,
        } = event
        else {
            panic!("not a reaction event: {event:?}")
        };
        assert_eq!((chat, timestamp), (ChatId("0/0/a".into()), 10));
        update
    }

    fn pushed(
        actor: Option<&str>,
        action: Option<ReactionAction>,
        code: Option<i64>,
        recent: Vec<WireReactor>,
    ) -> ReactionsUpdate {
        update_of(reactions_changed(
            "0/0/a".into(),
            10,
            actor,
            action,
            code,
            &wire_reactions(recent),
            "me",
        ))
    }

    #[test]
    fn a_history_row_maps_counts_and_version_one_to_one() {
        let mut row = wire(WireBody::Text("hi".into()), Relation::None, "u1");
        row.reactions = Some(wire_reactions(Vec::new()));
        let reactions = message(&row, "me", &HashMap::new()).reactions;
        let counts = [
            ReactionCount {
                code: code(100101),
                count: 2,
            },
            ReactionCount {
                code: code(7),
                count: 1,
            },
        ];
        assert_eq!(
            (
                reactions.counts.as_slice(),
                reactions.version,
                reactions.mine
            ),
            (counts.as_slice(), Some(4), None)
        );
    }

    #[test]
    fn a_row_without_reactions_carries_none() {
        let row = wire(WireBody::Text("hi".into()), Relation::None, "u1");
        assert_eq!(
            message(&row, "me", &HashMap::new()).reactions,
            Reactions::default()
        );
    }

    #[test]
    fn mine_is_the_newest_recent_entry_of_the_account() {
        let mut row = wire(WireBody::Text("hi".into()), Relation::None, "u1");
        row.reactions = Some(wire_reactions(vec![
            reactor("me", 1, 5),
            reactor("u2", 9, 30),
            reactor("me", 2, 20),
            reactor("me", 3, 10),
        ]));
        assert_eq!(
            message(&row, "me", &HashMap::new()).reactions.mine,
            Some(code(2))
        );
        row.reactions = Some(wire_reactions(vec![reactor("u2", 9, 30)]));
        assert_eq!(message(&row, "me", &HashMap::new()).reactions.mine, None);
    }

    #[test]
    fn an_own_replace_names_the_pushed_type() {
        let update = pushed(
            Some("me"),
            Some(ReactionAction::Replace),
            Some(100101),
            Vec::new(),
        );
        assert_eq!(update.mine, Some(Some(code(100101))));
        let counts = [
            ReactionCount {
                code: code(100101),
                count: 2,
            },
            ReactionCount {
                code: code(7),
                count: 1,
            },
        ];
        assert_eq!(
            (update.counts.as_slice(), update.version),
            (counts.as_slice(), Some(4))
        );
    }

    #[test]
    fn an_own_remove_says_the_account_has_none_even_with_stale_recent_entries() {
        let update = pushed(
            Some("me"),
            Some(ReactionAction::Remove),
            None,
            vec![reactor("me", 7, 5)],
        );
        assert_eq!(update.mine, Some(None));
    }

    #[test]
    fn another_actor_with_the_account_in_recent_gives_its_newest_type() {
        let update = pushed(
            Some("u2"),
            Some(ReactionAction::Replace),
            Some(9),
            vec![reactor("me", 7, 5), reactor("me", 100101, 6)],
        );
        assert_eq!(update.mine, Some(Some(code(100101))));
    }

    #[test]
    fn another_actor_without_the_account_in_recent_leaves_mine_unknown() {
        assert_eq!(
            pushed(
                Some("u2"),
                Some(ReactionAction::Replace),
                Some(9),
                vec![reactor("u3", 7, 5)]
            )
            .mine,
            None
        );
        assert_eq!(
            pushed(Some("u2"), Some(ReactionAction::Remove), None, Vec::new()).mine,
            None
        );
        assert_eq!(pushed(None, None, None, Vec::new()).mine, None);
    }

    #[test]
    fn an_own_push_without_a_usable_action_falls_back_to_recent() {
        assert_eq!(
            pushed(
                Some("me"),
                Some(ReactionAction::Replace),
                None,
                vec![reactor("me", 7, 5)]
            )
            .mine,
            Some(Some(code(7)))
        );
        assert_eq!(
            pushed(
                Some("me"),
                Some(ReactionAction::Other(3)),
                Some(7),
                Vec::new()
            )
            .mine,
            None
        );
    }

    #[test]
    fn react_builds_the_set_and_remove_requests() {
        let chat = ChatId("0/0/a".into());
        assert_eq!(
            reaction_request(&chat, 10, Some(code(100101))),
            WriteRequest::Reaction {
                chat: "0/0/a".into(),
                timestamp: 10,
                change: ReactionChange::Set(100101)
            }
        );
        assert_eq!(
            reaction_request(&chat, 10, None),
            WriteRequest::Reaction {
                chat: "0/0/a".into(),
                timestamp: 10,
                change: ReactionChange::Remove
            }
        );
    }

    #[tokio::test]
    async fn a_reaction_to_another_chat_is_blocked_by_the_write_only_chat() {
        let backend = YandexBackend::new(test_session(), Some(ChatId("self".into())));
        assert!(matches!(
            backend
                .react(&ChatId("0/0/other".into()), 1, Some(code(1)))
                .await,
            Err(BackendError::Rejected {
                permanent: true,
                ..
            })
        ));
        assert!(matches!(
            backend.react(&ChatId("0/0/other".into()), 1, None).await,
            Err(BackendError::Rejected {
                permanent: true,
                ..
            })
        ));
        // Unbootstrapped: only a request that passed the guard reaches the connection check.
        assert!(matches!(
            backend
                .react(&ChatId("self".into()), 1, Some(code(1)))
                .await,
            Err(BackendError::Protocol(_))
        ));
    }

    fn wire_member(guid: &str, name: Option<&str>, role: WireMemberRole) -> WireMember {
        WireMember {
            guid: guid.into(),
            display_name: name.map(Into::into),
            avatar_id: None,
            role,
            deleted: false,
        }
    }

    fn members_page(from: usize, count: usize) -> WireMembers {
        let users = (from..from + count)
            .map(|i| wire_member(&format!("u{i}"), Some("n"), WireMemberRole::Member))
            .collect();
        WireMembers { users, skipped: 0 }
    }

    /// Runs `list_members` against scripted pages; returns the result and the offsets asked.
    async fn list_scripted(
        pages: Vec<Result<WireMembers, BackendError>>,
    ) -> (Result<Listed, BackendError>, Vec<Option<String>>) {
        let script = std::cell::RefCell::new(std::collections::VecDeque::from(pages));
        let offsets = std::cell::RefCell::new(Vec::new());
        let listed = list_members(|offset| {
            offsets.borrow_mut().push(offset);
            let reply = script
                .borrow_mut()
                .pop_front()
                .expect("no page left in the script");
            async move { reply }
        })
        .await;
        (listed, offsets.into_inner())
    }

    #[test]
    fn a_member_maps_role_name_avatar_and_the_me_flag() {
        let users = HashMap::from([user("u3", "Cached")]);
        let mut admin = wire_member("u1", Some("Ann"), WireMemberRole::Admin);
        admin.avatar_id = Some("av1".into());
        let ann = member(&admin, "me", &users);
        assert_eq!(
            ann,
            Member {
                user: UserId("u1".into()),
                name: Some("Ann".into()),
                avatar: Some(MediaRef {
                    id: "av1".into(),
                    kind: MediaKind::Avatar
                }),
                role: MemberRole::Admin,
                me: false,
            }
        );
        // Any role but "admin", including one this client does not know, is a plain member.
        for role in [
            WireMemberRole::Member,
            WireMemberRole::Other("subscriber".into()),
        ] {
            assert_eq!(
                member(&wire_member("u2", Some("Bob"), role), "me", &users).role,
                MemberRole::Member
            );
        }
        assert!(
            member(
                &wire_member("me", Some("Me"), WireMemberRole::Member),
                "me",
                &users
            )
            .me
        );
        // A row the member reply left blank takes the cached name and avatar; a blank name counts as none.
        let filled = member(
            &wire_member("u3", Some("  "), WireMemberRole::Member),
            "me",
            &users,
        );
        assert_eq!(filled.name.as_deref(), Some("Cached"));
        assert_eq!(
            filled.avatar,
            Some(MediaRef {
                id: "av-u3".into(),
                kind: MediaKind::Avatar
            })
        );
        assert_eq!(
            member(
                &wire_member("u9", None, WireMemberRole::Member),
                "me",
                &users
            )
            .name,
            None
        );
    }

    #[tokio::test]
    async fn a_short_first_page_is_the_whole_list() {
        let (listed, offsets) = list_scripted(vec![Ok(members_page(0, 49))]).await;
        let listed = listed.unwrap();
        assert_eq!((listed.users.len(), listed.partial), (49, false));
        assert_eq!(offsets, [None]);
    }

    #[tokio::test]
    async fn full_pages_continue_from_the_last_guid_until_a_short_one() {
        let (listed, offsets) = list_scripted(vec![
            Ok(members_page(0, 50)),
            Ok(members_page(50, 50)),
            Ok(members_page(100, 7)),
        ])
        .await;
        let listed = listed.unwrap();
        assert_eq!((listed.users.len(), listed.partial), (107, false));
        assert_eq!(
            offsets,
            [None, Some("u49".to_owned()), Some("u99".to_owned())]
        );
        assert_eq!(listed.users[0].guid, "u0");
        assert_eq!(listed.users[106].guid, "u106");
    }

    #[tokio::test]
    async fn an_empty_page_after_a_full_one_ends_the_list_as_complete() {
        let (listed, offsets) =
            list_scripted(vec![Ok(members_page(0, 50)), Ok(members_page(50, 0))]).await;
        let listed = listed.unwrap();
        assert_eq!((listed.users.len(), listed.partial), (50, false));
        assert_eq!(offsets.len(), 2);
    }

    #[tokio::test]
    async fn the_paging_bound_makes_the_list_partial() {
        let pages = (0..MEMBER_PAGE_BOUND + 5)
            .map(|i| Ok(members_page(i * 50, 50)))
            .collect();
        let (listed, offsets) = list_scripted(pages).await;
        let listed = listed.unwrap();
        assert_eq!(offsets.len(), 20, "at most 20 requests");
        assert_eq!((listed.users.len(), listed.partial), (1000, true));
    }

    #[tokio::test]
    async fn the_last_allowed_page_being_short_is_not_partial() {
        let mut pages: Vec<_> = (0..MEMBER_PAGE_BOUND - 1)
            .map(|i| Ok(members_page(i * 50, 50)))
            .collect();
        pages.push(Ok(members_page(950, 3)));
        let (listed, offsets) = list_scripted(pages).await;
        let listed = listed.unwrap();
        assert_eq!(
            (listed.users.len(), listed.partial, offsets.len()),
            (953, false, 20)
        );
    }

    #[tokio::test]
    async fn a_server_that_ignores_the_offset_does_not_loop_and_the_list_is_partial() {
        let (listed, offsets) =
            list_scripted(vec![Ok(members_page(0, 50)), Ok(members_page(0, 50))]).await;
        let listed = listed.unwrap();
        assert_eq!((listed.users.len(), listed.partial), (50, true));
        assert_eq!(offsets.len(), 2);
    }

    #[tokio::test]
    async fn a_failing_later_page_fails_the_whole_load() {
        let pages = vec![
            Ok(members_page(0, 50)),
            Err(BackendError::Network("down".into())),
        ];
        let (listed, _) = list_scripted(pages).await;
        assert!(matches!(listed, Err(BackendError::Network(_))));
    }

    #[tokio::test]
    async fn users_without_a_guid_count_toward_a_full_page() {
        let mut first = members_page(0, 49);
        first.skipped = 1;
        let (listed, offsets) = list_scripted(vec![Ok(first), Ok(members_page(49, 2))]).await;
        let listed = listed.unwrap();
        assert_eq!(
            (listed.users.len(), listed.skipped, listed.partial),
            (51, 1, false)
        );
        assert_eq!(offsets, [None, Some("u48".to_owned())]);
    }

    #[test]
    fn a_refused_member_list_is_a_permanent_rejection_in_words() {
        let refused = ProtoError::Protocol(
            "get_chat_members failed: no_permissions_for_this_chat_operation".into(),
        );
        assert_eq!(
            members_error(refused),
            BackendError::Rejected {
                reason: MEMBERS_UNAVAILABLE.into(),
                permanent: true
            }
        );
        // Anything else keeps the ordinary mapping.
        assert_eq!(
            members_error(ProtoError::SessionExpired),
            BackendError::SessionExpired
        );
        assert_eq!(
            members_error(ProtoError::Network("down".into())),
            BackendError::Network("down".into())
        );
        assert_eq!(
            members_error(ProtoError::Protocol(
                "get_chat_members failed: bad_request".into()
            )),
            BackendError::Protocol("get_chat_members failed: bad_request".into())
        );
    }

    fn private_row(partner: Option<&str>) -> ChatSummary {
        ChatSummary {
            id: ChatId("0/0/u1".into()),
            kind: ChatKind::Private,
            title: "Ann".into(),
            avatar: Some(MediaRef {
                id: "av".into(),
                kind: MediaKind::Avatar,
            }),
            last_message: None,
            unread: 0,
            muted: false,
            pinned_rank: None,
            partner: partner.map(|p| UserId(p.into())),
        }
    }

    #[test]
    fn a_private_chat_is_one_row_from_the_chat_row() {
        let members = private_members(&private_row(Some("u1")), "me");
        assert_eq!(members.chat, ChatId("0/0/u1".into()));
        assert!(!members.partial);
        assert_eq!(
            members.users,
            [Member {
                user: UserId("u1".into()),
                name: Some("Ann".into()),
                avatar: Some(MediaRef {
                    id: "av".into(),
                    kind: MediaKind::Avatar
                }),
                role: MemberRole::Member,
                me: false,
            }]
        );
        assert!(private_members(&private_row(Some("me")), "me").users[0].me);
        assert!(private_members(&private_row(None), "me").users.is_empty());
    }

    #[test]
    fn the_summary_carries_the_partner_of_a_private_chat_only() {
        let chat = |kind, partner: Option<&str>| WireChat {
            id: "0/0/x".into(),
            kind,
            title: None,
            avatar_id: None,
            partner_guid: partner.map(Into::into),
            last_activity: None,
            unread: 0,
            muted: false,
            last_message: None,
            raw: serde_json::Value::Null,
        };
        let users = HashMap::new();
        assert_eq!(
            summary(&chat(WireChatKind::Private, Some("u1")), "me", &users).partner,
            Some(UserId("u1".into()))
        );
        assert_eq!(
            summary(&chat(WireChatKind::Private, None), "me", &users).partner,
            None
        );
        assert_eq!(
            summary(&chat(WireChatKind::Group, None), "me", &users).partner,
            None
        );
    }

    #[test]
    fn the_profile_takes_name_avatar_and_status_and_falls_back_to_the_session_guid() {
        let wire = WireProfile {
            guid: Some("g".into()),
            name: Some("Me Myself".into()),
            avatar_id: Some("av".into()),
            status_text: Some("busy".into()),
        };
        assert_eq!(
            profile_of(wire, "me"),
            Profile {
                user: UserId("g".into()),
                name: Some("Me Myself".into()),
                avatar: Some(MediaRef {
                    id: "av".into(),
                    kind: MediaKind::Avatar
                }),
                status_text: Some("busy".into()),
            }
        );
        let bare = WireProfile {
            guid: None,
            name: None,
            avatar_id: None,
            status_text: None,
        };
        assert_eq!(
            profile_of(bare, "me"),
            Profile {
                user: UserId("me".into()),
                name: None,
                avatar: None,
                status_text: None
            }
        );
    }

    #[test]
    fn each_manage_error_maps_to_failed_or_unknown() {
        assert_eq!(
            create_error(ManageError::NotSent(ProtoError::Network("no route".into()))),
            CreateError::Failed(BackendError::Network("no route".into()))
        );
        assert_eq!(
            create_error(ManageError::Rejected(ProtoError::Protocol(
                "create_private_chat failed: nope".into()
            ))),
            CreateError::Failed(BackendError::Protocol(
                "create_private_chat failed: nope".into()
            ))
        );
        assert_eq!(
            create_error(ManageError::Rejected(ProtoError::SessionExpired)),
            CreateError::Failed(BackendError::SessionExpired)
        );
        assert_eq!(
            create_error(ManageError::Unknown(ProtoError::Network(
                "timed out".into()
            ))),
            CreateError::Unknown("network error: timed out".into())
        );
    }

    #[test]
    fn write_only_chat_refuses_creation_whatever_the_chat() {
        assert_eq!(creation_guard(None), Ok(()));
        assert!(matches!(
            creation_guard(Some(&ChatId("self".into()))),
            Err(BackendError::Rejected {
                permanent: true,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn a_guarded_creation_never_reaches_the_connection() {
        let user = UserId("u1".into());
        let guarded = YandexBackend::new(test_session(), Some(ChatId("self".into())));
        // Unbootstrapped: anything that got past the guard would fail with "not bootstrapped".
        assert!(matches!(
            guarded.create_private_chat(&user).await,
            Err(CreateError::Failed(BackendError::Rejected {
                permanent: true,
                ..
            }))
        ));
        let open = YandexBackend::new(test_session(), None);
        assert!(matches!(
            open.create_private_chat(&user).await,
            Err(CreateError::Failed(BackendError::Protocol(_)))
        ));
    }

    #[tokio::test]
    async fn members_and_profile_need_a_connection() {
        let backend = YandexBackend::new(test_session(), None);
        assert!(matches!(
            backend.members(&private_row(Some("u1"))).await,
            Err(BackendError::Protocol(_))
        ));
        assert!(matches!(
            backend.profile().await,
            Err(BackendError::Protocol(_))
        ));
    }

    fn wire_entry(guid: &str, name: Option<&str>, code: i64, at: Option<i64>) -> WireReactorEntry {
        WireReactorEntry {
            guid: guid.into(),
            display_name: name.map(Into::into),
            avatar_id: None,
            code,
            timestamp: at,
        }
    }

    fn wire_page(
        entries: Vec<WireReactorEntry>,
        skipped: usize,
        oldest: Option<i64>,
    ) -> WireReactors {
        WireReactors {
            raw: entries.len() + skipped,
            entries,
            skipped,
            oldest,
        }
    }

    fn page_of(wire: WireReactors, users: &HashMap<String, WireUser>) -> ReactorsPage {
        reactors_page(&ChatId("0/0/c".into()), 111, wire, "me", users)
    }

    #[test]
    fn a_reactor_takes_its_name_from_the_entry_then_the_cache_then_none() {
        let users = HashMap::from([user("u2", "Cached"), user("u3", "  ")]);
        let page = page_of(
            wire_page(
                vec![
                    wire_entry("u1", Some("Own"), 1, Some(30)),
                    wire_entry("u2", None, 2, Some(20)),
                    wire_entry("u2", Some("   "), 2, Some(20)),
                    wire_entry("u9", None, 3, None),
                    // A blank name in the cache is no name either.
                    wire_entry("u3", None, 4, Some(10)),
                ],
                0,
                Some(10),
            ),
            &users,
        );
        let names: Vec<_> = page.reactors.iter().map(|r| r.name.as_deref()).collect();
        assert_eq!(
            names,
            [Some("Own"), Some("Cached"), Some("Cached"), None, None]
        );
    }

    #[test]
    fn a_reactor_takes_its_avatar_from_the_entry_then_the_cache() {
        let users = HashMap::from([user("u2", "Cached")]);
        let mut with_avatar = wire_entry("u2", Some("Own"), 1, None);
        with_avatar.avatar_id = Some("entry-av".into());
        let page = page_of(
            wire_page(
                vec![
                    with_avatar,
                    wire_entry("u2", None, 1, None),
                    wire_entry("u9", None, 1, None),
                ],
                0,
                None,
            ),
            &users,
        );
        let ids: Vec<_> = page
            .reactors
            .iter()
            .map(|r| r.avatar.as_ref().map(|a| a.id.as_str()))
            .collect();
        assert_eq!(ids, [Some("entry-av"), Some("av-u2"), None]);
        assert!(
            page.reactors
                .iter()
                .flat_map(|r| &r.avatar)
                .all(|a| a.kind == MediaKind::Avatar)
        );
    }

    #[test]
    fn a_reactor_carries_the_code_the_time_and_the_me_flag() {
        let page = page_of(
            wire_page(
                vec![
                    wire_entry("me", Some("Me"), 100101, Some(77)),
                    wire_entry("u1", Some("Ann"), 7, None),
                ],
                0,
                Some(77),
            ),
            &HashMap::new(),
        );
        assert_eq!(
            page.reactors,
            [
                Reactor {
                    user: UserId("me".into()),
                    name: Some("Me".into()),
                    avatar: None,
                    code: ReactionCode(100101),
                    at: Some(77),
                    me: true,
                },
                Reactor {
                    user: UserId("u1".into()),
                    name: Some("Ann".into()),
                    avatar: None,
                    code: ReactionCode(7),
                    at: None,
                    me: false,
                },
            ]
        );
    }

    #[test]
    fn raw_skipped_and_oldest_pass_through_from_the_wire() {
        // The oldest raw entry was dropped as malformed: its timestamp is still the page's oldest,
        // so the cursor does not move back to a newer entry.
        let wire = WireReactors {
            entries: vec![wire_entry("u1", Some("Ann"), 1, Some(50))],
            skipped: 2,
            raw: 3,
            oldest: Some(20),
        };
        let page = page_of(wire, &HashMap::new());
        assert_eq!(
            (page.reactors.len(), page.skipped, page.raw, page.oldest),
            (1, 2, 3, Some(20))
        );
        assert_eq!(page.chat, ChatId("0/0/c".into()));
        assert_eq!((page.timestamp, page.request), (111, 0));

        let empty = page_of(wire_page(Vec::new(), 0, None), &HashMap::new());
        assert_eq!(
            (empty.reactors.len(), empty.skipped, empty.raw, empty.oldest),
            (0, 0, 0, None)
        );
    }

    #[test]
    fn a_failed_user_lookup_keeps_the_fallbacks_and_an_expired_session_is_an_error() {
        let cache = Mutex::new(HashMap::from([user("known", "K")]));
        assert_eq!(
            uncached_users(
                &cache,
                vec!["b".into(), "known".into(), "a".into(), "b".into()]
            ),
            ["a", "b"]
        );
        for failure in [
            ProtoError::Network("no route".into()),
            ProtoError::Protocol("bad reply".into()),
            ProtoError::NotFound,
        ] {
            assert_eq!(absorb_users(&cache, Err(failure)), Ok(()));
        }
        assert_eq!(cache.lock().unwrap().len(), 1);
        assert_eq!(
            absorb_users(&cache, Err(ProtoError::SessionExpired)),
            Err(BackendError::SessionExpired)
        );
        assert_eq!(cache.lock().unwrap().len(), 1);
        let found = vec![user("a", "A").1];
        assert_eq!(absorb_users(&cache, Ok(found)), Ok(()));
        assert!(cache.lock().unwrap().contains_key("a"));
    }

    /// A socket that answers each request with the next scripted body (echoing its `RequestId`)
    /// and records the method and body it was asked for.
    struct ScriptedSocket {
        incoming: tokio::sync::mpsc::UnboundedReceiver<SocketEvent>,
        echo: tokio::sync::mpsc::UnboundedSender<SocketEvent>,
        sent: Arc<Mutex<Vec<(String, serde_json::Value)>>>,
        replies: std::collections::VecDeque<serde_json::Value>,
    }

    impl Socket for ScriptedSocket {
        async fn send(&mut self, frame: Vec<u8>) -> Result<(), String> {
            let IncomingFrame::Reply {
                req_id,
                method,
                body,
                ..
            } = IncomingFrame::decode(&frame).map_err(|e| e.to_string())?
            else {
                return Err("not a request frame".into());
            };
            let mut reply = self.replies.pop_front().ok_or("no reply scripted")?;
            reply["RequestId"] = body["RequestId"].clone();
            self.sent.lock().unwrap().push((method.clone(), body));
            let bytes = RequestFrame {
                service_index: 0,
                req_id,
                method,
                body: reply,
            }
            .encode();
            self.echo
                .send(SocketEvent::Binary(bytes))
                .map_err(|e| e.to_string())
        }
        async fn recv(&mut self) -> SocketEvent {
            self.incoming.recv().await.unwrap_or(SocketEvent::Closed {
                code: None,
                reason: "eof".into(),
            })
        }
        async fn close(&mut self) {}
    }

    struct ScriptedConnector(Mutex<Option<ScriptedSocket>>);

    impl Connector for ScriptedConnector {
        type Socket = ScriptedSocket;
        async fn connect(&self, _: &Target) -> Result<ScriptedSocket, ConnectError> {
            self.0
                .lock()
                .unwrap()
                .take()
                .ok_or_else(|| ConnectError::Failed("one connection only".into()))
        }
    }

    /// A backend wired to a socket that answers with `replies` in order; `cache` pre-fills the
    /// users cache so no name lookup leaves the process.
    async fn backend_with_replies(
        write_only: Option<ChatId>,
        replies: Vec<serde_json::Value>,
        cache: HashMap<String, WireUser>,
    ) -> (YandexBackend, Arc<Mutex<Vec<(String, serde_json::Value)>>>) {
        let (echo, incoming) = tokio::sync::mpsc::unbounded_channel();
        echo.send(SocketEvent::Text(
            serde_json::json!({ "operation": "subscribed", "subscription-id": "s1" }).to_string(),
        ))
        .unwrap();
        let sent = Arc::new(Mutex::new(Vec::new()));
        let socket = ScriptedSocket {
            incoming,
            echo,
            sent: sent.clone(),
            replies: replies.into(),
        };
        let backend = YandexBackend::new(test_session(), write_only);
        *backend.users.lock().unwrap() = cache;
        let (client, mut events) = Client::start_with(
            ScriptedConnector(Mutex::new(Some(socket))),
            &test_session(),
            ClientConfig::production(),
        )
        .unwrap();
        while !matches!(events.recv().await, Some(XivaEvent::Connected { .. })) {}
        *backend.inner.lock().unwrap() = Some(Connection {
            client,
            events: Some(events),
            me: "me".into(),
        });
        (backend, sent)
    }

    #[tokio::test(start_paused = true)]
    async fn reactors_pass_the_received_limit_and_cursor_to_the_server_and_map_the_page() {
        let reply = serde_json::json!({ "UserReactions": [
            { "Timestamp": 300, "Type": 100101,
              "UserInfo": { "Guid": "me", "DisplayName": "Me", "AvatarId": "av-me" } },
            { "Timestamp": 200, "Type": 7, "UserInfo": { "Guid": "u2" } },
            { "Timestamp": 100, "UserInfo": { "Guid": "u3", "DisplayName": "No type" } },
        ] });
        // BT_WRITE_ONLY_CHAT names another chat: a read is not a write and is not guarded.
        let (backend, sent) = backend_with_replies(
            Some(ChatId("self".into())),
            vec![reply.clone(), reply],
            HashMap::from([user("u2", "Cached")]),
        )
        .await;
        let chat = ChatId("0/0/other".into());

        let page = backend.reactors(&chat, 111, Some(500), 17).await.unwrap();
        assert_eq!(page.chat, chat);
        assert_eq!((page.timestamp, page.request), (111, 0));
        assert_eq!((page.raw, page.skipped, page.oldest), (3, 1, Some(100)));
        let who: Vec<_> = page
            .reactors
            .iter()
            .map(|r| (r.user.0.as_str(), r.name.as_deref(), r.me))
            .collect();
        assert_eq!(
            who,
            [("me", Some("Me"), true), ("u2", Some("Cached"), false)]
        );

        backend.reactors(&chat, 111, None, 5).await.unwrap();
        let sent = sent.lock().unwrap();
        let (method, body) = &sent[0];
        assert_eq!(method, "list_reactions");
        assert_eq!(
            (
                body["ChatId"].as_str(),
                body["Timestamp"].as_i64(),
                body["Limit"].as_u64(),
                body["MaxTimestamp"].as_i64()
            ),
            (Some("0/0/other"), Some(111), Some(17), Some(500))
        );
        let (_, body) = &sent[1];
        assert_eq!(body["Limit"].as_u64(), Some(5));
        assert!(body.get("MaxTimestamp").is_none(), "{body}");
    }

    #[tokio::test]
    async fn reactors_need_a_connection_and_the_write_guard_does_not_apply() {
        let guarded = YandexBackend::new(test_session(), Some(ChatId("self".into())));
        // Unbootstrapped: a guard refusal would be `Rejected`; a read reaches the connection check.
        assert!(matches!(
            guarded
                .reactors(&ChatId("0/0/other".into()), 1, None, 30)
                .await,
            Err(BackendError::Protocol(_))
        ));
    }

    /// The live channel refusal (2026-11-27): `Status` 4, `ErrorInfo.Message`, no `Details`.
    #[tokio::test(start_paused = true)]
    async fn a_refused_reactor_list_carries_the_servers_reason() {
        let refusal = |message: &str| serde_json::json!({ "ErrorInfo": { "Message": message, "WorkerId": 137 }, "Status": 4 });
        let (backend, _) = backend_with_replies(
            None,
            vec![
                refusal("reactions are anonymous"),
                serde_json::json!({ "Status": 4 }),
            ],
            HashMap::new(),
        )
        .await;
        let chat = ChatId("0/0/channel".into());

        assert_eq!(
            backend.reactors(&chat, 111, None, 30).await,
            Err(BackendError::Rejected {
                reason: "Сервер отказал: reactions are anonymous (status 4 ENTITY_NOT_FOUND)"
                    .into(),
                permanent: true
            })
        );
        // Without a reason the ordinary protocol failure stays.
        assert!(matches!(
            backend.reactors(&chat, 111, None, 30).await,
            Err(BackendError::Protocol(m)) if m == "status 4 ENTITY_NOT_FOUND"
        ));
    }

    #[test]
    fn only_a_refusal_with_a_reason_becomes_a_rejection_of_the_list() {
        assert_eq!(
            reactors_error(ProtoError::Protocol(
                "status 4 ENTITY_NOT_FOUND: message not found".into()
            )),
            BackendError::Rejected {
                reason: "Сервер отказал: message not found (status 4 ENTITY_NOT_FOUND)".into(),
                permanent: true
            }
        );
        assert_eq!(
            reactors_error(ProtoError::Protocol("UserReactions is not an array".into())),
            BackendError::Protocol("UserReactions is not an array".into())
        );
        assert_eq!(
            reactors_error(ProtoError::SessionExpired),
            BackendError::SessionExpired
        );
        assert_eq!(
            reactors_error(ProtoError::Network("down".into())),
            BackendError::Network("down".into())
        );
    }
}
