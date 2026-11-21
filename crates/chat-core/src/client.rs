//! `ChatClient`: a dedicated thread with a Tokio runtime that serves UI commands from the
//! cache first, syncs through a [`Backend`], and publishes [`ClientEvent`]s.
//!
//! All state lives in one single-threaded event loop. Network calls run as spawned tasks that
//! post their results back as inputs, so cache writes and event emission are totally ordered.

use crate::backend::{Backend, BackendError, LiveEvent};
use crate::cache::{self, Cache, CacheError};
use crate::model::*;
use crate::paths::{escape_component, media_dir};
use async_channel::{Receiver, Sender};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;

/// Concurrent media downloads.
const MEDIA_SLOTS: usize = 4;
/// Live changes remembered per chat to replay over history snapshots that raced them.
const LIVE_LOG_CAP: usize = 256;

enum LiveChange {
    Upsert(Message),
    Delete(MessageTs),
}

#[derive(Clone, Debug)]
pub struct ClientConfig {
    pub runtime_dir: PathBuf,
    /// Deleted on session expiry and on sign out.
    pub session_file: PathBuf,
    /// Account of the stored session; selects the cache shown before the server answers.
    pub account: UserId,
    /// Chunk size for history requests (the web client uses 50).
    pub history_limit: usize,
    /// First reconnect delay; doubles per failed attempt up to `reconnect_max`.
    pub reconnect_base: Duration,
    pub reconnect_max: Duration,
    /// How long media the server answered 404 for is not requested again.
    pub media_missing_ttl: Duration,
}

impl ClientConfig {
    pub fn new(runtime_dir: impl Into<PathBuf>, session_file: impl Into<PathBuf>, account: UserId) -> Self {
        Self {
            runtime_dir: runtime_dir.into(),
            session_file: session_file.into(),
            account,
            history_limit: 50,
            reconnect_base: Duration::from_secs(11),
            reconnect_max: Duration::from_secs(90),
            media_missing_ttl: Duration::from_secs(3600),
        }
    }
}

/// UI-side handle. Dropping it (and every `events()` receiver) stops the client thread.
pub struct ChatClient {
    commands: Sender<Command>,
    events: Receiver<ClientEvent>,
}

impl ChatClient {
    /// Starts the client thread. The cache is opened on that thread, so this returns at once
    /// and the cached chats arrive as the first event.
    pub fn spawn<B: Backend>(backend: B, config: ClientConfig) -> io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
        let (commands, command_rx) = async_channel::unbounded();
        let (event_tx, events) = async_channel::unbounded();
        std::thread::Builder::new().name("chat-client".into()).spawn(move || {
            runtime.block_on(Engine::run(Arc::new(backend), config, command_rx, event_tx));
        })?;
        Ok(Self { commands, events })
    }

    /// Never blocks. After `SignedOut` the client is finished and commands are dropped.
    pub fn send(&self, command: Command) {
        let _ = self.commands.try_send(command);
    }

    /// The single event stream; clones share the same queue.
    pub fn events(&self) -> Receiver<ClientEvent> {
        self.events.clone()
    }
}

type Connected = (UserId, Vec<ChatSummary>, Receiver<LiveEvent>);

struct HistoryFetch {
    chat: ChatId,
    before: Option<MessageTs>,
    /// Newest cached timestamp when the request started.
    known_newest: Option<MessageTs>,
    /// `Engine::live_seq` when the request started.
    mark: u64,
    result: Result<HistoryChunk, BackendError>,
}

enum Input {
    Command(Command),
    CommandsClosed,
    Connected { epoch: u64, result: Result<Connected, BackendError> },
    Retry(u64),
    Live { epoch: u64, event: LiveEvent },
    LiveClosed(u64),
    Chats(Result<Vec<ChatSummary>, BackendError>),
    History(HistoryFetch),
    Media { media: MediaRef, result: Result<Vec<u8>, BackendError> },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Initial or retried connect attempt running.
    Connecting,
    Online,
    /// The backend is reconnecting its live stream by itself.
    Reconnecting,
    /// Waiting for the next retry.
    Offline,
    Expired,
}

struct Engine<B> {
    backend: Arc<B>,
    config: ClientConfig,
    account: UserId,
    cache: Cache,
    events: Sender<ClientEvent>,
    inputs: Sender<Input>,
    phase: Phase,
    shown: Option<ConnectionState>,
    /// Bumped on every connection attempt; inputs of older attempts are stale.
    epoch: u64,
    attempt: u32,
    live_pump: Option<JoinHandle<()>>,
    open_chat: Option<ChatId>,
    /// Counts live message events; `live_log` keeps each chat's recent changes with their count.
    live_seq: u64,
    live_log: HashMap<ChatId, VecDeque<(u64, LiveChange)>>,
    media_inflight: HashSet<MediaRef>,
    /// Requested while we could not fetch; started once online.
    media_deferred: Vec<MediaRef>,
    /// Media the server answered 404 for, with the time of that answer.
    media_missing: HashMap<MediaRef, Instant>,
    media_slots: Arc<Semaphore>,
}

fn post(tx: &Sender<Input>, input: Input) {
    let _ = tx.try_send(input);
}

async fn pump_commands(commands: Receiver<Command>, inputs: Sender<Input>) {
    while let Ok(command) = commands.recv().await {
        post(&inputs, Input::Command(command));
    }
    post(&inputs, Input::CommandsClosed);
}

async fn pump_live(epoch: u64, live: Receiver<LiveEvent>, inputs: Sender<Input>) {
    while let Ok(event) = live.recv().await {
        post(&inputs, Input::Live { epoch, event });
    }
    post(&inputs, Input::LiveClosed(epoch));
}

async fn connect<B: Backend>(backend: &B) -> Result<Connected, BackendError> {
    let account = backend.bootstrap().await?;
    let chats = backend.chat_list().await?;
    let live = backend.subscribe().await?;
    Ok((account, chats, live))
}

/// A cache that cannot be opened is disposable: recreate it, and as a last resort run without
/// persistence rather than without a client.
fn open_cache(runtime_dir: &Path, account: &UserId) -> Cache {
    match Cache::open(runtime_dir, account) {
        Ok(cache) => cache,
        Err(first) => {
            tracing::warn!(error = %first, "cache unusable, recreating");
            let _ = cache::remove_files(runtime_dir, account);
            Cache::open(runtime_dir, account).unwrap_or_else(|second| {
                tracing::error!(error = %second, "cache unavailable, running in memory");
                Cache::open_in_memory().expect("in-memory sqlite always opens")
            })
        }
    }
}

