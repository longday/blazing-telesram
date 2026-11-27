//! Root view: chat list on the left, timeline and composer on the right.

mod composer;

use crate::format::{Clock, chat_time, log_time, one_line};
use crate::info_panel;
use crate::input::{InputEvent, TextInput};
use crate::logging::{LogBuffer, LogLine};
use crate::new_chat::{self, Mode, NewChat};
use crate::profile;
use crate::reactors;
use crate::search_results::{self, SearchView, StatusLine};
use crate::store::{
    ChatFilter, ChatStore, CreationState, MembersState, ProfileState, SEARCH_DEBOUNCE,
    SEARCH_MIN_CHARS, SearchStatus, SessionState, StoreEvent, TimelineChange,
};
use crate::theme::*;
use crate::timeline::{self, RowBounds, RowContext};
use crate::viewer::{self, Viewer};
use crate::widgets::{avatar, button};
use bt_chat_core::model::{
    ChatId, ChatKind, ChatSummary, ConnectionState, Delivery, GalleryItem, MessagePreview,
    MessageTs, UserId,
};
use composer::{Compose, MessageMenu, Toast};
use gpui::{prelude::*, *};
use std::ops::Range;
use std::time::Duration;

actions!(
    bt,
    [Search, Quit, ToggleLog, Dismiss, ViewerPrev, ViewerNext]
);

/// How often an open log view looks for new records.
const LOG_POLL: Duration = Duration::from_millis(250);

const ROW_HEIGHT: f32 = 64.;
const SIDEBAR_WIDTH: f32 = 340.;

