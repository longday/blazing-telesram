//! UI-side state: the single place where `ClientEvent`s land and `Command`s leave.

use async_channel::{Receiver, Sender};
use bt_chat_core::model::{
    ChatId, ChatKind, ChatMembers, ChatSummary, ClientEvent, Command, ConnectionState, Delivery, DownloadFailure,
    Draft, HistoryChunk, MediaKind, MediaRef, Message, MessageBody, MessageTs, PayloadId, Profile, ReplyTarget,
    SearchFailure, SearchHits, SearchId, UserId,
};
use bt_chat_core::reactions::{ReactionCode, Reactions};
use gpui::{Context, EventEmitter};
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long the search field must stay unchanged before its text is sent to the server.
pub const SEARCH_DEBOUNCE: Duration = Duration::from_millis(300);
/// Shortest query (in `char`s, after `trim()`) that is sent to the server.
pub const SEARCH_MIN_CHARS: usize = 2;

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
    Delta { prepended: usize, appended: usize, touched: Vec<usize> },
    /// The message at this (pre-removal) index is gone.
    Removed { at: usize },
    /// Only loading flags changed.
    Status,
}

/// Where a download of an original stands, keyed by the original's `MediaRef` (kind `File`).
/// No entry means "not started" (or cancelled).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DownloadState {
    Running { received: u64, total: Option<u64> },
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

/// Where the one chat creation the store lets run stands. There is no state while none was started or
/// the last one ended in a chat; the dialog that started it keeps its own data, so a failure leaves
/// everything for another attempt.
#[derive(Clone, Debug, PartialEq)]
pub enum CreationState {
    /// A request is out (or the core is answering from the list); a second one is not sent.
    InFlight,
    /// Nothing was created (`unknown` false) or the result is not known and the chat may exist
    /// (`unknown` true); `reason` is ready to show. The core refreshed the list when `unknown`.
    Failed { reason: String, unknown: bool },
}

#[derive(Clone, Debug, PartialEq)]
pub enum StoreEvent {
    Chats,
    Timeline { chat: ChatId, change: TimelineChange },
    Media(MediaRef),
    /// The download state of this original changed (or was cleared).
    Download(MediaRef),
    Connection,
    Session,
    /// The persisted drafts arrived.
    Drafts,
    /// An edit or deletion did not happen.
    ActionFailed { chat: ChatId, reason: String },
    /// A send was not queued; the UI puts the text and reply target back.
    SendRefused { draft: Draft, reason: String },
    /// The server search state changed (issued, answered, failed or cancelled).
    Search,
    /// The member list state of this chat changed (requested, loaded or failed).
    Members(ChatId),
    /// The profile state changed (requested, loaded or failed).
    Profile,
    /// The creation state changed (started, failed, ended or dismissed).
    Creation,
    /// A creation ended in a chat: `existing` when none had to be created, `chat` is `None` when it
    /// cannot be opened (the reply and the refreshed list do not name it). The chat is selected as
    /// soon as it is in the list (at once when the list already holds it).
    ChatCreated { chat: Option<ChatId>, existing: bool },
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
    /// A created chat to select once the list holds it.
    pending_open: Option<ChatId>,
}

impl EventEmitter<StoreEvent> for ChatStore {}

