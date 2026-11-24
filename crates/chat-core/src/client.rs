//! `ChatClient`: a dedicated thread with a Tokio runtime that serves UI commands from the
//! cache first, syncs through a [`Backend`], and publishes [`ClientEvent`]s.
//!
//! All state lives in one single-threaded event loop. Network calls run as spawned tasks that
//! post their results back as inputs, so cache writes and event emission are totally ordered.

use crate::backend::{Backend, BackendError, LiveEvent};
use crate::cache::{self, Cache, CacheError, OutboxEntry};
use crate::model::*;
use crate::paths::{commit_partial, create_partial, escape_component, media_dir, sanitize_file_name};
use async_channel::{Receiver, Sender};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;

/// Concurrent media downloads.
const MEDIA_SLOTS: usize = 4;
/// Concurrent original downloads; separate from `MEDIA_SLOTS` so a large file never starves previews.
const DOWNLOAD_SLOTS: usize = 3;
/// Minimum gap between progress events of one download, so the event channel does not fill with one event per chunk.
const DOWNLOAD_PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
/// Live changes remembered per chat to replay over history snapshots that raced them.
const LIVE_LOG_CAP: usize = 256;
/// Minimum gap between typing reports for one chat.
const TYPING_INTERVAL: Duration = Duration::from_secs(4);
/// Why an edit or deletion was not even attempted.
const OFFLINE_REASON: &str = "Нет соединения";
/// Why an edit was refused: the message is unknown, forwarded, or a reply that cannot be repeated.
const EDIT_REFUSED_REASON: &str = "Это сообщение нельзя изменить";
/// Row preview of a deleted last message, as the backend's chat summaries word it.
const DELETED_PREVIEW: &str = "Сообщение удалено";
/// Why a send was refused: it could not be stored, so it would not survive a restart.
const STORE_FAILED_REASON: &str = "Не удалось сохранить сообщение";
/// Results asked of the backend per kind: the first page only.
const SEARCH_LIMIT: usize = 10;

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
    /// After a transient send failure the queue waits `step * failures`, at most `max`.
    pub send_backoff_step: Duration,
    pub send_backoff_max: Duration,
    /// Where saved originals go; `None` when no destination is known (downloads then fail).
    pub downloads_dir: Option<PathBuf>,
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
            send_backoff_step: Duration::from_secs(5),
            send_backoff_max: Duration::from_secs(60),
            downloads_dir: None,
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
    /// `bytes` of the saved file, or why the transfer of generation `seq` produced no file.
    Download { media: MediaRef, seq: u64, result: Result<(PathBuf, u64), DownloadError> },
    DownloadProgress { media: MediaRef, seq: u64, received: u64, total: Option<u64> },
    Sent { payload: PayloadId, result: Result<Option<MessageTs>, BackendError> },
    /// The send queue's backoff of generation `.0` ended.
    OutboxResume(u64),
    Action { chat: ChatId, action: Action, result: Result<(), BackendError> },
    /// The answer to the search `id`, which may have been superseded since.
    Search { id: SearchId, result: Result<SearchHits, BackendError> },
}

/// A server write that is attempted once and not queued.
enum Action {
    Edit { timestamp: MessageTs, text: String, reply: Option<ReplyTarget> },
    Delete(MessageTs),
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
    /// Running or waiting original downloads, one per file id.
    downloads: HashMap<MediaRef, Transfer>,
    /// Generation of the next `Transfer`; lets a late result tell its own transfer from a newer one.
    download_seq: u64,
    download_slots: Arc<Semaphore>,
    /// Unconfirmed sends in queue order, mirrored in the cache.
    outbox: Vec<OutboxEntry>,
    outbox_inflight: Option<PayloadId>,
    /// Consecutive transient send failures; reset by a confirmation or a rejection.
    outbox_failures: u32,
    /// A transient failure holds the whole queue, to keep the order, until the backoff of
    /// generation `outbox_hold` ends or the connection is renewed.
    outbox_held: bool,
    outbox_hold: u64,
    /// Newest read marker issued per chat.
    read_marks: HashMap<ChatId, MessageTs>,
    /// Markers requested while we could not send; the newest per chat goes out once online.
    read_deferred: HashMap<ChatId, MessageTs>,
    typing_sent: HashMap<ChatId, tokio::time::Instant>,
    /// The newest search asked for and still awaited; an answer for any other id is dropped.
    search_latest: Option<SearchId>,
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

/// A cache that cannot be opened may hold the only copy of unsent messages and drafts, so it is
/// moved aside, never deleted, and a fresh one is created. As a last resort run without
/// persistence rather than without a client.
fn open_cache(runtime_dir: &Path, account: &UserId) -> Cache {
    match Cache::open(runtime_dir, account) {
        Ok(cache) => cache,
        Err(first) => {
            let secs = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
            match cache::move_aside(runtime_dir, account, secs) {
                Ok(moved) => tracing::error!(error = %first, moved = %moved.display(), "cache unusable, moved aside; starting a new one"),
                Err(e) => tracing::error!(error = %first, move_error = %e, "cache unusable and could not be moved aside"),
            }
            Cache::open(runtime_dir, account).unwrap_or_else(|second| {
                tracing::error!(error = %second, "cache unavailable, running in memory");
                Cache::open_in_memory().expect("in-memory sqlite always opens")
            })
        }
    }
}

/// Live changes of `chat` that arrived after `mark`, oldest first.
fn raced<'a>(log: &'a HashMap<ChatId, VecDeque<(u64, LiveChange)>>, chat: &ChatId, mark: u64) -> Vec<&'a LiveChange> {
    log.get(chat)
        .map(|log| log.iter().filter(|(seq, _)| *seq > mark).map(|(_, change)| change).collect())
        .unwrap_or_default()
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
        MessageBody::Image { caption, .. } | MessageBody::Gallery { caption, .. } => {
            Some(caption.clone().unwrap_or_else(|| "Фото".into()))
        }
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

/// A running or waiting download task; `seq` tells its late results from those of a newer transfer of the same file.
struct Transfer {
    seq: u64,
    handle: JoinHandle<()>,
}

/// Why a transfer produced no file, before it is worded for the UI.
#[derive(Debug)]
enum DownloadError {
    Backend(BackendError),
    /// The partial file could not be created in the Downloads folder.
    Destination(io::Error),
    /// Flushing or finishing the file failed.
    Disk(io::Error),
}

fn download_failure(error: &BackendError) -> DownloadFailure {
    match error {
        BackendError::NotFound => DownloadFailure::NotFound,
        BackendError::Storage(_) => DownloadFailure::Disk,
        BackendError::Network(_) => DownloadFailure::Network,
        // `SessionExpired` ends the session before this mapping is asked for.
        BackendError::SessionExpired | BackendError::Protocol(_) | BackendError::Rejected { .. } => DownloadFailure::Other,
    }
}

/// Names the kind of a failure without its text, which may echo what was asked.
fn error_kind(error: &BackendError) -> &'static str {
    match error {
        BackendError::SessionExpired => "session expired",
        BackendError::Network(_) => "network",
        BackendError::Protocol(_) => "protocol",
        BackendError::NotFound => "not found",
        BackendError::Rejected { .. } => "rejected",
        BackendError::Storage(_) => "storage",
    }
}

/// Lets a download's progress through at most once per [`DOWNLOAD_PROGRESS_INTERVAL`] and remembers
/// what was last let through, so the final report can complete it.
#[derive(Default)]
struct Throttle(Mutex<ThrottleState>);

#[derive(Default)]
struct ThrottleState {
    passed_at: Option<Instant>,
    passed: Option<u64>,
    /// The latest announced total, whether or not that report passed.
    total: Option<u64>,
}

impl Throttle {
    fn admit(&self, now: Instant, received: u64, total: Option<u64>) -> bool {
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if total.is_some() {
            state.total = total;
        }
        if state.passed_at.is_some_and(|at| now.duration_since(at) < DOWNLOAD_PROGRESS_INTERVAL) {
            return false;
        }
        state.passed_at = Some(now);
        state.passed = Some(received);
        true
    }

    /// The report that closes the stream, unless the last one let through already said `received`.
    fn last(&self, received: u64) -> Option<(u64, Option<u64>)> {
        let state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        (state.passed != Some(received)).then_some((received, state.total))
    }
}