/// Applies `changes` (oldest first) to sorted `messages`, limited to the range `lo..=hi`.
fn replay_live(messages: &mut Vec<Message>, changes: &[&LiveChange], lo: Option<MessageTs>, hi: Option<MessageTs>) {
    let in_range = |ts: MessageTs| lo.is_none_or(|lo| ts >= lo) && hi.is_none_or(|hi| ts <= hi);
    for change in changes {
        match change {
            LiveChange::Upsert(message) if in_range(message.timestamp) => {
                match messages.binary_search_by_key(&message.timestamp, |m| m.timestamp) {
                    Ok(i) => messages[i] = message.clone(),
                    Err(i) => messages.insert(i, message.clone()),
                }
            }
            LiveChange::Delete(ts) if in_range(*ts) => {
                if let Ok(i) = messages.binary_search_by_key(ts, |m| m.timestamp) {
                    messages[i].body = MessageBody::Deleted;
                }
            }
            _ => {}
        }
    }
}

fn preview_text(body: &MessageBody) -> Option<String> {
    match body {
        MessageBody::Text(text) => Some(text.clone()),
        MessageBody::Image { caption, .. } => Some(caption.clone().unwrap_or_else(|| "Фото".into())),
        MessageBody::File { name, .. } => Some(name.clone()),
        MessageBody::Unsupported(kind) => Some(format!("[{kind}]")),
        MessageBody::Deleted => None,
    }
}