impl ChatStore {
    /// Drains `events` on the foreground executor until the channel closes or the store is dropped.
    pub fn new(events: Receiver<ClientEvent>, commands: Sender<Command>, cx: &mut Context<Self>) -> Self {
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
            pending_open: None,
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
                if self.selected.as_ref().is_some_and(|id| !self.chats.contains_key(id)) {
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
            ClientEvent::MessageUpserted(message) => self.upsert(message, cx),
            ClientEvent::ReactionsChanged { chat, timestamp, reactions } => self.set_reactions(&chat, timestamp, reactions, cx),
            ClientEvent::MessageDeleted { chat, timestamp } => self.delete(&chat, timestamp, cx),
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
                self.creation = None;
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
            ClientEvent::SendRefused { draft, reason } => cx.emit(StoreEvent::SendRefused { draft, reason }),
            ClientEvent::Typing { chat, user, name, until } => self.typing_started(chat, user, name, until, cx),
            ClientEvent::ActionFailed { chat, reason } => cx.emit(StoreEvent::ActionFailed { chat, reason }),
            ClientEvent::DownloadProgress { media, received, total } => {
                self.set_download(media, Some(DownloadState::Running { received, total }), cx);
            }
            ClientEvent::Downloaded { media, path } => self.set_download(media, Some(DownloadState::Saved(path)), cx),
            ClientEvent::DownloadFailed { media, reason } => {
                self.set_download(media, Some(DownloadState::Failed(reason)), cx);
            }
            ClientEvent::DownloadCancelled(media) => self.set_download(media, None, cx),
            ClientEvent::SearchResults { id, hits } => self.finish_search(id, Some(hits), SearchStatus::Ready, cx),
            ClientEvent::SearchFailed { id, reason } => {
                self.finish_search(id, None, SearchStatus::Failed(reason), cx);
            }
            ClientEvent::Members(members) => self.finish_members(members.chat.clone(), MembersState::Ready(members), cx),
            ClientEvent::MembersFailed { chat, reason, offline } => {
                self.finish_members(chat, MembersState::Failed { reason, offline }, cx);
            }
            ClientEvent::Profile(profile) => self.finish_profile(ProfileState::Ready(profile), cx),
            ClientEvent::ProfileFailed { reason, offline } => {
                self.finish_profile(ProfileState::Failed { reason, offline }, cx);
            }
            ClientEvent::ChatCreated { chat, existing } => self.chat_created(chat, existing, cx),
            ClientEvent::CreationFailed { reason, unknown } => self.creation_failed(reason, unknown, cx),
        }
        cx.notify();
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
        let Some(timeline) = self.timelines.get_mut(chat) else { return };
        if !timeline.loaded || timeline.reached_start || timeline.loading_older {
            return;
        }
        let Some(before) = timeline.messages.first().map(|m| m.timestamp) else { return };
        timeline.loading_older = true;
        self.send(Command::LoadOlder { chat: chat.clone(), before });
        cx.emit(StoreEvent::Timeline { chat: chat.clone(), change: TimelineChange::Status });
        cx.notify();
    }

    pub fn sign_out(&mut self) {
        self.send(Command::SignOut);
    }

    /// Queues a message; the core answers with the pending copy. Sending replaces the chat's draft.
    pub fn send_text(&mut self, chat: ChatId, text: String, reply_to: Option<ReplyTarget>, cx: &mut Context<Self>) {
        self.drafts.remove(&chat);
        self.send(Command::Send { chat, text, reply_to });
        cx.notify();
    }

    pub fn retry(&self, payload: PayloadId) {
        self.send(Command::Retry(payload));
    }

    pub fn edit(&self, chat: ChatId, timestamp: MessageTs, text: String) {
        self.send(Command::Edit { chat, timestamp, text });
    }

    pub fn delete_message(&self, chat: ChatId, timestamp: MessageTs) {
        self.send(Command::Delete { chat, timestamp });
    }

    /// The user chose `code` on a message: the core decides between add, replace and remove.
    pub fn toggle_reaction(&self, chat: ChatId, timestamp: MessageTs, code: ReactionCode) {
        self.send(Command::ToggleReaction { chat, timestamp, code });
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
        if self.chats.get(chat).is_some_and(|c| c.kind == ChatKind::Private) {
            return Some("печатает…".into());
        }
        let first = typists[0].name.as_deref().unwrap_or("Кто-то");
        Some(match typists.len() {
            1 => format!("{first} печатает…"),
            n => format!("{first} и ещё {} печатают…", n - 1),
        })
    }

