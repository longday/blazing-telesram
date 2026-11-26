//! UI-side state: the single place where `ClientEvent`s land and `Command`s leave.

use async_channel::{Receiver, Sender};
use bt_chat_core::client::REACTORS_PAGE;
use bt_chat_core::model::{
    ChatId, ChatKind, ChatMembers, ChatSummary, ClientEvent, Command, ConnectionState, Delivery,
    DownloadFailure, Draft, HistoryChunk, MediaKind, MediaRef, Message, MessageBody, MessageTs,
    PayloadId, PinnedMessage, Profile, Reactor, ReactorsPage, ReplyTarget, SearchFailure,
    SearchHits, SearchId, UserId,
};
use bt_chat_core::reactions::{ReactionCode, Reactions};
use gpui::{Context, EventEmitter, Task};
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long the search field must stay unchanged before its text is sent to the server.
pub const SEARCH_DEBOUNCE: Duration = Duration::from_millis(300);
/// Shortest query (in `char`s, after `trim()`) that is sent to the server.
pub const SEARCH_MIN_CHARS: usize = 2;
/// How long the open reactor list waits after a change of its message's reactions before it reads
/// again from the first page; a burst of changes inside it causes one read.
pub const REACTORS_REFRESH_DELAY: Duration = Duration::from_secs(1);
/// The failure of an open reactor list whose message was deleted; there is nothing to retry.
pub const MESSAGE_GONE: &str = "Сообщение удалено";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionState {
    Active,
    Expired,
    SignedOut,
}

/// Sidebar filter: one of the kinds or unread.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChatFilter {
    #[default]
    All,
    Private,
    Groups,
    Channels,
    Unread,
}

impl ChatFilter {
    pub fn accepts(self, chat: &ChatSummary) -> bool {
        match self {
            Self::All => true,
            Self::Private => chat.kind == ChatKind::Private,
            Self::Groups => chat.kind == ChatKind::Group,
            Self::Channels => chat.kind == ChatKind::Channel,
            Self::Unread => chat.unread > 0,
        }
    }
}

/// Loaded messages of one chat, oldest first.
#[derive(Debug, Default)]
pub struct Timeline {
    pub messages: Vec<Message>,
    pub loaded: bool,
    pub reached_start: bool,
    pub loading_older: bool,
}

/// A chat member whose typing is shown until `until`.
#[derive(Debug)]
struct Typist {
    user: UserId,
    name: Option<String>,
    until: Instant,
}

/// How a timeline changed; lets a list view splice instead of rebuilding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TimelineChange {
    /// Contents replaced wholesale; list indexes are no longer meaningful.
    Reset,
    /// `prepended` messages added in front, `appended` at the end, and the (post-change) indexes in `touched` replaced in place.
    Delta {
        prepended: usize,
        appended: usize,
        touched: Vec<usize>,
    },
    /// The message at this (pre-removal) index is gone.
    Removed { at: usize },
    /// Only loading flags changed.
    Status,
}

/// Where a download of an original stands, keyed by the original's `MediaRef` (kind `File`).
/// No entry means "not started" (or cancelled).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DownloadState {
    Running {
        received: u64,
        total: Option<u64>,
    },
    /// Saved at this final path.
    Saved(PathBuf),
    Failed(DownloadFailure),
}

/// Where the server search of the sidebar stands. `hits` are those of the last successful reply and
/// stay until the next one replaces them or the search is cancelled, so a new query never blanks the list.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchState {
    pub id: SearchId,
    pub query: String,
    pub status: SearchStatus,
    pub hits: SearchHits,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchStatus {
    Loading,
    Ready,
    Failed(SearchFailure),
}

/// Where a plain read (the members of a chat, the own profile) stands. There is no entry before the
/// first request; a request always starts at `Loading`, so a list or profile from an earlier request
/// is never shown as current. `offline` is set when the core could not ask at all.
#[derive(Clone, Debug, PartialEq)]
pub enum LoadState<T> {
    Loading,
    Ready(T),
    Failed { reason: String, offline: bool },
}

pub type MembersState = LoadState<ChatMembers>;
pub type ProfileState = LoadState<Profile>;

/// Which request the open reactor list's outstanding `LoadReactors` is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestKind {
    /// The first page: its answer replaces the rows (open, retry, refresh).
    First,
    /// The page after the loaded rows: its answer is appended.
    More,
}

/// The one open list of the people who reacted to a message, held in memory only and dropped on close.
/// `generation` is the token of the request the list last sent; an answer is applied only when it
/// carries it, so an answer to a closed list, an earlier opening of the same message or a superseded
/// page is dropped. `base_version` is the message's reaction version when the first page was last
/// asked for.
#[derive(Clone, Debug, PartialEq)]
pub struct ReactorsView {
    pub chat: ChatId,
    pub timestamp: MessageTs,
    pub base_version: Option<i64>,
    pub generation: u64,
    pub kind: RequestKind,
    pub state: ReactorsState,
}

/// Where the open reactor list stands. `exhausted` comes from the server's pages (see
/// `page_exhausted`), never from the message's known counts.
#[derive(Clone, Debug, PartialEq)]
pub enum ReactorsState {
    Loading,
    Ready {
        rows: Vec<Reactor>,
        /// Entries without an identity, summed over the loaded pages.
        skipped: usize,
        /// The smallest timestamp seen; the next page is asked with it as `before`.
        oldest: Option<MessageTs>,
        exhausted: bool,
        /// A request that gates the next page is out: that page itself, or a read of the first page
        /// (refresh, retry) that will replace the rows. Nothing else is asked meanwhile.
        loading_more: bool,
        more_failed: Option<String>,
    },
    Failed {
        reason: String,
        offline: bool,
    },
}

/// The debounce of the open list's refresh as pure state, so tests drive it without a clock. Each
/// `schedule` hands out a ticket and disarms every earlier one; the timer holding the latest ticket
/// fires once, unless the schedule was cancelled meanwhile.
#[derive(Debug, Default)]
struct RefreshSchedule {
    last: u64,
    armed: Option<u64>,
}

impl RefreshSchedule {
    fn schedule(&mut self) -> u64 {
        self.last += 1;
        self.armed = Some(self.last);
        self.last
    }

    fn cancel(&mut self) {
        self.armed = None;
    }

    /// Whether the timer with `ticket` is the armed one; it disarms the schedule.
    fn fire(&mut self, ticket: u64) -> bool {
        if self.armed == Some(ticket) {
            self.armed = None;
            true
        } else {
            false
        }
    }
}

/// Where the one chat creation the store lets run stands. There is no state while none was started or
/// the last one ended in a chat; the dialog that started it keeps its own data, so a failure leaves
/// everything for another attempt.
#[derive(Clone, Debug, PartialEq)]
pub enum CreationState {
    /// A request is out (or the core is answering from the list); a second one is not sent.
    InFlight,
    /// Nothing was created (`unknown` false) or the result is not known and the chat may exist
    /// (`unknown` true); `reason` is ready to show. The core refreshed the list when `unknown` and it
    /// did not hold the chat.
    Failed { reason: String, unknown: bool },
    /// The chat may exist and the core could not read the chat list to tell (after an unknown result,
    /// or in the check before a new attempt). Another attempt is allowed: the core reads the list first.
    Unverified { reason: String },
}

/// A created chat the store selects as soon as the chat list holds it.
#[derive(Clone, Debug, PartialEq)]
enum PendingOpen {
    /// The chat the creation reply or the refreshed list named.
    Chat(ChatId),
    /// Nothing named it: the private chat with this person, whenever the list shows one.
    Partner(UserId),
}

#[derive(Clone, Debug, PartialEq)]
pub enum StoreEvent {
    Chats,
    Timeline {
        chat: ChatId,
        change: TimelineChange,
    },
    Media(MediaRef),
    /// The download state of this original changed (or was cleared).
    Download(MediaRef),
    Connection,
    Session,
    /// The persisted drafts arrived.
    Drafts,
    /// An edit or deletion did not happen.
    ActionFailed {
        chat: ChatId,
        reason: String,
    },
    /// A send was not queued; the UI puts the text and reply target back.
    SendRefused {
        draft: Draft,
        reason: String,
    },
    /// The server search state changed (issued, answered, failed or cancelled).
    Search,
    /// The member list state of this chat changed (requested, loaded or failed).
    Members(ChatId),
    /// The profile state changed (requested, loaded or failed).
    Profile,
    /// The open reactor list changed (opened, answered, failed, requested again or closed).
    Reactors,
    /// The creation state changed (started, failed, ended or dismissed).
    Creation,
    /// A creation ended in a chat: `existing` when none had to be created, `chat` is `None` when it
    /// cannot be opened (the reply and the refreshed list do not name it). The chat is selected as
    /// soon as it is in the list (at once when the list already holds it).
    ChatCreated {
        chat: Option<ChatId>,
        existing: bool,
    },
}

pub struct ChatStore {
    commands: Sender<Command>,
    chats: HashMap<ChatId, ChatSummary>,
    chats_loaded: bool,
    selected: Option<ChatId>,
    timelines: HashMap<ChatId, Timeline>,
    media: HashMap<MediaRef, PathBuf>,
    media_requested: HashSet<MediaRef>,
    downloads: HashMap<MediaRef, DownloadState>,
    connection: ConnectionState,
    session: SessionState,
    drafts: HashMap<ChatId, Draft>,
    typing: HashMap<ChatId, Vec<Typist>>,
    search: Option<SearchState>,
    /// The id of the last issued search; never reused, also after `cancel_search`.
    last_search: SearchId,
    /// Member lists by chat, held in memory only.
    members: HashMap<ChatId, MembersState>,
    profile: Option<ProfileState>,
    creation: Option<CreationState>,
    /// The person of the creation in flight.
    creating: Option<UserId>,
    /// A created chat to select once the list holds it.
    pending_open: Option<PendingOpen>,
    /// The open reactor list.
    reactors: Option<ReactorsView>,
    /// The last `LoadReactors` token handed out; only grows, so it is never reset by close, reopen
    /// or sign-out.
    next_request: u64,
    /// When the open list is read again after a change of its message's reactions.
    refresh: RefreshSchedule,
    /// The timer of `refresh`; replacing or dropping it cancels the wait.
    refresh_timer: Option<Task<()>>,
}

impl EventEmitter<StoreEvent> for ChatStore {}