fn fnv1a(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, b| (hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

fn media_file_name(media: &MediaRef) -> String {
    let prefix = cache::kind_name(media.kind);
    let name = format!("{prefix}-{}", escape_component(&media.id));
    if name.len() <= 200 { name } else { format!("{prefix}-{:016x}", fnv1a(&media.id)) }
}

fn remove_ignoring_missing(result: io::Result<()>) {
    if let Err(e) = result
        && e.kind() != io::ErrorKind::NotFound
    {
        tracing::warn!(error = %e, "sign-out cleanup failed");
    }
}

impl<B: Backend> Engine<B> {
    async fn run(backend: Arc<B>, config: ClientConfig, commands: Receiver<Command>, events: Sender<ClientEvent>) {
        let (inputs, input_rx) = async_channel::unbounded();
        let account = config.account.clone();
        let mut engine = Engine {
            cache: open_cache(&config.runtime_dir, &account),
            backend,
            config,
            account,
            events,
            inputs: inputs.clone(),
            phase: Phase::Connecting,
            shown: None,
            epoch: 0,
            attempt: 0,
            live_pump: None,
            open_chat: None,
            live_seq: 0,
            live_log: HashMap::new(),
            media_inflight: HashSet::new(),
            media_deferred: Vec::new(),
            media_missing: HashMap::new(),
            media_slots: Arc::new(Semaphore::new(MEDIA_SLOTS)),
        };
        tokio::spawn(pump_commands(commands, inputs));

        let cached = engine.db(engine.cache.chats()).unwrap_or_default();
        if !cached.is_empty() {
            engine.emit(ClientEvent::ChatsReplaced(cached));
        }
        engine.start_connect();
        while let Ok(input) = input_rx.recv().await {
            if engine.events.is_closed() || !engine.handle(input) {
                break;
            }
        }
        if let Some(pump) = engine.live_pump.take() {
            pump.abort();
        }
    }

    fn emit(&self, event: ClientEvent) {
        let _ = self.events.try_send(event);
    }

    fn set_connection(&mut self, state: ConnectionState) {
        if self.shown != Some(state) {
            self.shown = Some(state);
            self.emit(ClientEvent::Connection(state));
        }
    }

    /// Cache failures degrade to "not cached"; they must not take the client down.
    fn db<T>(&self, result: Result<T, CacheError>) -> Option<T> {
        result.map_err(|e| tracing::error!(error = %e, "cache error")).ok()
    }

    fn can_fetch(&self) -> bool {
        self.phase == Phase::Online
    }

    /// Returns false when the client is finished.
    fn handle(&mut self, input: Input) -> bool {
        match input {
            Input::Command(command) => return self.handle_command(command),
            Input::CommandsClosed => return false,
            Input::Connected { epoch, result } if epoch == self.epoch => self.handle_connected(result),
            Input::Retry(epoch) if epoch == self.epoch && self.phase == Phase::Offline => self.start_connect(),
            Input::Live { epoch, event } if epoch == self.epoch => self.handle_live(event),
            Input::LiveClosed(epoch) if epoch == self.epoch => self.connection_lost(),
            Input::Chats(result) => self.handle_chats(result),
            Input::History(fetch) => self.handle_history(fetch),
            Input::Media { media, result } => self.handle_media_result(media, result),
            Input::Connected { .. } | Input::Retry(_) | Input::Live { .. } | Input::LiveClosed(_) => {}
        }
        true
    }

    fn handle_command(&mut self, command: Command) -> bool {
        match command {
            Command::OpenChat(chat) => {
                let limit = self.config.history_limit;
                let cached = self.db(self.cache.history(&chat, None, limit));
                self.open_chat = Some(chat.clone());
                self.backend.watch(Some(&chat));
                let fetch = self.can_fetch();
                // With a fetch coming, an empty cache is not worth an (empty) event.
                match cached {
                    Some(chunk) if !fetch || !chunk.messages.is_empty() => self.emit(ClientEvent::History(chunk)),
                    None if !fetch => self.emit(ClientEvent::History(HistoryChunk {
                        chat: chat.clone(),
                        messages: Vec::new(),
                        reached_start: false,
                        replaces: None,
                    })),
                    _ => {}
                }
                if fetch {
                    self.spawn_history(chat, None);
                }
            }
            Command::LoadOlder { chat, before } => {
                if self.can_fetch() {
                    self.spawn_history(chat, Some(before));
                } else {
                    self.emit_cached_older(chat, before);
                }
            }
            Command::FetchMedia(media) => self.fetch_media(media),
            Command::SignOut => {
                self.sign_out();
                return false;
            }
        }
        true
    }

    fn emit_cached_older(&mut self, chat: ChatId, before: MessageTs) {
        let limit = self.config.history_limit;
        let chunk = self
            .db(self.cache.history(&chat, Some(before), limit))
            .unwrap_or(HistoryChunk { chat, messages: Vec::new(), reached_start: false, replaces: None });
        self.emit(ClientEvent::History(chunk));
    }

    // -- connection lifecycle -------------------------------------------------------------

    fn start_connect(&mut self) {
        self.epoch += 1;
        self.phase = Phase::Connecting;
        self.set_connection(ConnectionState::Connecting);
        let (backend, inputs, epoch) = (self.backend.clone(), self.inputs.clone(), self.epoch);
        tokio::spawn(async move {
            let result = connect(&*backend).await;
            post(&inputs, Input::Connected { epoch, result });
        });
    }

    fn handle_connected(&mut self, result: Result<Connected, BackendError>) {
        let (account, chats, live) = match result {
            Ok(connected) => connected,
            Err(BackendError::SessionExpired) => return self.expire(),
            Err(error) => {
                tracing::warn!(%error, "connecting failed");
                return self.connection_lost();
            }
        };
        tracing::info!(chats = chats.len(), "connected");
        if account != self.account {
            self.switch_account(account);
        }
        let stored = self.cache.replace_chats(&chats);
        if self.db(stored).is_some() {
            let sorted = self.db(self.cache.chats()).unwrap_or(chats);
            self.emit(ClientEvent::ChatsReplaced(sorted));
        } else {
            self.emit(ClientEvent::ChatsReplaced(chats));
        }
        self.live_pump = Some(tokio::spawn(pump_live(self.epoch, live, self.inputs.clone())));
        self.become_online(false);
    }

    fn become_online(&mut self, refresh_chats: bool) {
        self.phase = Phase::Online;
        self.attempt = 0;
        // A connection starts with nothing watched, including after a reconnect.
        self.backend.watch(self.open_chat.as_ref());
        self.set_connection(ConnectionState::Online);
        if refresh_chats {
            let (backend, inputs) = (self.backend.clone(), self.inputs.clone());
            tokio::spawn(async move { post(&inputs, Input::Chats(backend.chat_list().await)) });
        }
        self.spawn_catch_up();
        for media in std::mem::take(&mut self.media_deferred) {
            self.fetch_media(media);
        }
    }

    fn connection_lost(&mut self) {
        if self.phase == Phase::Expired {
            return;
        }
        if let Some(pump) = self.live_pump.take() {
            pump.abort();
        }
        self.phase = Phase::Offline;
        self.epoch += 1;
        self.set_connection(ConnectionState::Offline);
        let factor = 1u32 << self.attempt.min(16);
        let delay = self.config.reconnect_base.saturating_mul(factor).min(self.config.reconnect_max);
        self.attempt = self.attempt.saturating_add(1);
        tracing::info!(retry_in_ms = delay.as_millis() as u64, "offline");
        let (inputs, epoch) = (self.inputs.clone(), self.epoch);
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            post(&inputs, Input::Retry(epoch));
        });
    }

    fn expire(&mut self) {
        if self.phase == Phase::Expired {
            return;
        }
        if let Some(pump) = self.live_pump.take() {
            pump.abort();
        }
        self.phase = Phase::Expired;
        tracing::warn!("session expired");
        self.epoch += 1;
        self.media_deferred.clear();
        remove_ignoring_missing(fs::remove_file(&self.config.session_file));
        self.emit(ClientEvent::SessionExpired);
    }

    /// The server reports another account than the stored session claimed: never mix caches.
    fn switch_account(&mut self, account: UserId) {
        self.cache = open_cache(&self.config.runtime_dir, &account);
        self.account = account;
        self.open_chat = None;
        self.live_log.clear();
    }

    fn sign_out(&mut self) {
        if let Some(pump) = self.live_pump.take() {
            pump.abort();
        }
        self.phase = Phase::Expired;
        tracing::info!("signing out");
        self.epoch += 1;
        remove_ignoring_missing(fs::remove_file(&self.config.session_file));
        // Release the database file before deleting it.
        self.cache = Cache::open_in_memory().expect("in-memory sqlite always opens");
        remove_ignoring_missing(cache::remove_files(&self.config.runtime_dir, &self.account));
        remove_ignoring_missing(fs::remove_dir_all(media_dir(&self.config.runtime_dir, &self.account)));
        self.emit(ClientEvent::SignedOut);
    }

    // -- live events ----------------------------------------------------------------------

    fn handle_live(&mut self, event: LiveEvent) {
        match event {
            LiveEvent::MessageUpserted(message) => {
                self.record_live(&message.chat, LiveChange::Upsert(message.clone()));
                self.db(self.cache.upsert_message(&message));
                self.emit(ClientEvent::MessageUpserted(message.clone()));
                self.update_preview(&message);
            }
            LiveEvent::MessageDeleted { chat, timestamp } => {
                self.record_live(&chat, LiveChange::Delete(timestamp));
                self.db(self.cache.mark_deleted(&chat, timestamp));
                self.emit(ClientEvent::MessageDeleted { chat, timestamp });
            }
            LiveEvent::ChatChanged(summary) => {
                self.db(self.cache.upsert_chat(&summary));
                self.emit(ClientEvent::ChatUpdated(summary));
            }
            LiveEvent::Disconnected => {
                if self.phase == Phase::Online {
                    self.phase = Phase::Reconnecting;
                    self.set_connection(ConnectionState::Connecting);
                }
            }
            LiveEvent::Reconnected => {
                if matches!(self.phase, Phase::Online | Phase::Reconnecting) {
                    self.become_online(true);
                }
            }
            LiveEvent::Failed(BackendError::SessionExpired) => self.expire(),
            LiveEvent::Failed(error) => {
                tracing::warn!(%error, "live connection failed");
                self.connection_lost();
            }
        }
    }

    fn record_live(&mut self, chat: &ChatId, change: LiveChange) {
        self.live_seq += 1;
        let log = self.live_log.entry(chat.clone()).or_default();
        if log.len() == LIVE_LOG_CAP {
            log.pop_front();
        }
        log.push_back((self.live_seq, change));
    }

    /// Keeps the row preview and unread counter current even when the backend only forwards
    /// the message. A later `ChatChanged` carries the authoritative values and replaces this.
    fn update_preview(&mut self, message: &Message) {
        let Some(mut chat) = self.db(self.cache.chat(&message.chat)).flatten() else { return };
        let Some(text) = preview_text(&message.body) else { return };
        let last = chat.last_message.as_ref().map(|p| p.timestamp);
        if last.is_some_and(|last| last > message.timestamp) {
            return;
        }
        if last != Some(message.timestamp) && !message.outgoing {
            chat.unread = chat.unread.saturating_add(1);
        }
        chat.last_message =
            Some(MessagePreview { author: message.author_name.clone(), text, timestamp: message.timestamp });
        self.db(self.cache.upsert_chat(&chat));
        self.emit(ClientEvent::ChatUpdated(chat));
    }

    fn handle_chats(&mut self, result: Result<Vec<ChatSummary>, BackendError>) {
        match result {
            Ok(chats) => {
                let stored = self.cache.replace_chats(&chats);
                self.db(stored);
                let sorted = self.db(self.cache.chats()).unwrap_or(chats);
                self.emit(ClientEvent::ChatsReplaced(sorted));
            }
            Err(BackendError::SessionExpired) => self.expire(),
            Err(_) => {}
        }
    }

    // -- history --------------------------------------------------------------------------

    fn spawn_history(&self, chat: ChatId, before: Option<MessageTs>) {
        let known_newest = self.db(self.cache.newest_ts(&chat)).flatten();
        let (backend, inputs, limit, mark) =
            (self.backend.clone(), self.inputs.clone(), self.config.history_limit, self.live_seq);
        tokio::spawn(async move {
            let result = backend.history(&chat, before, limit).await;
            post(&inputs, Input::History(HistoryFetch { chat, before, known_newest, mark, result }));
        });
    }

    /// Refetches the newest chunk of the open chat and of every chat with cached history, one
    /// at a time, so messages missed while disconnected show up and deleted ones vanish.
    fn spawn_catch_up(&self) {
        let mut chats: Vec<ChatId> = self.open_chat.iter().cloned().collect();
        let cached: HashSet<ChatId> = self.db(self.cache.chats_with_messages()).unwrap_or_default().into_iter().collect();
        for summary in self.db(self.cache.chats()).unwrap_or_default() {
            if cached.contains(&summary.id) && !chats.contains(&summary.id) {
                chats.push(summary.id);
            }
        }
        let targets: Vec<(ChatId, Option<MessageTs>)> = chats
            .into_iter()
            .map(|chat| {
                let newest = self.db(self.cache.newest_ts(&chat)).flatten();
                (chat, newest)
            })
            .collect();
        let (backend, inputs, limit, mark) =
            (self.backend.clone(), self.inputs.clone(), self.config.history_limit, self.live_seq);
        tokio::spawn(async move {
            for (chat, known_newest) in targets {
                let result = backend.history(&chat, None, limit).await;
                let stop = matches!(result, Err(BackendError::SessionExpired | BackendError::Network(_)));
                post(&inputs, Input::History(HistoryFetch { chat, before: None, known_newest, mark, result }));
                if stop {
                    break;
                }
            }
        });
    }

    fn handle_history(&mut self, fetch: HistoryFetch) {
        match fetch.result {
            Ok(chunk) => {
                let chunk = self.apply_chunk(&fetch.chat, fetch.before, fetch.known_newest, fetch.mark, chunk);
                self.emit(ClientEvent::History(chunk));
            }
            Err(BackendError::SessionExpired) => self.expire(),
            Err(error) => {
                tracing::warn!(chat = %fetch.chat.0, %error, "history fetch failed");
                // Paging must always answer, or the UI would wait forever for the chunk.
                if let Some(before) = fetch.before {
                    self.emit_cached_older(fetch.chat, before);
                }
            }
        }
    }

    /// Replaces the cached time range the chunk covers (see `Cache::replace_range`), after
    /// replaying live changes that arrived while the request was in flight: the server snapshot
    /// predates them, so it would resurrect deleted messages or undo edits.
    fn apply_chunk(
        &mut self,
        chat: &ChatId,
        before: Option<MessageTs>,
        known_newest: Option<MessageTs>,
        mark: u64,
        mut chunk: HistoryChunk,
    ) -> HistoryChunk {
        let (min, max) = match (chunk.messages.first(), chunk.messages.last()) {
            (Some(first), Some(last)) => (Some(first.timestamp), Some(last.timestamp)),
            _ => (None, None),
        };
        // An empty chunk proves something only when it also says the chat starts here.
        if min.is_none() && !chunk.reached_start {
            return chunk;
        }
        let raced: Vec<&LiveChange> = self
            .live_log
            .get(chat)
            .map(|log| log.iter().filter(|(seq, _)| *seq > mark).map(|(_, change)| change).collect())
            .unwrap_or_default();
        let (lo, hi, history_start) = match before {
            None => {
                // Live messages newer than the server snapshot stay cached: keep everything
                // after the chunk then.
                let hi = if raced.is_empty() { None } else { max };
                if !raced.is_empty() && hi.is_none() {
                    return chunk;
                }
                if chunk.reached_start {
                    (None, hi, Some(true))
                } else if known_newest.is_none_or(|newest| newest >= min.unwrap_or(MessageTs::MAX)) {
                    (min, hi, None)
                } else {
                    // The chunk does not touch what we have cached: older cache would leave a
                    // hole in the timeline, so drop it.
                    (None, hi, Some(false))
                }
            }
            Some(before) => {
                let hi = Some(before.saturating_sub(1));
                if chunk.reached_start { (None, hi, Some(true)) } else { (min, hi, None) }
            }
        };
        replay_live(&mut chunk.messages, &raced, lo, hi);
        let stored = self.cache.replace_range(chat, lo, hi, &chunk.messages, history_start);
        self.db(stored);
        chunk.replaces = Some((lo, hi));
        chunk
    }

    // -- media ----------------------------------------------------------------------------

    fn fetch_media(&mut self, media: MediaRef) {
        if let Some((file, _)) = self.db(self.cache.media(&media)).flatten() {
            let path = media_dir(&self.config.runtime_dir, &self.account).join(file);
            if path.is_file() {
                return self.emit(ClientEvent::MediaReady { media, path });
            }
        }
        match self.media_missing.get(&media) {
            Some(at) if at.elapsed() < self.config.media_missing_ttl => return,
            Some(_) => {
                self.media_missing.remove(&media);
            }
            None => {}
        }
        match self.phase {
            Phase::Expired => {}
            Phase::Online => {
                if self.media_inflight.insert(media.clone()) {
                    let (backend, inputs, slots) = (self.backend.clone(), self.inputs.clone(), self.media_slots.clone());
                    tokio::spawn(async move {
                        let Ok(_permit) = slots.acquire().await else { return };
                        let result = backend.fetch_media(&media).await;
                        post(&inputs, Input::Media { media, result });
                    });
                }
            }
            _ => {
                if !self.media_deferred.contains(&media) {
                    self.media_deferred.push(media);
                }
            }
        }
    }

    fn handle_media_result(&mut self, media: MediaRef, result: Result<Vec<u8>, BackendError>) {
        self.media_inflight.remove(&media);
        match result {
            Ok(bytes) => match self.store_media(&media, &bytes) {
                Ok(path) => self.emit(ClientEvent::MediaReady { media, path }),
                Err(e) => tracing::error!(error = %e, "storing media failed"),
            },
            Err(BackendError::SessionExpired) => self.expire(),
            // Deleted users and bots have no avatar: expected, so not a warning.
            Err(BackendError::NotFound) => {
                tracing::debug!(media = %media.id, "media not found; not asking again for a while");
                self.media_missing.insert(media, Instant::now());
            }
            // Not cached, so the UI can simply ask again later.
            Err(error) => tracing::warn!(media = %media.id, %error, "media fetch failed"),
        }
    }

    fn store_media(&self, media: &MediaRef, bytes: &[u8]) -> io::Result<PathBuf> {
        let dir = media_dir(&self.config.runtime_dir, &self.account);
        DirBuilder::new().recursive(true).mode(0o700).create(&dir)?;
        let name = media_file_name(media);
        let partial = dir.join(format!("{name}.part"));
        let mut file = OpenOptions::new().create(true).write(true).truncate(true).mode(0o600).open(&partial)?;
        file.write_all(bytes)?;
        drop(file);
        let path = dir.join(&name);
        fs::rename(&partial, &path)?;
        self.db(self.cache.put_media(media, &name, bytes.len() as u64));
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{chat, id, msg};
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::time::Instant;

    #[derive(Default)]
    struct FakeState {
        account: String,
        chats: Vec<ChatSummary>,
        history: HashMap<ChatId, Vec<Message>>,
        media: HashMap<String, Vec<u8>>,
        bootstrap_errors: VecDeque<BackendError>,
        live: Option<Sender<LiveEvent>>,
        bootstrap_calls: usize,
        history_calls: Vec<(ChatId, Option<MessageTs>, usize)>,
        watch_calls: Vec<Option<ChatId>>,
        media_calls: usize,
        /// When set, `history` takes its snapshot, then waits here before answering.
        history_gate: Option<Arc<tokio::sync::Notify>>,
    }

    #[derive(Clone, Default)]
    struct Fake(Arc<Mutex<FakeState>>);

    impl Fake {
        fn state(&self) -> std::sync::MutexGuard<'_, FakeState> {
            self.0.lock().unwrap()
        }
        fn push(&self, event: LiveEvent) {
            self.state().live.as_ref().expect("subscribed").try_send(event).unwrap();
        }
        fn set_history(&self, chat: &str, messages: Vec<Message>) {
            self.state().history.insert(id(chat), messages);
        }
    }

    impl Backend for Fake {
        async fn bootstrap(&self) -> Result<UserId, BackendError> {
            let mut s = self.state();
            s.bootstrap_calls += 1;
            match s.bootstrap_errors.pop_front() {
                Some(error) => Err(error),
                None => Ok(UserId(s.account.clone())),
            }
        }
        async fn chat_list(&self) -> Result<Vec<ChatSummary>, BackendError> {
            Ok(self.state().chats.clone())
        }
        async fn history(
            &self,
            chat: &ChatId,
            before: Option<MessageTs>,
            limit: usize,
        ) -> Result<HistoryChunk, BackendError> {
            let (chunk, gate) = {
                let mut s = self.state();
                s.history_calls.push((chat.clone(), before, limit));
                let older: Vec<Message> = s
                    .history
                    .get(chat)
                    .into_iter()
                    .flatten()
                    .filter(|m| before.is_none_or(|b| m.timestamp < b))
                    .cloned()
                    .collect();
                let reached_start = older.len() <= limit;
                let messages = older[older.len().saturating_sub(limit)..].to_vec();
                (HistoryChunk { chat: chat.clone(), messages, reached_start, replaces: None }, s.history_gate.clone())
            };
            if let Some(gate) = gate {
                gate.notified().await;
            }
            Ok(chunk)
        }
        async fn fetch_media(&self, media: &MediaRef) -> Result<Vec<u8>, BackendError> {
            let mut s = self.state();
            s.media_calls += 1;
            s.media.get(&media.id).cloned().ok_or(BackendError::NotFound)
        }
        fn watch(&self, chat: Option<&ChatId>) {
            self.state().watch_calls.push(chat.cloned());
        }
        async fn subscribe(&self) -> Result<Receiver<LiveEvent>, BackendError> {
            let (tx, rx) = async_channel::unbounded();
            self.state().live = Some(tx);
            Ok(rx)
        }
    }

    struct Harness {
        dir: tempfile::TempDir,
        fake: Fake,
        events: Receiver<ClientEvent>,
        client: ChatClient,
    }

    const UID: &str = "42";

    fn account() -> UserId {
        UserId(UID.into())
    }

    /// `configure` runs before the client starts, with the runtime dir (to pre-populate the cache).
    fn start(configure: impl FnOnce(&mut FakeState, &Path), limit: usize) -> Harness {
        let dir = tempfile::tempdir().unwrap();
        let fake = Fake::default();
        {
            let mut state = fake.state();
            state.account = UID.into();
            configure(&mut state, dir.path());
        }
        let mut config = ClientConfig::new(dir.path(), dir.path().join("session.json"), account());
        config.history_limit = limit;
        config.reconnect_base = Duration::from_millis(5);
        config.reconnect_max = Duration::from_millis(20);
        config.media_missing_ttl = Duration::from_millis(300);
        fs::write(&config.session_file, "{}").unwrap();
        let client = ChatClient::spawn(fake.clone(), config).unwrap();
        Harness { dir, fake, events: client.events(), client }
    }

    impl Harness {
        fn next(&self, what: &str, pred: impl Fn(&ClientEvent) -> bool) -> ClientEvent {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut skipped = Vec::new();
            loop {
                match self.events.try_recv() {
                    Ok(event) if pred(&event) => return event,
                    Ok(event) => skipped.push(event),
                    Err(async_channel::TryRecvError::Empty) => {
                        assert!(Instant::now() < deadline, "timed out waiting for {what}; skipped {skipped:?}");
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    Err(async_channel::TryRecvError::Closed) => panic!("event stream closed waiting for {what}"),
                }
            }
        }

        fn online(&self) {
            self.next("online", |e| *e == ClientEvent::Connection(ConnectionState::Online));
        }

        fn history(&self, what: &str) -> HistoryChunk {
            match self.next(what, |e| matches!(e, ClientEvent::History(_))) {
                ClientEvent::History(chunk) => chunk,
                _ => unreachable!(),
            }
        }

        fn cache(&self) -> Cache {
            Cache::open(self.dir.path(), &account()).unwrap()
        }
    }

    fn stamps(chunk: &HistoryChunk) -> Vec<MessageTs> {
        chunk.messages.iter().map(|m| m.timestamp).collect()
    }

    fn seed_cache(dir: &Path, chats: &[ChatSummary], chat_id: &str, messages: &[Message], start: bool) {
        let mut cache = Cache::open(dir, &account()).unwrap();
        cache.replace_chats(chats).unwrap();
        cache.replace_range(&id(chat_id), None, None, messages, Some(start)).unwrap();
    }

    #[test]
    fn startup_shows_cache_before_the_server_answers() {
        let h = start(
            |s, dir| {
                seed_cache(dir, &[chat("cached", Some(1), None)], "cached", &[], false);
                s.chats = vec![chat("fresh", Some(2), None)];
            },
            50,
        );
        let mut order = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !order.contains(&"online") {
            assert!(Instant::now() < deadline, "timed out; saw {order:?}");
            match h.events.try_recv() {
                Ok(ClientEvent::ChatsReplaced(chats)) => order.push(if chats[0].id.0 == "cached" { "cached" } else { "fresh" }),
                Ok(ClientEvent::Connection(ConnectionState::Connecting)) => order.push("connecting"),
                Ok(ClientEvent::Connection(ConnectionState::Online)) => order.push("online"),
                Ok(_) => {}
                Err(_) => std::thread::sleep(Duration::from_millis(2)),
            }
        }
        assert_eq!(order, ["cached", "connecting", "fresh", "online"]);
    }


    #[test]
    fn offline_start_serves_cached_history_with_offline_indicator() {
        let h = start(
            |s, dir| {
                seed_cache(dir, &[chat("a", Some(2), None)], "a", &[msg("a", 1, "one"), msg("a", 2, "two")], true);
                s.bootstrap_errors = std::iter::repeat_n(BackendError::Network("down".into()), 1000).collect();
            },
            50,
        );
        h.next("offline", |e| *e == ClientEvent::Connection(ConnectionState::Offline));
        h.client.send(Command::OpenChat(id("a")));
        let chunk = h.history("cached history");
        assert_eq!(stamps(&chunk), [1, 2]);
        assert!(chunk.reached_start);
        assert!(h.fake.state().history_calls.is_empty());
        // Paging while offline answers from cache as well.
        h.client.send(Command::LoadOlder { chat: id("a"), before: 2 });
        assert_eq!(stamps(&h.history("cached older")), [1]);
    }

    #[test]
    fn open_chat_serves_cache_then_fetches_newest() {
        let h = start(
            |s, dir| {
                seed_cache(dir, &[chat("a", Some(3), None)], "a", &[msg("a", 1, "1"), msg("a", 2, "2")], false);
                s.chats = vec![chat("a", Some(3), None)];
                s.history.insert(id("a"), vec![msg("a", 1, "1"), msg("a", 2, "2"), msg("a", 3, "3")]);
            },
            50,
        );
        h.online();
        h.history("catch-up of the cached chat");
        h.client.send(Command::OpenChat(id("a")));
        assert_eq!(stamps(&h.history("cached chunk")), [1, 2, 3]);
        let fetched = h.history("server chunk");
        assert_eq!(stamps(&fetched), [1, 2, 3]);
        assert!(fetched.reached_start);
        assert_eq!(h.fake.state().history_calls.last(), Some(&(id("a"), None, 50)));
    }

    #[test]
    fn load_older_pages_back_to_the_start() {
        let h = start(
            |s, _| {
                s.chats = vec![chat("a", Some(120), None)];
                s.history.insert(id("a"), (1..=120).map(|i| msg("a", i, "m")).collect());
            },
            50,
        );
        h.online();
        h.client.send(Command::OpenChat(id("a")));
        let newest = h.history("newest");
        assert_eq!((newest.messages.first().unwrap().timestamp, newest.messages.len(), newest.reached_start), (71, 50, false));
        h.client.send(Command::LoadOlder { chat: id("a"), before: 71 });
        let middle = h.history("middle");
        assert_eq!((stamps(&middle)[0], middle.messages.len(), middle.reached_start), (21, 50, false));
        h.client.send(Command::LoadOlder { chat: id("a"), before: 21 });
        let first = h.history("first");
        assert_eq!((stamps(&first)[0], first.messages.len(), first.reached_start), (1, 20, true));

        let cached = h.cache().history(&id("a"), None, 500).unwrap();
        assert_eq!(cached.messages.len(), 120);
        assert!(cached.reached_start);
    }

    #[test]
    fn live_upsert_updates_cache_row_preview_and_unread() {
        let h = start(|s, _| s.chats = vec![chat("a", Some(1), None), chat("b", Some(5), None)], 50);
        h.online();
        h.fake.push(LiveEvent::MessageUpserted(msg("a", 10, "hello")));
        let ClientEvent::MessageUpserted(message) = h.next("upsert", |e| matches!(e, ClientEvent::MessageUpserted(_))) else {
            unreachable!()
        };
        assert_eq!(message.timestamp, 10);
        assert_eq!(h.cache().message(&id("a"), 10).unwrap().unwrap().body, MessageBody::Text("hello".into()));
        let ClientEvent::ChatUpdated(row) = h.next("row update", |e| matches!(e, ClientEvent::ChatUpdated(_))) else {
            unreachable!()
        };
        assert_eq!((row.id.0.as_str(), row.unread, row.last_message.unwrap().text.as_str()), ("a", 1, "hello"));

        // An edit of the latest message refreshes the preview but does not count as unread.
        let mut edited = msg("a", 10, "hello!");
        edited.edited = true;
        h.fake.push(LiveEvent::MessageUpserted(edited));
        let ClientEvent::ChatUpdated(row) = h.next("edit row update", |e| matches!(e, ClientEvent::ChatUpdated(_))) else {
            unreachable!()
        };
        assert_eq!((row.unread, row.last_message.unwrap().text.as_str()), (1, "hello!"));

        // The backend's authoritative summary is forwarded and cached as is.
        let mut summary = chat("b", Some(11), Some(0));
        summary.unread = 7;
        h.fake.push(LiveEvent::ChatChanged(summary.clone()));
        assert_eq!(h.next("chat changed", |e| matches!(e, ClientEvent::ChatUpdated(c) if c.id.0 == "b")), ClientEvent::ChatUpdated(summary.clone()));
        assert_eq!(h.cache().chat(&id("b")).unwrap(), Some(summary));
    }

    #[test]
    fn live_delete_leaves_a_placeholder_in_the_cache() {
        let h = start(|s, _| s.chats = vec![chat("a", None, None)], 50);
        h.online();
        h.fake.push(LiveEvent::MessageUpserted(msg("a", 3, "oops")));
        h.next("upsert", |e| matches!(e, ClientEvent::MessageUpserted(_)));
        h.fake.push(LiveEvent::MessageDeleted { chat: id("a"), timestamp: 3 });
        assert_eq!(
            h.next("delete", |e| matches!(e, ClientEvent::MessageDeleted { .. })),
            ClientEvent::MessageDeleted { chat: id("a"), timestamp: 3 }
        );
        assert_eq!(h.cache().message(&id("a"), 3).unwrap().unwrap().body, MessageBody::Deleted);
    }

    #[test]
    fn live_changes_during_a_history_fetch_survive_its_snapshot() {
        let gate = Arc::new(tokio::sync::Notify::new());
        let h = start(
            |s, _| {
                s.chats = vec![chat("a", Some(3), None)];
                s.history.insert(id("a"), vec![msg("a", 1, "one"), msg("a", 2, "two"), msg("a", 3, "three")]);
                s.history_gate = Some(gate.clone());
            },
            50,
        );
        h.online();
        h.client.send(Command::OpenChat(id("a")));
        let deadline = Instant::now() + Duration::from_secs(5);
        while h.fake.state().history_calls.is_empty() {
            assert!(Instant::now() < deadline, "history was never requested");
            std::thread::sleep(Duration::from_millis(2));
        }
        // The server snapshot is taken; now 2 is deleted and 3 edited before it arrives.
        h.fake.push(LiveEvent::MessageDeleted { chat: id("a"), timestamp: 2 });
        let mut edited = msg("a", 3, "three!");
        edited.edited = true;
        h.fake.push(LiveEvent::MessageUpserted(edited.clone()));
        h.next("edit", |e| matches!(e, ClientEvent::MessageUpserted(_)));
        gate.notify_one();

        let chunk = h.history("raced history");
        assert_eq!(stamps(&chunk), [1, 2, 3]);
        assert_eq!(chunk.messages[1].body, MessageBody::Deleted);
        assert_eq!(chunk.messages[2], edited);
        let cached = h.cache().history(&id("a"), None, 50).unwrap();
        assert_eq!(cached.messages[1].body, MessageBody::Deleted);
        assert_eq!(cached.messages[2], edited);
    }

    #[test]
    fn reconnect_catches_up_and_drops_server_deleted_messages() {
        let h = start(
            |s, _| {
                s.chats = vec![chat("a", Some(2), None)];
                s.history.insert(id("a"), vec![msg("a", 1, "1"), msg("a", 2, "2")]);
            },
            50,
        );
        h.online();
        h.client.send(Command::OpenChat(id("a")));
        h.history("initial fetch");
        assert_eq!(stamps(&h.cache().history(&id("a"), None, 50).unwrap()), [1, 2]);

        h.fake.push(LiveEvent::Disconnected);
        h.next("connecting", |e| *e == ClientEvent::Connection(ConnectionState::Connecting));
        // While we were away: 2 was deleted, 3 and 4 arrived, the list changed.
        h.fake.set_history("a", vec![msg("a", 1, "1"), msg("a", 3, "3"), msg("a", 4, "4")]);
        h.fake.state().chats = vec![chat("a", Some(4), None), chat("new", Some(9), None)];
        h.fake.push(LiveEvent::Reconnected);
        h.online();
        let ClientEvent::ChatsReplaced(chats) = h.next("chat list reload", |e| matches!(e, ClientEvent::ChatsReplaced(_))) else {
            unreachable!()
        };
        assert_eq!(chats.len(), 2);
        let caught_up = h.next("catch-up", |e| matches!(e, ClientEvent::History(c) if c.messages.len() == 3));
        let ClientEvent::History(chunk) = caught_up else { unreachable!() };
        assert_eq!(stamps(&chunk), [1, 3, 4]);
        assert_eq!(stamps(&h.cache().history(&id("a"), None, 50).unwrap()), [1, 3, 4]);
    }

    #[test]
    fn catch_up_over_a_gap_drops_the_disconnected_older_cache() {
        let h = start(
            |s, _| {
                s.chats = vec![chat("a", Some(2), None)];
                s.history.insert(id("a"), vec![msg("a", 1, "1"), msg("a", 2, "2")]);
            },
            2,
        );
        h.online();
        h.client.send(Command::OpenChat(id("a")));
        h.history("initial fetch");
        h.fake.push(LiveEvent::Disconnected);
        h.next("connecting", |e| *e == ClientEvent::Connection(ConnectionState::Connecting));
        h.fake.set_history("a", (1..=6).map(|i| msg("a", i, "m")).collect());
        h.fake.push(LiveEvent::Reconnected);
        let ClientEvent::History(chunk) = h.next("catch-up", |e| matches!(e, ClientEvent::History(c) if c.messages.len() == 2 && c.messages[0].timestamp == 5)) else {
            unreachable!()
        };
        assert!(!chunk.reached_start);
        // The UI must drop its older messages too: the whole history was replaced.
        assert_eq!(chunk.replaces, Some((None, None)));
        let cached = h.cache().history(&id("a"), None, 50).unwrap();
        assert_eq!(stamps(&cached), [5, 6]);
        assert!(!cached.reached_start);
    }

    #[test]
    fn rejected_session_expires_once_and_deletes_the_session_file() {
        let h = start(|s, _| s.bootstrap_errors = VecDeque::from([BackendError::SessionExpired]), 50);
        h.next("expiry", |e| *e == ClientEvent::SessionExpired);
        assert!(!h.dir.path().join("session.json").exists());
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(h.fake.state().bootstrap_calls, 1, "an expired session must not be retried");
    }

    #[test]
    fn expiry_during_live_stream_is_detected() {
        let h = start(|_, _| {}, 50);
        h.online();
        h.fake.push(LiveEvent::Failed(BackendError::SessionExpired));
        h.next("expiry", |e| *e == ClientEvent::SessionExpired);
        assert!(!h.dir.path().join("session.json").exists());
    }

    #[test]
    fn network_failures_retry_with_backoff_until_online() {
        let h = start(
            |s, _| s.bootstrap_errors = VecDeque::from([BackendError::Network("a".into()), BackendError::Protocol("b".into())]),
            50,
        );
        h.next("offline", |e| *e == ClientEvent::Connection(ConnectionState::Offline));
        h.online();
        assert_eq!(h.fake.state().bootstrap_calls, 3);
    }

    #[test]
    fn closed_live_stream_triggers_a_reconnect() {
        let h = start(|_, _| {}, 50);
        h.online();
        h.fake.state().live = None;
        h.next("offline", |e| *e == ClientEvent::Connection(ConnectionState::Offline));
        h.online();
        assert_eq!(h.fake.state().bootstrap_calls, 2);
    }

    #[test]
    fn open_chat_is_watched_again_on_every_connection() {
        let h = start(|s, _| s.chats = vec![chat("a", Some(1), None)], 50);
        h.online();
        h.client.send(Command::OpenChat(id("a")));
        h.history("open");
        assert_eq!(h.fake.state().watch_calls, [None, Some(id("a"))]);
        h.fake.state().live = None;
        h.next("offline", |e| *e == ClientEvent::Connection(ConnectionState::Offline));
        h.online();
        assert_eq!(h.fake.state().watch_calls, [None, Some(id("a")), Some(id("a"))]);
    }

    #[test]
    fn missing_media_is_not_requested_again_until_the_ttl_passes() {
        let h = start(|_, _| {}, 50);
        h.online();
        let gone = MediaRef { id: "deleted_user".into(), kind: MediaKind::Avatar };
        // Within the TTL (300 ms in tests) repeated requests reach the server once.
        for _ in 0..5 {
            h.client.send(Command::FetchMedia(gone.clone()));
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(h.fake.state().media_calls, 1);
        std::thread::sleep(Duration::from_millis(350));
        h.client.send(Command::FetchMedia(gone));
        let deadline = Instant::now() + Duration::from_secs(5);
        while h.fake.state().media_calls < 2 {
            assert!(Instant::now() < deadline, "not requested again after the TTL");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn media_is_downloaded_once_into_the_account_dir() {
        let h = start(|s, _| { s.media.insert("pic".into(), b"bytes".to_vec()); }, 50);
        let media = MediaRef { id: "pic".into(), kind: MediaKind::Thumbnail };
        // Requested right away, possibly before the connection is up: must be deferred, not lost.
        h.client.send(Command::FetchMedia(media.clone()));
        let ClientEvent::MediaReady { path, .. } = h.next("media", |e| matches!(e, ClientEvent::MediaReady { .. })) else {
            unreachable!()
        };
        assert!(path.starts_with(h.dir.path().join("media").join(UID)));
        assert_eq!(fs::read(&path).unwrap(), b"bytes");
        h.client.send(Command::FetchMedia(media));
        h.next("cached media", |e| matches!(e, ClientEvent::MediaReady { .. }));
        assert_eq!(h.fake.state().media_calls, 1);
    }

    #[test]
    fn sign_out_removes_session_cache_and_media() {
        let h = start(
            |s, dir| {
                seed_cache(dir, &[chat("a", Some(1), None)], "a", &[msg("a", 1, "x")], true);
                s.chats = vec![chat("a", Some(1), None)];
                s.media.insert("pic".into(), b"bytes".to_vec());
            },
            50,
        );
        h.online();
        h.client.send(Command::FetchMedia(MediaRef { id: "pic".into(), kind: MediaKind::Avatar }));
        h.next("media", |e| matches!(e, ClientEvent::MediaReady { .. }));
        let root = h.dir.path();
        assert!(root.join("session.json").exists());
        assert!(crate::paths::cache_path(root, &account()).exists());
        assert!(root.join("media").join(UID).exists());

        h.client.send(Command::SignOut);
        h.next("signed out", |e| *e == ClientEvent::SignedOut);
        assert!(!root.join("session.json").exists());
        assert!(!crate::paths::cache_path(root, &account()).exists());
        assert!(!root.join("media").join(UID).exists());
    }

    #[test]
    fn another_account_gets_its_own_cache() {
        let h = start(
            |s, dir| {
                seed_cache(dir, &[chat("old", Some(1), None)], "old", &[msg("old", 1, "x")], true);
                s.account = "other".into();
                s.chats = vec![chat("new", Some(1), None)];
            },
            50,
        );
        h.online();
        let other = Cache::open(h.dir.path(), &UserId("other".into())).unwrap();
        assert_eq!(other.chats().unwrap().len(), 1);
        assert_eq!(h.cache().chats().unwrap()[0].id.0, "old");
    }
}