    fn typing_started(&mut self, chat: ChatId, user: UserId, name: Option<String>, until: Instant, cx: &mut Context<Self>) {
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
        let Some(timeline) = self.timelines.get_mut(chat) else { return };
        let pending = |m: &Message| matches!(&m.delivery, Delivery::Pending(p) | Delivery::Failed { payload: p, .. } if p == payload);
        let Some(at) = timeline.messages.iter().position(pending) else { return };
        timeline.messages.remove(at);
        cx.emit(StoreEvent::Timeline { chat: chat.clone(), change: TimelineChange::Removed { at } });
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
        if matches!(self.downloads.get(&original), Some(DownloadState::Running { .. })) {
            return;
        }
        if self.connection != ConnectionState::Online {
            self.set_download(original, Some(DownloadState::Failed(DownloadFailure::Offline)), cx);
            cx.notify();
            return;
        }
        self.set_download(original.clone(), Some(DownloadState::Running { received: 0, total: None }), cx);
        self.send(Command::Download { media: original, name });
        cx.notify();
    }

    /// Asks the core to stop a running download; the state clears when `DownloadCancelled` arrives.
    pub fn cancel_download(&self, original: &MediaRef) {
        if matches!(self.downloads.get(original), Some(DownloadState::Running { .. })) {
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
        self.search = Some(SearchState { id, query: query.clone(), status: SearchStatus::Loading, hits });
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
        if let Some(state) = self.search.as_mut().filter(|s| s.status == SearchStatus::Loading && s.query != next) {
            state.id = SearchId(0);
            state.query.clear();
        }
    }

    pub fn search_state(&self) -> Option<&SearchState> {
        self.search.as_ref()
    }

    /// Stores the outcome of search `id`; an outcome of any other id (superseded or cancelled) is ignored.
    /// A failure keeps the hits already shown.
    fn finish_search(&mut self, id: SearchId, hits: Option<SearchHits>, status: SearchStatus, cx: &mut Context<Self>) {
        let Some(state) = self.search.as_mut().filter(|s| s.id == id) else { return };
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
        if matches!(self.creation, Some(CreationState::Failed { .. })) {
            self.creation = None;
            cx.emit(StoreEvent::Creation);
            cx.notify();
        }
    }

    /// A creation that is not in flight (late, or ended by an expired session) changes nothing.
    fn creation_failed(&mut self, reason: String, unknown: bool, cx: &mut Context<Self>) {
        if self.creation != Some(CreationState::InFlight) {
            return;
        }
        self.creation = Some(CreationState::Failed { reason, unknown });
        cx.emit(StoreEvent::Creation);
    }

    fn chat_created(&mut self, chat: Option<ChatId>, existing: bool, cx: &mut Context<Self>) {
        if self.creation != Some(CreationState::InFlight) {
            return;
        }
        self.creation = None;
        self.pending_open = chat.clone();
        cx.emit(StoreEvent::Creation);
        self.open_pending(cx);
        cx.emit(StoreEvent::ChatCreated { chat, existing });
    }

    /// Selects the created chat once the list holds it.
    fn open_pending(&mut self, cx: &mut Context<Self>) {
        if let Some(chat) = self.pending_open.take_if(|id| self.chats.contains_key(id)) {
            self.select(chat, cx);
        }
    }

    fn set_download(&mut self, media: MediaRef, state: Option<DownloadState>, cx: &mut Context<Self>) {
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
            .filter(|c| filter.accepts(c) && (query.is_empty() || c.title.to_lowercase().contains(&query)))
            .collect();
        chats.sort_by(|a, b| {
            let activity = |c: &ChatSummary| c.last_message.as_ref().map(|m| m.timestamp);
            (a.pinned_rank.is_none(), a.pinned_rank, Reverse(activity(a)), &a.title, &a.id)
                .cmp(&(b.pinned_rank.is_none(), b.pinned_rank, Reverse(activity(b)), &b.title, &b.id))
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
        cx.emit(StoreEvent::Timeline { chat: chunk.chat, change });
    }

    fn upsert(&mut self, message: Message, cx: &mut Context<Self>) {
        self.want_message_media(&message);
        if message.delivery == Delivery::Sent {
            self.typing_stopped(&message.chat, &message.author);
        }
        let Some(timeline) = self.timelines.get_mut(&message.chat).filter(|t| t.loaded) else { return };
        let chat = message.chat.clone();
        let ts = message.timestamp;
        let change = match timeline.messages.binary_search_by_key(&ts, |m| m.timestamp) {
            Ok(i) if timeline.messages[i] == message => return,
            Ok(i) => {
                timeline.messages[i] = message;
                TimelineChange::Delta { prepended: 0, appended: 0, touched: vec![i] }
            }
            Err(i) if i == timeline.messages.len() => {
                let was_empty = i == 0;
                timeline.messages.push(message);
                if was_empty {
                    TimelineChange::Reset
                } else {
                    TimelineChange::Delta { prepended: 0, appended: 1, touched: vec![] }
                }
            }
            Err(0) if timeline.reached_start => {
                timeline.messages.insert(0, message);
                TimelineChange::Delta { prepended: 1, appended: 0, touched: vec![] }
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
        let Some(timeline) = self.timelines.get_mut(chat) else { return };
        let Ok(i) = timeline.messages.binary_search_by_key(&ts, |m| m.timestamp) else { return };
        let message = &mut timeline.messages[i];
        if message.body == MessageBody::Deleted {
            return;
        }
        message.body = MessageBody::Deleted;
        message.reply_to = None;
        message.forwarded_from = None;
        cx.emit(StoreEvent::Timeline {
            chat: chat.clone(),
            change: TimelineChange::Delta { prepended: 0, appended: 0, touched: vec![i] },
        });
    }

    /// Replaces the reactions of a loaded message in place. Unlike `upsert` this leaves the author's
    /// typing indicator and the media requests alone: a reaction update carries no message body.
    fn set_reactions(&mut self, chat: &ChatId, ts: MessageTs, reactions: Reactions, cx: &mut Context<Self>) {
        let Some(timeline) = self.timelines.get_mut(chat).filter(|t| t.loaded) else { return };
        let Ok(i) = timeline.messages.binary_search_by_key(&ts, |m| m.timestamp) else { return };
        let message = &mut timeline.messages[i];
        if message.reactions == reactions {
            return;
        }
        message.reactions = reactions;
        cx.emit(StoreEvent::Timeline {
            chat: chat.clone(),
            change: TimelineChange::Delta { prepended: 0, appended: 0, touched: vec![i] },
        });
    }
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
    let span = replaces.or_else(|| incoming.first().zip(incoming.last()).map(|(a, b)| (Some(a.timestamp), Some(b.timestamp))));
    let Some((from, to)) = span else {
        return TimelineChange::Status;
    };
    let covered = |ts: MessageTs| from.is_none_or(|f| ts >= f) && to.is_none_or(|t| ts <= t);
    let mut merged = Vec::with_capacity(old.len() + incoming.len());
    let mut incoming = incoming.into_iter().peekable();
    for existing in old.iter() {
        while incoming.peek().is_some_and(|m| m.timestamp < existing.timestamp) {
            merged.extend(incoming.next());
        }
        if incoming.peek().is_some_and(|m| m.timestamp == existing.timestamp) {
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
    if middle.len() != old.len() || middle.iter().zip(old).any(|(n, o)| n.timestamp != o.timestamp) {
        return TimelineChange::Reset;
    }
    let touched = middle
        .iter()
        .zip(old)
        .enumerate()
        .filter(|(_, (n, o))| n != o)
        .map(|(i, _)| prepended + i)
        .collect();
    TimelineChange::Delta { prepended, appended: new.len() - end, touched }
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
        }
    }

    fn texts(ms: &[Message]) -> Vec<i64> {
        ms.iter().map(|m| m.timestamp).collect()
    }

    #[test]
    fn older_page_is_a_prepend() {
        let mut old = vec![msg(3, "c"), msg(4, "d")];
        let change = merge_chunk(&mut old, vec![msg(1, "a"), msg(2, "b")], None);
        assert_eq!(change, TimelineChange::Delta { prepended: 2, appended: 0, touched: vec![] });
        assert_eq!(texts(&old), [1, 2, 3, 4]);
    }

    #[test]
    fn catch_up_chunk_appends_and_updates() {
        let mut old = vec![msg(1, "a"), msg(2, "b"), msg(3, "c")];
        let change = merge_chunk(&mut old, vec![msg(2, "b"), msg(3, "C"), msg(4, "d")], None);
        assert_eq!(change, TimelineChange::Delta { prepended: 0, appended: 1, touched: vec![2] });
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
                    cx.subscribe(&store, move |_, event: &StoreEvent, _| log.borrow_mut().push(event.clone()))
                });
                Self { store, commands, events, _client: client, _subscription: subscription }
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
                self.store.read_with(cx, |s, _| s.timeline(&chat()).map(|t| t.messages.clone()).unwrap_or_default())
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
                counts: pairs.iter().map(|&(c, count)| ReactionCount { code: ReactionCode(c), count }).collect(),
                mine: mine.map(ReactionCode),
                version: Some(version),
            }
        }

        fn changed(ts: i64, reactions: Reactions) -> ClientEvent {
            ClientEvent::ReactionsChanged { chat: chat(), timestamp: ts, reactions }
        }

        fn delta(touched: Vec<usize>) -> StoreEvent {
            StoreEvent::Timeline { chat: chat(), change: TimelineChange::Delta { prepended: 0, appended: 0, touched } }
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
            assert!(rig.store.read_with(cx, |s, _| s.timeline(&chat()).is_none()), "no timeline is made up");

            let rig = Rig::loaded(cx);
            rig.apply(cx, changed(99, reactions(&[(7, 1)], None, 1)));
            rig.apply(
                cx,
                ClientEvent::ReactionsChanged { chat: ChatId("other".into()), timestamp: 2, reactions: reactions(&[(7, 1)], None, 1) },
            );
            assert!(rig.take_events().is_empty());
            assert!(rig.messages(cx).iter().all(|m| m.reactions == Reactions::default()));
        }

        #[gpui::test]
        fn the_authors_typing_indicator_survives_an_update(cx: &mut TestAppContext) {
            let rig = Rig::loaded(cx);
            let until = cx.executor().now() + Duration::from_secs(60);
            rig.apply(cx, ClientEvent::Typing { chat: chat(), user: UserId("u".into()), name: None, until });
            let typing = |cx: &mut TestAppContext| rig.store.read_with(cx, |s, _| s.typing_text(&chat()));
            assert!(typing(cx).is_some());

            rig.apply(cx, changed(1, reactions(&[(7, 1)], None, 1)));
            assert!(typing(cx).is_some(), "an upsert of the author's message would have cleared it");
        }

        #[gpui::test]
        fn toggling_sends_the_command_without_touching_the_timeline(cx: &mut TestAppContext) {
            let rig = Rig::loaded(cx);
            rig.store.update(cx, |s, _| s.toggle_reaction(chat(), 2, ReactionCode(7)));
            assert_eq!(rig.commands(), [Command::ToggleReaction { chat: chat(), timestamp: 2, code: ReactionCode(7) }]);
            assert!(rig.take_events().is_empty(), "the overlay arrives from the core as an event");
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
                    cx.subscribe(&store, move |_, event: &StoreEvent, _| log.borrow_mut().push(event.clone()))
                });
                Self { store, commands, events, _client: client, _subscription: subscription }
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
                chats: vec![ChatHit { id: ChatId(format!("c-{tag}")), kind: ChatKind::Group, title: tag.into(), avatar: None }],
                people: vec![PersonHit { user: UserId(format!("u-{tag}")), name: tag.into(), avatar: None, chat: None }],
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
            ClientEvent::SearchResults { id: SearchId(id), hits: hits(tag) }
        }