/// One transfer: wait for a slot, stream into `<name>.part` in `dir`, then give the file its final
/// name. Every early return drops the `Partial`, which removes the unfinished file; so does aborting
/// the task.
async fn run_download<B: Backend>(
    backend: &B,
    slots: &Semaphore,
    dir: &Path,
    media: &MediaRef,
    name: Option<&str>,
    inputs: &Sender<Input>,
    seq: u64,
) -> Result<(PathBuf, u64), DownloadError> {
    let Ok(_slot) = slots.acquire().await else {
        return Err(DownloadError::Backend(BackendError::Protocol("download slots closed".into())));
    };
    let name = sanitize_file_name(name);
    let (mut file, partial) = create_partial(dir, &name).map_err(DownloadError::Destination)?;
    let throttle = Throttle::default();
    let report = |received: u64, total: Option<u64>| {
        if throttle.admit(Instant::now(), received, total) {
            post(inputs, Input::DownloadProgress { media: media.clone(), seq, received, total });
        }
    };
    let bytes = backend.download(media, &mut file, &report).await.map_err(DownloadError::Backend)?;
    file.flush().map_err(DownloadError::Disk)?;
    drop(file);
    if let Some((received, total)) = throttle.last(bytes) {
        post(inputs, Input::DownloadProgress { media: media.clone(), seq, received, total });
    }
    let path = commit_partial(partial, dir, &name).map_err(DownloadError::Disk)?;
    Ok((path, bytes))
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
            downloads: HashMap::new(),
            download_seq: 0,
            download_slots: Arc::new(Semaphore::new(DOWNLOAD_SLOTS)),
            outbox: Vec::new(),
            outbox_inflight: None,
            outbox_failures: 0,
            outbox_held: false,
            outbox_hold: 0,
            read_marks: HashMap::new(),
            read_deferred: HashMap::new(),
            typing_sent: HashMap::new(),
            search_latest: None,
        };
        tokio::spawn(pump_commands(commands, inputs));

        let cached = engine.db(engine.cache.chats()).unwrap_or_default();
        if !cached.is_empty() {
            engine.emit(ClientEvent::ChatsReplaced(cached));
        }
        engine.outbox = engine.db(engine.cache.outbox()).unwrap_or_default();
        engine.emit_drafts();
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

    fn is_online(&self) -> bool {
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
            Input::Download { media, seq, result } => self.handle_download_result(media, seq, result),
            Input::DownloadProgress { media, seq, received, total } => self.handle_download_progress(&media, seq, received, total),
            Input::Sent { payload, result } => self.handle_sent(payload, result),
            Input::OutboxResume(hold) if hold == self.outbox_hold => {
                self.outbox_held = false;
                self.pump_outbox();
            }
            Input::Action { chat, action, result } => self.handle_action(chat, action, result),
            Input::Search { id, result } => self.handle_search(id, result),
            Input::Connected { .. } | Input::Retry(_) | Input::Live { .. } | Input::LiveClosed(_) | Input::OutboxResume(_) => {}
        }
        true
    }

    fn handle_command(&mut self, command: Command) -> bool {
        match command {
            Command::OpenChat(chat) => {
                let limit = self.config.history_limit;
                let cached = self.db(self.cache.history(&chat, None, limit)).map(|chunk| self.with_outbox(chunk));
                self.open_chat = Some(chat.clone());
                self.backend.watch(Some(&chat));
                let fetch = self.is_online();
                // With a fetch coming, an empty cache is not worth an (empty) event.
                match cached {
                    Some(chunk) if !fetch || !chunk.messages.is_empty() => self.emit(ClientEvent::History(chunk)),
                    None if !fetch => self.emit(ClientEvent::History(HistoryChunk {
                        chat: chat.clone(),
                        messages: Vec::new(),
                        reached_start: false,
                        replaces: None,
                        payloads: Vec::new(),
                    })),
                    _ => {}
                }
                if fetch {
                    self.spawn_history(chat, None);
                }
            }
            Command::LoadOlder { chat, before } => {
                if self.is_online() {
                    self.spawn_history(chat, Some(before));
                } else {
                    self.emit_cached_older(chat, before);
                }
            }
            Command::FetchMedia(media) => self.fetch_media(media),
            Command::Download { media, name } => self.start_download(media, name),
            Command::CancelDownload(media) => self.cancel_download(&media),
            Command::SignOut => {
                self.sign_out();
                return false;
            }
            Command::Send { chat, text, reply_to } => self.send(chat, text, reply_to),
            Command::Retry(payload) => self.retry(&payload),
            Command::Edit { chat, timestamp, text } => self.edit(chat, timestamp, text),
            Command::Delete { chat, timestamp } => self.run_action(chat, Action::Delete(timestamp)),
            Command::MarkRead { chat, timestamp } => self.mark_read(chat, timestamp),
            Command::Typing(chat) => self.typing(chat),
            Command::SaveDraft(draft) => self.save_draft(draft),
            Command::Search { id, query } => self.search(id, query),
            Command::CancelSearch => self.search_latest = None,
        }
        true
    }

    fn emit_cached_older(&mut self, chat: ChatId, before: MessageTs) {
        let limit = self.config.history_limit;
        let chunk = self
            .db(self.cache.history(&chat, Some(before), limit))
            .unwrap_or(HistoryChunk { chat, messages: Vec::new(), reached_start: false, replaces: None, payloads: Vec::new() });
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
        // A fresh connection retries a held queue at once instead of waiting out the backoff.
        self.outbox_hold += 1;
        self.outbox_held = false;
        self.pump_outbox();
        for (chat, timestamp) in std::mem::take(&mut self.read_deferred) {
            self.send_read_marker(chat, timestamp);
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
        self.abort_downloads();
        remove_ignoring_missing(fs::remove_file(&self.config.session_file));
        self.emit(ClientEvent::SessionExpired);
    }

    /// The server reports another account than the stored session claimed: never mix caches.
    fn switch_account(&mut self, account: UserId) {
        self.cache = open_cache(&self.config.runtime_dir, &account);
        self.account = account;
        self.open_chat = None;
        self.live_log.clear();
        self.outbox = self.db(self.cache.outbox()).unwrap_or_default();
        self.outbox_inflight = None;
        self.read_marks.clear();
        self.read_deferred.clear();
        self.typing_sent.clear();
        self.emit_drafts();
    }

    fn sign_out(&mut self) {
        if let Some(pump) = self.live_pump.take() {
            pump.abort();
        }
        self.phase = Phase::Expired;
        tracing::info!("signing out");
        self.epoch += 1;
        self.abort_downloads();
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
                self.emit(ClientEvent::MessageDeleted { chat: chat.clone(), timestamp });
                self.mark_preview_deleted(&chat, timestamp);
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
            LiveEvent::Echo { payload, message } => {
                // The entry is gone when a confirmation settled it first; then this is the
                // server copy of an ordinary message (also a send from another client).
                if let Some(entry) = self.take_outbox(&payload) {
                    self.emit(ClientEvent::PendingSettled { chat: entry.chat, payload });
                }
                self.handle_live(LiveEvent::MessageUpserted(message));
            }
            // The same account typing in another client is not "someone is typing".
            LiveEvent::Typing { user, .. } if user == self.account => {}
            LiveEvent::Typing { chat, user, name, timeout } => {
                self.emit(ClientEvent::Typing { chat, user, name, until: Instant::now() + timeout });
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
        // Same attribution as the backend's chat summaries.
        let author = if message.outgoing { Some("Вы".to_owned()) } else { message.author_name.clone() };
        chat.last_message = Some(MessagePreview { author, text, timestamp: message.timestamp });
        self.db(self.cache.upsert_chat(&chat));
        self.emit(ClientEvent::ChatUpdated(chat));
    }

    /// The backend's chat summaries show this text for a deleted last message; the row keeps
    /// showing the old text until then unless it is rewritten here.
    fn mark_preview_deleted(&mut self, chat: &ChatId, timestamp: MessageTs) {
        let Some(mut row) = self.db(self.cache.chat(chat)).flatten() else { return };
        let Some(preview) = row.last_message.as_mut().filter(|p| p.timestamp == timestamp) else { return };
        preview.text = DELETED_PREVIEW.into();
        self.db(self.cache.upsert_chat(&row));
        self.emit(ClientEvent::ChatUpdated(row));
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

    // -- sending --------------------------------------------------------------------------

    fn send(&mut self, chat: ChatId, text: String, reply_to: Option<ReplyTarget>) {
        let entry = OutboxEntry {
            payload: PayloadId(uuid::Uuid::new_v4().to_string()),
            created_us: self.next_local_ts(&chat),
            chat,
            text,
            reply_to,
            failed: None,
        };
        // Stored before any network call, together with the draft's removal: a restart
        // resends it under the same id. Without a durable entry nothing is sent, and the UI
        // gets the text back.
        if !self.cache.is_durable() || self.db(self.cache.enqueue(&entry)).is_none() {
            let draft = Draft { chat: entry.chat, text: entry.text, reply_to: entry.reply_to };
            return self.emit(ClientEvent::SendRefused { draft, reason: STORE_FAILED_REASON.into() });
        }
        let message = self.pending_message(&entry);
        self.outbox.push(entry);
        self.emit(ClientEvent::MessageUpserted(message));
        self.pump_outbox();
    }

    /// Sorts after everything known in the chat, so the pending message stays at the bottom
    /// even when the local clock is behind the server's.
    fn next_local_ts(&self, chat: &ChatId) -> MessageTs {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_micros() as MessageTs);
        let cached = self.db(self.cache.newest_ts(chat)).flatten();
        let listed = self.db(self.cache.chat(chat)).flatten().and_then(|c| c.last_message).map(|p| p.timestamp);
        let queued = self.outbox.iter().filter(|e| e.chat == *chat).map(|e| e.created_us).max();
        [cached, listed, queued].into_iter().flatten().max().map_or(now, |newest| now.max(newest + 1))
    }

    fn outgoing_message(&self, entry: &OutboxEntry, timestamp: MessageTs, delivery: Delivery) -> Message {
        Message {
            chat: entry.chat.clone(),
            timestamp,
            author: self.account.clone(),
            author_name: None,
            author_avatar: None,
            outgoing: true,
            body: MessageBody::Text(entry.text.clone()),
            edited: false,
            reply_to: entry.reply_to.as_ref().map(|target| ReplyQuote {
                author: target.author.clone(),
                text: target.text.clone(),
                timestamp: Some(target.timestamp),
            }),
            forwarded_from: None,
            delivery,
        }
    }

    fn pending_message(&self, entry: &OutboxEntry) -> Message {
        let payload = entry.payload.clone();
        let delivery = match &entry.failed {
            Some(reason) => Delivery::Failed { payload, reason: reason.clone() },
            None => Delivery::Pending(payload),
        };
        self.outgoing_message(entry, entry.created_us, delivery)
    }

    /// Settles the outbox entries whose `PayloadId` the chunk carries. Such an entry was
    /// delivered although its confirmation and echo were lost: the chunk (of any page) holds
    /// its server copy, so the entry leaves the outbox for good instead of being resent.
    fn settle_delivered(&mut self, chunk: &HistoryChunk) {
        let delivered: Vec<PayloadId> = self
            .outbox
            .iter()
            .filter(|e| e.chat == chunk.chat && chunk.payloads.contains(&e.payload))
            .map(|e| e.payload.clone())
            .collect();
        for payload in delivered {
            if let Some(entry) = self.take_outbox(&payload) {
                self.emit(ClientEvent::PendingSettled { chat: entry.chat, payload });
            }
        }
    }

    /// Adds the chat's unconfirmed messages to a chunk that ends at the newest message.
    fn with_outbox(&self, mut chunk: HistoryChunk) -> HistoryChunk {
        let pending: Vec<Message> =
            self.outbox.iter().filter(|e| e.chat == chunk.chat).map(|e| self.pending_message(e)).collect();
        if !pending.is_empty() {
            chunk.messages.extend(pending);
            chunk.messages.sort_by_key(|m| m.timestamp);
        }
        chunk
    }

    fn take_outbox(&mut self, payload: &PayloadId) -> Option<OutboxEntry> {
        let index = self.outbox.iter().position(|e| e.payload == *payload)?;
        self.db(self.cache.remove_outbox(payload));
        Some(self.outbox.remove(index))
    }

    /// Starts the next send unless one is in flight, the queue is held, or we are offline.
    /// Failed entries are skipped: they wait for `Retry` and must not block other chats.
    fn pump_outbox(&mut self) {
        if !self.is_online() || self.outbox_held || self.outbox_inflight.is_some() {
            return;
        }
        let Some(entry) = self.outbox.iter().find(|e| e.failed.is_none()).cloned() else { return };
        self.outbox_inflight = Some(entry.payload.clone());
        let (backend, inputs) = (self.backend.clone(), self.inputs.clone());
        tokio::spawn(async move {
            let result = backend.send_text(&entry.chat, &entry.payload, &entry.text, entry.reply_to.as_ref()).await;
            post(&inputs, Input::Sent { payload: entry.payload, result });
        });
    }

    fn handle_sent(&mut self, payload: PayloadId, result: Result<Option<MessageTs>, BackendError>) {
        if self.outbox_inflight.as_ref() == Some(&payload) {
            self.outbox_inflight = None;
        }
        match result {
            Ok(timestamp) => {
                self.outbox_failures = 0;
                self.confirm(payload, timestamp);
            }
            Err(BackendError::SessionExpired) => return self.expire(),
            Err(BackendError::Rejected { reason, permanent: true }) => {
                self.outbox_failures = 0;
                self.reject(&payload, reason);
            }
            Err(error) => {
                tracing::warn!(%error, "send failed, will retry");
                // An echo may have settled the entry while the request was in flight.
                if self.outbox.iter().any(|e| e.payload == payload) {
                    self.hold_outbox();
                }
            }
        }
        self.pump_outbox();
    }

    /// The server accepted the send. `timestamp` is missing when it answered a duplicate
    /// without details; then the copy is loaded from the server's history instead.
    fn confirm(&mut self, payload: PayloadId, timestamp: Option<MessageTs>) {
        // Already settled by the echo: nothing left to do.
        let Some(entry) = self.take_outbox(&payload) else { return };
        self.emit(ClientEvent::PendingSettled { chat: entry.chat.clone(), payload });
        match timestamp {
            Some(timestamp) => {
                let message = self.outgoing_message(&entry, timestamp, Delivery::Sent);
                self.handle_live(LiveEvent::MessageUpserted(message));
            }
            None => self.spawn_history(entry.chat, None),
        }
    }

    fn reject(&mut self, payload: &PayloadId, reason: String) {
        let Some(entry) = self.outbox.iter_mut().find(|e| e.payload == *payload) else { return };
        entry.failed = Some(reason);
        let entry = entry.clone();
        self.db(self.cache.put_outbox(&entry));
        self.emit(ClientEvent::MessageUpserted(self.pending_message(&entry)));
    }

    fn retry(&mut self, payload: &PayloadId) {
        let Some(entry) = self.outbox.iter_mut().find(|e| e.payload == *payload && e.failed.is_some()) else { return };
        entry.failed = None;
        let entry = entry.clone();
        self.db(self.cache.put_outbox(&entry));
        self.emit(ClientEvent::MessageUpserted(self.pending_message(&entry)));
        self.pump_outbox();
    }

    fn hold_outbox(&mut self) {
        self.outbox_failures = self.outbox_failures.saturating_add(1);
        let delay = self.config.send_backoff_step.saturating_mul(self.outbox_failures).min(self.config.send_backoff_max);
        self.outbox_held = true;
        self.outbox_hold += 1;
        let (inputs, hold) = (self.inputs.clone(), self.outbox_hold);
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            post(&inputs, Input::OutboxResume(hold));
        });
    }

    // -- edits, deletions, read markers, typing, drafts -----------------------------------

    /// An edit rewrites the whole message, so a reply must repeat its target or the server
    /// drops the quote. A reply whose original is unknown cannot be repeated, and forwarded
    /// messages are not edited at all (unverified, and other messengers forbid it): both are
    /// refused, as is a message that is not cached.
    fn edit(&mut self, chat: ChatId, timestamp: MessageTs, text: String) {
        let reply = match self.db(self.cache.message(&chat, timestamp)).flatten() {
            Some(Message { forwarded_from: None, reply_to: None, .. }) => Ok(None),
            Some(Message { forwarded_from: None, reply_to: Some(quote), .. }) => match quote.timestamp {
                Some(original) => Ok(Some(ReplyTarget { timestamp: original, author: quote.author, text: quote.text })),
                None => Err(()),
            },
            _ => Err(()),
        };
        match reply {
            Ok(reply) => self.run_action(chat, Action::Edit { timestamp, text, reply }),
            Err(()) => self.emit(ClientEvent::ActionFailed { chat, reason: EDIT_REFUSED_REASON.into() }),
        }
    }

    /// Edits and deletions target a message by timestamp, so they are never queued: a replay
    /// could undo a later change.
    fn run_action(&mut self, chat: ChatId, action: Action) {
        if !self.is_online() {
            return self.emit(ClientEvent::ActionFailed { chat, reason: OFFLINE_REASON.into() });
        }
        let (backend, inputs) = (self.backend.clone(), self.inputs.clone());
        tokio::spawn(async move {
            let result = match &action {
                Action::Edit { timestamp, text, reply } => backend.edit(&chat, *timestamp, text, reply.as_ref()).await,
                Action::Delete(timestamp) => backend.delete(&chat, *timestamp).await,
            };
            post(&inputs, Input::Action { chat, action, result });
        });
    }

    fn handle_action(&mut self, chat: ChatId, action: Action, result: Result<(), BackendError>) {
        match result {
            Ok(()) => match action {
                Action::Edit { timestamp, text, .. } => {
                    let Some(mut message) = self.db(self.cache.message(&chat, timestamp)).flatten() else { return };
                    message.body = MessageBody::Text(text);
                    message.edited = true;
                    self.handle_live(LiveEvent::MessageUpserted(message));
                }
                Action::Delete(timestamp) => self.handle_live(LiveEvent::MessageDeleted { chat, timestamp }),
            },
            Err(BackendError::SessionExpired) => self.expire(),
            Err(error) => {
                tracing::warn!(chat = %chat.0, %error, "edit or deletion failed");
                let reason = match error {
                    BackendError::Rejected { reason, .. } => reason,
                    BackendError::Network(_) => OFFLINE_REASON.into(),
                    other => other.to_string(),
                };
                self.emit(ClientEvent::ActionFailed { chat, reason });
            }
        }
    }

    // -- search ---------------------------------------------------------------------------

    /// Server search is read-only and neither queued nor cached: only the newest request's
    /// answer is delivered, and while we cannot ask it fails at once.
    fn search(&mut self, id: SearchId, query: String) {
        self.search_latest = Some(id);
        if !self.is_online() {
            return self.emit(ClientEvent::SearchFailed { id, reason: SearchFailure::Offline });
        }
        let (backend, inputs) = (self.backend.clone(), self.inputs.clone());
        tokio::spawn(async move {
            let result = backend.search(&query, SEARCH_LIMIT).await;
            post(&inputs, Input::Search { id, result });
        });
    }

    fn handle_search(&mut self, id: SearchId, result: Result<SearchHits, BackendError>) {
        if self.search_latest != Some(id) {
            return;
        }
        match result {
            Ok(hits) => self.emit(ClientEvent::SearchResults { id, hits }),
            Err(BackendError::SessionExpired) => self.expire(),
            Err(error) => {
                tracing::warn!(kind = error_kind(&error), "search failed");
                self.emit(ClientEvent::SearchFailed { id, reason: SearchFailure::Failed });
            }
        }
    }

    /// Never marks twice for the same or an older message. The counter drops at once; the
    /// request goes out now, or once online when it cannot (only the newest per chat is kept).
    fn mark_read(&mut self, chat: ChatId, timestamp: MessageTs) {
        if self.read_marks.get(&chat).is_some_and(|&marked| marked >= timestamp) {
            return;
        }
        self.read_marks.insert(chat.clone(), timestamp);
        if let Some(mut summary) = self.db(self.cache.chat(&chat)).flatten() {
            let newest = summary.last_message.as_ref().is_none_or(|last| timestamp >= last.timestamp);
            if summary.unread > 0 && newest {
                summary.unread = 0;
                self.db(self.cache.upsert_chat(&summary));
                self.emit(ClientEvent::ChatUpdated(summary));
            }
        }
        if self.is_online() {
            self.send_read_marker(chat, timestamp);
        } else {
            self.read_deferred.insert(chat, timestamp);
        }
    }

    fn send_read_marker(&self, chat: ChatId, timestamp: MessageTs) {
        let backend = self.backend.clone();
        tokio::spawn(async move {
            if let Err(error) = backend.mark_seen(&chat, timestamp).await {
                tracing::warn!(chat = %chat.0, %error, "read marker failed");
            }
        });
    }

    fn typing(&mut self, chat: ChatId) {
        let now = tokio::time::Instant::now();
        if !self.is_online() || self.typing_sent.get(&chat).is_some_and(|&at| now.duration_since(at) < TYPING_INTERVAL) {
            return;
        }
        self.typing_sent.insert(chat.clone(), now);
        let backend = self.backend.clone();
        tokio::spawn(async move {
            if let Err(error) = backend.typing(&chat).await {
                tracing::debug!(chat = %chat.0, %error, "typing report failed");
            }
        });
    }

    fn save_draft(&mut self, draft: Draft) {
        let stored = if draft.text.is_empty() && draft.reply_to.is_none() {
            self.cache.remove_draft(&draft.chat)
        } else {
            self.cache.put_draft(&draft)
        };
        self.db(stored);
    }

    fn emit_drafts(&self) {
        self.emit(ClientEvent::Drafts(self.db(self.cache.drafts()).unwrap_or_default()));
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
                self.settle_delivered(&chunk);
                let chunk = if fetch.before.is_none() { self.with_outbox(chunk) } else { chunk };
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

    /// Stores the chunk (see `store_chunk`) and adds the cached messages newer than it. They
    /// are live messages that raced the request; a chunk that ends at the server snapshot
    /// would hide them from a UI whose timeline is not loaded yet, which drops live upserts:
    /// a send confirmed while the first history was in flight would stay invisible. The cache,
    /// unlike the capped live log, never forgets them.
    fn apply_chunk(
        &mut self,
        chat: &ChatId,
        before: Option<MessageTs>,
        known_newest: Option<MessageTs>,
        mark: u64,
        chunk: HistoryChunk,
    ) -> HistoryChunk {
        // An empty chunk speaks for the whole chat only when it says the chat starts here.
        let covers_newest = before.is_none() && (!chunk.messages.is_empty() || chunk.reached_start);
        let mut chunk = self.store_chunk(chat, before, known_newest, mark, chunk);
        if covers_newest {
            let newest = chunk.messages.last().map(|m| m.timestamp);
            let tail = self.db(self.cache.newer_than(chat, newest)).unwrap_or_default();
            chunk.messages.extend(tail);
        }
        chunk
    }

    /// Replaces the cached time range the chunk covers (see `Cache::replace_range`), after
    /// replaying live changes that arrived while the request was in flight: the server snapshot
    /// predates them, so it would resurrect deleted messages or undo edits.
    fn store_chunk(
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
        let raced = raced(&self.live_log, chat, mark);
        let (lo, hi, history_start) = match before {
            None => {
                // Live messages newer than the server snapshot stay cached: keep everything
                // after the chunk then. An empty snapshot vouches only for what was cached when
                // the request started (`known_newest`); everything cached above that arrived
                // live meanwhile and is kept, even when the capped live log forgot it.
                let hi = if raced.is_empty() { None } else { max.or(known_newest).or(Some(MessageTs::MIN)) };
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
        // Originals go through `Command::Download`; this path is for previews only.
        if media.kind == MediaKind::File {
            tracing::warn!("fetch_media ignores originals");
            return;
        }
        if let Some((file, _)) = self.db(self.cache.media(&media)).flatten() {
            let path = media_dir(&self.config.runtime_dir, &self.account).join(file);
            if path.is_file() {
                return self.emit(ClientEvent::MediaReady { media, path });
            }
        }
        match self.media_missing.get(&media) {
            Some(at) if at.elapsed() < self.config.media_missing_ttl => return self.emit(ClientEvent::MediaUnavailable(media)),
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
                self.media_missing.insert(media.clone(), Instant::now());
                self.emit(ClientEvent::MediaUnavailable(media));
            }
            // Not cached; the UI asks again on its next refresh.
            Err(error) => {
                tracing::warn!(media = %media.id, %error, "media fetch failed");
                self.emit(ClientEvent::MediaUnavailable(media));
            }
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

    // -- downloads ------------------------------------------------------------------------

    fn start_download(&mut self, media: MediaRef, name: Option<String>) {
        if media.kind != MediaKind::File {
            tracing::warn!("download ignored: not an original");
            return;
        }
        // One transfer per file id, however many messages show it.
        if self.downloads.contains_key(&media) || self.phase == Phase::Expired {
            return;
        }
        // An explicit action is refused, never queued: a hidden queue would start a transfer much later.
        if !self.is_online() {
            return self.emit(ClientEvent::DownloadFailed { media, reason: DownloadFailure::Offline });
        }
        let Some(dir) = self.config.downloads_dir.clone().filter(|dir| dir.is_dir()) else {
            return self.emit(ClientEvent::DownloadFailed { media, reason: DownloadFailure::Destination });
        };
        self.download_seq += 1;
        let seq = self.download_seq;
        let (backend, inputs, slots) = (self.backend.clone(), self.inputs.clone(), self.download_slots.clone());
        let task_media = media.clone();
        let handle = tokio::spawn(async move {
            let result = run_download(&*backend, &slots, &dir, &task_media, name.as_deref(), &inputs, seq).await;
            post(&inputs, Input::Download { media: task_media, seq, result });
        });
        self.downloads.insert(media, Transfer { seq, handle });
    }

    fn cancel_download(&mut self, media: &MediaRef) {
        if let Some(transfer) = self.downloads.remove(media) {
            // The task's `Partial` removes the unfinished file when the aborted future drops.
            transfer.handle.abort();
            self.emit(ClientEvent::DownloadCancelled(media.clone()));
        }
    }

    /// Sign-out and expiry: every unfinished file goes, saved ones stay.
    fn abort_downloads(&mut self) {
        for (_, transfer) in self.downloads.drain() {
            transfer.handle.abort();
        }
    }

    fn handle_download_progress(&mut self, media: &MediaRef, seq: u64, received: u64, total: Option<u64>) {
        if self.downloads.get(media).is_some_and(|transfer| transfer.seq == seq) {
            self.emit(ClientEvent::DownloadProgress { media: media.clone(), received, total });
        }
    }

    fn handle_download_result(&mut self, media: MediaRef, seq: u64, result: Result<(PathBuf, u64), DownloadError>) {
        match self.downloads.get(&media) {
            // A newer transfer of the same file owns the state now; this one was cancelled meanwhile.
            Some(transfer) if transfer.seq != seq => return,
            Some(_) => {
                self.downloads.remove(&media);
            }
            // Cancelled, but the result was already on its way: reported as it happened.
            None => {}
        }
        let reason = match result {
            Ok((path, bytes)) => {
                tracing::info!(bytes, "download saved");
                return self.emit(ClientEvent::Downloaded { media, path });
            }
            Err(DownloadError::Backend(BackendError::SessionExpired)) => return self.expire(),
            Err(DownloadError::Backend(error)) => {
                tracing::warn!(%error, "download failed");
                download_failure(&error)
            }
            Err(DownloadError::Destination(error)) => {
                tracing::warn!(%error, "download destination unusable");
                DownloadFailure::Destination
            }
            Err(DownloadError::Disk(error)) => {
                tracing::warn!(%error, "download could not be written");
                DownloadFailure::Disk
            }
        };
        self.emit(ClientEvent::DownloadFailed { media, reason });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{chat, id, msg};
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::time::Instant;

    /// A write the engine asked the backend to perform.
    #[derive(Clone, Debug, PartialEq)]
    enum Call {
        Send { chat: ChatId, payload: PayloadId, text: String, reply: Option<ReplyTarget> },
        Edit(ChatId, MessageTs, String, Option<ReplyTarget>),
        Delete(ChatId, MessageTs),
        Seen(ChatId, MessageTs),
        Typing(ChatId),
    }

    /// Server timestamp `send_text` answers with when no result is scripted.
    const SERVER_TS: MessageTs = 100;

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
        /// Original files by id, for `download`.
        files: HashMap<String, Vec<u8>>,
        /// Every `download` call, in order.
        download_calls: Vec<MediaRef>,
        /// Answers of `download` (before any byte is written), then the bytes of `files`.
        download_errors: VecDeque<BackendError>,
        /// When set, `download` writes the first half of the bytes, then waits here.
        download_gate: Option<Arc<tokio::sync::Notify>>,
        /// When set, `history` takes its snapshot, then waits here before answering.
        history_gate: Option<Arc<tokio::sync::Notify>>,
        /// `PayloadId`s the server's history reports for a chat's own messages.
        history_payloads: HashMap<ChatId, Vec<PayloadId>>,
        /// Every write, in order.
        calls: Vec<Call>,
        /// Answers of `send_text`, then `Ok(Some(SERVER_TS))`.
        send_results: VecDeque<Result<Option<MessageTs>, BackendError>>,
        /// Answers of edit, delete, mark_seen and typing, then `Ok`.
        write_results: VecDeque<Result<(), BackendError>>,
        /// When set, `send_text` records its call, then waits here before answering.
        send_gate: Option<Arc<tokio::sync::Notify>>,
        /// Answer of `search`; empty hits when unset.
        search_result: Option<Result<SearchHits, BackendError>>,
        /// Every `search` call: query and limit.
        search_calls: Vec<(String, usize)>,
        /// When set, `search` records its call, then waits here before answering.
        search_gate: Option<Arc<tokio::sync::Notify>>,
        /// Searches answered so far (after the gate).
        search_done: usize,
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
        fn record(&self, call: Call) -> Result<(), BackendError> {
            let mut s = self.state();
            s.calls.push(call);
            s.write_results.pop_front().unwrap_or(Ok(()))
        }
        fn calls(&self) -> Vec<Call> {
            self.state().calls.clone()
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
                let payloads = s.history_payloads.get(chat).cloned().unwrap_or_default();
                (HistoryChunk { chat: chat.clone(), messages, reached_start, replaces: None, payloads }, s.history_gate.clone())
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
        async fn download(
            &self,
            media: &MediaRef,
            out: &mut (dyn Write + Send),
            progress: &(dyn Fn(u64, Option<u64>) + Send + Sync),
        ) -> Result<u64, BackendError> {
            let (bytes, gate) = {
                let mut s = self.state();
                s.download_calls.push(media.clone());
                if let Some(error) = s.download_errors.pop_front() {
                    return Err(error);
                }
                (s.files.get(&media.id).cloned().ok_or(BackendError::NotFound)?, s.download_gate.clone())
            };
            let total = Some(bytes.len() as u64);
            let (head, tail) = bytes.split_at(bytes.len() / 2);
            out.write_all(head).map_err(|e| BackendError::Storage(e.kind().to_string()))?;
            progress(head.len() as u64, total);
            if let Some(gate) = gate {
                gate.notified().await;
            }
            out.write_all(tail).map_err(|e| BackendError::Storage(e.kind().to_string()))?;
            progress(bytes.len() as u64, total);
            Ok(bytes.len() as u64)
        }
        fn watch(&self, chat: Option<&ChatId>) {
            self.state().watch_calls.push(chat.cloned());
        }
        async fn subscribe(&self) -> Result<Receiver<LiveEvent>, BackendError> {
            let (tx, rx) = async_channel::unbounded();
            self.state().live = Some(tx);
            Ok(rx)
        }
        async fn send_text(
            &self,
            chat: &ChatId,
            payload: &PayloadId,
            text: &str,
            reply_to: Option<&ReplyTarget>,
        ) -> Result<Option<MessageTs>, BackendError> {
            let (result, gate) = {
                let mut s = self.state();
                s.calls.push(Call::Send {
                    chat: chat.clone(),
                    payload: payload.clone(),
                    text: text.into(),
                    reply: reply_to.cloned(),
                });
                (s.send_results.pop_front().unwrap_or(Ok(Some(SERVER_TS))), s.send_gate.clone())
            };
            if let Some(gate) = gate {
                gate.notified().await;
            }
            result
        }
        async fn edit(
            &self,
            chat: &ChatId,
            timestamp: MessageTs,
            text: &str,
            reply_to: Option<&ReplyTarget>,
        ) -> Result<(), BackendError> {
            self.record(Call::Edit(chat.clone(), timestamp, text.into(), reply_to.cloned()))
        }
        async fn delete(&self, chat: &ChatId, timestamp: MessageTs) -> Result<(), BackendError> {
            self.record(Call::Delete(chat.clone(), timestamp))
        }
        async fn mark_seen(&self, chat: &ChatId, timestamp: MessageTs) -> Result<(), BackendError> {
            self.record(Call::Seen(chat.clone(), timestamp))
        }
        async fn typing(&self, chat: &ChatId) -> Result<(), BackendError> {
            self.record(Call::Typing(chat.clone()))
        }
        async fn search(&self, query: &str, limit: usize) -> Result<SearchHits, BackendError> {
            let (result, gate) = {
                let mut s = self.state();
                s.search_calls.push((query.into(), limit));
                (s.search_result.clone().unwrap_or_else(|| Ok(SearchHits::default())), s.search_gate.clone())
            };
            if let Some(gate) = gate {
                gate.notified().await;
            }
            self.state().search_done += 1;
            result
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
        start_in(tempfile::tempdir().unwrap(), configure, limit)
    }

    /// Like `start`, in an existing runtime dir: the next run of a client that was stopped.
    fn start_in(dir: tempfile::TempDir, configure: impl FnOnce(&mut FakeState, &Path), limit: usize) -> Harness {
        let fake = Fake::default();
        {
            let mut state = fake.state();
            state.account = UID.into();
            configure(&mut state, dir.path());
        }
        let mut config = ClientConfig::new(dir.path(), dir.path().join("session.json"), account());
        config.history_limit = limit;
        config.downloads_dir = Some(dir.path().join("Downloads"));
        config.reconnect_base = Duration::from_millis(5);
        config.reconnect_max = Duration::from_millis(20);
        config.media_missing_ttl = Duration::from_millis(300);
        config.send_backoff_step = Duration::from_millis(10);
        config.send_backoff_max = Duration::from_millis(40);
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

        /// Waits until the fake backend state satisfies `cond`.
        fn wait_for(&self, what: &str, cond: impl Fn(&FakeState) -> bool) {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !cond(&self.fake.state()) {
                assert!(Instant::now() < deadline, "timed out waiting for {what}");
                std::thread::sleep(Duration::from_millis(2));
            }
        }

        /// Gives the engine time to do something it should not do.
        fn quiet(&self) {
            std::thread::sleep(Duration::from_millis(120));
        }

        /// Everything up to and including the first event matching `pred`.
        fn until(&self, what: &str, pred: impl Fn(&ClientEvent) -> bool) -> Vec<ClientEvent> {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut seen = Vec::new();
            loop {
                match self.events.try_recv() {
                    Ok(event) => {
                        let done = pred(&event);
                        seen.push(event);
                        if done {
                            return seen;
                        }
                    }
                    Err(async_channel::TryRecvError::Empty) => {
                        assert!(Instant::now() < deadline, "timed out waiting for {what}; saw {seen:?}");
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    Err(async_channel::TryRecvError::Closed) => panic!("event stream closed waiting for {what}"),
                }
            }
        }

        fn upserted(&self, what: &str) -> Message {
            match self.next(what, |e| matches!(e, ClientEvent::MessageUpserted(_))) {
                ClientEvent::MessageUpserted(message) => message,
                _ => unreachable!(),
            }
        }

        /// Events delivered so far, without waiting.
        fn drain(&self) -> Vec<ClientEvent> {
            std::iter::from_fn(|| self.events.try_recv().ok()).collect()
        }

        fn send_text(&self, chat: &str, text: &str) {
            self.client.send(Command::Send { chat: id(chat), text: text.into(), reply_to: None });
        }

        fn search(&self, n: u64, query: &str) {
            self.client.send(Command::Search { id: SearchId(n), query: query.into() });
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

    fn outbox_entry(payload: &str, chat_id: &str, created_us: MessageTs, text: &str) -> OutboxEntry {
        OutboxEntry {
            payload: PayloadId(payload.into()),
            chat: id(chat_id),
            created_us,
            text: text.into(),
            reply_to: None,
            failed: None,
        }
    }

    fn payload_of(message: &Message) -> PayloadId {
        match &message.delivery {
            Delivery::Pending(payload) | Delivery::Failed { payload, .. } => payload.clone(),
            Delivery::Sent => panic!("expected an unconfirmed message, got {message:?}"),
        }
    }

    /// The server's copy of an own message, as the echo push delivers it.
    fn own_copy(chat_id: &str, ts: MessageTs, text: &str) -> Message {
        let mut message = msg(chat_id, ts, text);
        message.outgoing = true;
        message.author = account();
        message
    }

    fn rejected(reason: &str, permanent: bool) -> BackendError {
        BackendError::Rejected { reason: reason.into(), permanent }
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
    fn own_message_preview_is_attributed_to_you() {
        let h = start(|s, _| s.chats = vec![chat("a", Some(1), None)], 50);
        h.online();
        let mut own = msg("a", 10, "mine");
        own.outgoing = true;
        own.author_name = Some("Вадим".into());
        h.fake.push(LiveEvent::MessageUpserted(own));
        let ClientEvent::ChatUpdated(row) = h.next("row update", |e| matches!(e, ClientEvent::ChatUpdated(_))) else {
            unreachable!()
        };
        let preview = row.last_message.unwrap();
        assert_eq!((preview.author.as_deref(), preview.text.as_str(), row.unread), (Some("Вы"), "mine", 0));
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
    fn deleting_the_last_message_replaces_the_row_preview_only_for_that_message() {
        let h = start(|s, _| s.chats = vec![chat("a", None, None)], 50);
        h.online();
        h.fake.push(LiveEvent::MessageUpserted(msg("a", 3, "older")));
        h.fake.push(LiveEvent::MessageUpserted(msg("a", 4, "newest")));
        h.next("preview of 4", |e| matches!(e, ClientEvent::ChatUpdated(c) if c.last_message.as_ref().is_some_and(|p| p.timestamp == 4)));
        h.drain();

        // An older message going away leaves the preview alone.
        h.fake.push(LiveEvent::MessageDeleted { chat: id("a"), timestamp: 3 });
        h.next("delete of 3", |e| matches!(e, ClientEvent::MessageDeleted { timestamp: 3, .. }));
        h.quiet();
        assert!(!h.drain().iter().any(|e| matches!(e, ClientEvent::ChatUpdated(_))));
        assert_eq!(h.cache().chat(&id("a")).unwrap().unwrap().last_message.unwrap().text, "newest");

        let before = h.cache().chat(&id("a")).unwrap().unwrap().last_message.unwrap();
        h.fake.push(LiveEvent::MessageDeleted { chat: id("a"), timestamp: 4 });
        let ClientEvent::ChatUpdated(row) = h.next("row update", |e| matches!(e, ClientEvent::ChatUpdated(_))) else {
            unreachable!()
        };
        let preview = row.last_message.clone().unwrap();
        assert_eq!((preview.text.as_str(), preview.timestamp, &preview.author), ("Сообщение удалено", 4, &before.author));
        assert_eq!(h.cache().chat(&id("a")).unwrap(), Some(row));
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
        let unavailable = |h: &Harness| h.next("unavailable", |e| *e == ClientEvent::MediaUnavailable(gone.clone()));
        h.client.send(Command::FetchMedia(gone.clone()));
        unavailable(&h);
        // Within the TTL (300 ms in tests) the answer comes without asking the server.
        h.client.send(Command::FetchMedia(gone.clone()));
        unavailable(&h);
        assert_eq!(h.fake.state().media_calls, 1);
        std::thread::sleep(Duration::from_millis(350));
        h.client.send(Command::FetchMedia(gone.clone()));
        unavailable(&h);
        assert_eq!(h.fake.state().media_calls, 2, "asked again after the TTL");
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

    #[test]
    fn a_corrupt_cache_is_moved_aside_not_deleted_and_a_fresh_one_opens() {
        let dir = tempfile::tempdir().unwrap();
        let path = crate::paths::cache_path(dir.path(), &account());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"this is not a sqlite database, but it may hold unsent messages").unwrap();

        let cache = open_cache(dir.path(), &account());
        cache.put_draft(&Draft { chat: id("a"), text: "new".into(), reply_to: None }).unwrap();
        assert_eq!(cache.drafts().unwrap().len(), 1);
        let moved: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.to_string_lossy().contains(".sqlite.broken-"))
            .collect();
        assert_eq!(moved.len(), 1, "{moved:?}");
        assert_eq!(fs::read(&moved[0]).unwrap(), b"this is not a sqlite database, but it may hold unsent messages");
    }

    // -- sending ----------------------------------------------------------------------------

    fn is_settled(chat_id: &str, payload: &PayloadId) -> impl Fn(&ClientEvent) -> bool {
        let (chat_id, payload) = (id(chat_id), payload.clone());
        move |e| *e == ClientEvent::PendingSettled { chat: chat_id.clone(), payload: payload.clone() }
    }

    fn is_upsert_at(ts: MessageTs) -> impl Fn(&ClientEvent) -> bool {
        move |e| matches!(e, ClientEvent::MessageUpserted(m) if m.timestamp == ts)
    }

    #[test]
    fn send_shows_a_pending_message_and_the_confirmation_replaces_it_once() {
        let h = start(
            |s, _| {
                s.chats = vec![chat("a", Some(5), None)];
                s.send_results = VecDeque::from([Ok(Some(777))]);
            },
            50,
        );
        h.online();
        h.send_text("a", "hi");
        let pending = h.upserted("pending message");
        let payload = payload_of(&pending);
        assert_eq!(
            (pending.chat.clone(), pending.outgoing, pending.author.clone(), pending.body.clone()),
            (id("a"), true, account(), MessageBody::Text("hi".into()))
        );
        assert!(pending.timestamp > 5);

        let events = h.until("server copy", is_upsert_at(777));
        let settled = events.iter().position(is_settled("a", &payload)).expect("pending message settled");
        assert_eq!(settled, events.len() - 2, "the settlement directly precedes the server copy");
        let ClientEvent::MessageUpserted(copy) = events.last().unwrap() else { unreachable!() };
        assert_eq!((copy.delivery.clone(), copy.outgoing, copy.author.clone()), (Delivery::Sent, true, account()));
        assert_eq!(h.cache().message(&id("a"), 777).unwrap().unwrap().body, MessageBody::Text("hi".into()));
        assert!(h.cache().outbox().unwrap().is_empty());
        assert_eq!(h.fake.calls(), [Call::Send { chat: id("a"), payload: payload.clone(), text: "hi".into(), reply: None }]);

        // The echo of the same send arrives later: an ordinary upsert, no second settlement.
        h.fake.push(LiveEvent::Echo { payload, message: own_copy("a", 777, "hi") });
        h.fake.push(LiveEvent::MessageUpserted(msg("a", 900, "sentinel")));
        let events = h.until("sentinel", is_upsert_at(900));
        assert!(!events.iter().any(|e| matches!(e, ClientEvent::PendingSettled { .. })), "settled twice: {events:?}");
        assert_eq!(events.iter().filter(|e| is_upsert_at(777)(*e)).count(), 1);
    }

    #[test]
    fn echo_before_the_confirmation_settles_the_pending_message_exactly_once() {
        let gate = Arc::new(tokio::sync::Notify::new());
        let h = start(
            |s, _| {
                s.chats = vec![chat("a", Some(5), None)];
                s.send_gate = Some(gate.clone());
                s.send_results = VecDeque::from([Ok(Some(888))]);
            },
            50,
        );
        h.online();
        h.send_text("a", "hi");
        let payload = payload_of(&h.upserted("pending message"));
        h.wait_for("the send request", |s| s.calls.len() == 1);

        h.fake.push(LiveEvent::Echo { payload: payload.clone(), message: own_copy("a", 888, "hi") });
        let events = h.until("echo copy", is_upsert_at(888));
        let settled = events.iter().position(is_settled("a", &payload)).expect("echo settles the pending message");
        assert_eq!(settled, events.len() - 2);

        // The confirmation of the still-running request arrives after the echo.
        gate.notify_one();
        h.fake.push(LiveEvent::MessageUpserted(msg("a", 900, "sentinel")));
        let mut seen = h.until("sentinel", is_upsert_at(900));
        h.quiet();
        seen.extend(h.drain());
        assert!(
            !seen.iter().any(|e| matches!(e, ClientEvent::PendingSettled { .. }) || is_upsert_at(888)(e)),
            "second signal was not a no-op: {seen:?}"
        );
        assert!(h.cache().outbox().unwrap().is_empty());
        assert_eq!(h.fake.calls().len(), 1);
    }

    #[test]
    fn echo_of_a_send_from_another_client_is_an_ordinary_upsert() {
        let h = start(|s, _| s.chats = vec![chat("a", Some(5), None)], 50);
        h.online();
        h.fake.push(LiveEvent::Echo { payload: PayloadId("elsewhere".into()), message: own_copy("a", 50, "from phone") });
        let events = h.until("upsert", is_upsert_at(50));
        assert!(!events.iter().any(|e| matches!(e, ClientEvent::PendingSettled { .. })));
        assert_eq!(h.cache().message(&id("a"), 50).unwrap().unwrap().body, MessageBody::Text("from phone".into()));
    }

    #[test]
    fn sends_go_out_one_at_a_time_in_creation_order() {
        let gate = Arc::new(tokio::sync::Notify::new());
        let h = start(
            |s, _| {
                s.chats = vec![chat("a", Some(5), None)];
                s.send_gate = Some(gate.clone());
                s.send_results = VecDeque::from([Ok(Some(101)), Ok(Some(102)), Ok(Some(103))]);
            },
            50,
        );
        h.online();
        for text in ["one", "two", "three"] {
            h.send_text("a", text);
        }
        let pending: Vec<Message> = (0..3).map(|_| h.upserted("pending message")).collect();
        assert!(pending.windows(2).all(|w| w[0].timestamp < w[1].timestamp), "{pending:?}");

        let texts = |h: &Harness| -> Vec<String> {
            h.fake.calls().into_iter().filter_map(|c| if let Call::Send { text, .. } = c { Some(text) } else { None }).collect()
        };
        h.wait_for("first send", |s| s.calls.len() == 1);
        h.quiet();
        assert_eq!(texts(&h), ["one"], "the next send waits for the one in flight");
        gate.notify_one();
        h.wait_for("second send", |s| s.calls.len() == 2);
        h.quiet();
        assert_eq!(texts(&h), ["one", "two"]);
        gate.notify_one();
        h.wait_for("third send", |s| s.calls.len() == 3);
        gate.notify_one();

        let third = payload_of(&pending[2]);
        let events = h.until("third settled", is_settled("a", &third));
        let settled: Vec<PayloadId> = events
            .iter()
            .filter_map(|e| if let ClientEvent::PendingSettled { payload, .. } = e { Some(payload.clone()) } else { None })
            .collect();
        assert_eq!(settled, pending.iter().map(payload_of).collect::<Vec<_>>());
        assert_eq!(texts(&h), ["one", "two", "three"]);
    }

    #[test]
    fn pending_messages_sort_after_everything_known_even_with_a_behind_clock() {
        const FAR: MessageTs = i64::MAX / 4;
        let gate = Arc::new(tokio::sync::Notify::new());
        let h = start(
            |s, dir| {
                seed_cache(dir, &[chat("a", Some(FAR), None)], "a", &[msg("a", FAR, "future")], true);
                s.chats = vec![chat("a", Some(FAR), None)];
                s.history.insert(id("a"), vec![msg("a", FAR, "future")]);
                s.send_gate = Some(gate.clone());
            },
            50,
        );
        h.online();
        h.send_text("a", "one");
        assert_eq!(h.upserted("first").timestamp, FAR + 1);
        h.send_text("a", "two");
        assert_eq!(h.upserted("second").timestamp, FAR + 2);
    }

    #[test]
    fn reply_target_goes_to_the_backend_and_is_quoted_in_the_pending_message() {
        let h = start(|s, _| s.chats = vec![chat("a", Some(5), None)], 50);
        h.online();
        let target = ReplyTarget { timestamp: 7, author: Some("Ann".into()), text: "question".into() };
        h.client.send(Command::Send { chat: id("a"), text: "answer".into(), reply_to: Some(target.clone()) });
        let pending = h.upserted("pending reply");
        assert_eq!(
            pending.reply_to,
            Some(ReplyQuote { author: Some("Ann".into()), text: "question".into(), timestamp: Some(7) })
        );
        h.wait_for("the send request", |s| !s.calls.is_empty());
        let Call::Send { reply, .. } = h.fake.calls().remove(0) else { panic!("not a send") };
        assert_eq!(reply, Some(target));
    }

    #[test]
    fn permanent_rejection_marks_the_message_failed_and_only_retry_resends_it() {
        let h = start(
            |s, _| {
                s.chats = vec![chat("a", Some(5), None), chat("b", Some(5), None)];
                s.send_results = VecDeque::from([Err(rejected("NO_PERMISSION", true))]);
            },
            50,
        );
        h.online();
        h.send_text("a", "no");
        let pending = h.upserted("pending message");
        let payload = payload_of(&pending);
        let failed = h.upserted("failed message");
        assert_eq!(failed.timestamp, pending.timestamp);
        assert_eq!(failed.delivery, Delivery::Failed { payload: payload.clone(), reason: "NO_PERMISSION".into() });
        assert_eq!(h.cache().outbox().unwrap()[0].failed.as_deref(), Some("NO_PERMISSION"));
        h.quiet();
        assert_eq!(h.fake.calls().len(), 1, "no automatic retry");

        // A failed message must not hold back other chats.
        h.send_text("b", "other");
        h.next("other chat settled", |e| matches!(e, ClientEvent::PendingSettled { chat, .. } if chat.0 == "b"));

        h.client.send(Command::Retry(payload.clone()));
        let again = h.next("pending again", |e| {
            matches!(e, ClientEvent::MessageUpserted(m) if m.delivery == Delivery::Pending(payload.clone()))
        });
        assert!(matches!(again, ClientEvent::MessageUpserted(m) if m.timestamp == pending.timestamp));
        h.next("retry settled", is_settled("a", &payload));
        let calls = h.fake.calls();
        assert_eq!(calls.len(), 3);
        assert!(matches!(&calls[2], Call::Send { payload: p, text, .. } if *p == payload && text == "no"));
        assert!(h.cache().outbox().unwrap().is_empty());
    }

    #[test]
    fn message_sent_offline_waits_for_the_connection_and_goes_out_once() {
        let h = start(
            |s, _| {
                s.chats = vec![chat("a", Some(5), None)];
                s.bootstrap_errors = std::iter::repeat_n(BackendError::Network("down".into()), 1000).collect();
            },
            50,
        );
        h.next("offline", |e| *e == ClientEvent::Connection(ConnectionState::Offline));
        h.send_text("a", "later");
        let payload = payload_of(&h.upserted("pending message"));
        h.quiet();
        assert!(h.fake.calls().is_empty(), "nothing is sent while offline");

        h.fake.state().bootstrap_errors.clear();
        h.next("settled", is_settled("a", &payload));
        h.quiet();
        let calls = h.fake.calls();
        assert_eq!(calls.len(), 1, "delivered once: {calls:?}");
        assert!(matches!(&calls[0], Call::Send { payload: p, .. } if *p == payload));
    }

    #[test]
    fn restart_resends_the_pending_entry_under_the_same_payload_id() {
        let gate = Arc::new(tokio::sync::Notify::new());
        let h = start(
            |s, dir| {
                seed_cache(dir, &[chat("a", Some(5), None)], "a", &[msg("a", 5, "old")], true);
                let cache = Cache::open(dir, &account()).unwrap();
                cache.put_outbox(&outbox_entry("p1", "a", 50, "queued")).unwrap();
                let mut rejected_before = outbox_entry("p2", "a", 60, "refused");
                rejected_before.failed = Some("NO_PERMISSION".into());
                cache.put_outbox(&rejected_before).unwrap();
                s.chats = vec![chat("a", Some(5), None)];
                s.history.insert(id("a"), vec![msg("a", 5, "old")]);
                s.send_gate = Some(gate.clone());
            },
            50,
        );
        h.online();
        let catch_up = h.history("catch-up of the cached chat");
        h.client.send(Command::OpenChat(id("a")));
        let cached = h.history("cached chunk");
        let fetched = h.history("server chunk");
        for chunk in [catch_up, cached, fetched] {
            assert_eq!(stamps(&chunk), [5, 50, 60]);
            assert_eq!(chunk.messages[1].delivery, Delivery::Pending(PayloadId("p1".into())));
            assert_eq!(
                chunk.messages[2].delivery,
                Delivery::Failed { payload: PayloadId("p2".into()), reason: "NO_PERMISSION".into() }
            );
        }
        h.quiet();
        assert_eq!(
            h.fake.calls(),
            [Call::Send { chat: id("a"), payload: PayloadId("p1".into()), text: "queued".into(), reply: None }],
            "only the pending entry is resent"
        );
        gate.notify_one();
        h.next("settled", is_settled("a", &PayloadId("p1".into())));
        let left = h.cache().outbox().unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].payload.0, "p2");
    }

    #[test]
    fn duplicate_answer_without_a_timestamp_loads_the_server_copy() {
        let h = start(
            |s, _| {
                s.chats = vec![chat("a", Some(5), None)];
                s.history.insert(id("a"), vec![own_copy("a", 5, "hi")]);
                s.send_results = VecDeque::from([Ok(None)]);
            },
            50,
        );
        h.online();
        h.send_text("a", "hi");
        let payload = payload_of(&h.upserted("pending message"));
        h.next("settled", is_settled("a", &payload));
        let chunk = h.history("server copy");
        assert_eq!(stamps(&chunk), [5]);
        assert_eq!(h.fake.state().history_calls.last(), Some(&(id("a"), None, 50)));
        assert!(h.cache().outbox().unwrap().is_empty());
    }

    #[test]
    fn transient_failure_keeps_the_message_pending_and_retries_without_a_new_id() {
        let h = start(
            |s, _| {
                s.chats = vec![chat("a", Some(5), None)];
                s.send_results = VecDeque::from([Err(BackendError::Network("down".into())), Err(rejected("THROTTLED", false))]);
            },
            50,
        );
        h.online();
        h.send_text("a", "again");
        let payload = payload_of(&h.upserted("pending message"));
        let events = h.until("settled", is_settled("a", &payload));
        assert!(
            !events.iter().any(|e| matches!(e, ClientEvent::MessageUpserted(m) if matches!(m.delivery, Delivery::Failed { .. }))),
            "transient failures never mark a message failed: {events:?}"
        );
        let calls = h.fake.calls();
        assert_eq!(calls.len(), 3);
        assert!(calls.iter().all(|c| matches!(c, Call::Send { payload: p, .. } if *p == payload)));
    }

    // -- sends that cannot be stored or that race history -----------------------------------

    #[test]
    fn only_the_in_memory_fallback_is_not_durable() {
        let dir = tempfile::tempdir().unwrap();
        assert!(open_cache(dir.path(), &account()).is_durable());
        let blocked = tempfile::tempdir().unwrap();
        fs::write(blocked.path().join("cache"), b"").unwrap();
        assert!(!open_cache(blocked.path(), &account()).is_durable());
    }

    #[test]
    fn a_send_is_refused_with_its_draft_while_the_cache_only_lives_in_memory() {
        let h = start(
            |s, dir| {
                s.chats = vec![chat("a", Some(5), None)];
                // A file where the cache directory belongs: neither opening nor moving aside works.
                fs::write(dir.join("cache"), b"").unwrap();
            },
            50,
        );
        h.online();
        let reply = ReplyTarget { timestamp: 3, author: Some("Ann".into()), text: "quoted".into() };
        h.client.send(Command::Send { chat: id("a"), text: "hi".into(), reply_to: Some(reply.clone()) });
        let events = h.until("refusal", |e| matches!(e, ClientEvent::SendRefused { .. } | ClientEvent::ActionFailed { .. }));
        let draft = Draft { chat: id("a"), text: "hi".into(), reply_to: Some(reply) };
        assert_eq!(events.last(), Some(&ClientEvent::SendRefused { draft, reason: STORE_FAILED_REASON.into() }));
        h.quiet();
        let rest = h.drain();
        assert!(
            !events.iter().chain(&rest).any(|e| matches!(e, ClientEvent::MessageUpserted(_))),
            "no pending message: {events:?} {rest:?}"
        );
        assert_eq!(h.fake.calls(), [], "nothing reaches the server");
    }

    /// Opens the uncached chat "a" with its history held back, sends "hi" (confirmed at 777)
    /// meanwhile, lets `later` more messages (1000, 1001, ...) arrive live, then releases the
    /// history and returns the chunk the UI gets.
    fn first_history_after_a_confirmed_send(snapshot: Vec<Message>, later: MessageTs) -> (Harness, HistoryChunk) {
        let gate = Arc::new(tokio::sync::Notify::new());
        let h = start(
            |s, _| {
                s.chats = vec![chat("a", Some(3), None)];
                s.history.insert(id("a"), snapshot);
                s.history_gate = Some(gate.clone());
                s.send_results = VecDeque::from([Ok(Some(777))]);
            },
            50,
        );
        h.online();
        h.client.send(Command::OpenChat(id("a")));
        h.wait_for("the history request", |s| !s.history_calls.is_empty());
        h.send_text("a", "hi");
        let payload = payload_of(&h.upserted("pending message"));
        h.until("server copy", is_upsert_at(777));
        assert!(h.cache().outbox().unwrap().is_empty(), "settled before the history arrives: {payload:?}");
        for ts in 1000..1000 + later {
            h.fake.push(LiveEvent::MessageUpserted(msg("a", ts, "later")));
        }
        if later > 0 {
            h.until("the last live message", is_upsert_at(999 + later));
        }
        gate.notify_one();
        let chunk = h.history("first history");
        (h, chunk)
    }

    #[test]
    fn a_send_confirmed_during_the_first_history_load_is_part_of_that_history() {
        let snapshot = vec![msg("a", 1, "one"), msg("a", 2, "two"), msg("a", 3, "three")];
        let (h, chunk) = first_history_after_a_confirmed_send(snapshot, 0);
        assert_eq!(stamps(&chunk), [1, 2, 3, 777]);
        assert!(chunk.messages.iter().all(|m| m.delivery == Delivery::Sent), "no pending copy: {chunk:?}");
        assert_eq!(chunk.messages[3].body, MessageBody::Text("hi".into()));
        assert_eq!(stamps(&h.cache().history(&id("a"), None, 50).unwrap()), [1, 2, 3, 777]);
    }

    #[test]
    fn a_send_confirmed_during_the_first_load_of_an_empty_chat_is_part_of_that_history() {
        let (h, chunk) = first_history_after_a_confirmed_send(Vec::new(), 0);
        assert_eq!(stamps(&chunk), [777]);
        assert!(chunk.messages.iter().all(|m| m.delivery == Delivery::Sent), "no pending copy: {chunk:?}");
        assert_eq!(stamps(&h.cache().history(&id("a"), None, 50).unwrap()), [777]);
    }

    #[test]
    fn a_confirmed_send_stays_in_the_first_history_even_after_the_live_log_overflowed() {
        let snapshot = vec![msg("a", 1, "one"), msg("a", 2, "two"), msg("a", 3, "three")];
        let later = LIVE_LOG_CAP as MessageTs + 44;
        let (_h, chunk) = first_history_after_a_confirmed_send(snapshot, later);
        let expected: Vec<MessageTs> = [1, 2, 3, 777].into_iter().chain(1000..1000 + later).collect();
        assert_eq!(stamps(&chunk), expected);
    }

    #[test]
    fn a_confirmed_send_stays_in_the_first_history_of_an_empty_chat_after_the_live_log_overflowed() {
        let later = LIVE_LOG_CAP as MessageTs + 44;
        let (h, chunk) = first_history_after_a_confirmed_send(Vec::new(), later);
        let expected: Vec<MessageTs> = std::iter::once(777).chain(1000..1000 + later).collect();
        assert_eq!(stamps(&chunk), expected);
        assert_eq!(stamps(&h.cache().history(&id("a"), None, 500).unwrap()), expected, "the cache keeps them too");
    }

    #[test]
    fn history_carrying_a_pending_payload_settles_it_instead_of_showing_both_copies() {
        let gate = Arc::new(tokio::sync::Notify::new());
        let h = start(
            |s, dir| {
                seed_cache(dir, &[chat("a", Some(5), None)], "a", &[msg("a", 5, "old")], true);
                let cache = Cache::open(dir, &account()).unwrap();
                cache.put_outbox(&outbox_entry("p1", "a", 50, "queued")).unwrap();
                cache.put_outbox(&outbox_entry("p2", "a", 70, "later")).unwrap();
                s.chats = vec![chat("a", Some(5), None)];
                s.history.insert(id("a"), vec![msg("a", 5, "old"), own_copy("a", 60, "queued")]);
                s.history_payloads.insert(id("a"), vec![PayloadId("p1".into())]);
                // The resend of p1 stays outstanding: only the history can settle it.
                s.send_gate = Some(gate.clone());
            },
            50,
        );
        h.online();
        let p1 = PayloadId("p1".into());
        let events = h.until("catch-up history", |e| matches!(e, ClientEvent::History(_)));
        let settled = events.iter().position(is_settled("a", &p1)).expect("p1 settled by the history");
        assert_eq!(settled, events.len() - 2, "the settlement directly precedes the chunk");
        let Some(ClientEvent::History(chunk)) = events.last() else { unreachable!() };
        assert_eq!(stamps(chunk), [5, 60, 70]);
        assert_eq!(chunk.messages[1].delivery, Delivery::Sent);
        assert_eq!(chunk.messages[2].delivery, Delivery::Pending(PayloadId("p2".into())));
        let left = h.cache().outbox().unwrap();
        assert_eq!(left.iter().map(|e| e.payload.0.as_str()).collect::<Vec<_>>(), ["p2"]);

        // The answer of the still-running resend arrives late: nothing is left to do for it.
        gate.notify_one();
        h.fake.push(LiveEvent::MessageUpserted(msg("a", 900, "sentinel")));
        let mut seen = h.until("sentinel", is_upsert_at(900));
        h.quiet();
        seen.extend(h.drain());
        assert!(
            !seen.iter().any(|e| matches!(e, ClientEvent::PendingSettled { payload, .. } if *payload == p1) || is_upsert_at(SERVER_TS)(e)),
            "late confirmation was not a no-op: {seen:?}"
        );
    }

    #[test]
    fn an_older_page_carrying_a_pending_payload_settles_it_durably_without_appending_pending() {
        let gate = Arc::new(tokio::sync::Notify::new());
        let h = start(
            |s, dir| {
                seed_cache(dir, &[chat("a", Some(5), None)], "a", &[msg("a", 4, "four"), msg("a", 5, "five")], false);
                let cache = Cache::open(dir, &account()).unwrap();
                cache.put_outbox(&outbox_entry("p1", "a", 50, "queued")).unwrap();
                cache.put_outbox(&outbox_entry("p2", "a", 70, "later")).unwrap();
                s.chats = vec![chat("a", Some(5), None)];
                s.history.insert(
                    id("a"),
                    vec![msg("a", 2, "two"), own_copy("a", 3, "queued"), msg("a", 4, "four"), msg("a", 5, "five")],
                );
                s.send_gate = Some(gate.clone());
                s.send_results.push_back(Err(BackendError::Network("down".into())));
            },
            2,
        );
        h.online();
        let newest = h.history("catch-up");
        assert_eq!(stamps(&newest), [4, 5, 50, 70], "the newest page shows both pending messages");
        h.wait_for("the first resend", |s| !s.calls.is_empty());

        // Only a later, older page carries p1's payload.
        h.fake.state().history_payloads.insert(id("a"), vec![PayloadId("p1".into())]);
        h.client.send(Command::LoadOlder { chat: id("a"), before: 4 });
        let p1 = PayloadId("p1".into());
        let events = h.until("older page", |e| matches!(e, ClientEvent::History(_)));
        assert!(events.iter().any(is_settled("a", &p1)), "the older page settled p1: {events:?}");
        let Some(ClientEvent::History(chunk)) = events.last() else { unreachable!() };
        assert_eq!(stamps(chunk), [2, 3], "an older page never gets pending messages");
        let left = h.cache().outbox().unwrap();
        assert_eq!(left.iter().map(|e| e.payload.0.as_str()).collect::<Vec<_>>(), ["p2"], "p1 left the durable outbox");

        // The in-flight resend of p1 fails transiently once it is released. Were p1 still queued,
        // that would hold the queue and resend it after the backoff.
        gate.notify_waiters();
        h.quiet();
        let sends_of = |h: &Harness, payload: &PayloadId| {
            h.fake.calls().iter().filter(|c| matches!(c, Call::Send { payload: p, .. } if p == payload)).count()
        };
        assert_eq!(sends_of(&h, &p1), 1, "p1 was resent after it was settled");

        // Reopening against the same runtime dir shows the server copy and no pending p1.
        let Harness { dir, client, events, fake: _ } = h;
        drop((client, events));
        std::thread::sleep(Duration::from_millis(50));
        let reopened = start_in(
            dir,
            |s, _| {
                s.chats = vec![chat("a", Some(5), None)];
                s.history.insert(
                    id("a"),
                    vec![msg("a", 2, "two"), own_copy("a", 3, "queued"), msg("a", 4, "four"), msg("a", 5, "five")],
                );
                s.send_gate = Some(Arc::new(tokio::sync::Notify::new()));
            },
            50,
        );
        reopened.online();
        reopened.client.send(Command::OpenChat(id("a")));
        let cached = reopened.history("cached chunk");
        let fetched = reopened.history("server chunk");
        for chunk in [&cached, &fetched] {
            assert_eq!(stamps(chunk), [2, 3, 4, 5, 70]);
            assert_eq!(chunk.messages[1].delivery, Delivery::Sent, "the server copy of p1");
            let pending: Vec<_> = chunk.messages.iter().filter(|m| m.delivery != Delivery::Sent).map(payload_of).collect();
            assert_eq!(pending, [PayloadId("p2".into())], "p1 must not come back as a pending duplicate");
        }
        reopened.quiet();
        assert_eq!(sends_of(&reopened, &p1), 0, "a restart resent the settled p1");
    }

    // -- edits and deletions ----------------------------------------------------------------

    fn with_own_message(s: &mut FakeState, dir: &Path) {
        let own = own_copy("a", 3, "old");
        seed_cache(dir, &[chat("a", Some(3), None)], "a", std::slice::from_ref(&own), true);
        s.chats = vec![chat("a", Some(3), None)];
        s.history.insert(id("a"), vec![own]);
    }

    #[test]
    fn edit_applies_after_the_server_accepts_it() {
        let h = start(with_own_message, 50);
        h.online();
        h.history("catch-up");
        h.client.send(Command::Edit { chat: id("a"), timestamp: 3, text: "new".into() });
        let edited = h.upserted("edited message");
        assert_eq!((edited.timestamp, edited.body.clone(), edited.edited), (3, MessageBody::Text("new".into()), true));
        assert!(edited.outgoing);
        assert_eq!(h.fake.calls(), [Call::Edit(id("a"), 3, "new".into(), None)]);
        let cached = h.cache().message(&id("a"), 3).unwrap().unwrap();
        assert_eq!((cached.body, cached.edited), (MessageBody::Text("new".into()), true));
        let ClientEvent::ChatUpdated(row) = h.next("row preview", |e| matches!(e, ClientEvent::ChatUpdated(_))) else {
            unreachable!()
        };
        assert_eq!(row.last_message.unwrap().text, "new");
    }

    #[test]
    fn rejected_edit_leaves_the_message_unchanged_and_reports_why() {
        let h = start(
            |s, dir| {
                with_own_message(s, dir);
                s.write_results = VecDeque::from([Err(rejected("NO_PERMISSION", true))]);
            },
            50,
        );
        h.online();
        h.history("catch-up");
        h.client.send(Command::Edit { chat: id("a"), timestamp: 3, text: "new".into() });
        let events = h.until("failure", |e| matches!(e, ClientEvent::ActionFailed { .. }));
        assert_eq!(events.last(), Some(&ClientEvent::ActionFailed { chat: id("a"), reason: "NO_PERMISSION".into() }));
        assert!(!events.iter().any(|e| matches!(e, ClientEvent::MessageUpserted(_))));
        let cached = h.cache().message(&id("a"), 3).unwrap().unwrap();
        assert_eq!((cached.body, cached.edited), (MessageBody::Text("old".into()), false));
    }

    #[test]
    fn delete_applies_after_the_server_accepts_it() {
        let h = start(with_own_message, 50);
        h.online();
        h.history("catch-up");
        h.client.send(Command::Delete { chat: id("a"), timestamp: 3 });
        assert_eq!(
            h.next("deleted", |e| matches!(e, ClientEvent::MessageDeleted { .. })),
            ClientEvent::MessageDeleted { chat: id("a"), timestamp: 3 }
        );
        assert_eq!(h.fake.calls(), [Call::Delete(id("a"), 3)]);
        assert_eq!(h.cache().message(&id("a"), 3).unwrap().unwrap().body, MessageBody::Deleted);
    }

    #[test]
    fn rejected_delete_keeps_the_message() {
        let h = start(
            |s, dir| {
                with_own_message(s, dir);
                s.write_results = VecDeque::from([Err(rejected("NO_PERMISSION", true))]);
            },
            50,
        );
        h.online();
        h.history("catch-up");
        h.client.send(Command::Delete { chat: id("a"), timestamp: 3 });
        let events = h.until("failure", |e| matches!(e, ClientEvent::ActionFailed { .. }));
        assert!(!events.iter().any(|e| matches!(e, ClientEvent::MessageDeleted { .. })));
        assert_eq!(h.cache().message(&id("a"), 3).unwrap().unwrap().body, MessageBody::Text("old".into()));
    }

    #[test]
    fn edit_and_delete_while_offline_fail_without_calling_the_backend() {
        let h = start(
            |s, dir| {
                with_own_message(s, dir);
                s.bootstrap_errors = std::iter::repeat_n(BackendError::Network("down".into()), 1000).collect();
            },
            50,
        );
        h.next("offline", |e| *e == ClientEvent::Connection(ConnectionState::Offline));
        h.client.send(Command::Edit { chat: id("a"), timestamp: 3, text: "new".into() });
        h.client.send(Command::Delete { chat: id("a"), timestamp: 3 });
        let failure = ClientEvent::ActionFailed { chat: id("a"), reason: "Нет соединения".into() };
        assert_eq!(h.next("edit failure", |e| matches!(e, ClientEvent::ActionFailed { .. })), failure);
        assert_eq!(h.next("delete failure", |e| matches!(e, ClientEvent::ActionFailed { .. })), failure);
        assert!(h.fake.calls().is_empty());
        let cached = h.cache().message(&id("a"), 3).unwrap().unwrap();
        assert_eq!((cached.body, cached.edited), (MessageBody::Text("old".into()), false));
    }

    fn start_with_own_message(mutate: impl FnOnce(&mut Message)) -> Harness {
        start(
            |s, dir| {
                let mut own = own_copy("a", 3, "old");
                mutate(&mut own);
                seed_cache(dir, &[chat("a", Some(3), None)], "a", std::slice::from_ref(&own), true);
                s.chats = vec![chat("a", Some(3), None)];
                s.history.insert(id("a"), vec![own]);
            },
            50,
        )
    }

    fn quote(timestamp: Option<MessageTs>) -> ReplyQuote {
        ReplyQuote { author: Some("Ann".into()), text: "quoted".into(), timestamp }
    }

    #[test]
    fn editing_a_reply_repeats_its_target() {
        let h = start_with_own_message(|m| m.reply_to = Some(quote(Some(2))));
        h.online();
        h.history("catch-up");
        h.client.send(Command::Edit { chat: id("a"), timestamp: 3, text: "new".into() });
        h.upserted("edited message");
        let target = ReplyTarget { timestamp: 2, author: Some("Ann".into()), text: "quoted".into() };
        assert_eq!(h.fake.calls(), [Call::Edit(id("a"), 3, "new".into(), Some(target))]);
        let cached = h.cache().message(&id("a"), 3).unwrap().unwrap();
        assert_eq!(cached.reply_to, Some(quote(Some(2))), "the quote survives the local edit");
    }

    fn assert_edit_refused(h: &Harness, timestamp: MessageTs) {
        h.online();
        h.history("catch-up");
        h.client.send(Command::Edit { chat: id("a"), timestamp, text: "new".into() });
        let failure = ClientEvent::ActionFailed { chat: id("a"), reason: "Это сообщение нельзя изменить".into() };
        let events = h.until("refusal", |e| matches!(e, ClientEvent::ActionFailed { .. }));
        assert_eq!(events.last(), Some(&failure));
        h.quiet();
        assert!(h.fake.calls().is_empty(), "no backend call: {:?}", h.fake.calls());
        assert!(h.drain().iter().all(|e| !matches!(e, ClientEvent::MessageUpserted(_))));
        let cached = h.cache().message(&id("a"), 3).unwrap().unwrap();
        assert_eq!((cached.body, cached.edited), (MessageBody::Text("old".into()), false));
    }

    #[test]
    fn editing_a_reply_without_its_original_timestamp_is_refused() {
        assert_edit_refused(&start_with_own_message(|m| m.reply_to = Some(quote(None))), 3);
    }

    #[test]
    fn editing_a_forwarded_message_is_refused() {
        assert_edit_refused(&start_with_own_message(|m| m.forwarded_from = Some("Bob".into())), 3);
    }

    #[test]
    fn editing_an_uncached_message_is_refused() {
        assert_edit_refused(&start_with_own_message(|_| {}), 99);
    }

    // -- read markers -----------------------------------------------------------------------

    fn unread_chat(unread: u32) -> ChatSummary {
        ChatSummary { unread, ..chat("a", Some(10), None) }
    }

    #[test]
    fn read_markers_are_deduplicated_and_clear_the_counter_for_the_newest_message_only() {
        let h = start(|s, _| s.chats = vec![unread_chat(3)], 50);
        h.online();
        let mark = |timestamp| h.client.send(Command::MarkRead { chat: id("a"), timestamp });
        mark(5);
        mark(5);
        mark(4);
        h.wait_for("first marker", |s| !s.calls.is_empty());
        mark(10);
        h.wait_for("second marker", |s| s.calls.len() == 2);
        let ClientEvent::ChatUpdated(row) = h.next("row update", |e| matches!(e, ClientEvent::ChatUpdated(_))) else {
            unreachable!()
        };
        assert_eq!((row.id.0.as_str(), row.unread), ("a", 0), "marking an older message changes nothing");
        assert_eq!(h.cache().chat(&id("a")).unwrap().unwrap().unread, 0);
        mark(10);
        h.quiet();
        assert_eq!(h.fake.calls(), [Call::Seen(id("a"), 5), Call::Seen(id("a"), 10)]);
    }

    #[test]
    fn a_failed_read_marker_is_only_logged() {
        let h = start(
            |s, _| {
                s.chats = vec![unread_chat(2)];
                s.write_results = VecDeque::from([Err(BackendError::Network("down".into()))]);
            },
            50,
        );
        h.online();
        h.client.send(Command::MarkRead { chat: id("a"), timestamp: 10 });
        h.next("row update", |e| matches!(e, ClientEvent::ChatUpdated(c) if c.unread == 0));
        h.wait_for("marker request", |s| s.calls.len() == 1);
        h.quiet();
        assert!(!h.drain().iter().any(|e| matches!(e, ClientEvent::ActionFailed { .. })));
    }

    // -- typing -----------------------------------------------------------------------------

    #[test]
    fn incoming_typing_is_forwarded_with_its_expiry() {
        let h = start(|s, _| s.chats = vec![chat("a", Some(5), None)], 50);
        h.online();
        let before = Instant::now();
        h.fake.push(LiveEvent::Typing {
            chat: id("a"),
            user: UserId("u2".into()),
            name: Some("Ann".into()),
            timeout: Duration::from_secs(5),
        });
        let ClientEvent::Typing { chat, user, name, until } = h.next("typing", |e| matches!(e, ClientEvent::Typing { .. }))
        else {
            unreachable!()
        };
        assert_eq!((chat, user, name), (id("a"), UserId("u2".into()), Some("Ann".into())));
        assert!(until >= before + Duration::from_secs(5) && until <= Instant::now() + Duration::from_secs(5));
    }

    #[test]
    fn own_typing_from_another_client_is_not_forwarded() {
        let h = start(|s, _| s.chats = vec![chat("a", Some(5), None)], 50);
        h.online();
        let typing = |user: &str| LiveEvent::Typing {
            chat: id("a"),
            user: UserId(user.into()),
            name: None,
            timeout: Duration::from_secs(5),
        };
        h.fake.push(typing(UID));
        h.fake.push(typing("u2"));
        let events = h.until("someone else typing", |e| matches!(e, ClientEvent::Typing { .. }));
        assert_eq!(events.len(), 1, "own typing must be dropped: {events:?}");
        assert!(matches!(&events[0], ClientEvent::Typing { user, .. } if user.0 == "u2"));
    }

    #[test]
    fn typing_while_offline_is_not_reported() {
        let h = start(|s, _| s.bootstrap_errors = std::iter::repeat_n(BackendError::Network("down".into()), 1000).collect(), 50);
        h.next("offline", |e| *e == ClientEvent::Connection(ConnectionState::Offline));
        h.client.send(Command::Typing(id("a")));
        h.quiet();
        assert!(h.fake.calls().is_empty());
    }

    #[test]
    fn read_marker_requested_offline_is_sent_once_when_the_connection_returns() {
        let h = start(
            |s, dir| {
                seed_cache(dir, &[unread_chat(1)], "a", &[], false);
                s.chats = vec![unread_chat(1)];
                s.bootstrap_errors = std::iter::repeat_n(BackendError::Network("down".into()), 1000).collect();
            },
            50,
        );
        h.next("offline", |e| *e == ClientEvent::Connection(ConnectionState::Offline));
        for timestamp in [7, 10, 8] {
            h.client.send(Command::MarkRead { chat: id("a"), timestamp });
        }
        // The counter drops at once, even though nothing can be sent yet.
        h.next("row update", |e| matches!(e, ClientEvent::ChatUpdated(c) if c.unread == 0));
        h.quiet();
        assert!(h.fake.calls().is_empty());

        h.fake.state().bootstrap_errors.clear();
        h.wait_for("deferred marker", |s| !s.calls.is_empty());
        h.quiet();
        assert_eq!(h.fake.calls(), [Call::Seen(id("a"), 10)], "only the newest request is sent, once");
    }

    #[test]
    fn read_marker_requested_while_reconnecting_is_sent_after_the_reconnect() {
        let h = start(|s, _| s.chats = vec![unread_chat(2)], 50);
        h.online();
        h.fake.push(LiveEvent::Disconnected);
        h.next("connecting", |e| *e == ClientEvent::Connection(ConnectionState::Connecting));
        h.client.send(Command::MarkRead { chat: id("a"), timestamp: 10 });
        h.quiet();
        assert!(h.fake.calls().is_empty());
        h.fake.push(LiveEvent::Reconnected);
        h.wait_for("deferred marker", |s| !s.calls.is_empty());
        h.client.send(Command::MarkRead { chat: id("a"), timestamp: 10 });
        h.quiet();
        assert_eq!(h.fake.calls(), [Call::Seen(id("a"), 10)]);
    }

    /// Engine tests on a paused clock: the runtime advances virtual time whenever every task
    /// is idle, so `pass` returns once everything due within the span has run.
    struct Paused {
        fake: Fake,
        commands: Sender<Command>,
        events: Receiver<ClientEvent>,
        _dir: tempfile::TempDir,
    }

    fn start_paused(configure: impl FnOnce(&mut FakeState)) -> Paused {
        let dir = tempfile::tempdir().unwrap();
        let fake = Fake::default();
        {
            let mut state = fake.state();
            state.account = UID.into();
            configure(&mut state);
        }
        let config = ClientConfig::new(dir.path(), dir.path().join("session.json"), account());
        let (commands, command_rx) = async_channel::unbounded();
        let (event_tx, events) = async_channel::unbounded();
        tokio::spawn(Engine::run(Arc::new(fake.clone()), config, command_rx, event_tx));
        Paused { fake, commands, events, _dir: dir }
    }

    impl Paused {
        fn send(&self, command: Command) {
            self.commands.try_send(command).unwrap();
        }

        async fn pass(&self, span: Duration) {
            tokio::time::sleep(span).await;
        }

        async fn online(&self) {
            while self.events.recv().await.unwrap() != ClientEvent::Connection(ConnectionState::Online) {}
        }

        fn sends(&self) -> Vec<PayloadId> {
            self.fake
                .calls()
                .into_iter()
                .filter_map(|c| if let Call::Send { payload, .. } = c { Some(payload) } else { None })
                .collect()
        }
    }

    #[tokio::test(start_paused = true)]
    async fn typing_is_reported_at_most_every_four_seconds_per_chat() {
        let p = start_paused(|_| {});
        p.online().await;
        p.send(Command::Typing(id("a")));
        p.send(Command::Typing(id("a")));
        p.send(Command::Typing(id("b")));
        p.pass(Duration::from_secs(3)).await;
        assert_eq!(p.fake.calls(), [Call::Typing(id("a")), Call::Typing(id("b"))]);

        p.send(Command::Typing(id("a")));
        p.pass(Duration::from_millis(900)).await;
        assert_eq!(p.fake.calls().len(), 2, "3.9 s after the last report is too early");
        p.pass(Duration::from_millis(200)).await;
        p.send(Command::Typing(id("a")));
        p.pass(Duration::from_millis(10)).await;
        assert_eq!(p.fake.calls().len(), 3);
        assert_eq!(p.fake.calls()[2], Call::Typing(id("a")));

        // The window restarts at the last report, not at the first.
        p.pass(Duration::from_secs(3)).await;
        p.send(Command::Typing(id("a")));
        p.pass(Duration::from_millis(10)).await;
        assert_eq!(p.fake.calls().len(), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn transient_send_failures_back_off_5_10_up_to_60_seconds() {
        let p = start_paused(|s| {
            s.send_results = (0..14)
                .map(|i| if i % 2 == 0 { Err(BackendError::Network("down".into())) } else { Err(rejected("THROTTLED", false)) })
                .collect();
        });
        p.online().await;
        p.send(Command::Send { chat: id("a"), text: "hi".into(), reply_to: None });
        p.pass(Duration::from_millis(200)).await;
        // Invariant: the clock stands 0.2 s after the latest attempt.
        for failures in 1..=14u32 {
            let backoff = Duration::from_secs(u64::from((5 * failures).min(60)));
            assert_eq!(p.sends().len(), failures as usize);
            p.pass(backoff - Duration::from_millis(400)).await;
            assert_eq!(p.sends().len(), failures as usize, "retried before the {backoff:?} backoff ended");
            p.pass(Duration::from_millis(400)).await;
            assert_eq!(p.sends().len(), failures as usize + 1, "not retried after the {backoff:?} backoff");
        }
        let sends = p.sends();
        assert!(sends.iter().all(|payload| *payload == sends[0]));
        let mut events = Vec::new();
        while let Ok(event) = p.events.try_recv() {
            events.push(event);
        }
        assert!(events.iter().any(|e| matches!(e, ClientEvent::PendingSettled { .. })));
        assert!(!events.iter().any(|e| matches!(e, ClientEvent::MessageUpserted(m) if matches!(m.delivery, Delivery::Failed { .. }))));
    }

    #[tokio::test(start_paused = true)]
    async fn a_new_connection_retries_a_held_queue_at_once() {
        let p = start_paused(|s| s.send_results = VecDeque::from([Err(BackendError::Network("down".into()))]));
        p.online().await;
        p.send(Command::Send { chat: id("a"), text: "hi".into(), reply_to: None });
        p.pass(Duration::from_secs(1)).await;
        assert_eq!(p.sends().len(), 1);
        p.fake.push(LiveEvent::Disconnected);
        p.fake.push(LiveEvent::Reconnected);
        p.pass(Duration::from_secs(1)).await;
        assert_eq!(p.sends().len(), 2, "well inside the 5 s backoff");
    }

    // -- drafts -----------------------------------------------------------------------------

    fn wait_drafts(h: &Harness, what: &str, cond: impl Fn(&[Draft]) -> bool) -> Vec<Draft> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let drafts = h.cache().drafts().unwrap();
            if cond(&drafts) {
                return drafts;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}; drafts {drafts:?}");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn startup_emits_the_stored_drafts() {
        let reply = ReplyTarget { timestamp: 4, author: Some("Ann".into()), text: "hi".into() };
        let stored = vec![
            Draft { chat: id("a"), text: "unsent".into(), reply_to: Some(reply) },
            Draft { chat: id("b"), text: "also".into(), reply_to: None },
        ];
        let h = start(
            |_, dir| {
                let cache = Cache::open(dir, &account()).unwrap();
                for draft in &stored {
                    cache.put_draft(draft).unwrap();
                }
            },
            50,
        );
        assert_eq!(h.next("drafts", |e| matches!(e, ClientEvent::Drafts(_))), ClientEvent::Drafts(stored));
    }

    #[test]
    fn drafts_are_saved_and_removed_when_nothing_is_left() {
        let h = start(|_, _| {}, 50);
        assert_eq!(h.next("drafts", |e| matches!(e, ClientEvent::Drafts(_))), ClientEvent::Drafts(Vec::new()));
        let save = |text: &str, reply_to: Option<ReplyTarget>| {
            h.client.send(Command::SaveDraft(Draft { chat: id("a"), text: text.into(), reply_to }));
        };
        save("x", None);
        wait_drafts(&h, "the draft", |d| d.len() == 1);
        save("xy", None);
        wait_drafts(&h, "the newer text", |d| d.len() == 1 && d[0].text == "xy");
        save("", None);
        wait_drafts(&h, "the removal", |d| d.is_empty());
        // A reply target alone is a draft.
        let target = ReplyTarget { timestamp: 4, author: None, text: "q".into() };
        save("", Some(target.clone()));
        let drafts = wait_drafts(&h, "the reply-only draft", |d| d.len() == 1);
        assert_eq!(drafts[0].reply_to, Some(target));
    }

    #[test]
    fn sending_deletes_the_chats_draft() {
        let h = start(
            |s, dir| {
                s.chats = vec![chat("a", Some(5), None), chat("b", Some(5), None)];
                let cache = Cache::open(dir, &account()).unwrap();
                cache.put_draft(&Draft { chat: id("a"), text: "typing".into(), reply_to: None }).unwrap();
                cache.put_draft(&Draft { chat: id("b"), text: "keep".into(), reply_to: None }).unwrap();
            },
            50,
        );
        h.online();
        h.send_text("a", "typing");
        h.upserted("pending message");
        let drafts = h.cache().drafts().unwrap();
        assert_eq!(drafts.iter().map(|d| d.chat.0.as_str()).collect::<Vec<_>>(), ["b"]);
    }

    #[test]
    fn a_send_that_cannot_be_stored_is_refused_without_touching_the_draft_or_the_backend() {
        let h = start(
            |s, dir| {
                s.chats = vec![chat("a", Some(5), None)];
                let cache = Cache::open(dir, &account()).unwrap();
                cache.put_draft(&Draft { chat: id("a"), text: "typing".into(), reply_to: None }).unwrap();
                drop(cache);
                rusqlite::Connection::open(crate::paths::cache_path(dir, &account()))
                    .unwrap()
                    .execute_batch("CREATE TRIGGER refuse BEFORE INSERT ON outbox BEGIN SELECT RAISE(ABORT, 'disk full'); END;")
                    .unwrap();
            },
            50,
        );
        h.online();
        h.send_text("a", "typing");
        let events = h.until("refusal", |e| matches!(e, ClientEvent::SendRefused { .. }));
        let draft = Draft { chat: id("a"), text: "typing".into(), reply_to: None };
        assert_eq!(events.last(), Some(&ClientEvent::SendRefused { draft, reason: STORE_FAILED_REASON.into() }));
        assert!(!events.iter().any(|e| matches!(e, ClientEvent::MessageUpserted(_))), "no pending message: {events:?}");
        h.quiet();
        assert_eq!(h.fake.calls(), [], "nothing may be sent without a stored entry");
        assert_eq!(h.cache().drafts().unwrap().len(), 1, "the draft survives");
        assert!(h.cache().outbox().unwrap().is_empty());

        // Storage recovers: the next send works normally.
        rusqlite::Connection::open(crate::paths::cache_path(h.dir.path(), &account()))
            .unwrap()
            .execute_batch("DROP TRIGGER refuse")
            .unwrap();
        h.send_text("a", "typing");
        h.next("settled", |e| matches!(e, ClientEvent::PendingSettled { .. }));
        assert_eq!(h.fake.calls().len(), 1);
        assert!(h.cache().drafts().unwrap().is_empty());
    }

    // -- no hidden writes -------------------------------------------------------------------

    #[test]
    fn loading_chats_history_and_older_pages_never_write() {
        let h = start(
            |s, dir| {
                seed_cache(dir, &[chat("a", Some(120), None)], "a", &[msg("a", 119, "x"), msg("a", 120, "y")], false);
                s.chats = vec![chat("a", Some(120), None), chat("b", Some(1), None)];
                s.history.insert(id("a"), (1..=120).map(|i| msg("a", i, "m")).collect());
                s.history.insert(id("b"), vec![msg("b", 1, "only")]);
            },
            50,
        );
        h.online();
        h.history("startup catch-up");
        h.client.send(Command::OpenChat(id("a")));
        h.history("cached newest page");
        h.history("fetched newest page");
        h.client.send(Command::LoadOlder { chat: id("a"), before: 71 });
        h.history("older page");
        h.client.send(Command::OpenChat(id("b")));
        h.history("second chat");
        // A reconnect refetches the newest chunk of every cached chat.
        h.fake.push(LiveEvent::Disconnected);
        h.fake.push(LiveEvent::Reconnected);
        h.online();
        h.history("catch-up of the open chat");
        h.history("catch-up of the other chat");
        // Paging while offline answers from the cache.
        h.fake.state().live = None;
        h.next("offline", |e| *e == ClientEvent::Connection(ConnectionState::Offline));
        h.client.send(Command::LoadOlder { chat: id("a"), before: 71 });
        h.history("cached older page");

        h.quiet();
        assert!(h.fake.state().history_calls.len() >= 6, "the scenario must have loaded history");
        assert_eq!(h.fake.calls(), [], "loading must never write");
    }

    #[test]
    fn a_gallery_previews_like_an_image() {
        let item = GalleryItem { media: MediaRef { id: "b/1".into(), kind: MediaKind::Thumbnail }, width: 4, height: 3, name: None };
        let gallery = |caption: Option<&str>| MessageBody::Gallery { items: vec![item.clone()], caption: caption.map(Into::into) };
        assert_eq!(preview_text(&gallery(None)).as_deref(), Some("Фото"));
        assert_eq!(preview_text(&gallery(Some("отпуск"))).as_deref(), Some("отпуск"));
    }

    #[test]
    fn the_fake_streams_scripted_bytes_with_progress_and_scripted_errors() {
        let fake = Fake::default();
        let original = MediaRef { id: "b/1".into(), kind: MediaKind::File };
        fake.state().files.insert("b/1".into(), b"hello".to_vec());
        fake.state().download_errors.push_back(BackendError::Network("boom".into()));
        let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let run = |media: &MediaRef| {
            let seen = Mutex::new(Vec::new());
            let mut out = Vec::new();
            let result = runtime.block_on(fake.download(media, &mut out, &|received, total| seen.lock().unwrap().push((received, total))));
            (result, out, seen.into_inner().unwrap())
        };
        assert_eq!(run(&original).0, Err(BackendError::Network("boom".into())));
        let (result, out, progress) = run(&original);
        assert_eq!((result, out.as_slice()), (Ok(5), b"hello".as_slice()));
        assert_eq!(progress, [(2, Some(5)), (5, Some(5))]);
        let missing = MediaRef { id: "b/2".into(), kind: MediaKind::File };
        assert_eq!(run(&missing).0, Err(BackendError::NotFound));
        assert_eq!(fake.state().download_calls, [original.clone(), original, missing]);
    }

    // -- downloads ------------------------------------------------------------------------

    fn original(id: &str) -> MediaRef {
        MediaRef { id: id.into(), kind: MediaKind::File }
    }

    /// A started client whose Downloads folder exists and whose server holds `files`.
    fn start_downloads(files: &[(&str, &[u8])], configure: impl FnOnce(&mut FakeState)) -> Harness {
        start(
            |s, dir| {
                fs::create_dir(dir.join("Downloads")).unwrap();
                for (id, bytes) in files {
                    s.files.insert((*id).into(), bytes.to_vec());
                }
                configure(s);
            },
            50,
        )
    }

    impl Harness {
        fn downloads(&self) -> PathBuf {
            self.dir.path().join("Downloads")
        }

        /// Names in the Downloads folder, sorted; empty when the folder does not exist.
        fn listing(&self) -> Vec<String> {
            let mut names: Vec<String> = fs::read_dir(self.downloads())
                .map(|entries| entries.map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect())
                .unwrap_or_default();
            names.sort();
            names
        }

        fn download(&self, media: &MediaRef, name: &str) {
            self.client.send(Command::Download { media: media.clone(), name: Some(name.into()) });
        }

        fn downloaded(&self) -> PathBuf {
            match self.next("downloaded", |e| matches!(e, ClientEvent::Downloaded { .. })) {
                ClientEvent::Downloaded { path, .. } => path,
                _ => unreachable!(),
            }
        }

        /// Waits until `cond` holds for something the engine does asynchronously.
        fn eventually(&self, what: &str, cond: impl Fn() -> bool) {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !cond() {
                assert!(Instant::now() < deadline, "timed out waiting for {what}");
                std::thread::sleep(Duration::from_millis(2));
            }
        }

        /// Releases the transfers parked on `gate` until `done`. `notify_one` keeps a permit when
        /// nobody waits yet, so a release is never lost to the race with the engine thread.
        fn open_gate(&self, gate: &tokio::sync::Notify, what: &str, done: impl Fn() -> bool) {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !done() {
                assert!(Instant::now() < deadline, "timed out waiting for {what}");
                gate.notify_one();
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }

    fn is_download_event(event: &ClientEvent) -> bool {
        matches!(
            event,
            ClientEvent::DownloadProgress { .. }
                | ClientEvent::Downloaded { .. }
                | ClientEvent::DownloadFailed { .. }
                | ClientEvent::DownloadCancelled(_)
        )
    }

    #[test]
    fn a_download_saves_the_exact_bytes_and_reports_the_final_path() {
        let h = start_downloads(&[("b/1", b"hello world")], |_| {});
        h.online();
        h.download(&original("b/1"), "report.pdf");
        let events = h.until("saved", |e| matches!(e, ClientEvent::Downloaded { .. }));
        let Some(ClientEvent::Downloaded { media, path }) = events.last() else { unreachable!() };
        assert_eq!(*media, original("b/1"));
        assert_eq!(*path, h.downloads().join("report.pdf"));
        assert_eq!(fs::read(path).unwrap(), b"hello world");
        assert_eq!(h.listing(), ["report.pdf"], "no partial file is left behind");
        assert!(events.iter().any(|e| matches!(e, ClientEvent::DownloadProgress { .. })));
    }

    #[test]
    fn a_hostile_or_missing_name_becomes_one_harmless_name_inside_the_folder() {
        let h = start_downloads(&[("b/1", b"one"), ("b/2", b"two")], |_| {});
        h.online();
        h.download(&original("b/1"), "../../.ssh/authorized_keys");
        assert_eq!(h.downloaded(), h.downloads().join("authorized_keys"));
        h.client.send(Command::Download { media: original("b/2"), name: None });
        assert_eq!(h.downloaded(), h.downloads().join("Файл"));
        assert!(!h.dir.path().join(".ssh").exists());
        assert_eq!(h.listing().len(), 2);
    }

    #[test]
    fn downloading_again_saves_a_new_copy_and_changes_no_existing_file() {
        let h = start_downloads(&[("b/1", b"hello")], |_| {});
        h.online();
        fs::write(h.downloads().join("other.txt"), "mine").unwrap();
        h.download(&original("b/1"), "report.pdf");
        assert_eq!(h.downloaded(), h.downloads().join("report.pdf"));
        h.download(&original("b/1"), "report.pdf");
        assert_eq!(h.downloaded(), h.downloads().join("report (1).pdf"));
        h.download(&original("b/1"), "other.txt");
        assert_eq!(h.downloaded(), h.downloads().join("other (1).txt"));
        assert_eq!(fs::read(h.downloads().join("report.pdf")).unwrap(), b"hello");
        assert_eq!(fs::read(h.downloads().join("report (1).pdf")).unwrap(), b"hello");
        assert_eq!(fs::read(h.downloads().join("other.txt")).unwrap(), b"mine");
    }

    #[test]
    fn cancel_stops_the_transfer_and_removes_the_partial() {
        let gate = Arc::new(tokio::sync::Notify::new());
        let h = start_downloads(&[("b/1", b"hello world")], |s| s.download_gate = Some(gate.clone()));
        h.online();
        h.download(&original("b/1"), "report.pdf");
        h.wait_for("the transfer", |s| s.download_calls.len() == 1);
        assert_eq!(h.listing(), ["report.pdf.part"]);
        h.client.send(Command::CancelDownload(original("b/1")));
        h.next("cancelled", |e| *e == ClientEvent::DownloadCancelled(original("b/1")));
        h.eventually("the partial to go", || h.listing().is_empty());
        gate.notify_one();
        h.quiet();
        let seen = h.drain();
        assert!(!seen.iter().any(|e| matches!(e, ClientEvent::Downloaded { .. } | ClientEvent::DownloadFailed { .. })), "{seen:?}");
        assert!(h.listing().is_empty());
    }

    #[test]
    fn a_download_started_after_a_cancel_is_its_own_transfer() {
        let gate = Arc::new(tokio::sync::Notify::new());
        let h = start_downloads(&[("b/1", b"hello world")], |s| s.download_gate = Some(gate.clone()));
        h.online();
        h.download(&original("b/1"), "report.pdf");
        h.wait_for("the first transfer", |s| s.download_calls.len() == 1);
        h.client.send(Command::CancelDownload(original("b/1")));
        h.next("cancelled", |e| *e == ClientEvent::DownloadCancelled(original("b/1")));
        h.download(&original("b/1"), "report.pdf");
        h.wait_for("the second transfer", |s| s.download_calls.len() == 2);
        h.open_gate(&gate, "the file", || h.listing() == ["report.pdf"]);
        assert_eq!(h.downloaded(), h.downloads().join("report.pdf"));
        h.quiet();
        assert!(!h.drain().iter().any(|e| matches!(e, ClientEvent::Downloaded { .. } | ClientEvent::DownloadFailed { .. })));
    }

    #[test]
    fn backend_errors_remove_the_partial_and_map_to_one_reason_each() {
        let h = start_downloads(&[], |s| {
            s.download_errors = VecDeque::from([
                BackendError::NotFound,
                BackendError::Storage("StorageFull".into()),
                BackendError::Network("reset".into()),
                BackendError::Protocol("odd".into()),
            ]);
        });
        h.online();
        let media = original("b/1");
        for expected in [DownloadFailure::NotFound, DownloadFailure::Disk, DownloadFailure::Network, DownloadFailure::Other] {
            h.download(&media, "a.bin");
            h.next("failure", |e| *e == ClientEvent::DownloadFailed { media: media.clone(), reason: expected });
            assert!(h.listing().is_empty(), "{expected:?} left a file");
        }
        // Nothing scripted and no such file on the server: a 404.
        h.download(&media, "a.bin");
        h.next("not found", |e| *e == ClientEvent::DownloadFailed { media: media.clone(), reason: DownloadFailure::NotFound });
        assert!(h.listing().is_empty());
    }

    #[test]
    fn offline_fails_fast_without_asking_the_backend() {
        let h = start(
            |s, dir| {
                fs::create_dir(dir.join("Downloads")).unwrap();
                s.bootstrap_errors = std::iter::repeat_n(BackendError::Network("down".into()), 1000).collect();
            },
            50,
        );
        h.next("offline", |e| *e == ClientEvent::Connection(ConnectionState::Offline));
        h.download(&original("b/1"), "a.bin");
        h.next("refusal", |e| *e == ClientEvent::DownloadFailed { media: original("b/1"), reason: DownloadFailure::Offline });
        h.quiet();
        assert!(h.fake.state().download_calls.is_empty());
        assert!(h.listing().is_empty());
    }

    #[test]
    fn a_missing_or_unusable_folder_fails_fast_without_asking_the_backend() {
        let missing = start(|s, _| { s.files.insert("b/1".into(), b"x".to_vec()); }, 50);
        let not_a_folder = start(
            |s, dir| {
                fs::write(dir.join("Downloads"), "a file, not a folder").unwrap();
                s.files.insert("b/1".into(), b"x".to_vec());
            },
            50,
        );
        for h in [&missing, &not_a_folder] {
            h.online();
            h.download(&original("b/1"), "a.bin");
            h.next("refusal", |e| *e == ClientEvent::DownloadFailed { media: original("b/1"), reason: DownloadFailure::Destination });
            h.quiet();
            assert!(h.fake.state().download_calls.is_empty());
        }
        assert!(!missing.downloads().exists(), "a missing folder is not created");
        assert!(fs::metadata(not_a_folder.downloads()).unwrap().is_file());
    }

    #[test]
    fn a_second_download_of_a_running_file_starts_no_second_transfer() {
        let gate = Arc::new(tokio::sync::Notify::new());
        let h = start_downloads(&[("b/1", b"hello world")], |s| s.download_gate = Some(gate.clone()));
        h.online();
        h.download(&original("b/1"), "a.bin");
        h.download(&original("b/1"), "b.bin");
        h.wait_for("the transfer", |s| s.download_calls.len() == 1);
        h.quiet();
        assert_eq!(h.fake.state().download_calls.len(), 1);
        h.open_gate(&gate, "the file", || h.listing() == ["a.bin"]);
        h.downloaded();
        h.quiet();
        assert!(!h.drain().iter().any(|e| matches!(e, ClientEvent::Downloaded { .. })), "reported twice");
        assert_eq!(h.fake.state().download_calls.len(), 1);
    }

    #[test]
    fn the_fourth_concurrent_download_waits_for_a_slot() {
        let gate = Arc::new(tokio::sync::Notify::new());
        let files: Vec<(String, Vec<u8>)> = (1..=4).map(|i| (format!("f/{i}"), vec![i as u8; 8])).collect();
        let listed: Vec<(&str, &[u8])> = files.iter().map(|(id, bytes)| (id.as_str(), bytes.as_slice())).collect();
        let h = start_downloads(&listed, |s| s.download_gate = Some(gate.clone()));
        h.online();
        for i in 1..=4 {
            h.download(&original(&format!("f/{i}")), &format!("f{i}.bin"));
        }
        h.wait_for("three transfers", |s| s.download_calls.len() == 3);
        h.quiet();
        assert_eq!(h.fake.state().download_calls.len(), 3, "the fourth must wait");
        assert_eq!(h.listing().len(), 3, "the fourth has no partial yet");
        gate.notify_one();
        h.wait_for("the fourth transfer", |s| s.download_calls.len() == 4);
        h.open_gate(&gate, "all four files", || h.listing().iter().filter(|name| !name.ends_with(".part")).count() == 4);
        for _ in 0..4 {
            h.downloaded();
        }
    }

    #[test]
    fn progress_is_throttled_and_the_last_report_is_the_full_size() {
        let h = start_downloads(&[("b/1", b"hello")], |_| {});
        h.online();
        h.download(&original("b/1"), "a.bin");
        let events = h.until("saved", |e| matches!(e, ClientEvent::Downloaded { .. }));
        let progress: Vec<(u64, Option<u64>)> = events
            .iter()
            .filter_map(|e| match e {
                ClientEvent::DownloadProgress { received, total, .. } => Some((*received, *total)),
                _ => None,
            })
            .collect();
        // The Fake reports twice, back to back: the second is held back, and the engine's closing
        // report brings the full size exactly once.
        assert_eq!(progress, [(2, Some(5)), (5, Some(5))]);
    }

    #[test]
    fn the_throttle_passes_one_report_per_interval_and_completes_the_stream() {
        let throttle = Throttle::default();
        let t0 = Instant::now();
        assert!(throttle.admit(t0, 10, Some(100)));
        assert!(!throttle.admit(t0 + Duration::from_millis(50), 20, Some(100)));
        assert!(throttle.admit(t0 + DOWNLOAD_PROGRESS_INTERVAL, 30, None));
        assert_eq!(throttle.last(30), None, "already said");
        assert_eq!(throttle.last(100), Some((100, Some(100))));
    }

    #[test]
    fn sign_out_stops_running_downloads_and_keeps_saved_files() {
        let h = start_downloads(&[("b/1", b"one"), ("b/2", b"two")], |_| {});
        h.online();
        h.download(&original("b/1"), "a.bin");
        h.downloaded();
        let gate = Arc::new(tokio::sync::Notify::new());
        h.fake.state().download_gate = Some(gate);
        h.download(&original("b/2"), "b.bin");
        h.wait_for("the running transfer", |s| s.download_calls.len() == 2);
        assert_eq!(h.listing(), ["a.bin", "b.bin.part"]);
        h.client.send(Command::SignOut);
        h.next("signed out", |e| *e == ClientEvent::SignedOut);
        h.eventually("the partial to go", || h.listing() == ["a.bin"]);
        assert_eq!(fs::read(h.downloads().join("a.bin")).unwrap(), b"one");
    }

    #[test]
    fn session_expiry_during_a_download_ends_the_session_and_removes_the_partial() {
        let h = start_downloads(&[], |s| s.download_errors = VecDeque::from([BackendError::SessionExpired]));
        h.online();
        h.download(&original("b/1"), "a.bin");
        h.next("expired", |e| *e == ClientEvent::SessionExpired);
        assert!(h.listing().is_empty());
        assert!(!h.dir.path().join("session.json").exists());
        h.quiet();
        assert!(!h.drain().iter().any(|e| matches!(e, ClientEvent::DownloadFailed { .. })), "expiry is not a download failure");
    }

    #[test]
    fn expiry_aborts_every_running_download() {
        let gate = Arc::new(tokio::sync::Notify::new());
        let h = start_downloads(&[("b/1", b"one"), ("b/2", b"two")], |s| s.download_gate = Some(gate.clone()));
        h.online();
        h.download(&original("b/1"), "a.bin");
        h.download(&original("b/2"), "b.bin");
        h.wait_for("both transfers", |s| s.download_calls.len() == 2);
        assert_eq!(h.listing(), ["a.bin.part", "b.bin.part"]);
        h.fake.push(LiveEvent::Failed(BackendError::SessionExpired));
        h.next("expired", |e| *e == ClientEvent::SessionExpired);
        h.eventually("the partials to go", || h.listing().is_empty());
        gate.notify_one();
        h.quiet();
        assert!(!h.drain().iter().any(|e| matches!(e, ClientEvent::Downloaded { .. })));
        assert!(h.listing().is_empty());
    }

    #[test]
    fn originals_never_go_through_the_preview_path_and_previews_never_download() {
        let h = start_downloads(&[("b/1", b"one")], |s| { s.media.insert("b/1".into(), b"preview".to_vec()); });
        h.online();
        h.client.send(Command::FetchMedia(original("b/1")));
        h.client.send(Command::Download { media: MediaRef { id: "b/1".into(), kind: MediaKind::Thumbnail }, name: Some("a.bin".into()) });
        h.quiet();
        assert_eq!(h.fake.state().media_calls, 0);
        assert!(h.fake.state().download_calls.is_empty());
        let seen = h.drain();
        assert!(!seen.iter().any(|e| is_download_event(e) || matches!(e, ClientEvent::MediaReady { .. } | ClientEvent::MediaUnavailable(_))), "{seen:?}");
        assert!(h.listing().is_empty());
    }

    fn hits(title: &str) -> SearchHits {
        SearchHits {
            chats: vec![ChatHit { id: id("a"), kind: ChatKind::Group, title: title.into(), avatar: None }],
            ..SearchHits::default()
        }
    }

    fn search_events(events: &[ClientEvent]) -> Vec<&ClientEvent> {
        events.iter().filter(|e| matches!(e, ClientEvent::SearchResults { .. } | ClientEvent::SearchFailed { .. })).collect()
    }

    #[test]
    fn search_results_arrive_for_the_current_id_and_nothing_is_cached() {
        let expected = hits("Team");
        let h = start(|s, _| s.search_result = Some(Ok(expected.clone())), 50);
        h.online();
        h.search(1, "ab");
        let event = h.next("results", |e| matches!(e, ClientEvent::SearchResults { .. }));
        assert_eq!(event, ClientEvent::SearchResults { id: SearchId(1), hits: expected });
        assert_eq!(h.fake.state().search_calls, [("ab".to_string(), SEARCH_LIMIT)]);
        assert!(h.cache().chats().unwrap().is_empty(), "search results are held in memory only");
    }

    #[test]
    fn the_results_of_a_superseded_search_are_dropped() {
        let gate = Arc::new(tokio::sync::Notify::new());
        let h = start(
            |s, _| {
                s.search_gate = Some(gate.clone());
                s.search_result = Some(Ok(hits("Team")));
            },
            50,
        );
        h.online();
        h.search(1, "a");
        h.wait_for("the first request", |s| s.search_calls.len() == 1);
        h.search(2, "ab");
        h.wait_for("the second request", |s| s.search_calls.len() == 2);
        h.open_gate(&gate, "both answers", || h.fake.state().search_done == 2);
        h.quiet();
        let events = h.drain();
        assert_eq!(search_events(&events), [&ClientEvent::SearchResults { id: SearchId(2), hits: hits("Team") }]);
    }

    #[test]
    fn results_after_cancel_search_are_dropped() {
        let gate = Arc::new(tokio::sync::Notify::new());
        let h = start(|s, _| s.search_gate = Some(gate.clone()), 50);
        h.online();
        h.search(1, "ab");
        h.wait_for("the request", |s| s.search_calls.len() == 1);
        h.client.send(Command::CancelSearch);
        h.quiet();
        h.open_gate(&gate, "the answer", || h.fake.state().search_done == 1);
        h.quiet();
        let events = h.drain();
        assert!(search_events(&events).is_empty(), "{events:?}");
    }

    #[test]
    fn search_offline_fails_fast_without_asking_the_backend() {
        let h = start(|s, _| s.bootstrap_errors = std::iter::repeat_n(BackendError::Network("down".into()), 1000).collect(), 50);
        h.next("offline", |e| *e == ClientEvent::Connection(ConnectionState::Offline));
        h.search(1, "ab");
        h.next("refusal", |e| *e == ClientEvent::SearchFailed { id: SearchId(1), reason: SearchFailure::Offline });
        h.quiet();
        assert!(h.fake.state().search_calls.is_empty());
    }

    #[test]
    fn a_failed_search_reports_failed() {
        let h = start(|s, _| s.search_result = Some(Err(BackendError::Network("reset".into()))), 50);
        h.online();
        h.search(1, "ab");
        h.next("failure", |e| *e == ClientEvent::SearchFailed { id: SearchId(1), reason: SearchFailure::Failed });
        assert_eq!(h.fake.state().search_calls.len(), 1);
    }

    #[test]
    fn session_expiry_during_a_search_ends_the_session() {
        let h = start(|s, _| s.search_result = Some(Err(BackendError::SessionExpired)), 50);
        h.online();
        h.search(1, "ab");
        h.next("expired", |e| *e == ClientEvent::SessionExpired);
        assert!(!h.dir.path().join("session.json").exists());
        h.quiet();
        let events = h.drain();
        assert!(search_events(&events).is_empty(), "expiry is not a search failure: {events:?}");
    }
}