impl ChatStore {
    /// Drains `events` on the foreground executor until the channel closes or the store is dropped.
    pub fn new(
        events: Receiver<ClientEvent>,
        commands: Sender<Command>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.spawn(async move |this, cx| {
            while let Ok(event) = events.recv().await {
                if this.update(cx, |store, cx| store.apply(event, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        Self {
            commands,
            chats: HashMap::new(),
            chats_loaded: false,
            selected: None,
            timelines: HashMap::new(),
            media: HashMap::new(),
            media_requested: HashSet::new(),
            downloads: HashMap::new(),
            connection: ConnectionState::Connecting,
            session: SessionState::Active,
            drafts: HashMap::new(),
            typing: HashMap::new(),
            search: None,
            last_search: SearchId(0),
            members: HashMap::new(),
            profile: None,
            creation: None,
            creating: None,
            pending_open: None,
            reactors: None,
            next_request: 0,
            refresh: RefreshSchedule::default(),
            refresh_timer: None,
        }
    }

    pub fn apply(&mut self, event: ClientEvent, cx: &mut Context<Self>) {
        match event {
            ClientEvent::ChatsReplaced(chats) => {
                for chat in &chats {
                    self.want_media(chat.avatar.as_ref());
                }
                self.chats = chats.into_iter().map(|c| (c.id.clone(), c)).collect();
                self.chats_loaded = true;
                if self
                    .selected
                    .as_ref()
                    .is_some_and(|id| !self.chats.contains_key(id))
                {
                    self.selected = None;
                }
                cx.emit(StoreEvent::Chats);
                self.open_pending(cx);
            }
            ClientEvent::ChatUpdated(chat) => {
                self.want_media(chat.avatar.as_ref());
                self.chats.insert(chat.id.clone(), chat);
                cx.emit(StoreEvent::Chats);
                self.open_pending(cx);
            }
            ClientEvent::History(chunk) => self.apply_history(chunk, cx),
            ClientEvent::PinChanged { chat, pinned } => {
                // An equal value, or a chat that is not shown, is no UI update.
                if !self.set_pinned(&chat, pinned) {
                    return;
                }
            }
            ClientEvent::MessageUpserted(message) => self.upsert(message, cx),
            ClientEvent::ReactionsChanged {
                chat,
                timestamp,
                reactions,
            } => {
                let version = reactions.version;
                self.set_reactions(&chat, timestamp, reactions, cx);
                self.reactions_changed(&chat, timestamp, version, cx);
            }
            ClientEvent::MessageDeleted { chat, timestamp } => {
                self.delete(&chat, timestamp, cx);
                self.reactors_gone(&chat, timestamp, cx);
            }
            ClientEvent::MediaReady { media, path } => {
                self.media.insert(media.clone(), path);
                cx.emit(StoreEvent::Media(media));
            }
            // Re-requested when the chat or message that shows it is applied again.
            ClientEvent::MediaUnavailable(media) => {
                self.media_requested.remove(&media);
            }
            ClientEvent::Connection(state) => {
                self.connection = state;
                cx.emit(StoreEvent::Connection);
            }
            ClientEvent::SessionExpired => {
                self.session = SessionState::Expired;
                self.creation = None;
                self.creating = None;
                self.pending_open = None;
                cx.emit(StoreEvent::Session);
            }
            ClientEvent::SignedOut => {
                self.chats.clear();
                self.chats_loaded = false;
                self.selected = None;
                self.timelines.clear();
                self.media.clear();
                self.media_requested.clear();
                self.downloads.clear();
                self.search = None;
                self.members.clear();
                self.profile = None;
                self.reactors = None;
                self.cancel_refresh();
                self.creation = None;
                self.creating = None;
                self.pending_open = None;
                self.drafts.clear();
                self.typing.clear();
                self.session = SessionState::SignedOut;
                cx.emit(StoreEvent::Session);
            }
            ClientEvent::Drafts(drafts) => {
                self.drafts = drafts.into_iter().map(|d| (d.chat.clone(), d)).collect();
                cx.emit(StoreEvent::Drafts);
            }
            ClientEvent::PendingSettled { chat, payload } => self.settle(&chat, &payload, cx),
            ClientEvent::SendRefused { draft, reason } => {
                cx.emit(StoreEvent::SendRefused { draft, reason })
            }
            ClientEvent::Typing {
                chat,
                user,
                name,
                until,
            } => self.typing_started(chat, user, name, until, cx),
            ClientEvent::ActionFailed { chat, reason } => {
                cx.emit(StoreEvent::ActionFailed { chat, reason })
            }
            ClientEvent::DownloadProgress {
                media,
                received,
                total,
            } => {
                self.set_download(media, Some(DownloadState::Running { received, total }), cx);
            }
            ClientEvent::Downloaded { media, path } => {
                self.set_download(media, Some(DownloadState::Saved(path)), cx)
            }
            ClientEvent::DownloadFailed { media, reason } => {
                self.set_download(media, Some(DownloadState::Failed(reason)), cx);
            }
            ClientEvent::DownloadCancelled(media) => self.set_download(media, None, cx),
            ClientEvent::SearchResults { id, hits } => {
                self.finish_search(id, Some(hits), SearchStatus::Ready, cx)
            }
            ClientEvent::SearchFailed { id, reason } => {
                self.finish_search(id, None, SearchStatus::Failed(reason), cx);
            }
            ClientEvent::Members(members) => {
                self.finish_members(members.chat.clone(), MembersState::Ready(members), cx)
            }
            ClientEvent::MembersFailed {
                chat,
                reason,
                offline,
            } => {
                self.finish_members(chat, MembersState::Failed { reason, offline }, cx);
            }
            ClientEvent::Profile(profile) => self.finish_profile(ProfileState::Ready(profile), cx),
            ClientEvent::ProfileFailed { reason, offline } => {
                self.finish_profile(ProfileState::Failed { reason, offline }, cx);
            }
            ClientEvent::Reactors(page) => self.finish_reactors(page, cx),
            ClientEvent::ReactorsFailed {
                chat,
                timestamp,
                request,
                reason,
                offline,
            } => self.fail_reactors(&chat, timestamp, request, reason, offline, cx),
            ClientEvent::ChatCreated { chat, existing } => self.chat_created(chat, existing, cx),
            ClientEvent::CreationFailed { reason, unknown } => {
                self.creation_failed(CreationState::Failed { reason, unknown }, cx);
            }
            ClientEvent::CreationUnverified { reason } => {
                self.creation_failed(CreationState::Unverified { reason }, cx)
            }
        }
        cx.notify();
    }

    /// Stores the pin of a listed chat. Returns whether the open chat's bar has to be drawn
    /// again: the value differs and the chat is the open one. Touches nothing but the summary's
    /// `pinned_message`: no timeline, row preview or order, unread counter or typing state.
    fn set_pinned(&mut self, chat: &ChatId, pinned: Option<PinnedMessage>) -> bool {
        let Some(summary) = self.chats.get_mut(chat) else {
            return false;
        };
        if summary.pinned_message == pinned {
            return false;
        }
        summary.pinned_message = pinned;
        self.selected.as_ref() == Some(chat)
    }

    /// Opens a chat: the core answers with its newest chunk. Never marks anything as read.
    pub fn select(&mut self, chat: ChatId, cx: &mut Context<Self>) {
        // Any choice of the user ends the wait for a created chat.
        self.pending_open = None;
        if self.selected.as_ref() == Some(&chat) {
            return;
        }
        if let Some(timeline) = self.timelines.get_mut(&chat) {
            timeline.loading_older = false;
        }
        self.selected = Some(chat.clone());
        self.send(Command::OpenChat(chat));
        cx.notify();
    }

    /// Requests the chunk before the oldest loaded message, once, unless the start of history is known.
    pub fn load_older(&mut self, chat: &ChatId, cx: &mut Context<Self>) {
        let Some(timeline) = self.timelines.get_mut(chat) else {
            return;
        };
        if !timeline.loaded || timeline.reached_start || timeline.loading_older {
            return;
        }
        let Some(before) = timeline.messages.first().map(|m| m.timestamp) else {
            return;
        };
        timeline.loading_older = true;
        self.send(Command::LoadOlder {
            chat: chat.clone(),
            before,
        });
        cx.emit(StoreEvent::Timeline {
            chat: chat.clone(),
            change: TimelineChange::Status,
        });
        cx.notify();
    }

    pub fn sign_out(&mut self) {
        self.send(Command::SignOut);
    }

    /// Queues a message; the core answers with the pending copy. Sending replaces the chat's draft.
    pub fn send_text(
        &mut self,
        chat: ChatId,
        text: String,
        reply_to: Option<ReplyTarget>,
        cx: &mut Context<Self>,
    ) {
        self.drafts.remove(&chat);
        self.send(Command::Send {
            chat,
            text,
            reply_to,
        });
        cx.notify();
    }

    pub fn retry(&self, payload: PayloadId) {
        self.send(Command::Retry(payload));
    }

    pub fn edit(&self, chat: ChatId, timestamp: MessageTs, text: String) {
        self.send(Command::Edit {
            chat,
            timestamp,
            text,
        });
    }

    pub fn delete_message(&self, chat: ChatId, timestamp: MessageTs) {
        self.send(Command::Delete { chat, timestamp });
    }

    /// The user chose `code` on a message: the core decides between add, replace and remove.
    pub fn toggle_reaction(&self, chat: ChatId, timestamp: MessageTs, code: ReactionCode) {
        self.send(Command::ToggleReaction {
            chat,
            timestamp,
            code,
        });
    }

    pub fn mark_read(&self, chat: ChatId, timestamp: MessageTs) {
        self.send(Command::MarkRead { chat, timestamp });
    }

    /// The user changed the composer text of `chat`.
    pub fn typing(&self, chat: ChatId) {
        self.send(Command::Typing(chat));
    }

    pub fn draft(&self, chat: &ChatId) -> Option<&Draft> {
        self.drafts.get(chat)
    }

    /// Remembers the composer state of a chat and asks the core to persist it; nothing happens when
    /// it equals what is already stored. An empty text with no reply removes the draft.
    pub fn save_draft(&mut self, draft: Draft, cx: &mut Context<Self>) {
        if draft.text.is_empty() && draft.reply_to.is_none() {
            if self.drafts.remove(&draft.chat).is_none() {
                return;
            }
        } else if self.drafts.get(&draft.chat) == Some(&draft) {
            return;
        } else {
            self.drafts.insert(draft.chat.clone(), draft.clone());
        }
        self.send(Command::SaveDraft(draft));
        cx.notify();
    }

    /// Who is typing in `chat`, as shown in the header and the chat row.
    pub fn typing_text(&self, chat: &ChatId) -> Option<String> {
        let typists = self.typing.get(chat).filter(|t| !t.is_empty())?;
        if self
            .chats
            .get(chat)
            .is_some_and(|c| c.kind == ChatKind::Private)
        {
            return Some("печатает…".into());
        }
        let first = typists[0].name.as_deref().unwrap_or("Кто-то");
        Some(match typists.len() {
            1 => format!("{first} печатает…"),
            n => format!("{first} и ещё {} печатают…", n - 1),
        })
    }

    fn typing_started(
        &mut self,
        chat: ChatId,
        user: UserId,
        name: Option<String>,
        until: Instant,
        cx: &mut Context<Self>,
    ) {
        let now = cx.background_executor().now();
        if until <= now {
            return;
        }
        let typists = self.typing.entry(chat).or_default();
        match typists.iter_mut().find(|t| t.user == user) {
            Some(typist) => {
                typist.name = name.or(typist.name.take());
                typist.until = until;
            }
            None => typists.push(Typist { user, name, until }),
        }
        let wait = until - now;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            this.update(cx, |store, cx| store.expire_typing(cx)).ok();
        })
        .detach();
    }

    fn expire_typing(&mut self, cx: &mut Context<Self>) {
        let now = cx.background_executor().now();
        for typists in self.typing.values_mut() {
            typists.retain(|t| t.until > now);
        }
        self.typing.retain(|_, typists| !typists.is_empty());
        cx.notify();
    }

    /// A message by `user` ends their typing.
    fn typing_stopped(&mut self, chat: &ChatId, user: &UserId) {
        if let Some(typists) = self.typing.get_mut(chat) {
            typists.retain(|t| &t.user != user);
            if typists.is_empty() {
                self.typing.remove(chat);
            }
        }
    }

    /// Drops the local copy of a send the server has confirmed or echoed; its server copy follows.
    fn settle(&mut self, chat: &ChatId, payload: &PayloadId, cx: &mut Context<Self>) {
        let Some(timeline) = self.timelines.get_mut(chat) else {
            return;
        };
        let pending = |m: &Message| matches!(&m.delivery, Delivery::Pending(p) | Delivery::Failed { payload: p, .. } if p == payload);
        let Some(at) = timeline.messages.iter().position(pending) else {
            return;
        };
        timeline.messages.remove(at);
        cx.emit(StoreEvent::Timeline {
            chat: chat.clone(),
            change: TimelineChange::Removed { at },
        });
    }

    pub fn chat(&self, id: &ChatId) -> Option<&ChatSummary> {
        self.chats.get(id)
    }

    pub fn chats_loaded(&self) -> bool {
        self.chats_loaded
    }

    pub fn chat_count(&self) -> usize {
        self.chats.len()
    }

    pub fn selected(&self) -> Option<&ChatId> {
        self.selected.as_ref()
    }

    pub fn timeline(&self, id: &ChatId) -> Option<&Timeline> {
        self.timelines.get(id)
    }

    pub fn media_path(&self, media: &MediaRef) -> Option<&Path> {
        self.media.get(media).map(PathBuf::as_path)
    }

    /// Starts saving the original `original` (kind `File`) under `name`. The card shows `Running` at once;
    /// when the connection is not online nothing is sent and the state is `Failed(Offline)`. A download
    /// already running for the same file is not started twice.
    pub fn download(&mut self, original: MediaRef, name: Option<String>, cx: &mut Context<Self>) {
        if matches!(
            self.downloads.get(&original),
            Some(DownloadState::Running { .. })
        ) {
            return;
        }
        if self.connection != ConnectionState::Online {
            self.set_download(
                original,
                Some(DownloadState::Failed(DownloadFailure::Offline)),
                cx,
            );
            cx.notify();
            return;
        }
        self.set_download(
            original.clone(),
            Some(DownloadState::Running {
                received: 0,
                total: None,
            }),
            cx,
        );
        self.send(Command::Download {
            media: original,
            name,
        });
        cx.notify();
    }

    /// Asks the core to stop a running download; the state clears when `DownloadCancelled` arrives.
    pub fn cancel_download(&self, original: &MediaRef) {
        if matches!(
            self.downloads.get(original),
            Some(DownloadState::Running { .. })
        ) {
            self.send(Command::CancelDownload(original.clone()));
        }
    }

    pub fn download_state(&self, original: &MediaRef) -> Option<&DownloadState> {
        self.downloads.get(original)
    }

    /// Asks the server for `query`: the previous hits stay while the state is `Loading`. The UI applies the
    /// debounce, the minimum length and the offline rule before calling this.
    pub fn search(&mut self, query: String, cx: &mut Context<Self>) {
        self.last_search = SearchId(self.last_search.0 + 1);
        let id = self.last_search;
        let hits = self.search.take().map(|s| s.hits).unwrap_or_default();
        self.search = Some(SearchState {
            id,
            query: query.clone(),
            status: SearchStatus::Loading,
            hits,
        });
        self.send(Command::Search { id, query });
        cx.emit(StoreEvent::Search);
        cx.notify();
    }

    /// Forgets the query and its hits; a reply still on its way is dropped here and in the core.
    /// Nothing is sent when no search is active (the core has nothing to drop either).
    pub fn cancel_search(&mut self, cx: &mut Context<Self>) {
        if self.search.take().is_none() {
            return;
        }
        self.send(Command::CancelSearch);
        cx.emit(StoreEvent::Search);
        cx.notify();
    }

    /// The query text changed to `next` but the request for it is not sent yet. A reply still on its way
    /// belongs to the old text, so it is dropped on arrival instead of being shown (stale replies never
    /// change the view); the hits already shown stay until the new reply. The state gets the id 0, which
    /// is never issued, and no query, so the next `search` is not mistaken for a repeat.
    pub fn supersede_search(&mut self, next: &str) {
        if let Some(state) = self
            .search
            .as_mut()
            .filter(|s| s.status == SearchStatus::Loading && s.query != next)
        {
            state.id = SearchId(0);
            state.query.clear();
        }
    }

    pub fn search_state(&self) -> Option<&SearchState> {
        self.search.as_ref()
    }

    /// Stores the outcome of search `id`; an outcome of any other id (superseded or cancelled) is ignored.
    /// A failure keeps the hits already shown.
    fn finish_search(
        &mut self,
        id: SearchId,
        hits: Option<SearchHits>,
        status: SearchStatus,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.search.as_mut().filter(|s| s.id == id) else {
            return;
        };
        state.status = status;
        if let Some(hits) = hits {
            let avatars: Vec<MediaRef> = (hits.chats.iter().filter_map(|h| h.avatar.clone()))
                .chain(hits.people.iter().filter_map(|h| h.avatar.clone()))
                .collect();
            state.hits = hits;
            for avatar in &avatars {
                self.want_media(Some(avatar));
            }
        }
        cx.emit(StoreEvent::Search);
    }

    /// Asks the core for the people of `chat`. The state becomes `Loading` at once, so a list from an
    /// earlier request is never shown as current; while a request is already running nothing is sent
    /// again (its answer is on the way). Also the retry of a `Failed` state.
    pub fn load_members(&mut self, chat: ChatId, cx: &mut Context<Self>) {
        if self.members.get(&chat) == Some(&MembersState::Loading) {
            return;
        }
        self.members.insert(chat.clone(), MembersState::Loading);
        self.send(Command::LoadMembers(chat.clone()));
        cx.emit(StoreEvent::Members(chat));
        cx.notify();
    }

    pub fn members_state(&self, chat: &ChatId) -> Option<&MembersState> {
        self.members.get(chat)
    }

    /// Stores the outcome of a member load; an outcome for a chat that is not loading is ignored.
    fn finish_members(&mut self, chat: ChatId, outcome: MembersState, cx: &mut Context<Self>) {
        if self.members.get(&chat) != Some(&MembersState::Loading) {
            return;
        }
        if let MembersState::Ready(members) = &outcome {
            for avatar in members.users.iter().filter_map(|m| m.avatar.as_ref()) {
                self.want_media(Some(avatar));
            }
        }
        self.members.insert(chat.clone(), outcome);
        cx.emit(StoreEvent::Members(chat));
    }

    /// Asks the core for the account's own profile, with the same rules as `load_members`.
    pub fn load_profile(&mut self, cx: &mut Context<Self>) {
        if self.profile == Some(ProfileState::Loading) {
            return;
        }
        self.profile = Some(ProfileState::Loading);
        self.send(Command::LoadProfile);
        cx.emit(StoreEvent::Profile);
        cx.notify();
    }

    pub fn profile_state(&self) -> Option<&ProfileState> {
        self.profile.as_ref()
    }

    fn finish_profile(&mut self, outcome: ProfileState, cx: &mut Context<Self>) {
        if self.profile != Some(ProfileState::Loading) {
            return;
        }
        if let ProfileState::Ready(profile) = &outcome {
            self.want_media(profile.avatar.as_ref());
        }
        self.profile = Some(outcome);
        cx.emit(StoreEvent::Profile);
        cx.notify();
    }

    /// Opens the list of the people who reacted to `(chat, timestamp)` and asks for its first page.
    /// It replaces any list already open; a request still out for that one, or for an earlier opening
    /// of this message, is superseded by the new token.
    pub fn open_reactors(&mut self, chat: ChatId, timestamp: MessageTs, cx: &mut Context<Self>) {
        self.cancel_refresh();
        self.reactors = Some(ReactorsView {
            chat,
            timestamp,
            base_version: None,
            generation: 0,
            kind: RequestKind::First,
            state: ReactorsState::Loading,
        });
        self.request_reactors(RequestKind::First, cx);
    }

    /// Closes the list and forgets it; answers still on the way are dropped when they arrive.
    pub fn close_reactors(&mut self, cx: &mut Context<Self>) {
        self.cancel_refresh();
        if self.reactors.take().is_some() {
            cx.emit(StoreEvent::Reactors);
            cx.notify();
        }
    }

    pub fn reactors(&self) -> Option<&ReactorsView> {
        self.reactors.as_ref()
    }

    /// Tries again after a failure: a failed list goes back to `Loading` and asks for the first page;
    /// a shown list whose next page failed asks for that page while the connection is online; a
    /// shown list otherwise (the offline retry, which the view offers for a list it shows as
    /// «Нет соединения») asks for the first page again, so rows from before the outage are re-read,
    /// and keeps them until the answer. A list that is loading already has a request out and sends
    /// nothing.
    pub fn retry_reactors(&mut self, cx: &mut Context<Self>) {
        let online = self.connection == ConnectionState::Online;
        let Some(view) = self.reactors.as_mut() else {
            return;
        };
        let kind = match &view.state {
            ReactorsState::Loading => return,
            ReactorsState::Failed { .. } => RequestKind::First,
            ReactorsState::Ready {
                more_failed: Some(_),
                ..
            } if online => RequestKind::More,
            ReactorsState::Ready { .. } => RequestKind::First,
        };
        if matches!(view.state, ReactorsState::Failed { .. }) {
            view.state = ReactorsState::Loading;
        }
        self.request_reactors(kind, cx);
    }

    /// Asks for the page after the loaded rows («Показать ещё»): `LoadReactors { before: oldest }`
    /// with a fresh token, and the list shows `loading_more` until the answer. Only a shown list that
    /// is not exhausted and has no page out sends it; for anything else, including a second choice
    /// while the page is out, nothing happens, so one choice is one request.
    pub fn load_more_reactors(&mut self, cx: &mut Context<Self>) {
        self.request_reactors(RequestKind::More, cx);
    }

    /// Sends a `LoadReactors` for the open list with a fresh token, which supersedes any request
    /// still out. `First` records the message's current reaction version as `base_version`, takes
    /// over a refresh still waiting for its delay (this read is that refresh) and leaves the rows as
    /// they are (the caller sets `Loading` when the rows must go); on a shown list it marks
    /// `loading_more`, so that no next page is asked from the old rows while this read is out. `More`
    /// is sent only for a shown list that is not exhausted and has no request out, and marks it
    /// `loading_more`.
    fn request_reactors(&mut self, kind: RequestKind, cx: &mut Context<Self>) {
        let Some(view) = self.reactors.as_mut() else {
            return;
        };
        let before = match kind {
            RequestKind::First => {
                view.base_version = reactions_version(&self.timelines, &view.chat, view.timestamp);
                self.refresh.cancel();
                if let ReactorsState::Ready {
                    loading_more,
                    more_failed,
                    ..
                } = &mut view.state
                {
                    *loading_more = true;
                    *more_failed = None;
                }
                None
            }
            RequestKind::More => match &mut view.state {
                ReactorsState::Ready {
                    oldest: Some(oldest),
                    exhausted: false,
                    loading_more,
                    more_failed,
                    ..
                } if !*loading_more => {
                    *loading_more = true;
                    *more_failed = None;
                    Some(*oldest)
                }
                _ => return,
            },
        };
        self.next_request += 1;
        view.generation = self.next_request;
        view.kind = kind;
        let command = Command::LoadReactors {
            chat: view.chat.clone(),
            timestamp: view.timestamp,
            before,
            request: view.generation,
        };
        self.send(command);
        cx.emit(StoreEvent::Reactors);
        cx.notify();
    }

    /// The open list, if `(chat, timestamp, request)` names the request it is waiting for.
    fn awaited_reactors(
        &mut self,
        chat: &ChatId,
        timestamp: MessageTs,
        request: u64,
    ) -> Option<&mut ReactorsView> {
        self.reactors
            .as_mut()
            .filter(|v| v.chat == *chat && v.timestamp == timestamp && v.generation == request)
    }

    /// Applies a page: `First` replaces the rows, `More` appends them, both in server order and
    /// without a guid twice. An answer to anything but the awaited request is dropped.
    fn finish_reactors(&mut self, page: ReactorsPage, cx: &mut Context<Self>) {
        let Some(view) = self.awaited_reactors(&page.chat, page.timestamp, page.request) else {
            return;
        };
        let avatars: Vec<MediaRef> = page
            .reactors
            .iter()
            .filter_map(|r| r.avatar.clone())
            .collect();
        match (view.kind, &mut view.state) {
            (
                RequestKind::More,
                ReactorsState::Ready {
                    rows,
                    skipped,
                    oldest,
                    exhausted,
                    loading_more,
                    more_failed,
                },
            ) => {
                *exhausted = page_exhausted(*oldest, page.raw, page.oldest, REACTORS_PAGE);
                append_new(rows, page.reactors);
                *skipped = skipped.saturating_add(page.skipped);
                *oldest = match (page.oldest, *oldest) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (a, b) => a.or(b),
                };
                *loading_more = false;
                *more_failed = None;
            }
            // A next page can only be asked of a shown list; nothing else awaits one.
            (RequestKind::More, _) => return,
            (RequestKind::First, state) => {
                let mut rows = Vec::with_capacity(page.reactors.len());
                append_new(&mut rows, page.reactors);
                *state = ReactorsState::Ready {
                    rows,
                    skipped: page.skipped,
                    oldest: page.oldest,
                    exhausted: page_exhausted(None, page.raw, page.oldest, REACTORS_PAGE),
                    loading_more: false,
                    more_failed: None,
                };
            }
        }
        for avatar in &avatars {
            self.want_media(Some(avatar));
        }
        cx.emit(StoreEvent::Reactors);
    }

    /// Applies a failure of the awaited request: a failed next page keeps the rows and offers a
    /// retry, anything else leaves a `Failed` list (an earlier list must not look current).
    fn fail_reactors(
        &mut self,
        chat: &ChatId,
        timestamp: MessageTs,
        request: u64,
        reason: String,
        offline: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.awaited_reactors(chat, timestamp, request) else {
            return;
        };
        match (view.kind, &mut view.state) {
            (
                RequestKind::More,
                ReactorsState::Ready {
                    loading_more,
                    more_failed,
                    ..
                },
            ) => {
                *loading_more = false;
                *more_failed = Some(reason);
            }
            (_, state) => *state = ReactorsState::Failed { reason, offline },
        }
        cx.emit(StoreEvent::Reactors);
    }

    /// A change of the reactions of `(chat, ts)`, now at `version`, reached the store. When it is the
    /// open list's message, the message is not gone, and the version is not the one the list was
    /// read at, the read from the first page is (re)scheduled after `REACTORS_REFRESH_DELAY`,
    /// whatever the list shows meanwhile (a failure of an older read does not make it current).
    fn reactions_changed(
        &mut self,
        chat: &ChatId,
        ts: MessageTs,
        version: Option<i64>,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self
            .reactors
            .as_ref()
            .filter(|v| v.chat == *chat && v.timestamp == ts)
        else {
            return;
        };
        if view.base_version == version || is_gone(&view.state) {
            return;
        }
        let ticket = self.refresh.schedule();
        self.refresh_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(REACTORS_REFRESH_DELAY).await;
            this.update(cx, |store, cx| store.refresh_reactors(ticket, cx))
                .ok();
        }));
    }

    /// The delay of a refresh ran out: reads the first page again with a fresh token, which
    /// supersedes any request still out. A failed list goes back to `Loading` first, as for a
    /// retry. A timer that was cancelled or superseded, or a list that closed or whose message is
    /// gone meanwhile, does nothing.
    fn refresh_reactors(&mut self, ticket: u64, cx: &mut Context<Self>) {
        if !self.refresh.fire(ticket) {
            return;
        }
        let Some(view) = self.reactors.as_mut() else {
            return;
        };
        if is_gone(&view.state) {
            return;
        }
        if matches!(view.state, ReactorsState::Failed { .. }) {
            view.state = ReactorsState::Loading;
        }
        self.request_reactors(RequestKind::First, cx);
    }

    /// Turns the open list of `(chat, ts)` into the "message gone" failure, drops the answer it
    /// was waiting for (a fresh token, nothing sent) and cancels its refresh.
    fn reactors_gone(&mut self, chat: &ChatId, ts: MessageTs, cx: &mut Context<Self>) {
        let Some(view) = self
            .reactors
            .as_mut()
            .filter(|v| v.chat == *chat && v.timestamp == ts)
        else {
            return;
        };
        let gone = ReactorsState::Failed {
            reason: MESSAGE_GONE.into(),
            offline: false,
        };
        if view.state == gone {
            return;
        }
        self.next_request += 1;
        view.generation = self.next_request;
        view.state = gone;
        self.cancel_refresh();
        cx.emit(StoreEvent::Reactors);
        cx.notify();
    }

    /// Forgets a scheduled refresh and its timer. Not for use from the timer's own task.
    fn cancel_refresh(&mut self) {
        self.refresh.cancel();
        self.refresh_timer = None;
    }

    /// Asks the core to start a private chat with `user`. The state becomes `InFlight` at once; while
    /// one is running nothing is sent again (the core drops it too). The dialog must have shown the
    /// confirmation first: this is the only call that creates a chat.
    pub fn create_private_chat(&mut self, user: UserId, cx: &mut Context<Self>) {
        if self.creation == Some(CreationState::InFlight) {
            return;
        }
        self.creation = Some(CreationState::InFlight);
        self.pending_open = None;
        self.creating = Some(user.clone());
        self.send(Command::CreatePrivateChat { user });
        cx.emit(StoreEvent::Creation);
        cx.notify();
    }

    pub fn creation_state(&self) -> Option<&CreationState> {
        self.creation.as_ref()
    }

    /// Forgets a failure (the dialog closed or the user edited the choice). A creation that is in
    /// flight cannot be recalled and stays, so its outcome still reaches the UI.
    pub fn dismiss_creation(&mut self, cx: &mut Context<Self>) {
        if matches!(
            self.creation,
            Some(CreationState::Failed { .. } | CreationState::Unverified { .. })
        ) {
            self.creation = None;
            cx.emit(StoreEvent::Creation);
            cx.notify();
        }
    }

    /// A creation that is not in flight (late, or ended by an expired session) changes nothing.
    fn creation_failed(&mut self, failure: CreationState, cx: &mut Context<Self>) {
        if self.creation != Some(CreationState::InFlight) {
            return;
        }
        self.creating = None;
        self.creation = Some(failure);
        cx.emit(StoreEvent::Creation);
    }

    fn chat_created(&mut self, chat: Option<ChatId>, existing: bool, cx: &mut Context<Self>) {
        if self.creation != Some(CreationState::InFlight) {
            return;
        }
        self.creation = None;
        // A chat the answer does not name is opened once the list shows the private chat with that person.
        let creating = self.creating.take();
        self.pending_open = match (chat.clone(), creating) {
            (Some(chat), _) => Some(PendingOpen::Chat(chat)),
            (None, Some(user)) => Some(PendingOpen::Partner(user)),
            (None, None) => None,
        };
        cx.emit(StoreEvent::Creation);
        self.open_pending(cx);
        cx.emit(StoreEvent::ChatCreated { chat, existing });
    }

    /// Selects the created chat once the list holds it.
    fn open_pending(&mut self, cx: &mut Context<Self>) {
        let found = match self.pending_open.as_ref() {
            Some(PendingOpen::Chat(id)) => self.chats.contains_key(id).then(|| id.clone()),
            Some(PendingOpen::Partner(user)) => self
                .chats
                .values()
                .find(|c| c.kind == ChatKind::Private && c.partner.as_ref() == Some(user))
                .map(|c| c.id.clone()),
            None => None,
        };
        if let Some(chat) = found {
            self.select(chat, cx);
        }
    }

    fn set_download(
        &mut self,
        media: MediaRef,
        state: Option<DownloadState>,
        cx: &mut Context<Self>,
    ) {
        match state {
            Some(state) => self.downloads.insert(media.clone(), state),
            None => self.downloads.remove(&media),
        };
        cx.emit(StoreEvent::Download(media));
    }

    pub fn connection(&self) -> ConnectionState {
        self.connection
    }

    pub fn session(&self) -> SessionState {
        self.session
    }

    /// Chats matching the name query (case-insensitive substring) and filter, in display order:
    /// pinned by rank, then by last activity, newest first.
    pub fn visible_chats(&self, query: &str, filter: ChatFilter) -> Vec<&ChatSummary> {
        let query = query.trim().to_lowercase();
        let mut chats: Vec<&ChatSummary> = self
            .chats
            .values()
            .filter(|c| {
                filter.accepts(c) && (query.is_empty() || c.title.to_lowercase().contains(&query))
            })
            .collect();
        chats.sort_by(|a, b| {
            let activity = |c: &ChatSummary| c.last_message.as_ref().map(|m| m.timestamp);
            (
                a.pinned_rank.is_none(),
                a.pinned_rank,
                Reverse(activity(a)),
                &a.title,
                &a.id,
            )
                .cmp(&(
                    b.pinned_rank.is_none(),
                    b.pinned_rank,
                    Reverse(activity(b)),
                    &b.title,
                    &b.id,
                ))
        });
        chats
    }

    fn send(&self, command: Command) {
        // The channel is unbounded; it only fails once the core is gone, and then there is nobody to tell.
        let _ = self.commands.try_send(command);
    }

    /// Asks the core for avatars and image thumbnails once each. Files are never downloaded.
    fn want_media(&mut self, media: Option<&MediaRef>) {
        let Some(media) = media else { return };
        if media.kind == MediaKind::File || self.media.contains_key(media) {
            return;
        }
        if self.media_requested.insert(media.clone()) {
            self.send(Command::FetchMedia(media.clone()));
        }
    }

    /// Thumbnails of every picture of the message; originals are never requested here.
    fn want_message_media(&mut self, message: &Message) {
        self.want_media(message.author_avatar.as_ref());
        match &message.body {
            MessageBody::Image { media, .. } => self.want_media(Some(media)),
            MessageBody::Gallery { items, .. } => {
                for item in items {
                    self.want_media(Some(&item.media));
                }
            }
            _ => {}
        }
    }

    fn apply_history(&mut self, chunk: HistoryChunk, cx: &mut Context<Self>) {
        for message in &chunk.messages {
            self.want_message_media(message);
        }
        // A send whose confirmation and echo were lost is told apart by its payload: its server copy
        // arrives in this chunk, so the local one goes.
        for payload in &chunk.payloads {
            self.settle(&chunk.chat, payload, cx);
        }
        let timeline = self.timelines.entry(chunk.chat.clone()).or_default();
        let older_page = std::mem::take(&mut timeline.loading_older);
        // A page without messages cannot make progress; treat it as the start so loading stops.
        let start = chunk.reached_start || (older_page && chunk.messages.is_empty());
        // A replacement open at the bottom redefines where the history starts.
        if chunk.replaces.is_some_and(|(from, _)| from.is_none()) {
            timeline.reached_start = start;
        } else {
            timeline.reached_start |= start;
        }
        let change = if timeline.loaded {
            merge_chunk(&mut timeline.messages, chunk.messages, chunk.replaces)
        } else {
            timeline.messages = normalized(chunk.messages);
            TimelineChange::Reset
        };
        timeline.loaded = true;
        cx.emit(StoreEvent::Timeline {
            chat: chunk.chat,
            change,
        });
    }

    fn upsert(&mut self, message: Message, cx: &mut Context<Self>) {
        self.want_message_media(&message);
        if message.delivery == Delivery::Sent {
            self.typing_stopped(&message.chat, &message.author);
        }
        let Some(timeline) = self.timelines.get_mut(&message.chat).filter(|t| t.loaded) else {
            return;
        };
        let chat = message.chat.clone();
        let ts = message.timestamp;
        let change = match timeline.messages.binary_search_by_key(&ts, |m| m.timestamp) {
            Ok(i) if timeline.messages[i] == message => return,
            Ok(i) => {
                timeline.messages[i] = message;
                TimelineChange::Delta {
                    prepended: 0,
                    appended: 0,
                    touched: vec![i],
                }
            }
            Err(i) if i == timeline.messages.len() => {
                let was_empty = i == 0;
                timeline.messages.push(message);
                if was_empty {
                    TimelineChange::Reset
                } else {
                    TimelineChange::Delta {
                        prepended: 0,
                        appended: 1,
                        touched: vec![],
                    }
                }
            }
            Err(0) if timeline.reached_start => {
                timeline.messages.insert(0, message);
                TimelineChange::Delta {
                    prepended: 1,
                    appended: 0,
                    touched: vec![],
                }
            }
            // Older than the loaded window: it will arrive with its page.
            Err(0) => return,
            Err(i) => {
                timeline.messages.insert(i, message);
                TimelineChange::Reset
            }
        };
        cx.emit(StoreEvent::Timeline { chat, change });
    }

    fn delete(&mut self, chat: &ChatId, ts: MessageTs, cx: &mut Context<Self>) {
        let Some(timeline) = self.timelines.get_mut(chat) else {
            return;
        };
        let Ok(i) = timeline.messages.binary_search_by_key(&ts, |m| m.timestamp) else {
            return;
        };
        let message = &mut timeline.messages[i];
        if message.body == MessageBody::Deleted {
            return;
        }
        message.body = MessageBody::Deleted;
        message.reply_to = None;
        message.forwarded_from = None;
        cx.emit(StoreEvent::Timeline {
            chat: chat.clone(),
            change: TimelineChange::Delta {
                prepended: 0,
                appended: 0,
                touched: vec![i],
            },
        });
    }

    /// Replaces the reactions of a loaded message in place. Unlike `upsert` this leaves the author's
    /// typing indicator and the media requests alone: a reaction update carries no message body.
    fn set_reactions(
        &mut self,
        chat: &ChatId,
        ts: MessageTs,
        reactions: Reactions,
        cx: &mut Context<Self>,
    ) {
        let Some(timeline) = self.timelines.get_mut(chat).filter(|t| t.loaded) else {
            return;
        };
        let Ok(i) = timeline.messages.binary_search_by_key(&ts, |m| m.timestamp) else {
            return;
        };
        let message = &mut timeline.messages[i];
        if message.reactions == reactions {
            return;
        }
        message.reactions = reactions;
        cx.emit(StoreEvent::Timeline {
            chat: chat.clone(),
            change: TimelineChange::Delta {
                prepended: 0,
                appended: 0,
                touched: vec![i],
            },
        });
    }
}

