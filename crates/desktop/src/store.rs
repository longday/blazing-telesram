//! UI-side state: the single place where `ClientEvent`s land and `Command`s leave.

use async_channel::{Receiver, Sender};
use bt_chat_core::model::{
    ChatId, ChatKind, ChatSummary, ClientEvent, Command, ConnectionState, HistoryChunk, MediaKind,
    MediaRef, Message, MessageBody, MessageTs,
};
use gpui::{Context, EventEmitter};
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

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

/// How a timeline changed; lets a list view splice instead of rebuilding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TimelineChange {
    /// Contents replaced wholesale; list indexes are no longer meaningful.
    Reset,
    /// `prepended` messages added in front, `appended` at the end, and the (post-change) indexes in `touched` replaced in place.
    Delta { prepended: usize, appended: usize, touched: Vec<usize> },
    /// Only loading flags changed.
    Status,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreEvent {
    Chats,
    Timeline { chat: ChatId, change: TimelineChange },
    Media(MediaRef),
    Connection,
    Session,
}

pub struct ChatStore {
    commands: Sender<Command>,
    chats: HashMap<ChatId, ChatSummary>,
    chats_loaded: bool,
    selected: Option<ChatId>,
    timelines: HashMap<ChatId, Timeline>,
    media: HashMap<MediaRef, PathBuf>,
    media_requested: HashSet<MediaRef>,
    connection: ConnectionState,
    session: SessionState,
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
            connection: ConnectionState::Connecting,
            session: SessionState::Active,
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
            }
            ClientEvent::ChatUpdated(chat) => {
                self.want_media(chat.avatar.as_ref());
                self.chats.insert(chat.id.clone(), chat);
                cx.emit(StoreEvent::Chats);
            }
            ClientEvent::History(chunk) => self.apply_history(chunk, cx),
            ClientEvent::MessageUpserted(message) => self.upsert(message, cx),
            ClientEvent::MessageDeleted { chat, timestamp } => self.delete(&chat, timestamp, cx),
            ClientEvent::MediaReady { media, path } => {
                self.media.insert(media.clone(), path);
                cx.emit(StoreEvent::Media(media));
            }
            ClientEvent::Connection(state) => {
                self.connection = state;
                cx.emit(StoreEvent::Connection);
            }
            ClientEvent::SessionExpired => {
                self.session = SessionState::Expired;
                cx.emit(StoreEvent::Session);
            }
            ClientEvent::SignedOut => {
                self.chats.clear();
                self.chats_loaded = false;
                self.selected = None;
                self.timelines.clear();
                self.media.clear();
                self.media_requested.clear();
                self.session = SessionState::SignedOut;
                cx.emit(StoreEvent::Session);
            }
        }
        cx.notify();
    }

    /// Opens a chat: the core answers with its newest chunk. Never marks anything as read.
    pub fn select(&mut self, chat: ChatId, cx: &mut Context<Self>) {
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

    fn want_message_media(&mut self, message: &Message) {
        self.want_media(message.author_avatar.as_ref());
        if let MessageBody::Image { media, .. } = &message.body {
            self.want_media(Some(media));
        }
    }

    fn apply_history(&mut self, chunk: HistoryChunk, cx: &mut Context<Self>) {
        for message in &chunk.messages {
            self.want_message_media(message);
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
}