const FILTERS: [(&str, &str, ChatFilter); 5] = [
    ("filter-all", "Все", ChatFilter::All),
    ("filter-private", "Личные", ChatFilter::Private),
    ("filter-groups", "Группы", ChatFilter::Groups),
    ("filter-channels", "Каналы", ChatFilter::Channels),
    ("filter-unread", "Непрочитанные", ChatFilter::Unread),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccountMenu {
    Closed,
    Open,
    ConfirmSignOut,
}

struct OpenTimeline {
    chat: ChatId,
    list: ListState,
    rows: RowBounds,
}

/// Where the reactor list is drawn. Which list is open and what it holds is the store's
/// (`ChatStore::reactors`); this is only the place the entry that opened it was chosen.
struct ReactorsPopover {
    chat: ChatId,
    timestamp: MessageTs,
    position: Point<Pixels>,
}

struct LogView {
    lines: Vec<LogLine>,
    generation: u64,
    scroll: UniformListScrollHandle,
    _poll: Task<()>,
}

/// The search field as the server search sees it: which text was last looked at and the debounce state.
struct SearchInput {
    /// The field content the generation counts changes of.
    seen: SharedString,
    /// Bumped on every change of the content; a debounce timer acts only if it still holds its own value.
    generation: u64,
    /// The generation whose query was handled (sent, cancelled or refused); behind `generation` while a timer runs.
    settled: u64,
    timer: Option<Task<()>>,
}

/// What the server search is asked for: the trimmed field text, if it has at least `SEARCH_MIN_CHARS` characters.
fn search_query(content: &str) -> Option<&str> {
    let query = content.trim();
    (query.chars().count() >= SEARCH_MIN_CHARS).then_some(query)
}

pub struct Desktop {
    pub(crate) store: Entity<ChatStore>,
    clock: Clock,
    pub(crate) search: Entity<TextInput>,
    search_input: SearchInput,
    pub(crate) filter: ChatFilter,
    open: Option<OpenTimeline>,
    /// The chat whose info panel is open; always the open chat, or `None`.
    info: Option<ChatId>,
    /// Whether the read-only profile modal is open.
    profile: bool,
    /// The new chat dialog, when open.
    new_chat: Option<NewChat>,
    pub(crate) menu: AccountMenu,
    compose: Compose,
    message_menu: Option<MessageMenu>,
    reactors_popover: Option<ReactorsPopover>,
    delete_pending: Option<(ChatId, MessageTs)>,
    toast: Option<Toast>,
    viewer: Option<Viewer>,
    log: Option<LogView>,
    /// Whether a blocking overlay covered the chat when focus was last placed.
    covered: bool,
    /// Focused when nothing else is, so window shortcuts resolve in this view's key context.
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl Desktop {
    pub fn new(
        store: Entity<ChatStore>,
        clock: Clock,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| TextInput::single_line("Поиск", cx));
        let compose = Compose::new(cx);
        let subscriptions = vec![
            cx.observe(&search, |this, _, cx| {
                this.search_changed(cx);
                cx.notify();
            }),
            cx.observe(&compose.input, |_, _, cx| cx.notify()),
            cx.subscribe_in(
                &compose.input,
                window,
                |this, _, event: &InputEvent, _, cx| this.composer_event(*event, cx),
            ),
            cx.observe_in(&store, window, |this, _, window, cx| {
                this.sync_open(window, cx);
                this.sync_viewer(cx);
                this.sync_info();
                this.sync_profile(cx);
                this.sync_new_chat(cx);
                this.sync_reactors(cx);
                cx.notify();
            }),
            cx.subscribe_in(
                &store,
                window,
                |this, _, event: &StoreEvent, _, cx| match event {
                    StoreEvent::Timeline { chat, change } => {
                        this.apply_timeline_change(chat, change, cx)
                    }
                    StoreEvent::Drafts => this.restore_late_draft(cx),
                    StoreEvent::Connection => {
                        this.resume_search(cx);
                        this.resume_reads(cx);
                    }
                    StoreEvent::ActionFailed { reason, .. } => this.show_toast(reason.clone(), cx),
                    StoreEvent::SendRefused { draft, reason } => {
                        this.send_refused(draft.clone(), reason.clone(), cx)
                    }
                    StoreEvent::ChatCreated { chat, .. } => {
                        this.new_chat_created(chat.is_none(), cx)
                    }
                    _ => {}
                },
            ),
            // Whether the window is active decides if the visible messages count as read.
            cx.observe_window_activation(window, |_, _, cx| cx.notify()),
        ];
        // Keystrokes with nothing focused would skip this view's key context.
        let focus = cx.focus_handle();
        window.focus(&focus);
        Self {
            store,
            clock,
            search,
            search_input: SearchInput {
                seen: SharedString::default(),
                generation: 0,
                settled: 0,
                timer: None,
            },
            filter: ChatFilter::All,
            open: None,
            info: None,
            profile: false,
            new_chat: None,
            menu: AccountMenu::Closed,
            compose,
            message_menu: None,
            reactors_popover: None,
            delete_pending: None,
            toast: None,
            viewer: None,
            log: None,
            covered: false,
            focus,
            _subscriptions: subscriptions,
        }
    }

    /// True while the log view or a modal dialog covers the chat: the composer ignores input and no
    /// message counts as read until it is gone.
    pub(crate) fn blocked(&self, cx: &App) -> bool {
        self.log.is_some()
            || self.viewer.is_some()
            || self.delete_pending.is_some()
            || self.menu == AccountMenu::ConfirmSignOut
            || self.profile
            || self.new_chat.is_some()
            || self.store.read(cx).session() != SessionState::Active
    }

    /// Moves keyboard focus out of the composer when an overlay opens and back when it closes.
    fn place_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let covered = self.blocked(cx);
        if covered == self.covered {
            return;
        }
        self.covered = covered;
        if !covered && self.open.is_some() {
            window.focus(&self.compose.input.focus_handle(cx));
        } else if let Some(dialog) = self.new_chat.as_ref() {
            window.focus(&dialog.search.focus_handle(cx));
        } else {
            window.focus(&self.focus);
        }
        cx.notify();
    }

    /// Opens the chat info panel for the open chat (it asks for the member list) or closes it. Opening is
    /// ignored while something covers the chat or no chat is open.
    pub(crate) fn toggle_info(&mut self, cx: &mut Context<Self>) {
        if self.info.is_some() {
            self.close_info(cx);
            return;
        }
        let Some(chat) = self.open.as_ref().map(|o| o.chat.clone()) else {
            return;
        };
        if self.blocked(cx) {
            return;
        }
        self.info = Some(chat.clone());
        self.store
            .update(cx, |store, cx| store.load_members(chat, cx));
        cx.notify();
    }

    pub(crate) fn close_info(&mut self, cx: &mut Context<Self>) {
        if self.info.take().is_some() {
            cx.notify();
        }
    }

    /// The retry of a failed member load; the store ignores it while a load is running.
    pub(crate) fn reload_info(&mut self, cx: &mut Context<Self>) {
        let Some(chat) = self.info_chat() else { return };
        self.store
            .update(cx, |store, cx| store.load_members(chat, cx));
    }

    /// The chat whose panel is open, if it is still the open chat.
    pub(crate) fn info_chat(&self) -> Option<ChatId> {
        self.info
            .clone()
            .filter(|chat| self.open.as_ref().is_some_and(|o| &o.chat == chat))
    }

    /// The panel belongs to one chat: it closes when another chat (or none) is open.
    fn sync_info(&mut self) {
        if self.info_chat().is_none() {
            self.info = None;
        }
    }

    #[cfg(test)]
    pub(crate) fn info_is_open(&self) -> bool {
        self.info.is_some()
    }

    #[cfg(test)]
    pub(crate) fn reactors_popover_is_open(&self) -> bool {
        self.reactors_popover.is_some()
    }

    /// Opens the list of the people who reacted to `(chat, timestamp)` as a popover at `position`
    /// (where the message menu was) and asks the store for its first page. Ignored for a chat that
    /// is not the open one and while something covers the chat. Closes the message menu.
    pub(crate) fn open_reactors(
        &mut self,
        chat: ChatId,
        timestamp: MessageTs,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if self.open.as_ref().map(|o| &o.chat) != Some(&chat) || self.blocked(cx) {
            return;
        }
        self.message_menu = None;
        self.reactors_popover = Some(ReactorsPopover {
            chat: chat.clone(),
            timestamp,
            position,
        });
        self.store
            .update(cx, |store, cx| store.open_reactors(chat, timestamp, cx));
        cx.notify();
    }

    /// Closes the popover and drops the store's list; an answer still on the way is dropped there.
    pub(crate) fn close_reactors(&mut self, cx: &mut Context<Self>) {
        if self.reactors_popover.take().is_some() {
            self.store.update(cx, |store, cx| store.close_reactors(cx));
            cx.notify();
        }
    }

    /// The popover shows the store's open list of the open chat and nothing else: it closes when the
    /// list is gone (sign-out), is another one, the chat changed, or something covers the chat.
    fn sync_reactors(&mut self, cx: &mut Context<Self>) {
        let Some(popover) = self.reactors_popover.as_ref() else {
            return;
        };
        let live = self
            .store
            .read(cx)
            .reactors()
            .is_some_and(|v| v.chat == popover.chat && v.timestamp == popover.timestamp)
            && self.open.as_ref().map(|o| &o.chat) == Some(&popover.chat)
            && !self.blocked(cx);
        if !live {
            self.close_reactors(cx);
        }
    }

    /// The list surface at the popover's anchor; a click outside it closes it. «Показать ещё» and
    /// every «Повторить» go to the store, which decides what (if anything) to send.
    fn reactors_view(&self, cx: &mut Context<Self>) -> Option<Div> {
        let popover = self.reactors_popover.as_ref()?;
        let close = cx.listener(|this, _: &MouseDownEvent, _, cx| this.close_reactors(cx));
        let store = self.store.read(cx);
        let view = store
            .reactors()
            .filter(|v| v.chat == popover.chat && v.timestamp == popover.timestamp)?;
        let (more, retry) = (self.store.clone(), self.store.clone());
        let list = reactors::render(
            view,
            store,
            self.clock,
            move |cx| more.update(cx, |store, cx| store.load_more_reactors(cx)),
            move |cx| retry.update(cx, |store, cx| store.retry_reactors(cx)),
        );
        Some(
            div()
                .absolute()
                .size_full()
                .occlude()
                .on_any_mouse_down(close)
                .child(
                    anchored()
                        .position(popover.position)
                        .snap_to_window_with_margin(px(8.))
                        .child(
                            div()
                                .debug_selector(|| "reactors-popover".into())
                                .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
                                .child(list),
                        ),
                ),
        )
    }

    /// Opens the profile modal from the account menu; it always asks for the profile again, so nothing
    /// from an earlier open is shown as current. Ignored while something else covers the chat.
    pub(crate) fn open_profile(&mut self, cx: &mut Context<Self>) {
        self.menu = AccountMenu::Closed;
        if self.blocked(cx) {
            cx.notify();
            return;
        }
        // The message menu is drawn above the modal; it must not outlive the cover's start.
        self.message_menu = None;
        self.close_reactors(cx);
        self.profile = true;
        self.store.update(cx, |store, cx| store.load_profile(cx));
        cx.notify();
    }

    pub(crate) fn close_profile(&mut self, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.profile) {
            cx.notify();
        }
    }

    /// The retry of a failed profile load; the store ignores it while a load is running.
    pub(crate) fn reload_profile(&mut self, cx: &mut Context<Self>) {
        if self.profile {
            self.store.update(cx, |store, cx| store.load_profile(cx));
        }
    }

    /// The modal has nothing to show once the session is no longer active (the session dialogs take over).
    fn sync_profile(&mut self, cx: &App) {
        if self.store.read(cx).session() != SessionState::Active {
            self.profile = false;
        }
    }

    /// "Новый чат" is offered only while the client is online.
    fn new_chat_available(&self, cx: &App) -> bool {
        self.store.read(cx).connection() == ConnectionState::Online
    }

    /// Opens the new chat dialog, with `user` already chosen when it comes from "Написать". A person who
    /// already has a private chat with the account gets that chat opened instead. Ignored while something
    /// else covers the chat. The dialog needs the own guid to leave the current user out, so it asks for
    /// the profile (a read) unless one is loaded.
    pub(crate) fn open_new_chat(&mut self, user: Option<UserId>, cx: &mut Context<Self>) {
        self.menu = AccountMenu::Closed;
        if self.blocked(cx) || self.new_chat.is_some() {
            cx.notify();
            return;
        }
        if let Some(chat) = user
            .as_ref()
            .and_then(|u| new_chat::existing_chat(self.store.read(cx), u))
        {
            self.select(chat, cx);
            return;
        }
        // The message menu is drawn above the dialog; it must not outlive the cover's start.
        self.message_menu = None;
        self.close_reactors(cx);
        self.store.update(cx, |store, cx| {
            store.dismiss_creation(cx);
            if !matches!(store.profile_state(), Some(ProfileState::Ready(_))) {
                store.load_profile(cx);
            }
        });
        let mut dialog = NewChat::new(cx);
        dialog.chosen = user.and_then(|user| {
            new_chat::candidates(self.store.read(cx), self.info_chat().as_ref())
                .into_iter()
                .find(|c| c.user == user)
        });
        self.new_chat = Some(dialog);
        cx.notify();
    }

    fn creation_in_flight(&self, cx: &App) -> bool {
        self.store.read(cx).creation_state() == Some(&CreationState::InFlight)
    }

    /// "Отмена" and Escape. A creation in flight cannot be recalled, so the dialog stays until its
    /// outcome is shown.
    pub(crate) fn close_new_chat(&mut self, cx: &mut Context<Self>) {
        if self.new_chat.is_none() || self.creation_in_flight(cx) {
            return;
        }
        self.new_chat = None;
        self.store
            .update(cx, |store, cx| store.dismiss_creation(cx));
        cx.notify();
    }

    /// Closes the dialog and opens the chat that already exists.
    fn open_existing_chat(&mut self, chat: ChatId, cx: &mut Context<Self>) {
        self.new_chat = None;
        self.store
            .update(cx, |store, cx| store.dismiss_creation(cx));
        self.select(chat, cx);
        cx.notify();
    }

    /// The user picked a person. In a private chat someone who has a private chat with the account opens
    /// that chat, anyone else becomes the chosen participant (a failure from the earlier choice is
    /// forgotten). In a group the person is added to the participants or, when already in, taken out; no
    /// chat is opened.
    pub(crate) fn choose_person(&mut self, user: UserId, cx: &mut Context<Self>) {
        if self.new_chat.is_none() || self.creation_in_flight(cx) {
            return;
        }
        let group = self.new_chat_is_group();
        if !group && let Some(chat) = new_chat::existing_chat(self.store.read(cx), &user) {
            self.open_existing_chat(chat, cx);
            return;
        }
        let people = new_chat::candidates(self.store.read(cx), self.info_chat().as_ref());
        let Some(person) = people.into_iter().find(|c| c.user == user) else {
            return;
        };
        if let Some(dialog) = self.new_chat.as_mut() {
            if group {
                dialog.toggle_member(person);
            } else {
                dialog.chosen = Some(person);
            }
        }
        self.store
            .update(cx, |store, cx| store.dismiss_creation(cx));
        cx.notify();
    }

    /// "✕" on a participant of the group. Locked while a creation is in flight, like every choice.
    pub(crate) fn remove_member(&mut self, user: UserId, cx: &mut Context<Self>) {
        if self.creation_in_flight(cx) {
            return;
        }
        let Some(dialog) = self.new_chat.as_mut().filter(|d| d.mode == Mode::Group) else {
            return;
        };
        dialog.members.retain(|m| m.user != user);
        self.store
            .update(cx, |store, cx| store.dismiss_creation(cx));
        cx.notify();
    }

    /// Switches between the private and the group variant, keeping what was chosen in each. Locked while a
    /// creation is in flight. The field of the variant gets the focus.
    pub(crate) fn set_new_chat_mode(
        &mut self,
        mode: Mode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.creation_in_flight(cx) {
            return;
        }
        let Some(dialog) = self.new_chat.as_mut().filter(|d| d.mode != mode) else {
            return;
        };
        dialog.mode = mode;
        let field = match mode {
            Mode::Private => &dialog.search,
            Mode::Group => &dialog.title,
        };
        window.focus(&field.focus_handle(cx));
        self.store
            .update(cx, |store, cx| store.dismiss_creation(cx));
        cx.notify();
    }

    /// "Создать": the only call that creates a chat. A private chat with the person that appeared meanwhile
    /// is opened instead; a group is always created, with the trimmed title and the participants in the order
    /// they were picked. While a creation is in flight nothing is sent again.
    pub(crate) fn confirm_new_chat(&mut self, cx: &mut Context<Self>) {
        if self.creation_in_flight(cx) {
            return;
        }
        let Some(dialog) = self.new_chat.as_ref() else {
            return;
        };
        if dialog.mode == Mode::Group {
            let title = dialog.title.read(cx).content.to_string();
            if !new_chat::title_ok(&title) || dialog.members.is_empty() {
                return;
            }
            let users: Vec<UserId> = dialog.members.iter().map(|m| m.user.clone()).collect();
            self.store.update(cx, |store, cx| {
                store.create_group_chat(title.trim().to_owned(), users, cx)
            });
            return;
        }
        let Some(user) = dialog.chosen.as_ref().map(|c| c.user.clone()) else {
            return;
        };
        if let Some(chat) = new_chat::existing_chat(self.store.read(cx), &user) {
            self.open_existing_chat(chat, cx);
            return;
        }
        self.store
            .update(cx, |store, cx| store.create_private_chat(user, cx));
    }

    /// A creation ended in a chat (the store selects it): the dialog is done. A chat the answer and the
    /// refreshed list do not name cannot be opened; the user is told it will show up in the list.
    fn new_chat_created(&mut self, unopened: bool, cx: &mut Context<Self>) {
        if self.new_chat.take().is_none() {
            return;
        }
        if unopened {
            self.show_toast("Чат создан, он появится в списке".into(), cx);
        }
        cx.notify();
    }

    /// The dialog has nothing to show once the session is no longer active (the session dialogs take over).
    fn sync_new_chat(&mut self, cx: &App) {
        if self.store.read(cx).session() != SessionState::Active {
            self.new_chat = None;
        }
    }

    /// The person chosen in the open private dialog.
    #[cfg(test)]
    pub(crate) fn new_chat_choice(&self) -> Option<&new_chat::Candidate> {
        self.new_chat.as_ref()?.chosen.as_ref()
    }

    /// Whether the open dialog is the group variant.
    pub(crate) fn new_chat_is_group(&self) -> bool {
        self.new_chat
            .as_ref()
            .is_some_and(|d| d.mode == Mode::Group)
    }

    /// Whether `user` is selected in the open dialog's current variant.
    pub(crate) fn new_chat_selected(&self, user: &UserId) -> bool {
        self.new_chat.as_ref().is_some_and(|d| d.is_selected(user))
    }

    #[cfg(test)]
    pub(crate) fn new_chat_members(&self) -> Vec<String> {
        self.new_chat
            .as_ref()
            .map(|d| d.members.iter().map(|m| m.user.0.clone()).collect())
            .unwrap_or_default()
    }

    #[cfg(test)]
    pub(crate) fn new_chat_title(&self, cx: &App) -> Option<String> {
        Some(self.new_chat.as_ref()?.title.read(cx).content.to_string())
    }

    #[cfg(test)]
    pub(crate) fn new_chat_set_title(&self, text: &str, cx: &mut App) {
        if let Some(dialog) = &self.new_chat {
            dialog.title.update(cx, |t, cx| t.set_text(text, cx));
        }
    }

    #[cfg(test)]
    pub(crate) fn new_chat_is_open(&self) -> bool {
        self.new_chat.is_some()
    }

    #[cfg(test)]
    pub(crate) fn new_chat_query(&self, cx: &App) -> Option<String> {
        Some(self.new_chat.as_ref()?.search.read(cx).content.to_string())
    }

    #[cfg(test)]
    pub(crate) fn profile_is_open(&self) -> bool {
        self.profile
    }

    pub(crate) fn select(&mut self, chat: ChatId, cx: &mut Context<Self>) {
        self.store.update(cx, |store, cx| store.select(chat, cx));
    }

    /// Reacts to a change of the search field: a short or empty query cancels the server search at once,
    /// a longer one is sent after `SEARCH_DEBOUNCE` without further changes.
    fn search_changed(&mut self, cx: &mut Context<Self>) {
        let content = self.search.read(cx).content.clone();
        if content == self.search_input.seen {
            return;
        }
        self.search_input.seen = content;
        self.search_input.generation += 1;
        let generation = self.search_input.generation;
        let Some(query) = search_query(&self.search_input.seen) else {
            self.search_input.settled = generation;
            self.search_input.timer = None;
            self.store.update(cx, |store, cx| store.cancel_search(cx));
            return;
        };
        // A reply still on its way answers the old text: it must not show up before the new request is sent.
        self.store
            .update(cx, |store, _| store.supersede_search(query));
        self.search_input.timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            this.update(cx, |this, cx| {
                if this.search_input.generation == generation {
                    this.search_input.settled = generation;
                    this.run_search(cx);
                }
            })
            .ok();
        }));
    }

    /// Sends the current field text to the server unless it is too short, the connection is down or that
    /// query is already answered or in flight. While offline nothing is sent; any search state is dropped.
    fn run_search(&mut self, cx: &mut Context<Self>) {
        let content = self.search_input.seen.clone();
        let Some(query) = search_query(&content) else {
            return;
        };
        let store = self.store.read(cx);
        if store.connection() == ConnectionState::Offline {
            self.store.update(cx, |store, cx| store.cancel_search(cx));
            return;
        }
        let current = store.search_state().is_some_and(|state| {
            state.query == query && !matches!(state.status, SearchStatus::Failed(_))
        });
        if !current {
            self.store
                .update(cx, |store, cx| store.search(query.to_string(), cx));
        }
    }

    /// The connection came back: an open panel or profile whose data was withheld as offline (or whose
    /// load was refused offline) asks again, so nothing from before the drop is shown as current.
    fn resume_reads(&mut self, cx: &mut Context<Self>) {
        let store = self.store.read(cx);
        if store.connection() != ConnectionState::Online {
            return;
        }
        let members = self.info_chat().filter(|chat| {
            matches!(
                store.members_state(chat),
                Some(MembersState::Ready(_) | MembersState::Failed { offline: true, .. })
            )
        });
        let profile = self.profile
            && matches!(
                store.profile_state(),
                Some(ProfileState::Ready(_) | ProfileState::Failed { offline: true, .. })
            );
        if let Some(chat) = members {
            self.store
                .update(cx, |store, cx| store.load_members(chat, cx));
        }
        if profile {
            self.store.update(cx, |store, cx| store.load_profile(cx));
        }
    }

    /// The connection changed: a query that was refused or failed (typically for lack of a connection) is
    /// sent again, unless a debounce timer is still about to send the field text.
    fn resume_search(&mut self, cx: &mut Context<Self>) {
        if self.search_input.settled != self.search_input.generation {
            return;
        }
        let store = self.store.read(cx);
        if store
            .search_state()
            .is_none_or(|state| matches!(state.status, SearchStatus::Failed(_)))
        {
            self.run_search(cx);
        }
    }

    /// What lists below the local rows: the offline notice while offline, else the stored search state.
    /// `None` when there is nothing to show (short query, no search yet, nothing to say).
    fn search_view(&self, local: &[ChatId], cx: &App) -> Option<SearchView> {
        search_query(&self.search.read(cx).content)?;
        let store = self.store.read(cx);
        let view = if store.connection() == ConnectionState::Offline {
            SearchView {
                status: Some(StatusLine::Offline),
                sections: Vec::new(),
            }
        } else {
            search_results::view(store.search_state()?, local, self.clock)
        };
        (view.status.is_some() || !view.sections.is_empty()).then_some(view)
    }

    /// What the sidebar lists below the local rows right now.
    #[cfg(test)]
    pub(crate) fn search_listing(&self, cx: &App) -> Option<SearchView> {
        let query = self.search.read(cx).content.to_string();
        let local: Vec<ChatId> = self
            .store
            .read(cx)
            .visible_chats(&query, self.filter)
            .into_iter()
            .map(|c| c.id.clone())
            .collect();
        self.search_view(&local, cx)
    }

    /// Opens the viewer on picture `index` of the message `ts` in the open chat `chat`. Ignored while
    /// something else covers the chat, when the message is not loaded or has no such picture.
    pub(crate) fn open_viewer(
        &mut self,
        chat: ChatId,
        ts: MessageTs,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        let in_open_chat = self.open.as_ref().is_some_and(|o| o.chat == chat);
        if !in_open_chat || self.blocked(cx) || index >= self.gallery_of(&chat, ts, cx).len() {
            return;
        }
        self.message_menu = None;
        self.close_reactors(cx);
        self.viewer = Some(Viewer { chat, ts, index });
        cx.notify();
    }

    pub(crate) fn close_viewer(&mut self, cx: &mut Context<Self>) {
        if self.viewer.take().is_some() {
            cx.notify();
        }
    }

    /// Moves to the previous (negative) or next picture; the ends do not wrap.
    pub(crate) fn step_viewer(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some((chat, ts, index)) = self
            .viewer
            .as_ref()
            .map(|v| (v.chat.clone(), v.ts, v.index))
        else {
            return;
        };
        let next = viewer::step(index, self.gallery_of(&chat, ts, cx).len(), delta);
        if next != index {
            if let Some(viewer) = self.viewer.as_mut() {
                viewer.index = next;
            }
            cx.notify();
        }
    }

    /// The pictures of the loaded message `ts`; empty when the message is gone or holds none.
    fn gallery_of(&self, chat: &ChatId, ts: MessageTs, cx: &App) -> Vec<GalleryItem> {
        let Some(timeline) = self.store.read(cx).timeline(chat) else {
            return Vec::new();
        };
        match timeline.messages.binary_search_by_key(&ts, |m| m.timestamp) {
            Ok(ix) => timeline.messages[ix].body.gallery_items(),
            Err(_) => Vec::new(),
        }
    }

    /// Keeps the viewer on something that still exists: it closes when its message was deleted or lost
    /// its pictures, or the chat changed, and steps back when the message has fewer pictures than before.
    fn sync_viewer(&mut self, cx: &mut Context<Self>) {
        let Some(viewer) = self.viewer.as_ref() else {
            return;
        };
        let (chat, ts, index) = (viewer.chat.clone(), viewer.ts, viewer.index);
        let len = self.gallery_of(&chat, ts, cx).len();
        let in_open_chat = self.open.as_ref().is_some_and(|o| o.chat == chat);
        if len == 0 || !in_open_chat {
            self.close_viewer(cx);
        } else if let Some(viewer) = self.viewer.as_mut() {
            viewer.index = index.min(len - 1);
        }
    }

    fn viewer_view(&self, window: &Window, cx: &mut Context<Self>) -> Option<Div> {
        let viewer = self.viewer.as_ref()?;
        let items = self.gallery_of(&viewer.chat, viewer.ts, cx);
        let size = window.viewport_size();
        let frame = viewer::frame(
            viewer,
            &items,
            self.store.read(cx),
            (f32::from(size.width), f32::from(size.height)),
        )?;
        Some(viewer::render(frame, cx))
    }

    #[cfg(test)]
    pub(crate) fn viewer_at(&self) -> Option<(MessageTs, usize)> {
        self.viewer.as_ref().map(|v| (v.ts, v.index))
    }

    pub(crate) fn confirm_sign_out(&mut self, cx: &mut Context<Self>) {
        self.menu = AccountMenu::Closed;
        self.store.update(cx, |store, _| store.sign_out());
        cx.notify();
    }

    pub(crate) fn toggle_log(&mut self, cx: &mut Context<Self>) {
        self.log = match self.log.take() {
            Some(_) => None,
            None => {
                // The message menu is drawn above the log; it must not outlive the cover's start.
                self.message_menu = None;
                self.close_reactors(cx);
                let poll = cx.spawn(async move |this, cx| {
                    loop {
                        cx.background_executor().timer(LOG_POLL).await;
                        if this.update(cx, |this, cx| this.refresh_log(cx)).is_err() {
                            break;
                        }
                    }
                });
                let mut view = LogView {
                    lines: Vec::new(),
                    generation: u64::MAX,
                    scroll: UniformListScrollHandle::new(),
                    _poll: poll,
                };
                Self::load_log(&mut view, cx);
                Some(view)
            }
        };
        cx.notify();
    }

    fn refresh_log(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.log.as_mut() else {
            return;
        };
        if Self::load_log(view, cx) {
            cx.notify();
        }
    }

    /// Takes a new snapshot when records were added and keeps the newest in view.
    fn load_log(view: &mut LogView, cx: &mut Context<Self>) -> bool {
        let buffer = cx.default_global::<LogBuffer>().clone();
        let generation = buffer.generation();
        if generation == view.generation {
            return false;
        }
        view.generation = generation;
        view.lines = buffer.snapshot();
        if let Some(last) = view.lines.len().checked_sub(1) {
            view.scroll.scroll_to_item(last, ScrollStrategy::Bottom);
        }
        true
    }

    pub(crate) fn log_text(&self) -> Option<String> {
        let view = self.log.as_ref()?;
        Some(
            view.lines
                .iter()
                .map(|line| log_line(line, self.clock) + "\n")
                .collect(),
        )
    }

    #[cfg(test)]
    pub(crate) fn list_state(&self) -> Option<&ListState> {
        self.open.as_ref().map(|o| &o.list)
    }

    /// Keeps the list element's item count in step with the store's timeline.
    fn apply_timeline_change(
        &mut self,
        chat: &ChatId,
        change: &TimelineChange,
        cx: &mut Context<Self>,
    ) {
        let Some(open) = self.open.as_mut().filter(|o| &o.chat == chat) else {
            return;
        };
        let len = self
            .store
            .read(cx)
            .timeline(chat)
            .map_or(0, |t| t.messages.len());
        // A message the user just sent stays in view even when they had scrolled up.
        let follow = matches!(change, TimelineChange::Delta { appended, .. } if *appended > 0)
            && self
                .store
                .read(cx)
                .timeline(chat)
                .and_then(|t| t.messages.last())
                .is_some_and(|m| matches!(m.delivery, Delivery::Pending(_)));
        let list = &open.list;
        match change {
            TimelineChange::Reset => list.reset(len),
            TimelineChange::Status => {}
            TimelineChange::Removed { at } => {
                list.splice(*at..*at + 1, 0);
                // The neighbours of the gap may gain or lose their author line or date separator.
                if *at > 0 {
                    list.splice(*at - 1..*at, 1);
                }
                if *at < list.item_count() {
                    list.splice(*at..*at + 1, 1);
                }
            }
            TimelineChange::Delta {
                prepended,
                appended,
                touched,
            } => {
                let before = list.item_count();
                let anchor = list.logical_scroll_top();
                if *prepended > 0 {
                    list.splice(0..0, *prepended);
                    if before > 0 {
                        // The old first message may lose its author line or date separator.
                        list.splice(*prepended..*prepended + 1, 1);
                    }
                }
                for &i in touched {
                    list.splice(i..i + 1, 1);
                }
                if *appended > 0 {
                    let end = list.item_count();
                    list.splice(end..end, *appended);
                    if before > 0 {
                        // The old last message may now continue a group.
                        let last = before + prepended - 1;
                        list.splice(last..last + 1, 1);
                    }
                }
                if *prepended > 0 {
                    list.scroll_to(ListOffset {
                        item_ix: anchor.item_ix + prepended,
                        offset_in_item: anchor.offset_in_item,
                    });
                }
            }
        }
        if list.item_count() != len {
            list.reset(len);
        }
        if follow {
            list.scroll_to(ListOffset {
                item_ix: list.item_count(),
                offset_in_item: px(0.),
            });
        }
        cx.notify();
    }

    /// Click on the pinned-message bar: scrolls the open chat to the pinned message when it is
    /// loaded, and does nothing otherwise (no request, no loading state, no error).
    pub(crate) fn scroll_to_pinned(&mut self, cx: &mut Context<Self>) {
        let Some(open) = self.open.as_ref() else {
            return;
        };
        let store = self.store.read(cx);
        let Some(timestamp) = store
            .chat(&open.chat)
            .and_then(|c| c.pinned_message.as_ref())
            .map(|p| p.timestamp)
        else {
            return;
        };
        let Some(ix) = store
            .timeline(&open.chat)
            .and_then(|t| t.messages.iter().position(|m| m.timestamp == timestamp))
        else {
            return;
        };
        open.list.scroll_to(ListOffset {
            item_ix: ix,
            offset_in_item: px(0.),
        });
        cx.notify();
    }

    fn sidebar(&mut self, cx: &mut Context<Self>) -> Div {
        let query = self.search.read(cx).content.to_string();
        let store = self.store.read(cx);
        let rows: Vec<ChatId> = store
            .visible_chats(&query, self.filter)
            .into_iter()
            .map(|c| c.id.clone())
            .collect();
        let empty = if !store.chats_loaded() {
            Some("Загрузка чатов…")
        } else if store.chat_count() == 0 {
            Some("Чатов пока нет")
        } else if rows.is_empty() {
            Some("Ничего не найдено")
        } else {
            None
        };
        let connection = store.connection();
        let count = rows.len();
        let results = self
            .search_view(&rows, cx)
            .map(|view| search_results::render(&view, &self.store, cx));

        div()
            .relative()
            .w(px(SIDEBAR_WIDTH))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(PANEL))
            .border_r_1()
            .border_color(rgb(DIVIDER))
            .when_some(connection_text(connection), |d, text| {
                d.child(
                    div()
                        .debug_selector(|| "connection-indicator".into())
                        .flex_none()
                        .px(px(16.))
                        .py(px(5.))
                        .bg(rgb(WARNING))
                        .text_size(px(13.))
                        .child(text),
                )
            })
            .child(
                div()
                    .flex_none()
                    .p(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .id("menu-button")
                            .debug_selector(|| "menu-button".into())
                            .size(px(40.))
                            .flex_none()
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(20.))
                            .text_color(rgb(MUTED))
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(ROW_HOVER)))
                            .child("≡")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.menu = if this.menu == AccountMenu::Closed {
                                    AccountMenu::Open
                                } else {
                                    AccountMenu::Closed
                                };
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .debug_selector(|| "search".into())
                            .flex_1()
                            .min_w_0()
                            .px(px(12.))
                            .py(px(7.))
                            .rounded(px(20.))
                            .bg(rgb(FIELD))
                            .key_context("Search")
                            .child(self.search.clone()),
                    )
                    .child({
                        let online = connection == ConnectionState::Online;
                        div()
                            .id("new-chat-button")
                            .debug_selector(|| "new-chat-button".into())
                            .flex_none()
                            .px(px(10.))
                            .py(px(7.))
                            .rounded(px(20.))
                            .text_size(px(13.))
                            .when(online, |d| {
                                d.cursor_pointer()
                                    .text_color(rgb(ACCENT))
                                    .hover(|s| s.bg(rgb(ROW_HOVER)))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if this.new_chat_available(cx) {
                                            this.open_new_chat(None, cx);
                                        }
                                    }))
                            })
                            .when(!online, |d| d.text_color(rgb(MUTED)))
                            .child("Новый чат")
                    })
                    .child(
                        div()
                            .id("log-button")
                            .debug_selector(|| "log-button".into())
                            .size(px(40.))
                            .flex_none()
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(16.))
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(ROW_HOVER)))
                            .child("🐞")
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_log(cx))),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .px(px(8.))
                    .pb(px(6.))
                    .flex()
                    .flex_wrap()
                    .gap(px(4.))
                    .children(FILTERS.into_iter().map(|(id, label, filter)| {
                        let active = self.filter == filter;
                        div()
                            .id(id)
                            .debug_selector(move || id.to_string())
                            .px(px(10.))
                            .py(px(4.))
                            .rounded_full()
                            .text_size(px(13.))
                            .cursor_pointer()
                            .when(active, |d| d.bg(rgb(ROW_SELECTED)).text_color(rgb(INK)))
                            .when(!active, |d| {
                                d.text_color(rgb(MUTED)).hover(|s| s.bg(rgb(ROW_HOVER)))
                            })
                            .child(label)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.filter = filter;
                                cx.notify();
                            }))
                    })),
            )
            .child(match (results, empty) {
                // Server results below the local rows: one scrolling column, the rows themselves unchanged.
                (Some(results), empty) => {
                    let local = match empty {
                        Some(text) => div()
                            .debug_selector(|| "chat-list-empty".into())
                            .flex()
                            .justify_center()
                            .py(px(12.))
                            .text_color(rgb(MUTED))
                            .child(text),
                        None => div()
                            .debug_selector(|| "chat-list".into())
                            .flex()
                            .flex_col()
                            .children(rows.iter().map(|id| self.chat_row(id, cx))),
                    };
                    div()
                        .id("search-scroll")
                        .debug_selector(|| "search-scroll".into())
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .child(local)
                        .child(div().border_t_1().border_color(rgb(DIVIDER)).child(results))
                        .into_any_element()
                }
                (None, Some(text)) => div()
                    .debug_selector(|| "chat-list-empty".into())
                    .flex_1()
                    .flex()
                    .justify_center()
                    .pt(px(40.))
                    .text_color(rgb(MUTED))
                    .child(text)
                    .into_any_element(),
                (None, None) => div()
                    .debug_selector(|| "chat-list".into())
                    .flex_1()
                    .min_h_0()
                    .child(
                        uniform_list(
                            "chat-list",
                            count,
                            cx.processor(move |this, range: Range<usize>, _, cx| {
                                range
                                    .filter_map(|ix| rows.get(ix))
                                    .map(|id| this.chat_row(id, cx))
                                    .collect::<Vec<_>>()
                            }),
                        )
                        .size_full(),
                    )
                    .into_any_element(),
            })
            .when(self.menu == AccountMenu::Open, |d| {
                d.child(
                    div()
                        .absolute()
                        .top(px(52.))
                        .left(px(8.))
                        .w(px(220.))
                        .occlude()
                        .p(px(6.))
                        .rounded(px(10.))
                        .bg(rgb(FIELD))
                        .child(
                            button("menu-profile", "Профиль")
                                .debug_selector(|| "menu-profile".into())
                                .on_click(cx.listener(|this, _, _, cx| this.open_profile(cx))),
                        )
                        .child(
                            button("menu-sign-out", "Выйти из аккаунта")
                                .debug_selector(|| "menu-sign-out".into())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.menu = AccountMenu::ConfirmSignOut;
                                    cx.notify();
                                })),
                        ),
                )
            })
    }

    fn chat_row(&self, id: &ChatId, cx: &mut Context<Self>) -> AnyElement {
        let store = self.store.read(cx);
        let Some(chat) = store.chat(id) else {
            return div().into_any_element();
        };
        let selected = store.selected() == Some(id);
        let tag = format!("chat-{}", id.0);
        let preview = chat_preview(store, chat);
        let time = chat
            .last_message
            .as_ref()
            .map(|m| chat_time(m.timestamp, self.clock));
        let (unread, muted, pinned) = (chat.unread, chat.muted, chat.pinned_rank.is_some());
        let click_id = id.clone();
        let row_tag = tag.clone();

        div()
            .id(SharedString::from(tag.clone()))
            .debug_selector(move || format!("{row_tag}-row"))
            .h(px(ROW_HEIGHT))
            .w_full()
            .px(px(10.))
            .flex()
            .items_center()
            .gap(px(12.))
            .cursor_pointer()
            .bg(rgb(if selected { ROW_SELECTED } else { PANEL }))
            .hover(|s| s.bg(rgb(if selected { ROW_SELECTED } else { ROW_HOVER })))
            .on_click(cx.listener(move |this, _, _, cx| this.select(click_id.clone(), cx)))
            .child(avatar(
                store,
                chat.avatar.as_ref(),
                &id.0,
                &chat.title,
                48.,
                format!("{tag}-avatar"),
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(15.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(chat.title.clone()),
                            )
                            .when(muted, |d| marker(d, format!("{tag}-muted"), "🔇"))
                            .when_some(time, |d, time| {
                                d.child(
                                    div()
                                        .flex_none()
                                        .text_size(px(12.))
                                        .text_color(rgb(MUTED))
                                        .child(time),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(preview_view(preview, &tag))
                            .when(pinned, |d| marker(d, format!("{tag}-pinned"), "📌"))
                            .when(unread > 0, |d| {
                                d.child(
                                    div()
                                        .debug_selector(move || {
                                            format!(
                                                "{tag}-badge{}",
                                                if muted { "-muted" } else { "" }
                                            )
                                        })
                                        .flex_none()
                                        .min_w(px(20.))
                                        .h(px(20.))
                                        .px(px(6.))
                                        .rounded_full()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .text_size(px(12.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .bg(rgb(if muted { BADGE_MUTED } else { BADGE }))
                                        .child(unread.to_string()),
                                )
                            }),
                    ),
            )
            .into_any_element()
    }

    fn chat_pane(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let store = self.store.read(cx);
        let Some(selected) = store.selected().cloned() else {
            return div()
                .debug_selector(|| "no-chat".into())
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(MUTED))
                .child("Выберите чат, чтобы начать чтение")
                .into_any_element();
        };
        let Some((list, rows)) = self
            .open
            .as_ref()
            .filter(|o| o.chat == selected)
            .map(|o| (o.list.clone(), o.rows.clone()))
        else {
            return div().flex_1().into_any_element();
        };
        let summary: Option<&ChatSummary> = store.chat(&selected);
        let title = summary.map_or_else(|| "Чат".to_string(), |c| c.title.clone());
        let subtitle = summary.map_or("", |c| match c.kind {
            ChatKind::Private => "Личный чат",
            ChatKind::Group => "Группа",
            ChatKind::Channel => "Канал",
        });
        let typing = store.typing_text(&selected);
        let header_avatar = avatar(
            store,
            summary.and_then(|c| c.avatar.as_ref()),
            &selected.0,
            &title,
            40.,
            "header-avatar".into(),
        );
        let timeline = store.timeline(&selected);
        let loaded = timeline.is_some_and(|t| t.loaded);
        let empty = timeline.is_some_and(|t| t.loaded && t.messages.is_empty());
        let loading_older = timeline.is_some_and(|t| t.loading_older);

        // Author only outside private chats; no preview yet leaves the label alone.
        let pinned_bar = summary.and_then(|c| {
            let pinned = c.pinned_message.as_ref()?;
            let line = pinned_message_text(c.kind, pinned.preview.as_ref());
            Some(
                div()
                    .id("pinned-message")
                    .debug_selector(|| "pinned-message".into())
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| this.scroll_to_pinned(cx)))
                    .flex_none()
                    .px(px(16.))
                    .py(px(6.))
                    .flex()
                    .flex_col()
                    .bg(rgb(PANEL))
                    .border_b_1()
                    .border_color(rgb(DIVIDER))
                    .hover(|d| d.bg(rgb(ROW_HOVER)))
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(rgb(ACCENT))
                            .child("Закреплённое сообщение"),
                    )
                    .when_some(line, |d, text| {
                        d.child(
                            div()
                                .debug_selector(|| "pinned-message-text".into())
                                .min_w_0()
                                .truncate()
                                .text_size(px(13.))
                                .text_color(rgb(MUTED))
                                .child(text),
                        )
                    }),
            )
        });

        let content = if !loaded {
            centered("timeline-loading-initial", "Загрузка сообщений…")
        } else if empty {
            centered("timeline-empty", "Сообщений пока нет")
        } else {
            let ctx = RowContext {
                store: self.store.clone(),
                desktop: cx.weak_entity(),
                chat: selected.clone(),
                clock: self.clock,
                rows,
            };
            let probe_list = list.clone();
            let (probe_store, probe_chat, probe_desktop) =
                (self.store.clone(), selected.clone(), cx.weak_entity());
            div()
                .debug_selector(|| "timeline".into())
                .relative()
                .size_full()
                .child(
                    gpui::list(list, move |ix, _, cx| timeline::message_row(&ctx, ix, cx))
                        .size_full(),
                )
                // Runs after the list is laid out: asks for older history when the top is in view.
                .child(
                    canvas(
                        move |_, window, cx| {
                            if at_top(&probe_list) {
                                let (store, chat) = (probe_store.clone(), probe_chat.clone());
                                window.defer(cx, move |_, cx| {
                                    store.update(cx, |s, cx| s.load_older(&chat, cx))
                                });
                            }
                            let desktop = probe_desktop.clone();
                            window.defer(cx, move |window, cx| {
                                desktop
                                    .update(cx, |this, cx| this.check_read(window, cx))
                                    .ok();
                            });
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .size_0(),
                )
                .when(loading_older, |d| {
                    d.child(
                        div()
                            .absolute()
                            .top(px(8.))
                            .w_full()
                            .flex()
                            .justify_center()
                            .child(
                                div()
                                    .debug_selector(|| "timeline-loading".into())
                                    .px(px(12.))
                                    .py(px(3.))
                                    .rounded_full()
                                    .bg(rgb(PANEL))
                                    .text_size(px(13.))
                                    .text_color(rgb(MUTED))
                                    .child("Загрузка…"),
                            ),
                    )
                })
                .into_any_element()
        };

        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("chat-header")
                    .debug_selector(|| "chat-header".into())
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_info(cx)))
                    .h(px(56.))
                    .flex_none()
                    .px(px(16.))
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .bg(rgb(PANEL))
                    .border_b_1()
                    .border_color(rgb(DIVIDER))
                    .child(header_avatar)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(div().font_weight(FontWeight::MEDIUM).child(title))
                            .child(match typing {
                                Some(text) => div()
                                    .debug_selector(|| "header-typing".into())
                                    .text_size(px(13.))
                                    .text_color(rgb(ACCENT))
                                    .child(text),
                                None => div()
                                    .text_size(px(13.))
                                    .text_color(rgb(MUTED))
                                    .child(subtitle),
                            }),
                    ),
            )
            .children(pinned_bar)
            .child(div().flex_1().min_h_0().child(content))
            .child(self.composer_view(cx))
            .into_any_element()
    }
}

