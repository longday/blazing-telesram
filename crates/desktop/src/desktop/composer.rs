//! Writing in the open chat: composer, reply and edit modes, drafts, the message menu, read markers and error toasts.

use super::{AccountMenu, Desktop, OpenTimeline, SIDEBAR_WIDTH, overlay};
use crate::format::{messages_count, one_line};
use crate::input::{InputEvent, TextInput};
use crate::selection::{copy_text, eligible};
use crate::theme::*;
use crate::widgets::{Icon, button, icon};
use bt_chat_core::model::{ChatId, Delivery, Draft, Message, MessageBody, MessageTs, ReplyTarget};
use bt_chat_core::reactions::{KNOWN, ReactionCode};
use gpui::{prelude::*, *};
use std::borrow::Cow;
use std::collections::HashMap;
use std::time::Duration;

/// Quiet time after the last keystroke before the draft is handed to the core.
const DRAFT_DELAY: Duration = Duration::from_secs(1);
const TOAST_TIME: Duration = Duration::from_secs(5);
/// A quoted text is cut here; the server shows only the start of it anyway.
const QUOTE_CHARS: usize = 200;
const SEND_SIZE: f32 = 36.;

/// What the composer held before an edit began; Escape or Enter puts it back.
struct Restore {
    text: String,
    reply: Option<ReplyTarget>,
}

struct Editing {
    timestamp: MessageTs,
    original: String,
    before: Restore,
}

pub(super) struct Compose {
    pub(super) input: Entity<TextInput>,
    /// The chat whose draft the input holds.
    chat: Option<ChatId>,
    reply: Option<ReplyTarget>,
    editing: Option<Editing>,
    /// Dropping it cancels the pending draft save.
    draft_timer: Option<Task<()>>,
    /// Newest message reported as read per chat, so a position is reported once.
    marked: HashMap<ChatId, MessageTs>,
}

impl Compose {
    pub(super) fn new(cx: &mut Context<Desktop>) -> Self {
        Self {
            input: cx.new(|cx| TextInput::multi_line("Написать сообщение…", cx)),
            chat: None,
            reply: None,
            editing: None,
            draft_timer: None,
            marked: HashMap::new(),
        }
    }
}

pub(super) struct MessageMenu {
    chat: ChatId,
    timestamp: MessageTs,
    position: Point<Pixels>,
}

pub(super) struct Toast {
    text: String,
    /// Tells a stale timer from the one of the toast on screen.
    seq: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MenuAction {
    Reactors,
    Reply,
    Copy,
    Select,
    Edit,
    Delete,
}

impl MenuAction {
    fn selector(self) -> &'static str {
        match self {
            Self::Reactors => "menu-reactors",
            Self::Reply => "menu-reply",
            Self::Copy => "menu-copy",
            Self::Select => "menu-select",
            Self::Edit => "menu-edit",
            Self::Delete => "menu-delete",
        }
    }

    /// The line icon of the entry; «Кто отреагировал» has none (the entry keeps the icon's room).
    fn icon(self) -> Option<Icon> {
        match self {
            Self::Reactors => None,
            Self::Reply => Some(Icon::Reply),
            Self::Copy => Some(Icon::Copy),
            Self::Select => None,
            Self::Edit => Some(Icon::Edit),
            Self::Delete => Some(Icon::Trash),
        }
    }

    /// The entry's text; Copy reads «Скопировать выделенное» when it copies the text selection (D6).
    fn label(self, picked: bool) -> &'static str {
        match self {
            Self::Reactors => "Кто отреагировал",
            Self::Reply => "Ответить",
            Self::Copy if picked => "Скопировать выделенное",
            Self::Copy => "Скопировать текст",
            Self::Select => "Выбрать",
            Self::Edit => "Редактировать",
            Self::Delete => "Удалить",
        }
    }

    /// Reactors, Reply and Copy apply to any message; Edit and Delete only to the user's own.
    fn is_own_only(self) -> bool {
        matches!(self, Self::Edit | Self::Delete)
    }
}

fn menu_actions(m: &Message) -> Vec<MenuAction> {
    let sent = m.delivery == Delivery::Sent;
    let own = m.outgoing && sent;
    let content = !matches!(m.body, MessageBody::Deleted | MessageBody::Unsupported(_));
    let mut actions = Vec::new();
    if has_reactors(m) {
        actions.push(MenuAction::Reactors);
    }
    if sent && content {
        actions.push(MenuAction::Reply);
    }
    if copy_text(m).is_some() {
        actions.push(MenuAction::Copy);
    }
    if eligible(m) {
        actions.push(MenuAction::Select);
    }
    if editable(m) {
        actions.push(MenuAction::Edit);
    }
    if own && !matches!(m.body, MessageBody::Deleted) {
        actions.push(MenuAction::Delete);
    }
    actions
}