        #[gpui::test]
        fn a_result_with_the_current_id_is_stored_and_announced(cx: &mut TestAppContext) {
            let rig = Rig::new(cx);
            assert_eq!(rig.state(cx), None);
            rig.search(cx, "ab");
            assert_eq!(rig.commands(), [Command::Search { id: SearchId(1), query: "ab".into() }]);
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
            rig.apply(cx, ClientEvent::SearchFailed { id: SearchId(1), reason: SearchFailure::Failed });
            assert!(rig.take_events().is_empty(), "a superseded reply announces nothing");
            let state = rig.state(cx).unwrap();
            assert_eq!((state.id, state.query.as_str(), state.status), (SearchId(2), "abc", SearchStatus::Loading));
            assert_eq!(state.hits, SearchHits::default());

            rig.apply(cx, results(2, "abc"));
            let state = rig.state(cx).unwrap();
            assert_eq!((state.status, state.hits), (SearchStatus::Ready, hits("abc")));
        }

        #[gpui::test]
        fn a_new_query_keeps_the_old_hits_while_loading(cx: &mut TestAppContext) {
            let rig = Rig::new(cx);
            rig.search(cx, "ab");
            rig.apply(cx, results(1, "ab"));
            rig.search(cx, "abc");
            let state = rig.state(cx).unwrap();
            assert_eq!((state.id, state.status), (SearchId(2), SearchStatus::Loading));
            assert_eq!(state.hits, hits("ab"));
            assert_eq!(
                rig.commands(),
                [
                    Command::Search { id: SearchId(1), query: "ab".into() },
                    Command::Search { id: SearchId(2), query: "abc".into() }
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

            rig.apply(cx, ClientEvent::SearchFailed { id: SearchId(2), reason: SearchFailure::Offline });
            assert_eq!(rig.take_events(), [StoreEvent::Search]);
            let state = rig.state(cx).unwrap();
            assert_eq!(state.status, SearchStatus::Failed(SearchFailure::Offline));
            assert_eq!(state.hits, hits("ab"));
        }

        #[gpui::test]
        fn cancelling_clears_the_state_sends_the_command_and_drops_late_replies(cx: &mut TestAppContext) {
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
            assert_eq!(rig.commands(), [Command::Search { id: SearchId(3), query: "xy".into() }], "ids never repeat");
            rig.apply(cx, results(2, "abc"));
            assert_eq!(rig.state(cx).unwrap().status, SearchStatus::Loading);
        }

        #[gpui::test]
        fn avatars_of_chat_and_person_hits_are_requested_once(cx: &mut TestAppContext) {
            let rig = Rig::new(cx);
            let avatar = |id: &str| Some(MediaRef { id: id.into(), kind: MediaKind::Avatar });
            let mut found = hits("ab");
            found.chats[0].avatar = avatar("chat-av");
            found.people[0].avatar = avatar("person-av");
            rig.search(cx, "ab");
            rig.commands();

            rig.apply(cx, ClientEvent::SearchResults { id: SearchId(1), hits: found.clone() });
            rig.search(cx, "abc");
            rig.apply(cx, ClientEvent::SearchResults { id: SearchId(2), hits: found });
            let fetched: Vec<Command> = rig.commands().into_iter().filter(|c| matches!(c, Command::FetchMedia(_))).collect();
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
                    cx.subscribe(&store, move |_, event: &StoreEvent, _| log.borrow_mut().push(event.clone()))
                });
                Self { store, commands, events, _client: client, _subscription: subscription }
            }