/// True when the oldest loaded message is in view, or everything fits so no scrolling could reach it.
fn at_top(list: &ListState) -> bool {
    list.item_count() > 0
        && list.viewport_bounds().size.height > px(0.)
        && (list.logical_scroll_top().item_ix == 0
            || list.max_offset_for_scrollbar().height <= px(0.))
}

fn centered(selector: &'static str, text: &'static str) -> AnyElement {
    div()
        .debug_selector(move || selector.into())
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .text_color(rgb(MUTED))
        .child(text)
        .into_any_element()
}

fn marker(parent: Div, selector: String, glyph: &'static str) -> Div {
    parent.child(
        div()
            .debug_selector(move || selector)
            .flex_none()
            .text_size(px(12.))
            .child(glyph),
    )
}

fn connection_text(state: ConnectionState) -> Option<&'static str> {
    match state {
        ConnectionState::Online => None,
        ConnectionState::Connecting => Some("Подключение…"),
        ConnectionState::Offline => Some("Нет сети · переподключение…"),
    }
}

/// The line of the pinned-message bar under its label: the author only outside private chats, then the
/// text on one line. `None` while the text of the pinned message is not known: the label stays alone.
pub(crate) fn pinned_message_text(
    kind: ChatKind,
    preview: Option<&MessagePreview>,
) -> Option<String> {
    preview.map(|p| match &p.author {
        Some(author) if kind != ChatKind::Private => format!("{author}: {}", one_line(&p.text)),
        _ => one_line(&p.text),
    })
}

