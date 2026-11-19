//! Root view: chat list on the left, timeline on the right, read-only composer.

use crate::format::{chat_time, one_line, Clock};
use crate::input::TextInput;
use crate::store::{ChatFilter, ChatStore, SessionState, StoreEvent, TimelineChange};
use crate::theme::*;
use crate::timeline;
use crate::widgets::{avatar, button};
use bt_chat_core::model::{ChatId, ChatKind, ChatSummary, ConnectionState};
use gpui::{prelude::*, *};
use std::ops::Range;

actions!(bt, [Search, Quit]);

const ROW_HEIGHT: f32 = 64.;
const SIDEBAR_WIDTH: f32 = 340.;

pub const READ_ONLY_NOTICE: &str = "Режим только для чтения: отправка сообщений пока недоступна";

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
}

pub struct Desktop {
    pub(crate) store: Entity<ChatStore>,
    clock: Clock,
    pub(crate) search: Entity<TextInput>,
    pub(crate) filter: ChatFilter,
    open: Option<OpenTimeline>,
    pub(crate) menu: AccountMenu,
    _subscriptions: Vec<Subscription>,
}

impl Desktop {
    pub fn new(store: Entity<ChatStore>, clock: Clock, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| TextInput::new("Поиск", cx));
        let subscriptions = vec![
            cx.observe(&search, |_, _, cx| cx.notify()),
            cx.observe(&store, |_, _, cx| cx.notify()),
            cx.subscribe_in(&store, window, |this, _, event: &StoreEvent, _, cx| {
                if let StoreEvent::Timeline { chat, change } = event {
                    this.apply_timeline_change(chat, change, cx);
                }
            }),
        ];
        Self { store, clock, search, filter: ChatFilter::All, open: None, menu: AccountMenu::Closed, _subscriptions: subscriptions }
    }

    pub(crate) fn select(&mut self, chat: ChatId, cx: &mut Context<Self>) {
        self.store.update(cx, |store, cx| store.select(chat, cx));
    }

    pub(crate) fn confirm_sign_out(&mut self, cx: &mut Context<Self>) {
        self.menu = AccountMenu::Closed;
        self.store.update(cx, |store, _| store.sign_out());
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn list_state(&self) -> Option<&ListState> {
        self.open.as_ref().map(|o| &o.list)
    }

    /// Keeps the list element's item count in step with the store's timeline.
    fn apply_timeline_change(&mut self, chat: &ChatId, change: &TimelineChange, cx: &mut Context<Self>) {
        let Some(open) = self.open.as_mut().filter(|o| &o.chat == chat) else { return };
        let len = self.store.read(cx).timeline(chat).map_or(0, |t| t.messages.len());
        let list = &open.list;
        match change {
            TimelineChange::Reset => list.reset(len),
            TimelineChange::Status => {}
            TimelineChange::Delta { prepended, appended, touched } => {
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
                    list.scroll_to(ListOffset { item_ix: anchor.item_ix + prepended, offset_in_item: anchor.offset_in_item });
                }
            }
        }
        if list.item_count() != len {
            list.reset(len);
        }
        cx.notify();
    }

    fn sidebar(&mut self, cx: &mut Context<Self>) -> Div {
        let query = self.search.read(cx).content.to_string();
        let store = self.store.read(cx);
        let rows: Vec<ChatId> = store.visible_chats(&query, self.filter).into_iter().map(|c| c.id.clone()).collect();
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
                                this.menu = if this.menu == AccountMenu::Closed { AccountMenu::Open } else { AccountMenu::Closed };
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
                            .when(!active, |d| d.text_color(rgb(MUTED)).hover(|s| s.bg(rgb(ROW_HOVER))))
                            .child(label)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.filter = filter;
                                cx.notify();
                            }))
                    })),
            )
            .child(match empty {
                Some(text) => div()
                    .debug_selector(|| "chat-list-empty".into())
                    .flex_1()
                    .flex()
                    .justify_center()
                    .pt(px(40.))
                    .text_color(rgb(MUTED))
                    .child(text)
                    .into_any_element(),
                None => div()
                    .debug_selector(|| "chat-list".into())
                    .flex_1()
                    .min_h_0()
                    .child(
                        uniform_list(
                            "chat-list",
                            count,
                            cx.processor(move |this, range: Range<usize>, _, cx| {
                                range.filter_map(|ix| rows.get(ix)).map(|id| this.chat_row(id, cx)).collect::<Vec<_>>()
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
        let Some(chat) = store.chat(id) else { return div().into_any_element() };
        let selected = store.selected() == Some(id);
        let tag = format!("chat-{}", id.0);
        let preview = chat.last_message.as_ref().map(|m| match &m.author {
            Some(author) => format!("{author}: {}", one_line(&m.text)),
            None => one_line(&m.text),
        });
        let time = chat.last_message.as_ref().map(|m| chat_time(m.timestamp, self.clock));
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
            .child(avatar(store, chat.avatar.as_ref(), &id.0, &chat.title, 48., format!("{tag}-avatar")))
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
                                d.child(div().flex_none().text_size(px(12.)).text_color(rgb(MUTED)).child(time))
                            }),
                    )
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
                                    .text_size(px(14.))
                                    .text_color(rgb(MUTED))
                                    .child(preview.unwrap_or_default()),
                            )
                            .when(pinned, |d| marker(d, format!("{tag}-pinned"), "📌"))
                            .when(unread > 0, |d| {
                                d.child(
                                    div()
                                        .debug_selector(move || {
                                            format!("{tag}-badge{}", if muted { "-muted" } else { "" })
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
        if self.open.as_ref().is_none_or(|o| o.chat != selected) {
            let len = store.timeline(&selected).map_or(0, |t| t.messages.len());
            self.open = Some(OpenTimeline {
                chat: selected.clone(),
                list: ListState::new(len, ListAlignment::Bottom, px(600.)),
            });
        }
        let list = self.open.as_ref().map(|o| o.list.clone()).expect("open timeline was just set");
        let summary: Option<&ChatSummary> = store.chat(&selected);
        let title = summary.map_or_else(|| "Чат".to_string(), |c| c.title.clone());
        let subtitle = summary.map_or("", |c| match c.kind {
            ChatKind::Private => "Личный чат",
            ChatKind::Group => "Группа",
            ChatKind::Channel => "Канал",
        });
        let header_avatar = avatar(store, summary.and_then(|c| c.avatar.as_ref()), &selected.0, &title, 40., "header-avatar".into());
        let timeline = store.timeline(&selected);
        let loaded = timeline.is_some_and(|t| t.loaded);
        let empty = timeline.is_some_and(|t| t.loaded && t.messages.is_empty());
        let loading_older = timeline.is_some_and(|t| t.loading_older);

        let content = if !loaded {
            centered("timeline-loading-initial", "Загрузка сообщений…")
        } else if empty {
            centered("timeline-empty", "Сообщений пока нет")
        } else {
            let (message_store, chat, clock) = (self.store.clone(), selected.clone(), self.clock);
            let probe_list = list.clone();
            let (probe_store, probe_chat) = (self.store.clone(), selected.clone());
            div()
                .debug_selector(|| "timeline".into())
                .relative()
                .size_full()
                .child(
                    gpui::list(list, move |ix, _, cx| timeline::message_row(message_store.read(cx), &chat, ix, clock))
                        .size_full(),
                )
                // Runs after the list is laid out: asks for older history when the top is in view.
                .child(
                    canvas(
                        move |_, window, cx| {
                            if at_top(&probe_list) {
                                let (store, chat) = (probe_store.clone(), probe_chat.clone());
                                window.defer(cx, move |_, cx| store.update(cx, |s, cx| s.load_older(&chat, cx)));
                            }
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .size_0(),
                )
                .when(loading_older, |d| {
                    d.child(
                        div().absolute().top(px(8.)).w_full().flex().justify_center().child(
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
                    .debug_selector(|| "chat-header".into())
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
                            .child(div().text_size(px(13.)).text_color(rgb(MUTED)).child(subtitle)),
                    ),
            )
            .child(div().flex_1().min_h_0().child(content))
            .child(composer())
            .into_any_element()
    }
}

/// True when the oldest loaded message is in view, or everything fits so no scrolling could reach it.
fn at_top(list: &ListState) -> bool {
    list.item_count() > 0
        && list.viewport_bounds().size.height > px(0.)
        && (list.logical_scroll_top().item_ix == 0 || list.max_offset_for_scrollbar().height <= px(0.))
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
    parent.child(div().debug_selector(move || selector).flex_none().text_size(px(12.)).child(glyph))
}

fn connection_text(state: ConnectionState) -> Option<&'static str> {
    match state {
        ConnectionState::Online => None,
        ConnectionState::Connecting => Some("Подключение…"),
        ConnectionState::Offline => Some("Нет сети · переподключение…"),
    }
}

/// A composer that cannot take input: no text field exists, only a notice.
fn composer() -> Div {
    div()
        .debug_selector(|| "composer".into())
        .flex_none()
        .px(px(16.))
        .py(px(10.))
        .flex()
        .flex_col()
        .gap(px(4.))
        .bg(rgb(PANEL))
        .border_t_1()
        .border_color(rgb(DIVIDER))
        .child(
            div()
                .debug_selector(|| "composer-disabled".into())
                .h(px(42.))
                .px(px(16.))
                .rounded(px(21.))
                .flex()
                .items_center()
                .cursor_not_allowed()
                .bg(rgb(BG))
                .text_color(rgb(BADGE_MUTED))
                .child("Написать сообщение…"),
        )
        .child(
            div()
                .debug_selector(|| "composer-notice".into())
                .text_size(px(12.))
                .text_color(rgb(MUTED))
                .child(READ_ONLY_NOTICE),
        )
}

impl Render for Desktop {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let session = self.store.read(cx).session();
        let sidebar = self.sidebar(cx);
        let pane = self.chat_pane(cx);

        div()
            .key_context("BlazingTelesram")
            .relative()
            .size_full()
            .flex()
            .bg(rgb(BG))
            .text_color(rgb(INK))
            .text_size(px(14.))
            .on_action(cx.listener(|this, _: &Search, window, cx| window.focus(&this.search.focus_handle(cx))))
            .child(sidebar)
            .child(pane)
            .when(self.menu == AccountMenu::ConfirmSignOut && session == SessionState::Active, |d| {
                d.child(overlay(
                    "sign-out-dialog",
                    "Выйти из аккаунта?",
                    "Сессия, локальный кэш и загруженные файлы будут удалены.",
                    vec![
                        button("cancel-sign-out", "Отмена").debug_selector(|| "cancel-sign-out".into()).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.menu = AccountMenu::Closed;
                                cx.notify();
                            }),
                        ),
                        button("confirm-sign-out", "Выйти")
                            .debug_selector(|| "confirm-sign-out".into())
                            .text_color(rgb(ACCENT))
                            .on_click(cx.listener(|this, _, _, cx| this.confirm_sign_out(cx))),
                    ],
                ))
            })
            .when(session == SessionState::Expired, |d| {
                d.child(overlay(
                    "session-expired",
                    "Сессия истекла",
                    "Выйдите и войдите снова, чтобы продолжить чтение.",
                    vec![button("expired-sign-out", "Выйти")
                        .debug_selector(|| "expired-sign-out".into())
                        .on_click(cx.listener(|this, _, _, cx| this.confirm_sign_out(cx)))],
                ))
            })
            .when(session == SessionState::SignedOut, |d| {
                d.child(overlay("signed-out", "Вы вышли из аккаунта", "Данные сессии удалены.", vec![]))
            })
    }
}

/// Full-window modal that blocks everything beneath it.
fn overlay(selector: &'static str, title: &'static str, text: &'static str, actions: Vec<Stateful<Div>>) -> Div {
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
                .child(div().text_size(px(17.)).font_weight(FontWeight::MEDIUM).child(title))
                .child(div().text_color(rgb(MUTED)).child(text))
                .child(div().flex().justify_end().gap(px(8.)).children(actions)),
        )
}