            fn load(&self, cx: &mut TestAppContext, chat: &ChatId) {
                self.store.update(cx, |s, cx| s.load_members(chat.clone(), cx));
            }

            fn load_profile(&self, cx: &mut TestAppContext) {
                self.store.update(cx, |s, cx| s.load_profile(cx));
            }

            fn apply(&self, cx: &mut TestAppContext, event: ClientEvent) {
                self.store.update(cx, |s, cx| s.apply(event, cx));
            }

            fn state(&self, cx: &mut TestAppContext, chat: &ChatId) -> Option<MembersState> {
                self.store.read_with(cx, |s, _| s.members_state(chat).cloned())
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
            Some(MediaRef { id: id.into(), kind: MediaKind::Avatar })
        }

        fn listed(tag: &str) -> ChatMembers {
            ChatMembers {
                chat: chat(tag),
                users: vec![
                    Member { user: UserId("u1".into()), name: Some("Анна".into()), avatar: avatar("av1"), role: MemberRole::Admin, me: false },
                    Member { user: UserId("u2".into()), name: Some("Борис".into()), avatar: None, role: MemberRole::Member, me: true },
                ],
                partial: false,
            }
        }

        fn failed(chat_tag: &str, reason: &str, offline: bool) -> ClientEvent {
            ClientEvent::MembersFailed { chat: chat(chat_tag), reason: reason.into(), offline }
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
            assert_eq!(rig.state(cx, &chat("a")), Some(MembersState::Ready(listed("a"))));
            assert_eq!(rig.take_events(), [StoreEvent::Members(chat("a"))]);
            assert_eq!(rig.commands(), [Command::FetchMedia(avatar("av1").unwrap())]);
        }