/// The failure of a list whose message was deleted: the one state a change of reactions cannot mend.
fn is_gone(state: &ReactorsState) -> bool {
    matches!(state, ReactorsState::Failed { reason, .. } if reason == MESSAGE_GONE)
}

/// The reaction version of the loaded message `(chat, ts)`; `None` when it is not loaded or has none.
fn reactions_version(
    timelines: &HashMap<ChatId, Timeline>,
    chat: &ChatId,
    ts: MessageTs,
) -> Option<i64> {
    let messages = &timelines.get(chat)?.messages;
    let i = messages.binary_search_by_key(&ts, |m| m.timestamp).ok()?;
    messages[i].reactions.version
}

/// Whether a page ends the list. Decided from the page alone, with strict cursor progress: no
/// timestamp seen, a page shorter than the `limit` asked, or an `oldest` that did not get below
/// the `before` cursor (a repeat of the boundary; continuing would loop). The message's known
/// counts never decide it.
fn page_exhausted(
    before: Option<MessageTs>,
    raw: usize,
    oldest: Option<MessageTs>,
    limit: usize,
) -> bool {
    match oldest {
        None => true,
        Some(oldest) => raw < limit || before.is_some_and(|b| oldest >= b),
    }
}

/// Appends `incoming` in order, skipping a person already in `rows` (or earlier in `incoming`).
fn append_new(rows: &mut Vec<Reactor>, incoming: Vec<Reactor>) {
    let mut seen: HashSet<UserId> = rows.iter().map(|r| r.user.clone()).collect();
    rows.extend(incoming.into_iter().filter(|r| seen.insert(r.user.clone())));
}

/// Sorted by timestamp, duplicates keep the last occurrence.
fn normalized(mut messages: Vec<Message>) -> Vec<Message> {
    messages.sort_by_key(|m| m.timestamp);
    let mut out: Vec<Message> = Vec::with_capacity(messages.len());
    for message in messages {
        match out.last_mut() {
            Some(last) if last.timestamp == message.timestamp => *last = message,
            _ => out.push(message),
        }
    }
    out
}

/// Replaces the range the core replaced (else the span covered by `incoming`), so server-side
/// removals vanish, and reports the change.
fn merge_chunk(
    old: &mut Vec<Message>,
    incoming: Vec<Message>,
    replaces: Option<(Option<MessageTs>, Option<MessageTs>)>,
) -> TimelineChange {
    let incoming = normalized(incoming);
    let span = replaces.or_else(|| {
        incoming
            .first()
            .zip(incoming.last())
            .map(|(a, b)| (Some(a.timestamp), Some(b.timestamp)))
    });
    let Some((from, to)) = span else {
        return TimelineChange::Status;
    };
    let covered = |ts: MessageTs| from.is_none_or(|f| ts >= f) && to.is_none_or(|t| ts <= t);
    let mut merged = Vec::with_capacity(old.len() + incoming.len());
    let mut incoming = incoming.into_iter().peekable();
    for existing in old.iter() {
        while incoming
            .peek()
            .is_some_and(|m| m.timestamp < existing.timestamp)
        {
            merged.extend(incoming.next());
        }
        if incoming
            .peek()
            .is_some_and(|m| m.timestamp == existing.timestamp)
        {
            merged.extend(incoming.next());
        } else if !covered(existing.timestamp) {
            merged.push(existing.clone());
        }
    }
    merged.extend(incoming);
    let change = diff(old, &merged);
    *old = merged;
    change
}