/// What a chat row shows under the title.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Preview {
    Typing(String),
    Draft(String),
    Message(String),
}

/// Typing outranks the draft, and the draft outranks the last message.
pub(crate) fn chat_preview(store: &ChatStore, chat: &ChatSummary) -> Preview {
    if let Some(text) = store.typing_text(&chat.id) {
        return Preview::Typing(text);
    }
    if let Some(draft) = store.draft(&chat.id) {
        let text = if draft.text.trim().is_empty() {
            draft.reply_to.as_ref().map_or("", |r| r.text.as_str())
        } else {
            &draft.text
        };
        return Preview::Draft(one_line(text));
    }
    Preview::Message(
        chat.last_message
            .as_ref()
            .map_or_else(String::new, |m| match &m.author {
                Some(author) => format!("{author}: {}", one_line(&m.text)),
                None => one_line(&m.text),
            }),
    )
}

fn preview_view(preview: Preview, tag: &str) -> Div {
    let line = div()
        .flex_1()
        .min_w_0()
        .text_size(px(14.))
        .text_color(rgb(MUTED));
    match preview {
        Preview::Typing(text) => {
            let selector = format!("{tag}-typing");
            line.debug_selector(move || selector)
                .truncate()
                .text_color(rgb(ACCENT))
                .child(text)
        }
        Preview::Draft(text) => {
            let selector = format!("{tag}-draft");
            line.debug_selector(move || selector)
                .flex()
                .gap(px(4.))
                .child(div().flex_none().text_color(rgb(DANGER)).child("Черновик:"))
                .child(div().flex_1().min_w_0().truncate().child(text))
        }
        Preview::Message(text) => line.truncate().child(text),
    }
}