/// «Кто отреагировал» is offered for a sent, non-deleted message with at least one reaction
/// (`counts` never holds a zero count); a pending or failed one has none to list.
fn has_reactors(m: &Message) -> bool {
    m.delivery == Delivery::Sent
        && !matches!(m.body, MessageBody::Deleted)
        && !m.reactions.counts.is_empty()
}

/// The menu offers the reaction row on any sent, non-deleted message, as the chips do; the row
/// is hidden while the table of reaction types is empty.
fn reactable(m: &Message) -> bool {
    !KNOWN.is_empty() && m.delivery == Delivery::Sent && !matches!(m.body, MessageBody::Deleted)
}

fn quote_text(m: &Message) -> String {
    let text = match &m.body {
        MessageBody::Text(text) => Cow::Borrowed(text.as_str()),
        MessageBody::Image { caption, .. } | MessageBody::Gallery { caption, .. } => {
            Cow::Borrowed(caption.as_deref().unwrap_or("Фото"))
        }
        MessageBody::File { name, .. } => Cow::Borrowed(name.as_str()),
        MessageBody::Voice { .. } | MessageBody::Video { .. } => Cow::Owned(m.body.preview_text()),
        MessageBody::Deleted | MessageBody::Unsupported(_) => Cow::Borrowed(""),
    };
    text.chars().take(QUOTE_CHARS).collect()
}

/// Only messages the server lets the user edit: an edit has to repeat the forward and reply
/// references, so one that cannot (a forward, a reply without a quoted message id) is refused.
fn editable(m: &Message) -> bool {
    m.outgoing
        && m.delivery == Delivery::Sent
        && matches!(m.body, MessageBody::Text(_))
        && m.forwarded_from.is_none()
        && m.reply_to
            .as_ref()
            .is_none_or(|quote| quote.timestamp.is_some())
}

/// The refused text in front of `current`; the reply target the user has chosen since wins.
fn with_refused(
    refused: &Draft,
    current: &str,
    reply: Option<ReplyTarget>,
) -> (String, Option<ReplyTarget>) {
    let text = if current.is_empty() {
        refused.text.clone()
    } else {
        format!("{}\n{current}", refused.text)
    };
    (text, reply.or_else(|| refused.reply_to.clone()))
}