fn diff(old: &[Message], new: &[Message]) -> TimelineChange {
    let (Some(first), Some(last)) = (old.first(), old.last()) else {
        return TimelineChange::Reset;
    };
    let prepended = new.partition_point(|m| m.timestamp < first.timestamp);
    let end = new.partition_point(|m| m.timestamp <= last.timestamp);
    let middle = &new[prepended..end];
    if middle.len() != old.len()
        || middle
            .iter()
            .zip(old)
            .any(|(n, o)| n.timestamp != o.timestamp)
    {
        return TimelineChange::Reset;
    }
    let touched = middle
        .iter()
        .zip(old)
        .enumerate()
        .filter(|(_, (n, o))| n != o)
        .map(|(i, _)| prepended + i)
        .collect();
    TimelineChange::Delta {
        prepended,
        appended: new.len() - end,
        touched,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(ts: i64, text: &str) -> Message {
        Message {
            chat: ChatId("c".into()),
            timestamp: ts,
            author: bt_chat_core::model::UserId("u".into()),
            author_name: None,
            author_avatar: None,
            outgoing: false,
            body: MessageBody::Text(text.into()),
            edited: false,
            reply_to: None,
            forwarded_from: None,
            delivery: bt_chat_core::model::Delivery::Sent,
            reactions: Default::default(),
            important: false,
        }
    }

    fn texts(ms: &[Message]) -> Vec<i64> {
        ms.iter().map(|m| m.timestamp).collect()
    }

    #[test]
    fn older_page_is_a_prepend() {
        let mut old = vec![msg(3, "c"), msg(4, "d")];
        let change = merge_chunk(&mut old, vec![msg(1, "a"), msg(2, "b")], None);
        assert_eq!(
            change,
            TimelineChange::Delta {
                prepended: 2,
                appended: 0,
                touched: vec![]
            }
        );
        assert_eq!(texts(&old), [1, 2, 3, 4]);
    }

    #[test]
    fn catch_up_chunk_appends_and_updates() {
        let mut old = vec![msg(1, "a"), msg(2, "b"), msg(3, "c")];
        let change = merge_chunk(&mut old, vec![msg(2, "b"), msg(3, "C"), msg(4, "d")], None);
        assert_eq!(
            change,
            TimelineChange::Delta {
                prepended: 0,
                appended: 1,
                touched: vec![2]
            }
        );
        assert_eq!(texts(&old), [1, 2, 3, 4]);
    }

    #[test]
    fn chunk_replaces_its_span() {
        let mut old = vec![msg(1, "a"), msg(2, "b"), msg(3, "c")];
        let change = merge_chunk(&mut old, vec![msg(1, "a"), msg(3, "c")], None);
        assert_eq!(change, TimelineChange::Reset);
        assert_eq!(texts(&old), [1, 3]);
    }

    #[test]
    fn empty_chunk_changes_nothing() {
        let mut old = vec![msg(1, "a")];
        assert_eq!(merge_chunk(&mut old, vec![], None), TimelineChange::Status);
        assert_eq!(texts(&old), [1]);
    }

    #[test]
    fn normalized_sorts_and_dedups() {
        let out = normalized(vec![msg(2, "x"), msg(1, "a"), msg(2, "b")]);
        assert_eq!(texts(&out), [1, 2]);
        assert_eq!(out[1].body, MessageBody::Text("b".into()));
    }

    mod reactions {
        use super::*;
        use bt_chat_core::reactions::ReactionCount;
        use gpui::{AppContext, Entity, Subscription, TestAppContext};
        use std::cell::RefCell;
        use std::rc::Rc;

        struct Rig {
            store: Entity<ChatStore>,
            commands: Receiver<Command>,
            events: Rc<RefCell<Vec<StoreEvent>>>,
            _client: Sender<ClientEvent>,
            _subscription: Subscription,
        }

        impl Rig {
            fn new(cx: &mut TestAppContext) -> Self {
                let (client, event_rx) = async_channel::unbounded();
                let (command_tx, commands) = async_channel::unbounded();
                let store = cx.new(|cx| ChatStore::new(event_rx, command_tx, cx));
                let events = Rc::new(RefCell::new(Vec::new()));
                let log = events.clone();
                let subscription = cx.update(|cx| {
                    cx.subscribe(&store, move |_, event: &StoreEvent, _| {
                        log.borrow_mut().push(event.clone())
                    })
                });
                Self {
                    store,
                    commands,
                    events,
                    _client: client,
                    _subscription: subscription,
                }
            }

            /// A rig whose chat `c` holds the messages 1 and 2, loaded.
            fn loaded(cx: &mut TestAppContext) -> Self {
                let rig = Self::new(cx);
                let chunk = HistoryChunk {
                    chat: chat(),
                    messages: vec![msg(1, "a"), msg(2, "b")],
                    reached_start: true,
                    replaces: None,
                    payloads: vec![],
                };
                rig.apply(cx, ClientEvent::History(chunk));
                rig.take_events();
                rig
            }

            fn apply(&self, cx: &mut TestAppContext, event: ClientEvent) {
                self.store.update(cx, |s, cx| s.apply(event, cx));
            }

            fn messages(&self, cx: &mut TestAppContext) -> Vec<Message> {
                self.store.read_with(cx, |s, _| {
                    s.timeline(&chat())
                        .map(|t| t.messages.clone())
                        .unwrap_or_default()
                })
            }

            fn commands(&self) -> Vec<Command> {
                std::iter::from_fn(|| self.commands.try_recv().ok()).collect()
            }

            fn take_events(&self) -> Vec<StoreEvent> {
                std::mem::take(&mut *self.events.borrow_mut())
            }
        }

        fn chat() -> ChatId {
            ChatId("c".into())
        }

        fn reactions(pairs: &[(i64, u32)], mine: Option<i64>, version: i64) -> Reactions {
            Reactions {
                counts: pairs
                    .iter()
                    .map(|&(c, count)| ReactionCount {
                        code: ReactionCode(c),
                        count,
                    })
                    .collect(),
                mine: mine.map(ReactionCode),
                version: Some(version),
            }
        }

        fn changed(ts: i64, reactions: Reactions) -> ClientEvent {
            ClientEvent::ReactionsChanged {
                chat: chat(),
                timestamp: ts,
                reactions,
            }
        }

        fn delta(touched: Vec<usize>) -> StoreEvent {
            StoreEvent::Timeline {
                chat: chat(),
                change: TimelineChange::Delta {
                    prepended: 0,
                    appended: 0,
                    touched,
                },
            }
        }

        #[gpui::test]
        fn an_update_changes_only_the_reactions_of_that_message(cx: &mut TestAppContext) {
            let rig = Rig::loaded(cx);
            let before = rig.messages(cx);

            rig.apply(cx, changed(2, reactions(&[(7, 2)], Some(7), 3)));
            assert_eq!(rig.take_events(), [delta(vec![1])]);
            let after = rig.messages(cx);
            assert_eq!(after[0], before[0], "the other message is untouched");
            assert_eq!(after[1].reactions, reactions(&[(7, 2)], Some(7), 3));
            let mut rest = after[1].clone();
            rest.reactions = Reactions::default();
            assert_eq!(rest, before[1], "nothing but the reactions changed");
        }

        #[gpui::test]
        fn an_equal_update_emits_nothing(cx: &mut TestAppContext) {
            let rig = Rig::loaded(cx);
            rig.apply(cx, changed(2, reactions(&[(7, 2)], None, 3)));
            rig.take_events();

            rig.apply(cx, changed(2, reactions(&[(7, 2)], None, 3)));
            assert!(rig.take_events().is_empty());
            assert_eq!(rig.messages(cx)[1].reactions, reactions(&[(7, 2)], None, 3));
        }

        #[gpui::test]
        fn an_unloaded_timeline_or_unknown_message_is_a_no_op(cx: &mut TestAppContext) {
            let rig = Rig::new(cx);
            rig.apply(cx, changed(2, reactions(&[(7, 1)], None, 1)));
            assert!(rig.take_events().is_empty());
            assert!(
                rig.store
                    .read_with(cx, |s, _| s.timeline(&chat()).is_none()),
                "no timeline is made up"
            );

            let rig = Rig::loaded(cx);
            rig.apply(cx, changed(99, reactions(&[(7, 1)], None, 1)));
            rig.apply(
                cx,
                ClientEvent::ReactionsChanged {
                    chat: ChatId("other".into()),
                    timestamp: 2,
                    reactions: reactions(&[(7, 1)], None, 1),
                },
            );
            assert!(rig.take_events().is_empty());
            assert!(
                rig.messages(cx)
                    .iter()
                    .all(|m| m.reactions == Reactions::default())
            );
        }

        #[gpui::test]
        fn the_authors_typing_indicator_survives_an_update(cx: &mut TestAppContext) {
            let rig = Rig::loaded(cx);
            let until = cx.executor().now() + Duration::from_secs(60);
            rig.apply(
                cx,
                ClientEvent::Typing {
                    chat: chat(),
                    user: UserId("u".into()),
                    name: None,
                    until,
                },
            );
            let typing =
                |cx: &mut TestAppContext| rig.store.read_with(cx, |s, _| s.typing_text(&chat()));
            assert!(typing(cx).is_some());

            rig.apply(cx, changed(1, reactions(&[(7, 1)], None, 1)));
            assert!(
                typing(cx).is_some(),
                "an upsert of the author's message would have cleared it"
            );
        }

        #[gpui::test]
        fn toggling_sends_the_command_without_touching_the_timeline(cx: &mut TestAppContext) {
            let rig = Rig::loaded(cx);
            rig.store
                .update(cx, |s, _| s.toggle_reaction(chat(), 2, ReactionCode(7)));
            assert_eq!(
                rig.commands(),
                [Command::ToggleReaction {
                    chat: chat(),
                    timestamp: 2,
                    code: ReactionCode(7)
                }]
            );
            assert!(
                rig.take_events().is_empty(),
                "the overlay arrives from the core as an event"
            );
        }
    }

    mod search {
        use super::*;
        use bt_chat_core::model::{ChatHit, MessageHit, PersonHit};
        use gpui::{AppContext, Entity, Subscription, TestAppContext};
        use std::cell::RefCell;
        use std::rc::Rc;

        struct Rig {
            store: Entity<ChatStore>,
            commands: Receiver<Command>,
            events: Rc<RefCell<Vec<StoreEvent>>>,
            // Keep the channel and the subscription alive for the whole test.
            _client: Sender<ClientEvent>,
            _subscription: Subscription,
        }

        impl Rig {
            fn new(cx: &mut TestAppContext) -> Self {
                let (client, event_rx) = async_channel::unbounded();
                let (command_tx, commands) = async_channel::unbounded();
                let store = cx.new(|cx| ChatStore::new(event_rx, command_tx, cx));
                let events = Rc::new(RefCell::new(Vec::new()));
                let log = events.clone();
                let subscription = cx.update(|cx| {
                    cx.subscribe(&store, move |_, event: &StoreEvent, _| {
                        log.borrow_mut().push(event.clone())
                    })
                });
                Self {
                    store,
                    commands,
                    events,
                    _client: client,
                    _subscription: subscription,
                }
            }

            fn search(&self, cx: &mut TestAppContext, query: &str) {
                self.store.update(cx, |s, cx| s.search(query.into(), cx));
            }

            fn cancel(&self, cx: &mut TestAppContext) {
                self.store.update(cx, |s, cx| s.cancel_search(cx));
            }

            fn apply(&self, cx: &mut TestAppContext, event: ClientEvent) {
                self.store.update(cx, |s, cx| s.apply(event, cx));
            }

            fn state(&self, cx: &mut TestAppContext) -> Option<SearchState> {
                self.store.read_with(cx, |s, _| s.search_state().cloned())
            }

            fn commands(&self) -> Vec<Command> {
                std::iter::from_fn(|| self.commands.try_recv().ok()).collect()
            }

            fn take_events(&self) -> Vec<StoreEvent> {
                std::mem::take(&mut *self.events.borrow_mut())
            }
        }

        fn hits(tag: &str) -> SearchHits {
            SearchHits {
                chats: vec![ChatHit {
                    id: ChatId(format!("c-{tag}")),
                    kind: ChatKind::Group,
                    title: tag.into(),
                    avatar: None,
                }],
                people: vec![PersonHit {
                    user: UserId(format!("u-{tag}")),
                    name: tag.into(),
                    avatar: None,
                    chat: None,
                }],
                messages: vec![MessageHit {
                    chat: ChatId(format!("c-{tag}")),
                    chat_title: tag.into(),
                    timestamp: 7,
                    author_name: Some("A".into()),
                    text: tag.into(),
                }],
            }
        }

        fn results(id: u64, tag: &str) -> ClientEvent {
            ClientEvent::SearchResults {
                id: SearchId(id),
                hits: hits(tag),
            }
        }

        #[gpui::test]
        fn a_result_with_the_current_id_is_stored_and_announced(cx: &mut TestAppContext) {
            let rig = Rig::new(cx);
            assert_eq!(rig.state(cx), None);
            rig.search(cx, "ab");
            assert_eq!(
                rig.commands(),
                [Command::Search {
                    id: SearchId(1),
                    query: "ab".into()
                }]
            );
            assert_eq!(rig.take_events(), [StoreEvent::Search]);
            assert_eq!(
                rig.state(cx),
                Some(SearchState {
                    id: SearchId(1),
                    query: "ab".into(),
                    status: SearchStatus::Loading,
                    hits: SearchHits::default()
                })
            );

            rig.apply(cx, results(1, "ab"));
            assert_eq!(rig.take_events(), [StoreEvent::Search]);
            let state = rig.state(cx).unwrap();
            assert_eq!(state.status, SearchStatus::Ready);
            assert_eq!(state.hits, hits("ab"));
            assert!(rig.commands().is_empty(), "no hit has an avatar to fetch");
        }

        #[gpui::test]
        fn a_stale_id_is_ignored(cx: &mut TestAppContext) {
            let rig = Rig::new(cx);
            rig.search(cx, "ab");
            rig.search(cx, "abc");
            rig.take_events();
            rig.commands();

            rig.apply(cx, results(1, "ab"));
            rig.apply(
                cx,
                ClientEvent::SearchFailed {
                    id: SearchId(1),
                    reason: SearchFailure::Failed,
                },
            );
            assert!(
                rig.take_events().is_empty(),
                "a superseded reply announces nothing"
            );
            let state = rig.state(cx).unwrap();
            assert_eq!(
                (state.id, state.query.as_str(), state.status),
                (SearchId(2), "abc", SearchStatus::Loading)
            );
            assert_eq!(state.hits, SearchHits::default());

            rig.apply(cx, results(2, "abc"));
            let state = rig.state(cx).unwrap();
            assert_eq!(
                (state.status, state.hits),
                (SearchStatus::Ready, hits("abc"))
            );
        }

        #[gpui::test]
        fn a_new_query_keeps_the_old_hits_while_loading(cx: &mut TestAppContext) {
            let rig = Rig::new(cx);
            rig.search(cx, "ab");
            rig.apply(cx, results(1, "ab"));
            rig.search(cx, "abc");
            let state = rig.state(cx).unwrap();
            assert_eq!(
                (state.id, state.status),
                (SearchId(2), SearchStatus::Loading)
            );
            assert_eq!(state.hits, hits("ab"));
            assert_eq!(
                rig.commands(),
                [
                    Command::Search {
                        id: SearchId(1),
                        query: "ab".into()
                    },
                    Command::Search {
                        id: SearchId(2),
                        query: "abc".into()
                    }
                ]
            );

            rig.apply(cx, results(2, "abc"));
            assert_eq!(rig.state(cx).unwrap().hits, hits("abc"));
        }

        #[gpui::test]
        fn a_failure_for_the_current_id_is_shown_and_keeps_the_hits(cx: &mut TestAppContext) {
            let rig = Rig::new(cx);
            rig.search(cx, "ab");
            rig.apply(cx, results(1, "ab"));
            rig.search(cx, "abc");
            rig.take_events();

            rig.apply(
                cx,
                ClientEvent::SearchFailed {
                    id: SearchId(2),
                    reason: SearchFailure::Offline,
                },
            );
            assert_eq!(rig.take_events(), [StoreEvent::Search]);
            let state = rig.state(cx).unwrap();
            assert_eq!(state.status, SearchStatus::Failed(SearchFailure::Offline));
            assert_eq!(state.hits, hits("ab"));
        }

        #[gpui::test]
        fn cancelling_clears_the_state_sends_the_command_and_drops_late_replies(
            cx: &mut TestAppContext,
        ) {
            let rig = Rig::new(cx);
            rig.search(cx, "ab");
            rig.apply(cx, results(1, "ab"));
            rig.search(cx, "abc");
            rig.commands();
            rig.take_events();

            rig.cancel(cx);
            assert_eq!(rig.state(cx), None);
            assert_eq!(rig.commands(), [Command::CancelSearch]);
            assert_eq!(rig.take_events(), [StoreEvent::Search]);

            rig.apply(cx, results(2, "abc"));
            assert_eq!(rig.state(cx), None, "a reply after the cancel is dropped");
            assert!(rig.take_events().is_empty());

            rig.cancel(cx);
            assert!(rig.commands().is_empty(), "nothing to cancel, nothing sent");
            assert!(rig.take_events().is_empty());

            rig.search(cx, "xy");
            assert_eq!(
                rig.commands(),
                [Command::Search {
                    id: SearchId(3),
                    query: "xy".into()
                }],
                "ids never repeat"
            );
            rig.apply(cx, results(2, "abc"));
            assert_eq!(rig.state(cx).unwrap().status, SearchStatus::Loading);
        }

        #[gpui::test]
        fn avatars_of_chat_and_person_hits_are_requested_once(cx: &mut TestAppContext) {
            let rig = Rig::new(cx);
            let avatar = |id: &str| {
                Some(MediaRef {
                    id: id.into(),
                    kind: MediaKind::Avatar,
                })
            };
            let mut found = hits("ab");
            found.chats[0].avatar = avatar("chat-av");
            found.people[0].avatar = avatar("person-av");
            rig.search(cx, "ab");
            rig.commands();

            rig.apply(
                cx,
                ClientEvent::SearchResults {
                    id: SearchId(1),
                    hits: found.clone(),
                },
            );
            rig.search(cx, "abc");
            rig.apply(
                cx,
                ClientEvent::SearchResults {
                    id: SearchId(2),
                    hits: found,
                },
            );
            let fetched: Vec<Command> = rig
                .commands()
                .into_iter()
                .filter(|c| matches!(c, Command::FetchMedia(_)))
                .collect();
            assert_eq!(
                fetched,
                [
                    Command::FetchMedia(avatar("chat-av").unwrap()),
                    Command::FetchMedia(avatar("person-av").unwrap())
                ]
            );
        }

        #[gpui::test]
        fn signing_out_forgets_the_search(cx: &mut TestAppContext) {
            let rig = Rig::new(cx);
            rig.search(cx, "ab");
            rig.apply(cx, results(1, "ab"));
            rig.apply(cx, ClientEvent::SignedOut);
            assert_eq!(rig.state(cx), None);
        }
    }

    mod members {
        use super::*;
        use bt_chat_core::model::{Member, MemberRole};
        use gpui::{AppContext, Entity, Subscription, TestAppContext};
        use std::cell::RefCell;
        use std::rc::Rc;

        struct Rig {
            store: Entity<ChatStore>,
            commands: Receiver<Command>,
            events: Rc<RefCell<Vec<StoreEvent>>>,
            _client: Sender<ClientEvent>,
            _subscription: Subscription,
        }

        impl Rig {
            fn new(cx: &mut TestAppContext) -> Self {
                let (client, event_rx) = async_channel::unbounded();
                let (command_tx, commands) = async_channel::unbounded();
                let store = cx.new(|cx| ChatStore::new(event_rx, command_tx, cx));
                let events = Rc::new(RefCell::new(Vec::new()));
                let log = events.clone();
                let subscription = cx.update(|cx| {
                    cx.subscribe(&store, move |_, event: &StoreEvent, _| {
                        log.borrow_mut().push(event.clone())
                    })
                });
                Self {
                    store,
                    commands,
                    events,
                    _client: client,
                    _subscription: subscription,
                }
            }

            fn load(&self, cx: &mut TestAppContext, chat: &ChatId) {
                self.store
                    .update(cx, |s, cx| s.load_members(chat.clone(), cx));
            }

            fn load_profile(&self, cx: &mut TestAppContext) {
                self.store.update(cx, |s, cx| s.load_profile(cx));
            }

            fn apply(&self, cx: &mut TestAppContext, event: ClientEvent) {
                self.store.update(cx, |s, cx| s.apply(event, cx));
            }

            fn state(&self, cx: &mut TestAppContext, chat: &ChatId) -> Option<MembersState> {
                self.store
                    .read_with(cx, |s, _| s.members_state(chat).cloned())
            }

            fn profile(&self, cx: &mut TestAppContext) -> Option<ProfileState> {
                self.store.read_with(cx, |s, _| s.profile_state().cloned())
            }

            fn commands(&self) -> Vec<Command> {
                std::iter::from_fn(|| self.commands.try_recv().ok()).collect()
            }

            fn take_events(&self) -> Vec<StoreEvent> {
                std::mem::take(&mut *self.events.borrow_mut())
            }
        }

        fn chat(tag: &str) -> ChatId {
            ChatId(tag.into())
        }

        fn avatar(id: &str) -> Option<MediaRef> {
            Some(MediaRef {
                id: id.into(),
                kind: MediaKind::Avatar,
            })
        }

        fn listed(tag: &str) -> ChatMembers {
            ChatMembers {
                chat: chat(tag),
                users: vec![
                    Member {
                        user: UserId("u1".into()),
                        name: Some("Анна".into()),
                        avatar: avatar("av1"),
                        role: MemberRole::Admin,
                        me: false,
                    },
                    Member {
                        user: UserId("u2".into()),
                        name: Some("Борис".into()),
                        avatar: None,
                        role: MemberRole::Member,
                        me: true,
                    },
                ],
                partial: false,
            }
        }

        fn failed(chat_tag: &str, reason: &str, offline: bool) -> ClientEvent {
            ClientEvent::MembersFailed {
                chat: chat(chat_tag),
                reason: reason.into(),
                offline,
            }
        }

        #[gpui::test]
        fn loading_becomes_ready_and_asks_for_the_avatars_once(cx: &mut TestAppContext) {
            let rig = Rig::new(cx);
            assert_eq!(rig.state(cx, &chat("a")), None);

            rig.load(cx, &chat("a"));
            assert_eq!(rig.state(cx, &chat("a")), Some(MembersState::Loading));
            assert_eq!(rig.commands(), [Command::LoadMembers(chat("a"))]);
            assert_eq!(rig.take_events(), [StoreEvent::Members(chat("a"))]);

            rig.apply(cx, ClientEvent::Members(listed("a")));
            assert_eq!(
                rig.state(cx, &chat("a")),
                Some(MembersState::Ready(listed("a")))
            );
            assert_eq!(rig.take_events(), [StoreEvent::Members(chat("a"))]);
            assert_eq!(
                rig.commands(),
                [Command::FetchMedia(avatar("av1").unwrap())]
            );
        }

        #[gpui::test]
        fn loading_becomes_failed_with_the_reason_and_the_offline_flag(cx: &mut TestAppContext) {
            let rig = Rig::new(cx);
            rig.load(cx, &chat("a"));
            rig.take_events();

            rig.apply(cx, failed("a", "нет прав", false));
            assert_eq!(
                rig.state(cx, &chat("a")),
                Some(MembersState::Failed {
                    reason: "нет прав".into(),
                    offline: false
                })
            );
            assert_eq!(rig.take_events(), [StoreEvent::Members(chat("a"))]);

            rig.load(cx, &chat("a"));
            rig.apply(cx, failed("a", "Нет соединения", true));
            assert_eq!(
                rig.state(cx, &chat("a")),
                Some(MembersState::Failed {
                    reason: "Нет соединения".into(),
                    offline: true
                })
            );
        }

        #[gpui::test]
        fn a_retry_after_a_failure_or_a_list_goes_back_to_loading_and_asks_again(
            cx: &mut TestAppContext,
        ) {
            let rig = Rig::new(cx);
            rig.load(cx, &chat("a"));
            rig.apply(cx, failed("a", "x", false));
            rig.commands();
            rig.take_events();

            rig.load(cx, &chat("a"));
            assert_eq!(rig.state(cx, &chat("a")), Some(MembersState::Loading));
            assert_eq!(rig.commands(), [Command::LoadMembers(chat("a"))]);
            assert_eq!(rig.take_events(), [StoreEvent::Members(chat("a"))]);

            rig.apply(cx, ClientEvent::Members(listed("a")));
            rig.load(cx, &chat("a"));
            assert_eq!(
                rig.state(cx, &chat("a")),
                Some(MembersState::Loading),
                "a list is never shown as current again"
            );
            assert_eq!(
                rig.commands()
                    .iter()
                    .filter(|c| **c == Command::LoadMembers(chat("a")))
                    .count(),
                1
            );
        }

        #[gpui::test]
        fn a_load_that_is_running_is_not_sent_twice(cx: &mut TestAppContext) {
            let rig = Rig::new(cx);
            rig.load(cx, &chat("a"));
            rig.take_events();
            rig.commands();

            rig.load(cx, &chat("a"));
            assert!(rig.commands().is_empty());
            assert!(rig.take_events().is_empty());
        }

        #[gpui::test]
        fn an_answer_for_a_chat_that_is_not_loading_is_ignored(cx: &mut TestAppContext) {
            let rig = Rig::new(cx);
            rig.apply(cx, ClientEvent::Members(listed("a")));
            rig.apply(cx, failed("a", "x", false));
            assert_eq!(rig.state(cx, &chat("a")), None);
            assert!(rig.take_events().is_empty());

            rig.load(cx, &chat("b"));
            rig.apply(cx, ClientEvent::Members(listed("a")));
            assert_eq!(
                rig.state(cx, &chat("a")),
                None,
                "the answer for another chat changes nothing"
            );
            assert_eq!(rig.state(cx, &chat("b")), Some(MembersState::Loading));

            rig.apply(cx, ClientEvent::Members(listed("b")));
            rig.apply(cx, failed("b", "late", false));
            assert_eq!(
                rig.state(cx, &chat("b")),
                Some(MembersState::Ready(listed("b"))),
                "a late duplicate does not overwrite"
            );
        }

        #[gpui::test]
        fn the_profile_goes_loading_to_ready_and_asks_for_its_avatar(cx: &mut TestAppContext) {
            let rig = Rig::new(cx);
            assert_eq!(rig.profile(cx), None);
            rig.load_profile(cx);
            assert_eq!(rig.profile(cx), Some(ProfileState::Loading));
            assert_eq!(rig.commands(), [Command::LoadProfile]);
            assert_eq!(rig.take_events(), [StoreEvent::Profile]);

            let own = Profile {
                user: UserId("me".into()),
                name: Some("Ян".into()),
                avatar: avatar("me-av"),
                status_text: None,
            };
            rig.apply(cx, ClientEvent::Profile(own.clone()));
            assert_eq!(rig.profile(cx), Some(ProfileState::Ready(own)));
            assert_eq!(rig.take_events(), [StoreEvent::Profile]);
            assert_eq!(
                rig.commands(),
                [Command::FetchMedia(avatar("me-av").unwrap())]
            );
        }

        #[gpui::test]
        fn the_profile_goes_loading_to_failed_and_a_retry_asks_again(cx: &mut TestAppContext) {
            let rig = Rig::new(cx);
            rig.load_profile(cx);
            rig.apply(
                cx,
                ClientEvent::ProfileFailed {
                    reason: "Нет соединения".into(),
                    offline: true,
                },
            );
            assert_eq!(
                rig.profile(cx),
                Some(ProfileState::Failed {
                    reason: "Нет соединения".into(),
                    offline: true
                })
            );
            rig.commands();

            rig.load_profile(cx);
            assert_eq!(
                rig.profile(cx),
                Some(ProfileState::Loading),
                "no data of the failed request stays"
            );
            assert_eq!(rig.commands(), [Command::LoadProfile]);
            rig.load_profile(cx);
            assert!(
                rig.commands().is_empty(),
                "a running load is not sent twice"
            );

            rig.apply(
                cx,
                ClientEvent::ProfileFailed {
                    reason: "x".into(),
                    offline: false,
                },
            );
            rig.apply(
                cx,
                ClientEvent::ProfileFailed {
                    reason: "y".into(),
                    offline: false,
                },
            );
            assert_eq!(
                rig.profile(cx),
                Some(ProfileState::Failed {
                    reason: "x".into(),
                    offline: false
                })
            );
        }

        #[gpui::test]
        fn signing_out_forgets_members_and_profile(cx: &mut TestAppContext) {
            let rig = Rig::new(cx);
            rig.load(cx, &chat("a"));
            rig.apply(cx, ClientEvent::Members(listed("a")));
            rig.load_profile(cx);
            rig.apply(cx, ClientEvent::SignedOut);
            assert_eq!(rig.state(cx, &chat("a")), None);
            assert_eq!(rig.profile(cx), None);
        }
    }

    mod reactors {
        use super::*;
        use bt_chat_core::reactions::ReactionCount;
        use gpui::{AppContext, Entity, Subscription, TestAppContext};
        use std::cell::RefCell;
        use std::rc::Rc;

        const TS: MessageTs = 500;

        struct Rig {
            store: Entity<ChatStore>,
            commands: Receiver<Command>,
            events: Rc<RefCell<Vec<StoreEvent>>>,
            _client: Sender<ClientEvent>,
            _subscription: Subscription,
        }

        impl Rig {
            fn new(cx: &mut TestAppContext) -> Self {
                let (client, event_rx) = async_channel::unbounded();
                let (command_tx, commands) = async_channel::unbounded();
                let store = cx.new(|cx| ChatStore::new(event_rx, command_tx, cx));
                let events = Rc::new(RefCell::new(Vec::new()));
                let log = events.clone();
                let subscription = cx.update(|cx| {
                    cx.subscribe(&store, move |_, event: &StoreEvent, _| {
                        log.borrow_mut().push(event.clone())
                    })
                });
                Self {
                    store,
                    commands,
                    events,
                    _client: client,
                    _subscription: subscription,
                }
            }

            /// A rig that has opened the list of message `TS` of chat `c`, with the open's
            /// command and event already taken.
            fn opened(cx: &mut TestAppContext) -> Self {
                let rig = Self::new(cx);
                rig.open(cx);
                rig.commands();
                rig.take_events();
                rig
            }

            fn apply(&self, cx: &mut TestAppContext, event: ClientEvent) {
                self.store.update(cx, |s, cx| s.apply(event, cx));
            }

            fn open(&self, cx: &mut TestAppContext) {
                self.store
                    .update(cx, |s, cx| s.open_reactors(chat(), TS, cx));
            }

            fn close(&self, cx: &mut TestAppContext) {
                self.store.update(cx, |s, cx| s.close_reactors(cx));
            }

            fn retry(&self, cx: &mut TestAppContext) {
                self.store.update(cx, |s, cx| s.retry_reactors(cx));
            }

            fn more(&self, cx: &mut TestAppContext) {
                self.store.update(cx, |s, cx| s.load_more_reactors(cx));
            }

            fn request(&self, cx: &mut TestAppContext, kind: RequestKind) {
                self.store.update(cx, |s, cx| s.request_reactors(kind, cx));
            }

            fn view(&self, cx: &mut TestAppContext) -> Option<ReactorsView> {
                self.store.read_with(cx, |s, _| s.reactors().cloned())
            }

            fn commands(&self) -> Vec<Command> {
                std::iter::from_fn(|| self.commands.try_recv().ok()).collect()
            }

            fn take_events(&self) -> Vec<StoreEvent> {
                std::mem::take(&mut *self.events.borrow_mut())
            }
        }

        fn chat() -> ChatId {
            ChatId("c".into())
        }

        fn reactor(guid: &str) -> Reactor {
            Reactor {
                user: UserId(guid.into()),
                name: Some(format!("name {guid}")),
                avatar: None,
                code: ReactionCode(1),
                at: Some(1),
                me: false,
            }
        }

        /// `n` guids `{prefix}0 ..`.
        fn guids(prefix: &str, n: usize) -> Vec<String> {
            (0..n).map(|i| format!("{prefix}{i}")).collect()
        }

        fn page_for(
            chat: ChatId,
            request: u64,
            who: &[String],
            raw: usize,
            oldest: Option<i64>,
        ) -> ReactorsPage {
            ReactorsPage {
                chat,
                timestamp: TS,
                request,
                reactors: who.iter().map(|g| reactor(g)).collect(),
                skipped: 0,
                raw,
                oldest,
            }
        }

        fn page(request: u64, who: &[String], raw: usize, oldest: Option<i64>) -> ClientEvent {
            ClientEvent::Reactors(page_for(chat(), request, who, raw, oldest))
        }

        /// A full first page (`REACTORS_PAGE` entries `a0 ..`) whose oldest entry is at 200.
        fn full(request: u64) -> ClientEvent {
            page(
                request,
                &guids("a", REACTORS_PAGE),
                REACTORS_PAGE,
                Some(200),
            )
        }

        fn failure(request: u64, reason: &str, offline: bool) -> ClientEvent {
            ClientEvent::ReactorsFailed {
                chat: chat(),
                timestamp: TS,
                request,
                reason: reason.into(),
                offline,
            }
        }

        fn load(before: Option<i64>, request: u64) -> Command {
            Command::LoadReactors {
                chat: chat(),
                timestamp: TS,
                before,
                request,
            }
        }

        fn rows(view: &ReactorsView) -> Vec<String> {
            match &view.state {
                ReactorsState::Ready { rows, .. } => {
                    rows.iter().map(|r| r.user.0.clone()).collect()
                }
                other => panic!("not ready: {other:?}"),
            }
        }

        /// `(exhausted, loading_more, more_failed)` of a ready list.
        fn flags(view: &ReactorsView) -> (bool, bool, Option<String>) {
            match &view.state {
                ReactorsState::Ready {
                    exhausted,
                    loading_more,
                    more_failed,
                    ..
                } => (*exhausted, *loading_more, more_failed.clone()),
                other => panic!("not ready: {other:?}"),
            }
        }

        fn with_reactions(cx: &mut TestAppContext, rig: &Rig, count: u32, version: i64) {
            let mut message = msg(TS, "a");
            message.reactions = Reactions {
                counts: vec![ReactionCount {
                    code: ReactionCode(1),
                    count,
                }],
                mine: None,
                version: Some(version),
            };
            rig.apply(
                cx,
                ClientEvent::History(HistoryChunk {
                    chat: chat(),
                    messages: vec![message],
                    reached_start: true,
                    replaces: None,
                    payloads: vec![],
                }),
            );
            rig.take_events();
        }

        #[gpui::test]
        fn opening_sends_the_first_command_with_a_fresh_token_and_is_loading(
            cx: &mut TestAppContext,
        ) {
            let rig = Rig::new(cx);
            assert_eq!(rig.view(cx), None);

            rig.open(cx);
            let view = rig.view(cx).unwrap();
            assert_eq!(
                (view.chat, view.timestamp, view.generation, view.kind),
                (chat(), TS, 1, RequestKind::First)
            );
            assert_eq!(view.state, ReactorsState::Loading);
            assert_eq!(view.base_version, None, "no timeline, no version");
            assert_eq!(rig.commands(), [load(None, 1)]);
            assert_eq!(rig.take_events(), [StoreEvent::Reactors]);
        }

        #[gpui::test]
        fn the_base_version_is_the_messages_current_one(cx: &mut TestAppContext) {
            let rig = Rig::new(cx);
            with_reactions(cx, &rig, 2, 4);
            rig.open(cx);
            assert_eq!(rig.view(cx).unwrap().base_version, Some(4));
        }

        #[gpui::test]
        fn a_page_becomes_ready_and_asks_for_the_avatars(cx: &mut TestAppContext) {
            let rig = Rig::opened(cx);
            let mut with_avatar = page_for(chat(), 1, &guids("a", 2), 2, Some(100));
            with_avatar.reactors[0].avatar = Some(MediaRef {
                id: "av".into(),
                kind: MediaKind::Avatar,
            });
            with_avatar.skipped = 1;
            rig.apply(cx, ClientEvent::Reactors(with_avatar));

            let view = rig.view(cx).unwrap();
            assert_eq!(rows(&view), ["a0", "a1"]);
            assert!(matches!(
                view.state,
                ReactorsState::Ready {
                    skipped: 1,
                    oldest: Some(100),
                    ..
                }
            ));
            assert_eq!(flags(&view), (true, false, None));
            assert_eq!(rig.take_events(), [StoreEvent::Reactors]);
            assert_eq!(
                rig.commands(),
                [Command::FetchMedia(MediaRef {
                    id: "av".into(),
                    kind: MediaKind::Avatar
                })]
            );
        }

        #[gpui::test]
        fn a_second_page_appends_in_server_order_and_drops_a_repeated_guid(
            cx: &mut TestAppContext,
        ) {
            let rig = Rig::opened(cx);
            rig.apply(cx, full(1));
            assert_eq!(flags(&rig.view(cx).unwrap()), (false, false, None));
            rig.take_events();

            rig.more(cx);
            let view = rig.view(cx).unwrap();
            assert_eq!((view.generation, view.kind), (2, RequestKind::More));
            assert_eq!(flags(&view), (false, true, None));
            assert_eq!(rig.commands(), [load(Some(200), 2)]);
            assert_eq!(rig.take_events(), [StoreEvent::Reactors]);

            let last = format!("a{}", REACTORS_PAGE - 1);
            rig.apply(cx, page(2, &[last, "x".into(), "y".into()], 3, Some(150)));
            let view = rig.view(cx).unwrap();
            let mut expected = guids("a", REACTORS_PAGE);
            expected.extend(["x".to_string(), "y".to_string()]);
            assert_eq!(
                rows(&view),
                expected,
                "the repeated boundary entry is dropped"
            );
            assert_eq!(flags(&view), (true, false, None), "a short page is the end");
            assert!(matches!(
                view.state,
                ReactorsState::Ready {
                    oldest: Some(150),
                    ..
                }
            ));
            assert_eq!(rig.take_events(), [StoreEvent::Reactors]);
        }

        #[gpui::test]
        fn a_failure_is_failed_with_its_offline_flag(cx: &mut TestAppContext) {
            let rig = Rig::opened(cx);
            rig.apply(cx, failure(1, "нет прав", false));
            assert_eq!(
                rig.view(cx).unwrap().state,
                ReactorsState::Failed {
                    reason: "нет прав".into(),
                    offline: false
                }
            );
            assert_eq!(rig.take_events(), [StoreEvent::Reactors]);

            rig.retry(cx);
            rig.apply(cx, failure(2, "Нет соединения", true));
            assert_eq!(
                rig.view(cx).unwrap().state,
                ReactorsState::Failed {
                    reason: "Нет соединения".into(),
                    offline: true
                }
            );
        }

        #[gpui::test]
        fn an_answer_for_a_closed_or_different_list_is_ignored(cx: &mut TestAppContext) {
            let rig = Rig::new(cx);
            rig.apply(cx, page(1, &guids("a", 1), 1, Some(1)));
            rig.apply(cx, failure(1, "x", false));
            assert_eq!(rig.view(cx), None);
            assert!(rig.take_events().is_empty());

            rig.open(cx);
            rig.take_events();
            let other_chat = page_for(ChatId("d".into()), 1, &guids("a", 1), 1, Some(1));
            let mut other_message = page_for(chat(), 1, &guids("a", 1), 1, Some(1));
            other_message.timestamp = TS + 1;
            rig.apply(cx, ClientEvent::Reactors(other_chat));
            rig.apply(cx, ClientEvent::Reactors(other_message));
            rig.apply(
                cx,
                ClientEvent::ReactorsFailed {
                    chat: chat(),
                    timestamp: TS + 1,
                    request: 1,
                    reason: "x".into(),
                    offline: false,
                },
            );
            rig.apply(cx, page(9, &guids("a", 1), 1, Some(1)));
            assert_eq!(rig.view(cx).unwrap().state, ReactorsState::Loading);
            assert!(rig.take_events().is_empty());
        }

        #[gpui::test]
        fn closing_drops_the_state_and_a_late_answer(cx: &mut TestAppContext) {
            let rig = Rig::opened(cx);
            rig.close(cx);
            assert_eq!(rig.view(cx), None);
            assert_eq!(rig.take_events(), [StoreEvent::Reactors]);

            rig.apply(cx, page(1, &guids("a", 1), 1, Some(1)));
            assert_eq!(rig.view(cx), None);
            assert!(rig.take_events().is_empty());

            rig.close(cx);
            assert!(
                rig.take_events().is_empty(),
                "nothing open, nothing announced"
            );
            assert!(rig.commands().is_empty(), "closing sends nothing");
        }

        #[gpui::test]
        fn reopening_the_same_message_drops_the_first_openings_answers(cx: &mut TestAppContext) {
            for failing in [false, true] {
                let rig = Rig::opened(cx);
                rig.close(cx);
                rig.open(cx);
                assert_eq!(rig.commands(), [load(None, 2)]);
                rig.take_events();

                if failing {
                    rig.apply(cx, failure(1, "старый", false));
                } else {
                    rig.apply(cx, page(1, &guids("old", 3), 3, Some(1)));
                }
                assert_eq!(rig.view(cx).unwrap().state, ReactorsState::Loading);
                assert!(rig.take_events().is_empty());

                rig.apply(cx, page(2, &guids("new", 2), 2, Some(2)));
                assert_eq!(rows(&rig.view(cx).unwrap()), ["new0", "new1"]);
            }
        }

        #[gpui::test]
        fn a_refresh_supersedes_a_page_in_flight_and_the_late_page_changes_nothing(
            cx: &mut TestAppContext,
        ) {
            let rig = Rig::opened(cx);
            rig.apply(cx, full(1));
            rig.more(cx);
            rig.commands();
            rig.take_events();

            rig.request(cx, RequestKind::First);
            assert_eq!(rig.commands(), [load(None, 3)]);
            let view = rig.view(cx).unwrap();
            assert_eq!((view.generation, view.kind), (3, RequestKind::First));
            assert_eq!(
                rows(&view),
                guids("a", REACTORS_PAGE),
                "rows stay until the answer"
            );
            rig.take_events();

            let late_page = page(2, &guids("late", 3), 3, Some(100));
            rig.apply(cx, late_page.clone());
            rig.apply(cx, failure(2, "x", false));
            assert_eq!(rows(&rig.view(cx).unwrap()), guids("a", REACTORS_PAGE));
            assert!(rig.take_events().is_empty());

            rig.apply(cx, page(3, &guids("n", 2), 2, Some(900)));
            let view = rig.view(cx).unwrap();
            assert_eq!(rows(&view), ["n0", "n1"], "the answer replaces the rows");
            assert_eq!(flags(&view), (true, false, None), "exhausted is recomputed");

            rig.apply(cx, late_page);
            rig.apply(cx, failure(2, "x", false));
            assert_eq!(
                rows(&rig.view(cx).unwrap()),
                ["n0", "n1"],
                "also after the refresh's answer"
            );
        }

        #[gpui::test]
        fn a_failed_refresh_is_failed_and_not_the_old_list(cx: &mut TestAppContext) {
            let rig = Rig::opened(cx);
            rig.apply(cx, full(1));
            rig.request(cx, RequestKind::First);
            rig.apply(cx, failure(2, "x", true));
            assert_eq!(
                rig.view(cx).unwrap().state,
                ReactorsState::Failed {
                    reason: "x".into(),
                    offline: true
                }
            );
        }

        #[gpui::test]
        fn a_retry_takes_a_fresh_token_and_the_failed_requests_answer_is_dropped(
            cx: &mut TestAppContext,
        ) {
            let rig = Rig::opened(cx);
            rig.apply(cx, failure(1, "x", false));
            rig.take_events();

            rig.retry(cx);
            assert_eq!(rig.view(cx).unwrap().state, ReactorsState::Loading);
            assert_eq!(rig.commands(), [load(None, 2)]);
            assert_eq!(rig.take_events(), [StoreEvent::Reactors]);

            rig.apply(cx, page(1, &guids("old", 1), 1, Some(1)));
            rig.apply(cx, failure(1, "again", false));
            assert_eq!(rig.view(cx).unwrap().state, ReactorsState::Loading);
            assert!(rig.take_events().is_empty());

            rig.retry(cx);
            assert!(
                rig.commands().is_empty(),
                "a loading list has its request out"
            );
        }

        #[gpui::test]
        fn a_retry_of_a_shown_list_asks_for_the_first_page_and_keeps_the_rows(
            cx: &mut TestAppContext,
        ) {
            let rig = Rig::opened(cx);
            rig.apply(cx, full(1));
            rig.retry(cx);
            assert_eq!(rig.commands(), [load(None, 2)]);
            assert_eq!(rows(&rig.view(cx).unwrap()), guids("a", REACTORS_PAGE));
        }

        #[gpui::test]
        fn the_token_counter_does_not_restart_on_close(cx: &mut TestAppContext) {
            let rig = Rig::new(cx);
            rig.open(cx);
            rig.close(cx);
            rig.open(cx);
            rig.apply(cx, ClientEvent::SignedOut);
            rig.open(cx);
            assert_eq!(
                rig.commands(),
                [load(None, 1), load(None, 2), load(None, 3)]
            );
        }

        #[test]
        fn the_end_of_the_list_follows_the_page_and_the_cursor() {
            let l = REACTORS_PAGE;
            assert!(page_exhausted(None, l - 1, Some(10), l), "a short page");
            assert!(page_exhausted(None, 0, None, l), "an empty page");
            assert!(
                page_exhausted(None, l, None, l),
                "no timestamp to continue from"
            );
            assert!(
                !page_exhausted(None, l, Some(10), l),
                "a full page that progresses"
            );
            assert!(
                !page_exhausted(Some(100), l, Some(99), l),
                "the cursor moved"
            );
            assert!(
                page_exhausted(Some(100), l, Some(100), l),
                "a repeated boundary"
            );
            assert!(
                page_exhausted(Some(100), l, Some(120), l),
                "a page newer than the cursor"
            );
        }

        #[gpui::test]
        fn a_page_that_does_not_pass_the_cursor_ends_the_list(cx: &mut TestAppContext) {
            let rig = Rig::opened(cx);
            rig.apply(cx, full(1));
            rig.more(cx);
            rig.apply(
                cx,
                page(2, &guids("a", REACTORS_PAGE), REACTORS_PAGE, Some(200)),
            );
            let view = rig.view(cx).unwrap();
            assert_eq!(flags(&view), (true, false, None));
            assert_eq!(
                rows(&view).len(),
                REACTORS_PAGE,
                "all repeated, nothing appended"
            );

            rig.commands();
            rig.more(cx);
            assert!(rig.commands().is_empty(), "no further page is asked");
        }

        #[gpui::test]
        fn known_counts_neither_stop_nor_extend_paging(cx: &mut TestAppContext) {
            // A count below the loaded rows does not stop paging.
            let rig = Rig::new(cx);
            with_reactions(cx, &rig, 1, 1);
            rig.open(cx);
            rig.apply(cx, full(1));
            assert_eq!(flags(&rig.view(cx).unwrap()), (false, false, None));
            rig.commands();
            rig.more(cx);
            assert_eq!(rig.commands(), [load(Some(200), 2)]);

            // A count above all the rows does not keep it going after the end.
            let rig = Rig::new(cx);
            with_reactions(cx, &rig, 100, 1);
            rig.open(cx);
            rig.apply(cx, page(1, &guids("a", 3), 3, Some(10)));
            assert_eq!(flags(&rig.view(cx).unwrap()), (true, false, None));
            rig.commands();
            rig.more(cx);
            assert!(rig.commands().is_empty());
        }

        #[gpui::test]
        fn a_failed_next_page_keeps_the_rows_and_a_retry_asks_for_it_again(
            cx: &mut TestAppContext,
        ) {
            let rig = Rig::opened(cx);
            rig.apply(cx, ClientEvent::Connection(ConnectionState::Online));
            rig.apply(cx, full(1));
            rig.more(cx);
            rig.commands();
            rig.take_events();

            rig.apply(cx, failure(2, "сбой", false));
            let view = rig.view(cx).unwrap();
            assert_eq!(rows(&view), guids("a", REACTORS_PAGE));
            assert_eq!(flags(&view), (false, false, Some("сбой".into())));
            assert_eq!(rig.take_events(), [StoreEvent::Reactors]);

            rig.retry(cx);
            let view = rig.view(cx).unwrap();
            assert_eq!((view.generation, view.kind), (3, RequestKind::More));
            assert_eq!(flags(&view), (false, true, None));
            assert_eq!(rig.commands(), [load(Some(200), 3)]);

            rig.apply(cx, failure(2, "старый", false));
            assert_eq!(flags(&rig.view(cx).unwrap()), (false, true, None));

            rig.apply(cx, page(3, &guids("b", 2), 2, Some(100)));
            assert_eq!(rows(&rig.view(cx).unwrap()).len(), REACTORS_PAGE + 2);
        }

        #[gpui::test]
        fn an_offline_retry_of_a_list_with_a_failed_page_asks_for_the_first_page(
            cx: &mut TestAppContext,
        ) {
            for state in [ConnectionState::Offline, ConnectionState::Connecting] {
                let rig = Rig::opened(cx);
                rig.apply(cx, full(1));
                rig.more(cx);
                rig.apply(cx, failure(2, "сбой", false));
                rig.apply(cx, ClientEvent::Connection(state));
                rig.commands();
                rig.take_events();

                rig.retry(cx);
                let view = rig.view(cx).unwrap();
                assert_eq!((view.generation, view.kind), (3, RequestKind::First));
                assert_eq!(rig.commands(), [load(None, 3)]);
                assert_eq!(
                    rows(&view),
                    guids("a", REACTORS_PAGE),
                    "rows stay until the answer"
                );

                rig.apply(cx, page(3, &guids("n", 2), 2, Some(900)));
                let view = rig.view(cx).unwrap();
                assert_eq!(rows(&view), ["n0", "n1"]);
                assert_eq!(flags(&view), (true, false, None));
            }
        }

        #[gpui::test]
        fn only_one_next_page_is_out_at_a_time(cx: &mut TestAppContext) {
            let rig = Rig::opened(cx);
            rig.apply(cx, full(1));
            rig.more(cx);
            rig.commands();
            rig.more(cx);
            assert!(rig.commands().is_empty());
            assert_eq!(rig.view(cx).unwrap().generation, 2);
        }

        #[gpui::test]
        fn choosing_more_asks_for_the_page_after_the_oldest_and_appends_it(
            cx: &mut TestAppContext,
        ) {
            let rig = Rig::opened(cx);
            rig.apply(cx, full(1));
            rig.take_events();

            rig.more(cx);
            assert_eq!(rig.commands(), [load(Some(200), 2)]);
            let view = rig.view(cx).unwrap();
            assert_eq!((view.generation, view.kind), (2, RequestKind::More));
            assert_eq!(flags(&view), (false, true, None), "loading the next page");
            assert_eq!(rig.take_events(), [StoreEvent::Reactors]);

            rig.apply(cx, page(2, &guids("b", 2), 2, Some(100)));
            let view = rig.view(cx).unwrap();
            let mut expected = guids("a", REACTORS_PAGE);
            expected.extend(guids("b", 2));
            assert_eq!(rows(&view), expected);
            assert_eq!(flags(&view), (true, false, None));

            // A full second page that progresses keeps the row, and the next choice asks again
            // from the new oldest entry with a fresh token.
            let rig = Rig::opened(cx);
            rig.apply(cx, full(1));
            rig.more(cx);
            rig.apply(
                cx,
                page(2, &guids("b", REACTORS_PAGE), REACTORS_PAGE, Some(100)),
            );
            assert_eq!(flags(&rig.view(cx).unwrap()), (false, false, None));
            rig.commands();
            rig.more(cx);
            assert_eq!(rig.commands(), [load(Some(100), 3)]);
        }

        #[gpui::test]
        fn choosing_more_sends_nothing_unless_a_next_page_can_be_asked(cx: &mut TestAppContext) {
            // No list open.
            let rig = Rig::new(cx);
            rig.more(cx);
            assert!(rig.commands().is_empty());
            assert!(rig.take_events().is_empty());

            // Loading, and failed.
            let rig = Rig::opened(cx);
            rig.more(cx);
            assert!(rig.commands().is_empty(), "the first page is still out");
            rig.apply(cx, failure(1, "x", false));
            rig.take_events();
            rig.more(cx);
            assert!(rig.commands().is_empty());
            assert!(rig.take_events().is_empty());
            assert_eq!(rig.view(cx).unwrap().generation, 1);

            // A short page is the end.
            let rig = Rig::opened(cx);
            rig.apply(cx, page(1, &guids("a", 3), 3, Some(10)));
            rig.take_events();
            rig.more(cx);
            assert!(rig.commands().is_empty());
            assert!(rig.take_events().is_empty());
        }

        #[gpui::test]
        fn a_failed_next_page_does_not_change_the_loaded_rows_or_the_end_of_the_list(
            cx: &mut TestAppContext,
        ) {
            let rig = Rig::opened(cx);
            rig.apply(cx, full(1));
            rig.more(cx);
            rig.apply(cx, failure(2, "сбой", true));
            let view = rig.view(cx).unwrap();
            assert_eq!(rows(&view), guids("a", REACTORS_PAGE));
            assert!(matches!(
                view.state,
                ReactorsState::Ready {
                    oldest: Some(200),
                    exhausted: false,
                    ..
                }
            ));
            assert_eq!(flags(&view), (false, false, Some("сбой".into())));
        }

        #[gpui::test]
        fn signing_out_closes_the_list(cx: &mut TestAppContext) {
            let rig = Rig::opened(cx);
            rig.apply(cx, full(1));
            rig.apply(cx, ClientEvent::SignedOut);
            assert_eq!(rig.view(cx), None);
        }

        /// A live change of the open message's reactions to `version`.
        fn changed_at(ts: MessageTs, version: i64) -> ClientEvent {
            ClientEvent::ReactionsChanged {
                chat: chat(),
                timestamp: ts,
                reactions: Reactions {
                    counts: vec![ReactionCount {
                        code: ReactionCode(1),
                        count: version as u32,
                    }],
                    mine: None,
                    version: Some(version),
                },
            }
        }

        fn changed(version: i64) -> ClientEvent {
            changed_at(TS, version)
        }

        fn deleted(ts: MessageTs) -> ClientEvent {
            ClientEvent::MessageDeleted {
                chat: chat(),
                timestamp: ts,
            }
        }

        /// Lets `millis` of the test clock pass and runs what became due.
        fn wait(cx: &mut TestAppContext, millis: u64) {
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(millis));
            cx.run_until_parked();
        }

        /// A rig whose message `TS` is loaded at reaction version 1 and whose list is open
        /// (token 1); the open's command and events are taken.
        fn live(cx: &mut TestAppContext) -> Rig {
            let rig = Rig::new(cx);
            with_reactions(cx, &rig, 1, 1);
            rig.open(cx);
            rig.commands();
            rig.take_events();
            rig
        }

        #[test]
        fn only_the_latest_ticket_fires_and_only_once() {
            let mut schedule = RefreshSchedule::default();
            let first = schedule.schedule();
            let second = schedule.schedule();
            assert!(second > first);
            assert!(!schedule.fire(first), "superseded by the second");
            assert!(schedule.fire(second));
            assert!(!schedule.fire(second), "fired already");

            let third = schedule.schedule();
            schedule.cancel();
            assert!(!schedule.fire(third), "cancelled");
            assert!(!schedule.fire(0), "nothing was scheduled with this ticket");
        }

        #[gpui::test]
        fn a_burst_of_changes_gives_one_read_after_the_delay(cx: &mut TestAppContext) {
            let rig = live(cx);
            rig.apply(cx, full(1));
            rig.apply(cx, changed(2));
            wait(cx, 600);
            rig.apply(cx, changed(3));
            wait(cx, 600);
            assert!(
                rig.commands().is_empty(),
                "the delay restarted with the second change"
            );

            wait(cx, 400);
            assert_eq!(rig.commands(), [load(None, 2)]);
            let view = rig.view(cx).unwrap();
            assert_eq!(
                (view.generation, view.kind, view.base_version),
                (2, RequestKind::First, Some(3))
            );
            assert_eq!(
                rows(&view),
                guids("a", REACTORS_PAGE),
                "the rows stay until the answer"
            );

            wait(cx, 5000);
            assert!(rig.commands().is_empty(), "one read for the whole burst");
        }

        #[gpui::test]
        fn no_refresh_for_an_unchanged_version_another_message_or_a_gone_message(
            cx: &mut TestAppContext,
        ) {
            let rig = live(cx);
            rig.apply(cx, full(1));
            rig.apply(cx, changed(1));
            rig.apply(cx, changed_at(TS + 1, 2));
            wait(cx, 5000);
            assert!(rig.commands().is_empty());

            let rig = live(cx);
            rig.apply(cx, deleted(TS));
            rig.apply(cx, changed(2));
            wait(cx, 5000);
            assert!(rig.commands().is_empty(), "a gone message is terminal");
        }

        #[gpui::test]
        fn a_change_before_the_first_answer_is_refreshed(cx: &mut TestAppContext) {
            // That answer may predate the change.
            let rig = live(cx);
            rig.apply(cx, changed(2));
            wait(cx, 1000);
            assert_eq!(rig.commands(), [load(None, 2)]);
        }

        #[gpui::test]
        fn a_failure_during_the_delay_does_not_lose_the_refresh(cx: &mut TestAppContext) {
            for offline in [false, true] {
                let rig = live(cx);
                rig.apply(cx, changed(2));
                rig.apply(cx, failure(1, "x", offline));
                assert!(matches!(
                    rig.view(cx).unwrap().state,
                    ReactorsState::Failed { .. }
                ));
                assert!(rig.commands().is_empty());

                wait(cx, 1000);
                assert_eq!(rig.commands(), [load(None, 2)]);
                let view = rig.view(cx).unwrap();
                assert_eq!(view.state, ReactorsState::Loading);
                assert_eq!(view.base_version, Some(2));

                rig.apply(cx, page(2, &guids("n", 2), 2, Some(900)));
                assert_eq!(rows(&rig.view(cx).unwrap()), ["n0", "n1"]);
            }
        }

        #[gpui::test]
        fn a_change_while_the_list_is_failed_refreshes_it(cx: &mut TestAppContext) {
            let rig = live(cx);
            rig.apply(cx, failure(1, "x", false));
            rig.apply(cx, changed(2));
            wait(cx, 1000);
            assert_eq!(rig.commands(), [load(None, 2)]);
            assert_eq!(rig.view(cx).unwrap().state, ReactorsState::Loading);
        }

        #[gpui::test]
        fn a_failed_refresh_is_refreshed_again_by_a_later_change(cx: &mut TestAppContext) {
            let rig = live(cx);
            rig.apply(cx, full(1));
            rig.apply(cx, changed(2));
            wait(cx, 1000);
            rig.commands();
            rig.apply(cx, failure(2, "x", false));
            rig.apply(cx, changed(3));
            wait(cx, 1000);
            assert_eq!(rig.commands(), [load(None, 3)]);
        }

        #[gpui::test]
        fn signing_out_with_a_refresh_waiting_sends_nothing(cx: &mut TestAppContext) {
            let rig = live(cx);
            rig.apply(cx, full(1));
            rig.apply(cx, changed(2));
            rig.apply(cx, ClientEvent::SignedOut);
            assert_eq!(rig.view(cx), None);
            wait(cx, 5000);
            assert!(rig.commands().is_empty(), "the timer was cancelled");
        }

        #[gpui::test]
        fn closing_or_reopening_cancels_the_refresh(cx: &mut TestAppContext) {
            let rig = live(cx);
            rig.apply(cx, full(1));
            rig.apply(cx, changed(2));
            rig.close(cx);
            wait(cx, 5000);
            assert!(rig.commands().is_empty(), "closed before the delay ran out");

            rig.open(cx);
            assert_eq!(rig.commands(), [load(None, 2)]);
            rig.apply(cx, changed(3));
            rig.close(cx);
            rig.open(cx);
            assert_eq!(rig.commands(), [load(None, 3)]);
            wait(cx, 5000);
            assert!(
                rig.commands().is_empty(),
                "the timer of the closed list does nothing"
            );
        }

        #[gpui::test]
        fn a_refresh_records_the_version_it_read(cx: &mut TestAppContext) {
            let rig = live(cx);
            rig.apply(cx, full(1));
            rig.apply(cx, changed(2));
            wait(cx, 1000);
            assert_eq!(rig.commands(), [load(None, 2)]);
            assert_eq!(rig.view(cx).unwrap().base_version, Some(2));

            rig.apply(cx, changed(2));
            wait(cx, 5000);
            assert!(
                rig.commands().is_empty(),
                "the list was read at this version"
            );

            rig.apply(cx, changed(3));
            wait(cx, 1000);
            assert_eq!(rig.commands(), [load(None, 3)]);
        }

        #[gpui::test]
        fn a_refresh_supersedes_a_next_page_and_the_late_page_is_dropped(cx: &mut TestAppContext) {
            let rig = live(cx);
            rig.apply(cx, full(1));
            rig.more(cx);
            assert_eq!(rig.commands(), [load(Some(200), 2)]);
            rig.apply(cx, changed(2));
            wait(cx, 1000);
            assert_eq!(rig.commands(), [load(None, 3)]);
            let view = rig.view(cx).unwrap();
            assert_eq!((view.generation, view.kind), (3, RequestKind::First));
            assert_eq!(rows(&view), guids("a", REACTORS_PAGE));

            rig.take_events();
            let late = page(2, &guids("late", 3), 3, Some(100));
            rig.apply(cx, late.clone());
            rig.apply(cx, failure(2, "x", false));
            assert_eq!(rows(&rig.view(cx).unwrap()), guids("a", REACTORS_PAGE));
            assert!(rig.take_events().is_empty());

            rig.more(cx);
            assert!(
                rig.commands().is_empty(),
                "no next page is asked from the old rows while the refresh is out"
            );

            rig.apply(cx, page(3, &guids("n", 2), 2, Some(900)));
            let view = rig.view(cx).unwrap();
            assert_eq!(rows(&view), ["n0", "n1"]);
            assert_eq!(flags(&view), (true, false, None));

            rig.apply(cx, late);
            rig.apply(cx, failure(2, "x", false));
            assert_eq!(rows(&rig.view(cx).unwrap()), ["n0", "n1"]);
        }

        #[gpui::test]
        fn a_refresh_of_a_deep_list_shows_the_first_page_and_paging_continues_from_it(
            cx: &mut TestAppContext,
        ) {
            let rig = live(cx);
            rig.apply(cx, full(1));
            rig.more(cx);
            rig.apply(
                cx,
                page(2, &guids("b", REACTORS_PAGE), REACTORS_PAGE, Some(100)),
            );
            assert_eq!(rows(&rig.view(cx).unwrap()).len(), 2 * REACTORS_PAGE);
            rig.commands();

            rig.apply(cx, changed(2));
            wait(cx, 1000);
            assert_eq!(rig.commands(), [load(None, 3)]);
            assert_eq!(
                rows(&rig.view(cx).unwrap()).len(),
                2 * REACTORS_PAGE,
                "the deep list stays until the answer"
            );

            rig.apply(cx, full(3));
            let view = rig.view(cx).unwrap();
            assert_eq!(rows(&view), guids("a", REACTORS_PAGE));
            assert_eq!(flags(&view), (false, false, None));
            assert!(matches!(
                view.state,
                ReactorsState::Ready {
                    oldest: Some(200),
                    ..
                }
            ));
            rig.more(cx);
            assert_eq!(rig.commands(), [load(Some(200), 4)]);
        }

        #[gpui::test]
        fn a_failed_refresh_is_failed_and_not_the_old_list_with_a_retry(cx: &mut TestAppContext) {
            let rig = live(cx);
            rig.apply(cx, full(1));
            rig.apply(cx, changed(2));
            wait(cx, 1000);
            rig.commands();
            rig.apply(cx, failure(2, "сбой", false));
            assert_eq!(
                rig.view(cx).unwrap().state,
                ReactorsState::Failed {
                    reason: "сбой".into(),
                    offline: false
                }
            );

            rig.retry(cx);
            assert_eq!(rig.commands(), [load(None, 3)]);
            assert_eq!(rig.view(cx).unwrap().state, ReactorsState::Loading);
        }

        #[gpui::test]
        fn a_retry_takes_over_the_refresh_that_is_waiting(cx: &mut TestAppContext) {
            let rig = live(cx);
            rig.apply(cx, full(1));
            rig.apply(cx, changed(2));
            rig.retry(cx);
            assert_eq!(rig.commands(), [load(None, 2)]);
            assert_eq!(rig.view(cx).unwrap().base_version, Some(2));
            wait(cx, 5000);
            assert!(rig.commands().is_empty(), "one read, not two");
        }

        #[gpui::test]
        fn a_deletion_says_the_message_is_gone_and_cancels_the_refresh(cx: &mut TestAppContext) {
            for ready in [false, true] {
                let rig = live(cx);
                if ready {
                    rig.apply(cx, full(1));
                }
                rig.apply(cx, changed(2));
                rig.apply(cx, deleted(TS + 1));
                assert_ne!(
                    rig.view(cx).unwrap().state,
                    ReactorsState::Failed {
                        reason: MESSAGE_GONE.into(),
                        offline: false
                    },
                    "another message was deleted"
                );
                rig.take_events();

                rig.apply(cx, deleted(TS));
                let gone = ReactorsState::Failed {
                    reason: MESSAGE_GONE.into(),
                    offline: false,
                };
                assert_eq!(rig.view(cx).unwrap().state, gone);
                assert!(rig.take_events().contains(&StoreEvent::Reactors));

                wait(cx, 5000);
                assert!(rig.commands().is_empty(), "the timer was cancelled");

                rig.apply(cx, page(1, &guids("late", 2), 2, Some(1)));
                rig.apply(cx, failure(1, "x", false));
                assert_eq!(
                    rig.view(cx).unwrap().state,
                    gone,
                    "the answer the list was waiting for is dropped"
                );
            }
        }
    }

    mod creation {
        use super::*;
        use gpui::{AppContext, Entity, Subscription, TestAppContext};
        use std::cell::RefCell;
        use std::rc::Rc;

        struct Rig {
            store: Entity<ChatStore>,
            commands: Receiver<Command>,
            events: Rc<RefCell<Vec<StoreEvent>>>,
            _client: Sender<ClientEvent>,
            _subscription: Subscription,
        }

        impl Rig {
            fn new(cx: &mut TestAppContext) -> Self {
                let (client, event_rx) = async_channel::unbounded();
                let (command_tx, commands) = async_channel::unbounded();
                let store = cx.new(|cx| ChatStore::new(event_rx, command_tx, cx));
                let events = Rc::new(RefCell::new(Vec::new()));
                let log = events.clone();
                let subscription = cx.update(|cx| {
                    cx.subscribe(&store, move |_, event: &StoreEvent, _| {
                        log.borrow_mut().push(event.clone())
                    })
                });
                Self {
                    store,
                    commands,
                    events,
                    _client: client,
                    _subscription: subscription,
                }
            }

            fn create(&self, cx: &mut TestAppContext, user: &str) {
                self.store
                    .update(cx, |s, cx| s.create_private_chat(UserId(user.into()), cx));
            }

            fn dismiss(&self, cx: &mut TestAppContext) {
                self.store.update(cx, |s, cx| s.dismiss_creation(cx));
            }

            fn select(&self, cx: &mut TestAppContext, tag: &str) {
                self.store.update(cx, |s, cx| s.select(chat(tag), cx));
            }

            fn apply(&self, cx: &mut TestAppContext, event: ClientEvent) {
                self.store.update(cx, |s, cx| s.apply(event, cx));
            }

            fn list(&self, cx: &mut TestAppContext, tags: &[&str]) {
                self.apply(
                    cx,
                    ClientEvent::ChatsReplaced(tags.iter().map(|t| row(t)).collect()),
                );
            }

            fn state(&self, cx: &mut TestAppContext) -> Option<CreationState> {
                self.store.read_with(cx, |s, _| s.creation_state().cloned())
            }

            fn selected(&self, cx: &mut TestAppContext) -> Option<ChatId> {
                self.store.read_with(cx, |s, _| s.selected().cloned())
            }

            fn commands(&self) -> Vec<Command> {
                std::iter::from_fn(|| self.commands.try_recv().ok()).collect()
            }

            fn take_events(&self) -> Vec<StoreEvent> {
                std::mem::take(&mut *self.events.borrow_mut())
            }
        }

        fn chat(tag: &str) -> ChatId {
            ChatId(tag.into())
        }

        fn row(tag: &str) -> ChatSummary {
            ChatSummary {
                id: chat(tag),
                kind: ChatKind::Private,
                title: format!("chat {tag}"),
                avatar: None,
                last_message: None,
                unread: 0,
                muted: false,
                pinned_rank: None,
                partner: Some(UserId(format!("u-{tag}"))),
                pinned_message: None,
                pin_action_ts: None,
            }
        }

        fn created(tag: &str, existing: bool) -> ClientEvent {
            ClientEvent::ChatCreated {
                chat: Some(chat(tag)),
                existing,
            }
        }

        fn user(tag: &str) -> UserId {
            UserId(tag.into())
        }

        #[gpui::test]
        fn starting_goes_in_flight_sends_one_command_and_ignores_a_second_press(
            cx: &mut TestAppContext,
        ) {
            let rig = Rig::new(cx);
            assert_eq!(rig.state(cx), None);
            rig.create(cx, "u9");
            assert_eq!(rig.state(cx), Some(CreationState::InFlight));
            assert_eq!(
                rig.commands(),
                [Command::CreatePrivateChat { user: user("u9") }]
            );
            assert_eq!(rig.take_events(), [StoreEvent::Creation]);

            rig.create(cx, "u9");
            rig.create(cx, "u8");
            assert!(rig.commands().is_empty(), "nothing is sent while one runs");
            assert!(rig.take_events().is_empty());
        }

        #[gpui::test]
        fn a_created_chat_that_is_in_the_list_is_selected_at_once(cx: &mut TestAppContext) {
            let rig = Rig::new(cx);
            rig.list(cx, &["a", "p"]);
            rig.create(cx, "u9");
            rig.commands();
            rig.take_events();

            rig.apply(cx, created("p", false));
            assert_eq!(rig.state(cx), None);
            assert_eq!(rig.selected(cx), Some(chat("p")));
            assert_eq!(rig.commands(), [Command::OpenChat(chat("p"))]);
            assert_eq!(
                rig.take_events(),
                [
                    StoreEvent::Creation,
                    StoreEvent::ChatCreated {
                        chat: Some(chat("p")),
                        existing: false
                    }
                ]
            );
        }

        #[gpui::test]
        fn a_created_chat_is_selected_once_it_appears_in_the_list(cx: &mut TestAppContext) {
            let rig = Rig::new(cx);
            rig.list(cx, &["a"]);
            rig.create(cx, "u9");
            rig.commands();

            rig.apply(cx, created("p", false));
            assert_eq!(rig.selected(cx), None, "not in the list yet");
            assert!(rig.commands().is_empty());

            rig.list(cx, &["a"]);
            assert_eq!(rig.selected(cx), None);

            rig.apply(cx, ClientEvent::ChatUpdated(row("p")));
            assert_eq!(rig.selected(cx), Some(chat("p")));
            assert_eq!(rig.commands(), [Command::OpenChat(chat("p"))]);

            rig.list(cx, &["a", "p"]);
            assert!(rig.commands().is_empty(), "it is opened once");
        }

        #[gpui::test]
        fn an_existing_chat_is_opened_the_same_way(cx: &mut TestAppContext) {
            let rig = Rig::new(cx);
            rig.list(cx, &["p"]);
            rig.create(cx, "u-p");
            rig.commands();
            rig.take_events();
            rig.apply(cx, created("p", true));
            assert_eq!(rig.selected(cx), Some(chat("p")));
            assert_eq!(
                rig.take_events(),
                [
                    StoreEvent::Creation,
                    StoreEvent::ChatCreated {
                        chat: Some(chat("p")),
                        existing: true
                    }
                ]
            );
        }

        #[gpui::test]
        fn a_created_chat_without_an_id_is_selected_once_the_list_shows_the_person(
            cx: &mut TestAppContext,
        ) {
            let rig = Rig::new(cx);
            rig.list(cx, &["a"]);
            rig.create(cx, "u-p");
            rig.commands();
            rig.take_events();
            rig.apply(
                cx,
                ClientEvent::ChatCreated {
                    chat: None,
                    existing: false,
                },
            );
            assert_eq!(rig.state(cx), None);
            assert_eq!(
                rig.selected(cx),
                None,
                "the list does not show the person yet"
            );
            assert!(rig.commands().is_empty());
            assert_eq!(
                rig.take_events(),
                [
                    StoreEvent::Creation,
                    StoreEvent::ChatCreated {
                        chat: None,
                        existing: false
                    }
                ]
            );

            rig.list(cx, &["a", "b"]);
            assert_eq!(
                rig.selected(cx),
                None,
                "other chats are not the created one"
            );

            rig.list(cx, &["a", "b", "p"]);
            assert_eq!(rig.selected(cx), Some(chat("p")));
            assert_eq!(rig.commands(), [Command::OpenChat(chat("p"))]);

            rig.list(cx, &["a", "b", "p", "q"]);
            assert!(rig.commands().is_empty(), "it is opened once");
        }

        #[gpui::test]
        fn the_wait_for_an_unnamed_created_chat_ends_with_any_choice_of_the_user(
            cx: &mut TestAppContext,
        ) {
            let rig = Rig::new(cx);
            rig.list(cx, &["a"]);
            rig.create(cx, "u-p");
            rig.apply(
                cx,
                ClientEvent::ChatCreated {
                    chat: None,
                    existing: false,
                },
            );
            rig.select(cx, "a");
            rig.commands();

            rig.list(cx, &["a", "p"]);
            assert_eq!(rig.selected(cx), Some(chat("a")), "the user's choice wins");
            assert!(rig.commands().is_empty());
        }

        #[gpui::test]
        fn an_unverified_creation_is_kept_until_dismissed_and_allows_another_attempt(
            cx: &mut TestAppContext,
        ) {
            let rig = Rig::new(cx);
            rig.create(cx, "u9");
            rig.commands();
            rig.take_events();

            rig.apply(
                cx,
                ClientEvent::CreationUnverified {
                    reason: "список не прочитан".into(),
                },
            );
            assert_eq!(
                rig.state(cx),
                Some(CreationState::Unverified {
                    reason: "список не прочитан".into()
                })
            );
            assert_eq!(rig.take_events(), [StoreEvent::Creation]);
            assert_eq!(rig.selected(cx), None);

            rig.create(cx, "u9");
            assert_eq!(
                rig.state(cx),
                Some(CreationState::InFlight),
                "the next attempt is the user's"
            );
            assert_eq!(
                rig.commands(),
                [Command::CreatePrivateChat { user: user("u9") }]
            );
            rig.apply(
                cx,
                ClientEvent::CreationUnverified {
                    reason: "список не прочитан".into(),
                },
            );
            rig.dismiss(cx);
            assert_eq!(rig.state(cx), None);
        }

        #[gpui::test]
        fn a_failure_is_kept_until_dismissed_and_another_attempt_is_allowed(
            cx: &mut TestAppContext,
        ) {
            let rig = Rig::new(cx);
            rig.create(cx, "u9");
            rig.commands();
            rig.take_events();

            rig.apply(
                cx,
                ClientEvent::CreationFailed {
                    reason: "нет прав".into(),
                    unknown: false,
                },
            );
            assert_eq!(
                rig.state(cx),
                Some(CreationState::Failed {
                    reason: "нет прав".into(),
                    unknown: false
                })
            );
            assert_eq!(rig.take_events(), [StoreEvent::Creation]);

            rig.dismiss(cx);
            assert_eq!(rig.state(cx), None);
            assert_eq!(rig.take_events(), [StoreEvent::Creation]);
            rig.dismiss(cx);
            assert!(rig.take_events().is_empty(), "nothing to dismiss");

            rig.create(cx, "u9");
            rig.apply(
                cx,
                ClientEvent::CreationFailed {
                    reason: "результат неизвестен".into(),
                    unknown: true,
                },
            );
            assert_eq!(
                rig.state(cx),
                Some(CreationState::Failed {
                    reason: "результат неизвестен".into(),
                    unknown: true
                })
            );
            rig.commands();
            rig.create(cx, "u9");
            assert_eq!(
                rig.state(cx),
                Some(CreationState::InFlight),
                "the next attempt is the user's"
            );
            assert_eq!(
                rig.commands(),
                [Command::CreatePrivateChat { user: user("u9") }]
            );
        }

        #[gpui::test]
        fn dismissing_never_recalls_a_creation_in_flight(cx: &mut TestAppContext) {
            let rig = Rig::new(cx);
            rig.create(cx, "u9");
            rig.take_events();
            rig.dismiss(cx);
            assert_eq!(rig.state(cx), Some(CreationState::InFlight));
            assert!(rig.take_events().is_empty());
        }

        #[gpui::test]
        fn an_outcome_without_a_creation_in_flight_changes_nothing(cx: &mut TestAppContext) {
            let rig = Rig::new(cx);
            rig.list(cx, &["p"]);
            rig.take_events();
            rig.apply(cx, created("p", false));
            rig.apply(
                cx,
                ClientEvent::CreationFailed {
                    reason: "x".into(),
                    unknown: false,
                },
            );
            assert_eq!(rig.state(cx), None);
            assert_eq!(rig.selected(cx), None);
            assert!(rig.take_events().is_empty());
        }

        #[gpui::test]
        fn choosing_another_chat_cancels_the_wait_for_the_created_one(cx: &mut TestAppContext) {
            let rig = Rig::new(cx);
            rig.list(cx, &["a"]);
            rig.create(cx, "u9");
            rig.apply(cx, created("p", false));
            rig.select(cx, "a");
            rig.commands();

            rig.apply(cx, ClientEvent::ChatUpdated(row("p")));
            assert_eq!(rig.selected(cx), Some(chat("a")), "the user's choice wins");
            assert!(rig.commands().is_empty());
        }

        #[gpui::test]
        fn signing_out_or_an_expired_session_forgets_the_creation(cx: &mut TestAppContext) {
            let rig = Rig::new(cx);
            rig.create(cx, "u9");
            rig.apply(cx, ClientEvent::SessionExpired);
            assert_eq!(rig.state(cx), None);

            rig.list(cx, &["a"]);
            rig.create(cx, "u9");
            rig.apply(cx, created("p", false));
            rig.apply(cx, ClientEvent::SignedOut);
            assert_eq!(rig.state(cx), None);
            rig.apply(cx, ClientEvent::ChatUpdated(row("p")));
            assert_eq!(
                rig.selected(cx),
                None,
                "a chat awaited before the sign out is not opened"
            );
        }
    }

    mod pins {
        use super::*;
        use bt_chat_core::model::{ChatKind, MessagePreview};
        use gpui::{AppContext, Entity, Subscription, TestAppContext};
        use std::cell::{Cell, RefCell};
        use std::rc::Rc;

        struct Rig {
            store: Entity<ChatStore>,
            events: Rc<RefCell<Vec<StoreEvent>>>,
            redraws: Rc<Cell<usize>>,
            _client: Sender<ClientEvent>,
            _commands: Receiver<Command>,
            _subscriptions: [Subscription; 2],
        }

        impl Rig {
            /// Chats `c` (open, history loaded) and `d` (listed only).
            fn new(cx: &mut TestAppContext) -> Self {
                let (client, event_rx) = async_channel::unbounded();
                let (command_tx, commands) = async_channel::unbounded();
                let store = cx.new(|cx| ChatStore::new(event_rx, command_tx, cx));
                let events = Rc::new(RefCell::new(Vec::new()));
                let redraws = Rc::new(Cell::new(0));
                let log = events.clone();
                let count = redraws.clone();
                let subscriptions = cx.update(|cx| {
                    [
                        cx.subscribe(&store, move |_, event: &StoreEvent, _| {
                            log.borrow_mut().push(event.clone())
                        }),
                        cx.observe(&store, move |_, _| count.set(count.get() + 1)),
                    ]
                });
                let rig = Self {
                    store,
                    events,
                    redraws,
                    _client: client,
                    _commands: commands,
                    _subscriptions: subscriptions,
                };
                rig.apply(
                    cx,
                    ClientEvent::ChatsReplaced(vec![summary("c", None), summary("d", None)]),
                );
                rig.store.update(cx, |s, cx| s.select(chat("c"), cx));
                rig.apply(
                    cx,
                    ClientEvent::History(HistoryChunk {
                        chat: chat("c"),
                        messages: vec![msg(1, "a"), msg(2, "b")],
                        reached_start: true,
                        replaces: None,
                        payloads: vec![],
                    }),
                );
                rig.settle();
                rig
            }

            fn settle(&self) {
                self.events.borrow_mut().clear();
                self.redraws.set(0);
            }

            fn apply(&self, cx: &mut TestAppContext, event: ClientEvent) {
                self.store.update(cx, |s, cx| s.apply(event, cx));
            }

            fn pin(&self, cx: &mut TestAppContext, tag: &str, pinned: Option<PinnedMessage>) {
                self.apply(
                    cx,
                    ClientEvent::PinChanged {
                        chat: chat(tag),
                        pinned,
                    },
                );
            }

            fn pinned(&self, cx: &mut TestAppContext, tag: &str) -> Option<PinnedMessage> {
                self.store.read_with(cx, |s, _| {
                    s.chat(&chat(tag)).and_then(|c| c.pinned_message.clone())
                })
            }

            fn row(&self, cx: &mut TestAppContext, tag: &str) -> ChatSummary {
                self.store
                    .read_with(cx, |s, _| s.chat(&chat(tag)).cloned().unwrap())
            }

            fn messages(&self, cx: &mut TestAppContext) -> Vec<Message> {
                self.store
                    .read_with(cx, |s, _| s.timeline(&chat("c")).unwrap().messages.clone())
            }

            fn events(&self) -> Vec<StoreEvent> {
                std::mem::take(&mut *self.events.borrow_mut())
            }
        }

        fn chat(tag: &str) -> ChatId {
            ChatId(tag.into())
        }

        fn msg(ts: i64, text: &str) -> Message {
            Message {
                chat: chat("c"),
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
                important: false,
            }
        }

        fn summary(tag: &str, pinned: Option<PinnedMessage>) -> ChatSummary {
            ChatSummary {
                id: chat(tag),
                kind: ChatKind::Group,
                title: format!("chat {tag}"),
                avatar: None,
                last_message: Some(MessagePreview {
                    author: Some("u".into()),
                    text: "b".into(),
                    timestamp: 2,
                }),
                unread: 3,
                muted: false,
                pinned_rank: Some(1),
                partner: None,
                pinned_message: pinned,
                pin_action_ts: Some(7),
            }
        }

        fn pin(ts: i64, text: Option<&str>) -> PinnedMessage {
            PinnedMessage {
                timestamp: ts,
                preview: text.map(|t| MessagePreview {
                    author: Some("u".into()),
                    text: t.into(),
                    timestamp: ts,
                }),
            }
        }

        #[gpui::test]
        fn a_pin_change_of_the_open_chat_redraws_once_and_touches_only_the_pin(
            cx: &mut TestAppContext,
        ) {
            let rig = Rig::new(cx);
            let before = rig.row(cx, "c");
            let messages = rig.messages(cx);

            rig.pin(cx, "c", Some(pin(1, None)));
            assert_eq!(rig.pinned(cx, "c"), Some(pin(1, None)));
            assert_eq!(rig.redraws.get(), 1);
            assert!(rig.events().is_empty(), "no timeline, row or typing event");

            let after = rig.row(cx, "c");
            assert_eq!(after.pinned_message, Some(pin(1, None)));
            assert_eq!(
                ChatSummary {
                    pinned_message: None,
                    ..after
                },
                before,
                "title, preview, unread, rank and clock are untouched"
            );
            assert_eq!(rig.messages(cx), messages);
        }

        #[gpui::test]
        fn a_resolved_preview_replacement_and_removal_each_redraw_once(cx: &mut TestAppContext) {
            let rig = Rig::new(cx);
            rig.pin(cx, "c", Some(pin(1, None)));
            rig.settle();

            rig.pin(cx, "c", Some(pin(1, Some("a"))));
            assert_eq!(rig.pinned(cx, "c"), Some(pin(1, Some("a"))));
            assert_eq!(rig.redraws.get(), 1);

            rig.pin(cx, "c", Some(pin(2, None)));
            assert_eq!(rig.pinned(cx, "c"), Some(pin(2, None)));
            assert_eq!(rig.redraws.get(), 2);

            rig.pin(cx, "c", None);
            assert_eq!(rig.pinned(cx, "c"), None);
            assert_eq!(rig.redraws.get(), 3);
            assert!(rig.events().is_empty());
        }

        #[gpui::test]
        fn an_equal_value_emits_nothing(cx: &mut TestAppContext) {
            let rig = Rig::new(cx);
            rig.pin(cx, "c", None);
            assert_eq!(rig.redraws.get(), 0, "no pin and still no pin");

            rig.pin(cx, "c", Some(pin(1, Some("a"))));
            rig.settle();
            rig.pin(cx, "c", Some(pin(1, Some("a"))));
            assert_eq!(rig.redraws.get(), 0);
            assert!(rig.events().is_empty());
        }

        #[gpui::test]
        fn a_pin_of_a_chat_that_is_not_open_is_kept_without_a_redraw(cx: &mut TestAppContext) {
            let rig = Rig::new(cx);
            rig.pin(cx, "d", Some(pin(1, None)));
            assert_eq!(rig.pinned(cx, "d"), Some(pin(1, None)));
            assert_eq!(rig.redraws.get(), 0);
            assert!(rig.events().is_empty());

            // Opening it later shows what was kept.
            rig.store.update(cx, |s, cx| s.select(chat("d"), cx));
            assert_eq!(rig.pinned(cx, "d"), Some(pin(1, None)));
        }

        #[gpui::test]
        fn a_pin_of_an_unknown_chat_is_ignored(cx: &mut TestAppContext) {
            let rig = Rig::new(cx);
            rig.pin(cx, "zzz", Some(pin(1, None)));
            assert!(
                rig.store
                    .read_with(cx, |s, _| s.chat(&chat("zzz")).is_none())
            );
            assert_eq!(rig.store.read_with(cx, |s, _| s.chat_count()), 2);
            assert_eq!(rig.redraws.get(), 0);
            assert!(rig.events().is_empty());
        }

        #[gpui::test]
        fn the_pin_of_a_summary_is_the_open_chats_pin(cx: &mut TestAppContext) {
            let rig = Rig::new(cx);
            rig.apply(
                cx,
                ClientEvent::ChatUpdated(summary("c", Some(pin(2, Some("b"))))),
            );
            assert_eq!(rig.pinned(cx, "c"), Some(pin(2, Some("b"))));

            rig.apply(
                cx,
                ClientEvent::ChatsReplaced(vec![summary("c", None), summary("d", None)]),
            );
            assert_eq!(
                rig.pinned(cx, "c"),
                None,
                "a list without the pin removes it"
            );
        }
    }
}