        #[gpui::test]
        fn loading_becomes_failed_with_the_reason_and_the_offline_flag(cx: &mut TestAppContext) {
            let rig = Rig::new(cx);
            rig.load(cx, &chat("a"));
            rig.take_events();

            rig.apply(cx, failed("a", "нет прав", false));
            assert_eq!(rig.state(cx, &chat("a")), Some(MembersState::Failed { reason: "нет прав".into(), offline: false }));
            assert_eq!(rig.take_events(), [StoreEvent::Members(chat("a"))]);

            rig.load(cx, &chat("a"));
            rig.apply(cx, failed("a", "Нет соединения", true));
            assert_eq!(rig.state(cx, &chat("a")), Some(MembersState::Failed { reason: "Нет соединения".into(), offline: true }));
        }

        #[gpui::test]
        fn a_retry_after_a_failure_or_a_list_goes_back_to_loading_and_asks_again(cx: &mut TestAppContext) {
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
            assert_eq!(rig.state(cx, &chat("a")), Some(MembersState::Loading), "a list is never shown as current again");
            assert_eq!(rig.commands().iter().filter(|c| **c == Command::LoadMembers(chat("a"))).count(), 1);
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
            assert_eq!(rig.state(cx, &chat("a")), None, "the answer for another chat changes nothing");
            assert_eq!(rig.state(cx, &chat("b")), Some(MembersState::Loading));

            rig.apply(cx, ClientEvent::Members(listed("b")));
            rig.apply(cx, failed("b", "late", false));
            assert_eq!(rig.state(cx, &chat("b")), Some(MembersState::Ready(listed("b"))), "a late duplicate does not overwrite");
        }