impl Render for Desktop {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let session = self.store.read(cx).session();
        if self.blocked(cx) != self.covered {
            cx.defer_in(window, |this, window, cx| this.place_focus(window, cx));
        }
        let sidebar = self.sidebar(cx);
        let pane = self.chat_pane(cx);
        let info = self
            .info_chat()
            .map(|chat| info_panel::render(&chat, &self.store, cx));
        let profile = self.profile.then(|| profile::render(&self.store, cx));
        let new_chat = self
            .new_chat
            .as_ref()
            .map(|dialog| new_chat::render(dialog, self.info_chat(), &self.store, cx));
        let log = self.log.as_ref().map(|view| self.log_panel(view, cx));
        let toast = self.toast_view();
        let menu = self.message_menu_view(cx);
        let reactors = self.reactors_view(cx);
        let viewer = self.viewer_view(window, cx);
        let delete = self.delete_dialog(cx);

        div()
            .key_context("BlazingTelesram")
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .flex()
            .bg(rgb(BG))
            .text_color(rgb(INK))
            .text_size(px(14.))
            .on_action(cx.listener(|this, _: &Search, window, cx| {
                if !this.blocked(cx) {
                    window.focus(&this.search.focus_handle(cx));
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleLog, _, cx| this.toggle_log(cx)))
            .on_action(cx.listener(|this, _: &Dismiss, _, cx| this.dismiss(cx)))
            .on_action(cx.listener(|this, _: &ViewerPrev, _, cx| this.step_viewer(-1, cx)))
            .on_action(cx.listener(|this, _: &ViewerNext, _, cx| this.step_viewer(1, cx)))
            .child(sidebar)
            .child(pane)
            .children(info)
            .children(profile)
            .children(new_chat)
            .children(viewer)
            .children(log)
            .children(toast)
            .children(menu)
            .children(reactors)
            .children(delete)
            .when(
                self.menu == AccountMenu::ConfirmSignOut && session == SessionState::Active,
                |d| {
                    d.child(overlay(
                        "sign-out-dialog",
                        "Выйти из аккаунта?",
                        "Сессия, локальный кэш и загруженные файлы будут удалены.",
                        vec![
                            button("cancel-sign-out", "Отмена")
                                .debug_selector(|| "cancel-sign-out".into())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.menu = AccountMenu::Closed;
                                    cx.notify();
                                })),
                            button("confirm-sign-out", "Выйти")
                                .debug_selector(|| "confirm-sign-out".into())
                                .text_color(rgb(ACCENT))
                                .on_click(cx.listener(|this, _, _, cx| this.confirm_sign_out(cx))),
                        ],
                    ))
                },
            )
            .when(session == SessionState::Expired, |d| {
                d.child(overlay(
                    "session-expired",
                    "Сессия истекла",
                    "Выйдите и войдите снова, чтобы продолжить чтение.",
                    vec![
                        button("expired-sign-out", "Выйти")
                            .debug_selector(|| "expired-sign-out".into())
                            .on_click(cx.listener(|this, _, _, cx| this.confirm_sign_out(cx))),
                    ],
                ))
            })
            .when(session == SessionState::SignedOut, |d| {
                d.child(overlay(
                    "signed-out",
                    "Вы вышли из аккаунта",
                    "Данные сессии удалены.",
                    vec![],
                ))
            })
    }
}