impl Desktop {
    /// Makes the open timeline and the composer follow the store's selected chat.
    pub(super) fn sync_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let store = self.store.read(cx);
        let selected = store.selected().cloned();
        if self.open.as_ref().map(|o| &o.chat) == selected.as_ref() {
            return;
        }
        let list = selected.as_ref().map(|chat| {
            let len = store.timeline(chat).map_or(0, |t| t.messages.len());
            ListState::new(len, ListAlignment::Bottom, px(600.))
        });
        self.leave_chat(cx);
        self.message_menu = None;
        self.close_reactors(cx);
        self.delete_pending.clear();
        self.open = selected.zip(list).map(|(chat, list)| OpenTimeline {
            chat,
            list,
            rows: Default::default(),
        });
        self.enter_chat(window, cx);
        cx.notify();
    }

    fn leave_chat(&mut self, cx: &mut Context<Self>) {
        self.cancel_edit(cx);
        if self.store.read(cx).session() == crate::store::SessionState::Active {
            self.save_draft(cx);
        }
        self.compose.draft_timer = None;
        self.compose.chat = None;
    }

    fn enter_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(chat) = self.open.as_ref().map(|o| o.chat.clone()) else {
            return;
        };
        let draft = self.store.read(cx).draft(&chat).cloned();
        self.compose.chat = Some(chat);
        self.compose.reply = draft.as_ref().and_then(|d| d.reply_to.clone());
        let text = draft.map(|d| d.text).unwrap_or_default();
        self.compose
            .input
            .update(cx, |input, cx| input.set_text(&text, cx));
        window.focus(&self.compose.input.focus_handle(cx));
    }

    /// Fills an untouched composer with a draft that arrived after the chat was opened.
    pub(super) fn restore_late_draft(&mut self, cx: &mut Context<Self>) {
        let Some(chat) = self.compose.chat.clone() else {
            return;
        };
        let untouched = self.compose.editing.is_none()
            && self.compose.reply.is_none()
            && self.compose.input.read(cx).content.is_empty();
        let Some(draft) = self
            .store
            .read(cx)
            .draft(&chat)
            .cloned()
            .filter(|_| untouched)
        else {
            return;
        };
        self.compose.reply = draft.reply_to;
        self.compose
            .input
            .update(cx, |input, cx| input.set_text(&draft.text, cx));
        cx.notify();
    }

    /// The account changed: what the composer holds (text, reply or edit target, a pending save)
    /// belongs to the previous account. It is dropped unsaved, so no draft of it reaches the new
    /// account's cache, and the chat the composer followed is left without a save.
    pub(super) fn discard_composer(&mut self, cx: &mut Context<Self>) {
        self.compose.draft_timer = None;
        self.compose.editing = None;
        self.compose.reply = None;
        self.compose.chat = None;
        self.compose.marked.clear();
        self.compose
            .input
            .update(cx, |input, cx| input.set_text("", cx));
        cx.notify();
    }

    pub(super) fn composer_event(&mut self, event: InputEvent, cx: &mut Context<Self>) {
        if self.blocked(cx) {
            return;
        }
        match event {
            InputEvent::Submit => self.submit(cx),
            InputEvent::UpInEmpty => self.edit_newest(cx),
            InputEvent::Edited => {
                if self.compose.editing.is_some() {
                    return;
                }
                if let Some(chat) = self.compose.chat.clone() {
                    // The server reads the chat for an outgoing typing, so it is withheld over unseen messages.
                    let store = self.store.read(cx);
                    if store.chat(&chat).is_none_or(|c| c.unread == 0) {
                        store.typing(chat);
                    }
                }
                self.schedule_draft_save(cx);
            }
        }
    }

    fn schedule_draft_save(&mut self, cx: &mut Context<Self>) {
        if self.compose.editing.is_some() {
            return;
        }
        self.compose.draft_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DRAFT_DELAY).await;
            this.update(cx, |this, cx| this.save_draft(cx)).ok();
        }));
    }

    /// Hands the composer state of the current chat to the store; edit mode has no draft of its own.
    fn save_draft(&mut self, cx: &mut Context<Self>) {
        let Some(chat) = self
            .compose
            .chat
            .clone()
            .filter(|_| self.compose.editing.is_none())
        else {
            return;
        };
        let text = self.compose.input.read(cx).content.to_string();
        let draft = Draft {
            chat,
            text,
            reply_to: self.compose.reply.clone(),
        };
        self.store
            .update(cx, |store, cx| store.save_draft(draft, cx));
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        if self.blocked(cx) {
            return;
        }
        let Some(chat) = self.compose.chat.clone() else {
            return;
        };
        let text = self.compose.input.read(cx).content.trim().to_string();
        if text.is_empty() {
            return;
        }
        match self.compose.editing.take() {
            Some(edit) => {
                if text != edit.original.trim() {
                    self.store.read(cx).edit(chat, edit.timestamp, text);
                }
                self.restore(edit.before, cx);
            }
            None => {
                self.compose.draft_timer = None;
                let reply = self.compose.reply.take();
                self.compose
                    .input
                    .update(cx, |input, cx| input.set_text("", cx));
                self.store
                    .update(cx, |store, cx| store.send_text(chat, text, reply, cx));
            }
        }
        cx.notify();
    }

    /// A send was not queued: the refused text goes in front of whatever the user has in the composer
    /// since (or in the saved draft of another chat), so neither text is lost.
    pub(super) fn send_refused(&mut self, draft: Draft, reason: String, cx: &mut Context<Self>) {
        self.show_toast(reason, cx);
        if self.compose.chat.as_ref() == Some(&draft.chat) {
            if let Some(edit) = self.compose.editing.as_mut() {
                // The composer shows the edited text; what it held before the edit is the draft, and it
                // is saved now because edit mode never autosaves and the app may quit mid-edit.
                let (text, reply_to) =
                    with_refused(&draft, &edit.before.text, edit.before.reply.take());
                edit.before = Restore {
                    text: text.clone(),
                    reply: reply_to.clone(),
                };
                let merged = Draft {
                    chat: draft.chat,
                    text,
                    reply_to,
                };
                self.store
                    .update(cx, |store, cx| store.save_draft(merged, cx));
                return;
            }
            let current = self.compose.input.read(cx).content.to_string();
            let (text, reply) = with_refused(&draft, &current, self.compose.reply.take());
            self.compose.reply = reply;
            self.compose
                .input
                .update(cx, |input, cx| input.set_text(&text, cx));
            self.schedule_draft_save(cx);
            cx.notify();
            return;
        }
        self.store.update(cx, |store, cx| {
            let merged = match store.draft(&draft.chat).cloned() {
                Some(stored) => {
                    let (text, reply_to) = with_refused(&draft, &stored.text, stored.reply_to);
                    Draft {
                        chat: stored.chat,
                        text,
                        reply_to,
                    }
                }
                None => draft,
            };
            store.save_draft(merged, cx);
        });
    }

    fn restore(&mut self, before: Restore, cx: &mut Context<Self>) {
        self.compose.reply = before.reply;
        self.compose
            .input
            .update(cx, |input, cx| input.set_text(&before.text, cx));
    }

    fn find_message(&self, timestamp: MessageTs, cx: &App) -> Option<Message> {
        let chat = &self.open.as_ref()?.chat;
        let messages = &self.store.read(cx).timeline(chat)?.messages;
        let ix = messages
            .binary_search_by_key(&timestamp, |m| m.timestamp)
            .ok()?;
        Some(messages[ix].clone())
    }

    /// Whether the text selection is live and reaches message `ts` (D6): what the menu's Copy label and
    /// the right click ask. Only the validity check and the range of this one message, no text is built.
    fn selection_touches(&self, ts: MessageTs, cx: &App) -> bool {
        let Some(chat) = self.open.as_ref().map(|o| &o.chat) else {
            return false;
        };
        let Some(timeline) = self.store.read(cx).timeline(chat) else {
            return false;
        };
        let messages = &timeline.messages;
        self.picking
            .read(cx)
            .text()
            .is_some_and(|s| s.validate(messages) && s.range_in(messages, ts).is_some())
    }

    /// What Copy puts on the clipboard for message `ts` when the text selection touches it (D6, D7): the
    /// whole selection, also across messages. `None` when nothing is selected, the selection is stale
    /// or it does not reach this message; the menu then copies the message's own text. Built only when
    /// the user chooses Copy.
    fn selection_copy(&self, ts: MessageTs, cx: &App) -> Option<String> {
        if !self.selection_touches(ts, cx) {
            return None;
        }
        let chat = &self.open.as_ref()?.chat;
        let messages = &self.store.read(cx).timeline(chat)?.messages;
        self.picking.read(cx).selected_text(messages, |m| {
            crate::format::copy_header(m.author_name.as_deref(), m.timestamp, self.clock)
        })
    }

    /// The label of the open menu's Copy entry.
    #[cfg(test)]
    pub(crate) fn menu_copy_label(&self, cx: &App) -> Option<&'static str> {
        let menu = self.message_menu.as_ref()?;
        Some(MenuAction::Copy.label(self.selection_touches(menu.timestamp, cx)))
    }

    fn start_reply(&mut self, timestamp: MessageTs, window: &mut Window, cx: &mut Context<Self>) {
        let Some(message) = self
            .find_message(timestamp, cx)
            .filter(|m| m.delivery == Delivery::Sent && !self.blocked(cx))
        else {
            return;
        };
        self.cancel_edit(cx);
        self.compose.reply = Some(ReplyTarget {
            timestamp,
            author: message.author_name.clone(),
            text: quote_text(&message),
        });
        self.schedule_draft_save(cx);
        window.focus(&self.compose.input.focus_handle(cx));
        cx.notify();
    }

    fn start_edit(&mut self, timestamp: MessageTs, cx: &mut Context<Self>) {
        let Some(message) = self
            .find_message(timestamp, cx)
            .filter(|m| editable(m) && !self.blocked(cx))
        else {
            return;
        };
        self.clear_selection(cx);
        let MessageBody::Text(original) = message.body else {
            return;
        };
        let before = match self.compose.editing.take() {
            Some(edit) => edit.before,
            None => {
                self.save_draft(cx);
                self.compose.draft_timer = None;
                Restore {
                    text: self.compose.input.read(cx).content.to_string(),
                    reply: self.compose.reply.take(),
                }
            }
        };
        self.compose
            .input
            .update(cx, |input, cx| input.set_text(&original, cx));
        self.compose.editing = Some(Editing {
            timestamp,
            original,
            before,
        });
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn composer_editing(&self) -> Option<MessageTs> {
        self.compose.editing.as_ref().map(|e| e.timestamp)
    }

    #[cfg(test)]
    pub(crate) fn menu_is_open(&self) -> bool {
        self.message_menu.is_some()
    }

    #[cfg(test)]
    pub(crate) fn delete_is_pending(&self) -> bool {
        !self.delete_pending.is_empty()
    }

    fn edit_newest(&mut self, cx: &mut Context<Self>) {
        let Some(chat) = self.compose.chat.as_ref() else {
            return;
        };
        let newest = self.store.read(cx).timeline(chat).and_then(|t| {
            t.messages
                .iter()
                .rev()
                .find(|m| editable(m))
                .map(|m| m.timestamp)
        });
        if let Some(timestamp) = newest {
            self.start_edit(timestamp, cx);
        }
    }

    fn cancel_edit(&mut self, cx: &mut Context<Self>) {
        if let Some(edit) = self.compose.editing.take() {
            self.restore(edit.before, cx);
            cx.notify();
        }
    }

    /// Leaves edit mode, or else drops the reply target.
    fn cancel_mode(&mut self, cx: &mut Context<Self>) {
        if self.compose.editing.is_some() {
            self.cancel_edit(cx);
        } else if self.compose.reply.take().is_some() {
            self.schedule_draft_save(cx);
            cx.notify();
        }
    }

    /// Escape: closes the topmost thing that can be closed.
    pub(super) fn dismiss(&mut self, cx: &mut Context<Self>) {
        // What covers the chat closes before anything beneath it is touched.
        if self.log.is_some() {
            self.toggle_log(cx);
        } else if !self.delete_pending.is_empty() {
            self.delete_pending.clear();
            cx.notify();
        } else if self.menu == AccountMenu::ConfirmSignOut {
            self.menu = AccountMenu::Closed;
            cx.notify();
        } else if self.viewer.is_some() {
            self.close_viewer(cx);
        } else if self.profile {
            self.close_profile(cx);
        } else if self.new_chat.is_some() {
            self.close_new_chat(cx);
        } else if self.blocked(cx) {
            // The session dialogs offer only sign-out.
        } else if self.reactors_popover.is_some() {
            self.close_reactors(cx);
        } else if self.message_menu.take().is_some() {
            cx.notify();
        } else if self.clear_selection(cx) {
            // The text selection, or message mode, closes before the info panel and the reply or edit mode.
        } else if self.info.is_some() {
            self.close_info(cx);
        } else {
            self.cancel_mode(cx);
        }
    }

    pub(crate) fn open_menu(
        &mut self,
        chat: ChatId,
        timestamp: MessageTs,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if self.open.as_ref().map(|o| &o.chat) != Some(&chat)
            || self.blocked(cx)
            || self.in_mode(cx)
        {
            return;
        }
        let Some(message) = self.find_message(timestamp, cx) else {
            return;
        };
        // A right click keeps a selection that touches the clicked message and drops any other (D6).
        if !self.selection_touches(timestamp, cx) {
            self.clear_selection(cx);
        }
        if menu_actions(&message).is_empty() && !reactable(&message) {
            return;
        }
        self.message_menu = Some(MessageMenu {
            chat,
            timestamp,
            position,
        });
        cx.notify();
    }

    /// The user chose reaction `code` on a message: ignored while the session is blocked or the
    /// chat is not the open one. The core decides between add, replace and remove.
    pub(crate) fn toggle_reaction(
        &mut self,
        chat: ChatId,
        timestamp: MessageTs,
        code: ReactionCode,
        cx: &mut Context<Self>,
    ) {
        if self.open.as_ref().map(|o| &o.chat) != Some(&chat) || self.blocked(cx) {
            return;
        }
        self.store.read(cx).toggle_reaction(chat, timestamp, code);
    }

    fn run_menu_reaction(&mut self, code: ReactionCode, cx: &mut Context<Self>) {
        let Some(menu) = self.message_menu.take() else {
            return;
        };
        self.toggle_reaction(menu.chat, menu.timestamp, code, cx);
        cx.notify();
    }

    fn run_menu_action(&mut self, action: MenuAction, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.message_menu.take() else {
            return;
        };
        if self.blocked(cx) {
            cx.notify();
            return;
        }
        match action {
            MenuAction::Reactors => {
                self.open_reactors(menu.chat, menu.timestamp, menu.position, cx)
            }
            MenuAction::Reply => self.start_reply(menu.timestamp, window, cx),
            MenuAction::Copy => {
                let text = self.selection_copy(menu.timestamp, cx).or_else(|| {
                    let message = self.find_message(menu.timestamp, cx)?;
                    copy_text(&message).map(str::to_string)
                });
                if let Some(text) = text {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
            }
            MenuAction::Select => self.enter_mode(menu.chat, menu.timestamp, window, cx),
            MenuAction::Edit => {
                self.start_edit(menu.timestamp, cx);
                window.focus(&self.compose.input.focus_handle(cx));
            }
            MenuAction::Delete => self.delete_pending = vec![(menu.chat, menu.timestamp)],
        }
        cx.notify();
    }

    /// Sends one delete per entry of the dialog, in timeline order, and closes message mode. A refused
    /// one is reported through the store's usual action failure and leaves its message in place.
    fn confirm_delete(&mut self, cx: &mut Context<Self>) {
        let pending = std::mem::take(&mut self.delete_pending);
        if pending.is_empty() {
            return;
        }
        let store = self.store.read(cx);
        for (chat, timestamp) in pending {
            store.delete_message(chat, timestamp);
        }
        self.clear_selection(cx);
        cx.notify();
    }

    pub(super) fn show_toast(&mut self, text: String, cx: &mut Context<Self>) {
        let seq = self.toast.as_ref().map_or(0, |t| t.seq + 1);
        self.toast = Some(Toast { text, seq });
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(TOAST_TIME).await;
            this.update(cx, |this, cx| {
                if this.toast.as_ref().is_some_and(|t| t.seq == seq) {
                    this.toast = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn toast_text(&self) -> Option<&str> {
        self.toast.as_ref().map(|t| t.text.as_str())
    }

    #[cfg(test)]
    pub(crate) fn composer_input(&self) -> &Entity<TextInput> {
        &self.compose.input
    }

    #[cfg(test)]
    pub(crate) fn composer_reply(&self) -> Option<&ReplyTarget> {
        self.compose.reply.as_ref()
    }

    /// Reports the newest sent message that has been seen in the viewport as read, while the window
    /// is the active one and nothing covers the chat. A row is seen when its bottom edge is inside the
    /// viewport: a row taller than the viewport can never be entirely inside it.
    pub(super) fn check_read(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(open) = self.open.as_ref() else {
            return;
        };
        // Drained even when unused, so a frame's report never outlives it.
        let rows = std::mem::take(&mut *open.rows.borrow_mut());
        if !window.is_window_active() || self.blocked(cx) {
            return;
        }
        let store = self.store.read(cx);
        let Some(messages) = store
            .timeline(&open.chat)
            .filter(|t| t.loaded)
            .map(|t| &t.messages)
        else {
            return;
        };
        let viewport = open.list.viewport_bounds();
        let slack = px(1.);
        let newest = rows
            .iter()
            .filter(|(_, row)| {
                row.bottom() > viewport.top() && row.bottom() <= viewport.bottom() + slack
            })
            .filter_map(|(timestamp, _)| {
                let ix = messages
                    .binary_search_by_key(timestamp, |m| m.timestamp)
                    .ok()?;
                (messages[ix].delivery == Delivery::Sent).then_some(*timestamp)
            })
            .max();
        let Some(timestamp) = newest else { return };
        let chat = open.chat.clone();
        if self
            .compose
            .marked
            .get(&chat)
            .is_some_and(|&last| last >= timestamp)
        {
            return;
        }
        self.compose.marked.insert(chat.clone(), timestamp);
        store.mark_read(chat, timestamp);
    }

    pub(super) fn composer_view(&mut self, cx: &mut Context<Self>) -> Div {
        // Message mode replaces the composer; its input and draft stay as they are.
        if let Some(bar) = self.selection_bar(cx) {
            return bar;
        }
        let blank = self.compose.input.read(cx).content.trim().is_empty();
        let bar = match (&self.compose.editing, &self.compose.reply) {
            (Some(edit), _) => Some(self.quote_bar(
                "edit-bar",
                "edit-bar-close",
                Icon::Edit,
                "Редактирование".into(),
                &edit.original,
                cx,
            )),
            (None, Some(reply)) => {
                let title = reply.author.clone().unwrap_or_else(|| "Сообщение".into());
                Some(self.quote_bar(
                    "reply-bar",
                    "reply-bar-close",
                    Icon::Reply,
                    title,
                    &reply.text,
                    cx,
                ))
            }
            (None, None) => None,
        };
        div()
            .debug_selector(|| "composer".into())
            .flex_none()
            .flex()
            .flex_col()
            .bg(rgb(PANEL))
            .border_t_1()
            .border_color(rgb(DIVIDER))
            .children(bar)
            .child(
                div()
                    .px(px(16.))
                    .py(px(9.))
                    .flex()
                    .items_end()
                    .gap(px(12.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .min_h(px(SEND_SIZE))
                            .flex()
                            .items_center()
                            .child(
                                div()
                                    .debug_selector(|| "composer-input".into())
                                    .w_full()
                                    .child(self.compose.input.clone()),
                            ),
                    )
                    .child(
                        div()
                            .id("composer-send")
                            .debug_selector(|| "composer-send".into())
                            .size(px(SEND_SIZE))
                            .flex_none()
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(16.))
                            .bg(rgb(if blank { BADGE_MUTED } else { SEND_BUTTON }))
                            .when(!blank, |d| d.cursor_pointer())
                            .child("➤")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.submit(cx);
                                window.focus(&this.compose.input.focus_handle(cx));
                            })),
                    ),
            )
    }

    fn quote_bar(
        &self,
        selector: &'static str,
        close: &'static str,
        glyph: Icon,
        title: String,
        text: &str,
        cx: &mut Context<Self>,
    ) -> Div {
        div()
            .debug_selector(move || selector.into())
            .px(px(16.))
            .pt(px(8.))
            .flex()
            .items_center()
            .gap(px(12.))
            .child(icon(glyph, 16., ACCENT))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .pl(px(8.))
                    .border_l(px(3.))
                    .border_color(rgb(ACCENT))
                    .child(
                        div()
                            .truncate()
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(ACCENT))
                            .child(title),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(12.))
                            .text_color(rgb(MUTED))
                            .child(one_line(text)),
                    ),
            )
            .child(
                div()
                    .id(close)
                    .debug_selector(move || close.into())
                    .flex_none()
                    .size(px(28.))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(rgb(MUTED))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(ROW_HOVER)))
                    .child("✕")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.cancel_mode(cx);
                        window.focus(&this.compose.input.focus_handle(cx));
                    })),
            )
    }

    pub(super) fn message_menu_view(&self, cx: &mut Context<Self>) -> Option<Div> {
        let menu = self.message_menu.as_ref()?;
        let message = self.find_message(menu.timestamp, cx)?;
        let actions = menu_actions(&message);
        let picked = self.selection_touches(menu.timestamp, cx);
        let reactions = reactable(&message);
        if actions.is_empty() && !reactions {
            return None;
        }
        let mut items: Vec<AnyElement> = Vec::new();
        if reactions {
            items.push(Self::reaction_row(&message, cx).into_any_element());
            if !actions.is_empty() {
                items.push(
                    div()
                        .h(px(1.))
                        .my(px(4.))
                        .bg(rgb(MENU_SEPARATOR))
                        .into_any_element(),
                );
            }
        }
        for (i, &action) in actions.iter().enumerate() {
            if i > 0 && action.is_own_only() != actions[i - 1].is_own_only() {
                items.push(
                    div()
                        .h(px(1.))
                        .my(px(4.))
                        .bg(rgb(MENU_SEPARATOR))
                        .into_any_element(),
                );
            }
            let selector = action.selector();
            items.push(
                div()
                    .id(selector)
                    .debug_selector(move || selector.into())
                    .h(px(34.))
                    .px(px(14.))
                    .flex()
                    .items_center()
                    .gap(px(14.))
                    .cursor_pointer()
                    .text_size(px(13.))
                    .text_color(rgb(if action == MenuAction::Delete {
                        DANGER
                    } else {
                        INK
                    }))
                    .hover(|s| s.bg(rgb(MENU_HOVER)))
                    .child(match action.icon() {
                        Some(kind) => icon(kind, 16., MENU_ICON).into_any_element(),
                        None => div().size(px(16.)).flex_none().into_any_element(),
                    })
                    .child(action.label(picked))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.run_menu_action(action, window, cx)
                    }))
                    .into_any_element(),
            );
        }
        let close = cx.listener(|this, _: &MouseDownEvent, _, cx| {
            this.message_menu = None;
            cx.notify();
        });
        Some(
            div()
                .absolute()
                .size_full()
                .occlude()
                .on_any_mouse_down(close)
                .child(
                    anchored()
                        .position(menu.position)
                        .snap_to_window_with_margin(px(8.))
                        .child(
                            div()
                                .debug_selector(|| "message-menu".into())
                                .w(px(220.))
                                .py(px(4.))
                                .rounded(px(8.))
                                .bg(rgb(PANEL))
                                .border_1()
                                .border_color(rgb(MENU_SEPARATOR))
                                // The mockup's shadow: the surface offset down and right, 35% black.
                                .shadow(vec![BoxShadow {
                                    color: hsla(0., 0., 0., 0.35),
                                    offset: point(px(2.), px(4.)),
                                    blur_radius: px(10.),
                                    spread_radius: px(0.),
                                }])
                                .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
                                .children(items),
                        ),
                ),
        )
    }

    /// One button per [`KNOWN`] type above the entries; the account's own type is highlighted.
    fn reaction_row(message: &Message, cx: &mut Context<Self>) -> Div {
        let buttons = KNOWN.iter().map(|known| {
            let code = known.code;
            div()
                .id(SharedString::from(format!("menu-reaction-{}", code.0)))
                .debug_selector(move || format!("menu-reaction-{}", code.0))
                .size(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .cursor_pointer()
                .text_size(px(16.))
                .when(message.reactions.mine == Some(code), |d| {
                    d.bg(rgb(REACTION_OWN))
                })
                .hover(|s| s.bg(rgb(MENU_HOVER)))
                .child(known.emoji)
                .on_click(cx.listener(move |this, _, _, cx| this.run_menu_reaction(code, cx)))
        });
        div()
            .debug_selector(|| "menu-reactions".into())
            .px(px(6.))
            .py(px(2.))
            .flex()
            .flex_wrap()
            .justify_between()
            .children(buttons)
    }

    pub(super) fn delete_dialog(&self, cx: &mut Context<Self>) -> Option<Div> {
        let count = self.delete_pending.len();
        if count == 0 {
            return None;
        }
        let (title, text) = if count == 1 {
            (
                "Удалить сообщение?".to_string(),
                "Сообщение будет удалено для всех участников чата.",
            )
        } else {
            (
                format!("Удалить {}?", messages_count(count)),
                "Сообщения будут удалены для всех участников чата.",
            )
        };
        Some(overlay(
            "delete-dialog",
            title,
            text,
            vec![
                button("cancel-delete", "Отмена")
                    .debug_selector(|| "cancel-delete".into())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.delete_pending.clear();
                        cx.notify();
                    })),
                button("confirm-delete", "Удалить")
                    .debug_selector(|| "confirm-delete".into())
                    .text_color(rgb(DANGER))
                    .on_click(cx.listener(|this, _, _, cx| this.confirm_delete(cx))),
            ],
        ))
    }

    pub(super) fn toast_view(&self) -> Option<Div> {
        let toast = self.toast.as_ref()?;
        Some(
            div()
                .absolute()
                .bottom(px(88.))
                .left(px(SIDEBAR_WIDTH))
                .right_0()
                .flex()
                .justify_center()
                .child(
                    div()
                        .debug_selector(|| "toast".into())
                        .max_w(px(480.))
                        .px(px(14.))
                        .py(px(8.))
                        .rounded(px(10.))
                        .bg(rgb(FIELD))
                        .text_color(rgb(INK))
                        .child(toast.text.clone()),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: it would bring gpui's `test` attribute macro over `#[test]`.
    use super::{MenuAction, copy_text, editable, menu_actions, quote_text};
    use bt_chat_core::model::{
        ChatId, Delivery, MediaKind, MediaRef, Message, MessageBody, PayloadId, UserId,
    };

    fn voice() -> Message {
        Message {
            chat: ChatId("0/0/c".into()),
            timestamp: 1,
            author: UserId("u".into()),
            author_name: None,
            author_avatar: None,
            outgoing: false,
            body: MessageBody::Voice {
                media: MediaRef {
                    id: "b/1".into(),
                    kind: MediaKind::File,
                },
                duration_ms: Some(5_000),
                size: 4_000,
                name: Some("v.ogg".into()),
                waveform: None,
            },
            edited: false,
            reply_to: None,
            forwarded_from: None,
            delivery: Delivery::Sent,
            reactions: Default::default(),
            important: false,
        }
    }

    #[test]
    fn a_voice_message_can_be_replied_to_and_quotes_its_preview_text() {
        let m = voice();
        assert_eq!(quote_text(&m), "Голосовое сообщение");
        assert!(menu_actions(&m).contains(&MenuAction::Reply));
    }

    #[test]
    fn a_voice_message_has_no_text_to_copy_and_cannot_be_edited() {
        let m = voice();
        assert_eq!(copy_text(&m), None);
        assert!(!menu_actions(&m).contains(&MenuAction::Copy));
        assert!(!editable(&m));
    }

    #[test]
    fn a_video_can_be_replied_to_quotes_its_preview_text_and_copies_its_name() {
        let mut m = voice();
        m.body = MessageBody::Video {
            media: MediaRef {
                id: "b/2".into(),
                kind: MediaKind::File,
            },
            name: "clip.mp4".into(),
            size: 1,
            width: 4,
            height: 3,
            duration_ms: None,
        };
        assert_eq!(quote_text(&m), "Видео");
        assert_eq!(copy_text(&m), Some("clip.mp4"));
        assert!(menu_actions(&m).contains(&MenuAction::Reply));
        assert!(!editable(&m));
    }

    #[test]
    fn select_follows_copy_for_a_sent_message_and_is_not_offered_otherwise() {
        let mut m = voice();
        m.body = MessageBody::Text("hi".into());
        let actions = menu_actions(&m);
        let copy = actions.iter().position(|a| *a == MenuAction::Copy).unwrap();
        assert_eq!(actions[copy + 1], MenuAction::Select, "after Copy");
        assert!(!MenuAction::Select.is_own_only());
        assert_eq!(MenuAction::Select.selector(), "menu-select");
        assert_eq!(MenuAction::Select.label(false), "Выбрать");
        assert!(MenuAction::Select.icon().is_none());

        let mut own = m.clone();
        own.outgoing = true;
        assert!(menu_actions(&own).contains(&MenuAction::Select));
        assert!(menu_actions(&voice()).contains(&MenuAction::Select));

        let mut pending = own.clone();
        pending.delivery = Delivery::Pending(PayloadId("p".into()));
        assert!(!menu_actions(&pending).contains(&MenuAction::Select));
        let mut failed = own.clone();
        failed.delivery = Delivery::Failed {
            payload: PayloadId("p".into()),
            reason: "no".into(),
        };
        assert!(!menu_actions(&failed).contains(&MenuAction::Select));
        let mut deleted = own;
        deleted.body = MessageBody::Deleted;
        assert!(!menu_actions(&deleted).contains(&MenuAction::Select));
    }

    #[test]
    fn copy_reads_selected_only_for_a_message_the_selection_touches() {
        assert_eq!(MenuAction::Copy.label(true), "Скопировать выделенное");
        assert_eq!(MenuAction::Copy.label(false), "Скопировать текст");
        // Only Copy changes with the selection; every other entry reads the same.
        for action in [
            MenuAction::Reactors,
            MenuAction::Reply,
            MenuAction::Select,
            MenuAction::Edit,
            MenuAction::Delete,
        ] {
            assert_eq!(action.label(true), action.label(false));
        }
    }
}