        #[gpui::test]
        fn the_profile_goes_loading_to_ready_and_asks_for_its_avatar(cx: &mut TestAppContext) {
            let rig = Rig::new(cx);
            assert_eq!(rig.profile(cx), None);
            rig.load_profile(cx);
            assert_eq!(rig.profile(cx), Some(ProfileState::Loading));
            assert_eq!(rig.commands(), [Command::LoadProfile]);
            assert_eq!(rig.take_events(), [StoreEvent::Profile]);

            let own = Profile { user: UserId("me".into()), name: Some("Ян".into()), avatar: avatar("me-av"), status_text: None };
            rig.apply(cx, ClientEvent::Profile(own.clone()));
            assert_eq!(rig.profile(cx), Some(ProfileState::Ready(own)));
            assert_eq!(rig.take_events(), [StoreEvent::Profile]);
            assert_eq!(rig.commands(), [Command::FetchMedia(avatar("me-av").unwrap())]);
        }

        #[gpui::test]
        fn the_profile_goes_loading_to_failed_and_a_retry_asks_again(cx: &mut TestAppContext) {
            let rig = Rig::new(cx);
            rig.load_profile(cx);
            rig.apply(cx, ClientEvent::ProfileFailed { reason: "Нет соединения".into(), offline: true });
            assert_eq!(rig.profile(cx), Some(ProfileState::Failed { reason: "Нет соединения".into(), offline: true }));
            rig.commands();

            rig.load_profile(cx);
            assert_eq!(rig.profile(cx), Some(ProfileState::Loading), "no data of the failed request stays");
            assert_eq!(rig.commands(), [Command::LoadProfile]);
            rig.load_profile(cx);
            assert!(rig.commands().is_empty(), "a running load is not sent twice");

            rig.apply(cx, ClientEvent::ProfileFailed { reason: "x".into(), offline: false });
            rig.apply(cx, ClientEvent::ProfileFailed { reason: "y".into(), offline: false });
            assert_eq!(rig.profile(cx), Some(ProfileState::Failed { reason: "x".into(), offline: false }));
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
                    cx.subscribe(&store, move |_, event: &StoreEvent, _| log.borrow_mut().push(event.clone()))
                });
                Self { store, commands, events, _client: client, _subscription: subscription }
            }

            fn create(&self, cx: &mut TestAppContext, user: &str) {
                self.store.update(cx, |s, cx| s.create_private_chat(UserId(user.into()), cx));
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
                self.apply(cx, ClientEvent::ChatsReplaced(tags.iter().map(|t| row(t)).collect()));
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
            }
        }

        fn created(tag: &str, existing: bool) -> ClientEvent {
            ClientEvent::ChatCreated { chat: Some(chat(tag)), existing }
        }

        fn user(tag: &str) -> UserId {
            UserId(tag.into())
        }

        #[gpui::test]
        fn starting_goes_in_flight_sends_one_command_and_ignores_a_second_press(cx: &mut TestAppContext) {
            let rig = Rig::new(cx);
            assert_eq!(rig.state(cx), None);
            rig.create(cx, "u9");
            assert_eq!(rig.state(cx), Some(CreationState::InFlight));
            assert_eq!(rig.commands(), [Command::CreatePrivateChat { user: user("u9") }]);
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
            assert_eq!(rig.take_events(), [StoreEvent::Creation, StoreEvent::ChatCreated { chat: Some(chat("p")), existing: false }]);
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
            assert_eq!(rig.take_events(), [StoreEvent::Creation, StoreEvent::ChatCreated { chat: Some(chat("p")), existing: true }]);
        }

        #[gpui::test]
        fn a_created_chat_without_an_id_ends_the_creation_and_selects_nothing(cx: &mut TestAppContext) {
            let rig = Rig::new(cx);
            rig.list(cx, &["a"]);
            rig.create(cx, "u9");
            rig.commands();
            rig.take_events();
            rig.apply(cx, ClientEvent::ChatCreated { chat: None, existing: false });
            assert_eq!(rig.state(cx), None);
            assert_eq!(rig.selected(cx), None);
            assert!(rig.commands().is_empty());
            assert_eq!(rig.take_events(), [StoreEvent::Creation, StoreEvent::ChatCreated { chat: None, existing: false }]);
        }

        #[gpui::test]
        fn a_failure_is_kept_until_dismissed_and_another_attempt_is_allowed(cx: &mut TestAppContext) {
            let rig = Rig::new(cx);
            rig.create(cx, "u9");
            rig.commands();
            rig.take_events();

            rig.apply(cx, ClientEvent::CreationFailed { reason: "нет прав".into(), unknown: false });
            assert_eq!(rig.state(cx), Some(CreationState::Failed { reason: "нет прав".into(), unknown: false }));
            assert_eq!(rig.take_events(), [StoreEvent::Creation]);

            rig.dismiss(cx);
            assert_eq!(rig.state(cx), None);
            assert_eq!(rig.take_events(), [StoreEvent::Creation]);
            rig.dismiss(cx);
            assert!(rig.take_events().is_empty(), "nothing to dismiss");

            rig.create(cx, "u9");
            rig.apply(cx, ClientEvent::CreationFailed { reason: "результат неизвестен".into(), unknown: true });
            assert_eq!(rig.state(cx), Some(CreationState::Failed { reason: "результат неизвестен".into(), unknown: true }));
            rig.commands();
            rig.create(cx, "u9");
            assert_eq!(rig.state(cx), Some(CreationState::InFlight), "the next attempt is the user's");
            assert_eq!(rig.commands(), [Command::CreatePrivateChat { user: user("u9") }]);
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
            rig.apply(cx, ClientEvent::CreationFailed { reason: "x".into(), unknown: false });
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
            assert_eq!(rig.selected(cx), None, "a chat awaited before the sign out is not opened");
        }
    }
}