impl Desktop {
    fn log_panel(&self, view: &LogView, cx: &mut Context<Self>) -> Div {
        let count = view.lines.len();
        div()
            .debug_selector(|| "log-view".into())
            .absolute()
            .size_full()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x000000aa))
            .child(
                div()
                    .w(relative(0.9))
                    .h(relative(0.85))
                    .p(px(16.))
                    .rounded(px(12.))
                    .bg(rgb(PANEL))
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(17.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child("Журнал"),
                            )
                            .child(
                                div()
                                    .text_color(rgb(MUTED))
                                    .text_size(px(12.))
                                    .child(format!("{count} записей")),
                            )
                            .child(
                                button("log-copy", "Копировать")
                                    .debug_selector(|| "log-copy".into())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if let Some(text) = this.log_text() {
                                            cx.write_to_clipboard(ClipboardItem::new_string(text));
                                        }
                                    })),
                            )
                            .child(
                                button("log-close", "Закрыть")
                                    .debug_selector(|| "log-close".into())
                                    .on_click(cx.listener(|this, _, _, cx| this.toggle_log(cx))),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .p(px(8.))
                            .rounded(px(8.))
                            .bg(rgb(BG))
                            .font_family("Menlo")
                            .text_size(px(12.))
                            .child(
                                uniform_list(
                                    "log-lines",
                                    count,
                                    cx.processor(|this, range: Range<usize>, _, _| {
                                        let Some(view) = this.log.as_ref() else {
                                            return Vec::new();
                                        };
                                        range
                                            .filter_map(|ix| view.lines.get(ix))
                                            .map(|line| {
                                                let color = match line.level {
                                                    tracing::Level::ERROR => LOG_ERROR,
                                                    tracing::Level::WARN => LOG_WARN,
                                                    tracing::Level::INFO => INK,
                                                    _ => MUTED,
                                                };
                                                div()
                                                    .truncate()
                                                    .text_color(rgb(color))
                                                    .child(log_line(line, this.clock))
                                            })
                                            .collect()
                                    }),
                                )
                                .track_scroll(view.scroll.clone())
                                .size_full(),
                            ),
                    ),
            )
    }
}

/// One record as shown and copied: `HH:MM:SS.mmm LEVEL target message fields`.
fn log_line(line: &LogLine, clock: Clock) -> String {
    format!(
        "{} {:<5} {} {}",
        log_time(line.time_us, clock),
        line.level,
        line.target,
        line.text
    )
}

/// Full-window modal that blocks everything beneath it.
fn overlay(
    selector: &'static str,
    title: &'static str,
    text: &'static str,
    actions: Vec<Stateful<Div>>,
) -> Div {
    div()
        .debug_selector(move || selector.into())
        .absolute()
        .size_full()
        .occlude()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgba(0x000000aa))
        .child(
            div()
                .w(px(380.))
                .p(px(20.))
                .rounded(px(12.))
                .bg(rgb(PANEL))
                .flex()
                .flex_col()
                .gap(px(10.))
                .child(
                    div()
                        .text_size(px(17.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(title),
                )
                .child(div().text_color(rgb(MUTED)).child(text))
                .child(div().flex().justify_end().gap(px(8.)).children(actions)),
        )
}
