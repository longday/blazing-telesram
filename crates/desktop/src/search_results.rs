//! Server search results of the sidebar: the view model (a pure function of `SearchState`) and its rendering.
//!
//! The sections come from the server reply only; the local name matches are listed by the sidebar itself and
//! the chats among them are not repeated here.

use crate::format::{Clock, chat_time, one_line};
use crate::store::{ChatStore, SearchState, SearchStatus};
use crate::theme::*;
use crate::widgets::avatar;
use bt_chat_core::model::{ChatId, ChatKind, MediaRef, MessageTs, SearchFailure, UserId};
use gpui::{prelude::*, *};

const ROW_HEIGHT: f32 = 64.;

/// What the status line above the sections says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusLine {
    /// A request is in flight; rows of the previous query may still be listed.
    Loading,
    /// The query was not sent: no connection.
    Offline,
    /// The request failed; local matches are unaffected.
    Failed,
    /// The server answered with nothing for this query.
    NothingFound,
}

impl StatusLine {
    pub fn text(self) -> &'static str {
        match self {
            Self::Loading => "Поиск…",
            Self::Offline => "Для поиска по серверу нужно подключение",
            Self::Failed => "Не удалось выполнить поиск",
            Self::NothingFound => "На сервере ничего не найдено",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    Chat {
        open: ChatId,
        kind: ChatKind,
        title: String,
        avatar: Option<MediaRef>,
    },
    /// `open` is the existing private chat with this person; without it the row is shown but not clickable.
    Person {
        user: UserId,
        name: String,
        avatar: Option<MediaRef>,
        open: Option<ChatId>,
    },
    Message {
        open: ChatId,
        timestamp: MessageTs,
        chat_title: String,
        author: Option<String>,
        time: String,
        text: String,
    },
}

impl Row {
    /// The chat a click opens.
    pub fn open(&self) -> Option<&ChatId> {
        match self {
            Self::Chat { open, .. } | Self::Message { open, .. } => Some(open),
            Self::Person { open, .. } => open.as_ref(),
        }
    }
}

/// A non-empty group of rows under a title.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub title: &'static str,
    pub rows: Vec<Row>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchView {
    pub status: Option<StatusLine>,
    /// "Чаты", "Люди", "Сообщения" in this order, empty ones omitted.
    pub sections: Vec<Section>,
}

/// Turns the search state into what is shown. `local` are the chats already listed among the local name
/// matches: chat hits among them are dropped. Hits stay listed while a new request is in flight and after a
/// failure (the store keeps them), with the status line saying so.
pub fn view(state: &SearchState, local: &[ChatId], clock: Clock) -> SearchView {
    let hits = &state.hits;
    let chats: Vec<Row> = hits
        .chats
        .iter()
        .filter(|hit| !local.contains(&hit.id))
        .map(|hit| Row::Chat {
            open: hit.id.clone(),
            kind: hit.kind,
            title: hit.title.clone(),
            avatar: hit.avatar.clone(),
        })
        .collect();
    let dropped = chats.len() != hits.chats.len();
    let people: Vec<Row> = hits
        .people
        .iter()
        .map(|hit| Row::Person {
            user: hit.user.clone(),
            name: hit.name.clone(),
            avatar: hit.avatar.clone(),
            open: hit.chat.clone(),
        })
        .collect();
    let messages: Vec<Row> = hits
        .messages
        .iter()
        .map(|hit| Row::Message {
            open: hit.chat.clone(),
            timestamp: hit.timestamp,
            chat_title: hit.chat_title.clone(),
            author: hit.author_name.clone(),
            time: chat_time(hit.timestamp, clock),
            text: one_line(&hit.text),
        })
        .collect();
    let sections: Vec<Section> = [("Чаты", chats), ("Люди", people), ("Сообщения", messages)]
        .into_iter()
        .filter(|(_, rows)| !rows.is_empty())
        .map(|(title, rows)| Section { title, rows })
        .collect();
    // A chat hit dropped as a local match is a result the list above already shows, so the reply was not empty.
    let status = match state.status {
        SearchStatus::Loading => Some(StatusLine::Loading),
        SearchStatus::Failed(SearchFailure::Offline) => Some(StatusLine::Offline),
        SearchStatus::Failed(SearchFailure::Failed) => Some(StatusLine::Failed),
        SearchStatus::Ready if sections.is_empty() && !dropped => Some(StatusLine::NothingFound),
        SearchStatus::Ready => None,
    };
    SearchView { status, sections }
}

/// Status line and sections. A click on a row selects its chat in `store` and nothing else (no read marker).
/// Debug selectors: `search-status`, `search-section-<title>`, `search-chat-<id>`, `search-person-<guid>`,
/// `search-message-<chat>-<timestamp>`, each row with `-avatar` inside.
pub fn render(view: &SearchView, store: &Entity<ChatStore>, cx: &App) -> Div {
    let state = store.read(cx);
    let selected = state.selected();
    let status = view.status.map(|status| {
        div()
            .debug_selector(|| "search-status".into())
            .flex_none()
            .px(px(16.))
            .py(px(10.))
            .text_size(px(13.))
            .text_color(rgb(MUTED))
            .child(status.text())
    });
    div()
        .w_full()
        .flex()
        .flex_col()
        .children(status)
        .children(view.sections.iter().map(|section| {
            let selector = format!("search-section-{}", section.title);
            div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .debug_selector(move || selector)
                        .flex_none()
                        .px(px(16.))
                        .py(px(4.))
                        .bg(rgb(BG))
                        .text_size(px(13.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(MUTED))
                        .child(section.title),
                )
                .children(section.rows.iter().map(|row| {
                    let open = row.open();
                    row_view(row, store, state, open.is_some() && selected == open)
                }))
        }))
}

fn row_view(
    row: &Row,
    store: &Entity<ChatStore>,
    state: &ChatStore,
    selected: bool,
) -> Stateful<Div> {
    let (tag, lead, title, subtitle, time) = match row {
        Row::Chat {
            open,
            kind,
            title,
            avatar: media,
        } => {
            let tag = format!("search-chat-{}", open.0);
            let lead = avatar(
                state,
                media.as_ref(),
                &open.0,
                title,
                48.,
                format!("{tag}-avatar"),
            );
            let kind = match kind {
                ChatKind::Private => "Личный чат",
                ChatKind::Group => "Группа",
                ChatKind::Channel => "Канал",
            };
            (tag, lead, title.clone(), vec![Line::muted(kind)], None)
        }
        Row::Person {
            user,
            name,
            avatar: media,
            ..
        } => {
            let tag = format!("search-person-{}", user.0);
            let lead = avatar(
                state,
                media.as_ref(),
                &user.0,
                name,
                48.,
                format!("{tag}-avatar"),
            );
            (tag, lead, name.clone(), Vec::new(), None)
        }
        Row::Message {
            open,
            timestamp,
            chat_title,
            author,
            time,
            text,
        } => {
            let tag = format!("search-message-{}-{timestamp}", open.0);
            // The chat's own avatar, when the chat is in the loaded list.
            let media = state.chat(open).and_then(|chat| chat.avatar.as_ref());
            let lead = avatar(
                state,
                media,
                &open.0,
                chat_title,
                48.,
                format!("{tag}-avatar"),
            );
            let mut lines = Vec::new();
            if let Some(author) = author {
                lines.push(Line::accent(author.clone()));
            }
            lines.push(Line::text(text.clone()));
            (tag, lead, chat_title.clone(), lines, Some(time.clone()))
        }
    };

    let open = row.open().cloned();
    let clickable = open.is_some();
    let store = store.clone();
    div()
        .id(SharedString::from(tag.clone()))
        .debug_selector(move || format!("{tag}-row"))
        .min_h(px(ROW_HEIGHT))
        .w_full()
        .px(px(10.))
        .py(px(8.))
        .flex()
        .items_center()
        .gap(px(12.))
        .bg(rgb(if selected { ROW_SELECTED } else { PANEL }))
        .when(clickable, |d| {
            d.cursor_pointer()
                .hover(|s| s.bg(rgb(if selected { ROW_SELECTED } else { ROW_HOVER })))
        })
        .when_some(open, |d, chat| {
            d.on_click(move |_, _, cx| store.update(cx, |store, cx| store.select(chat.clone(), cx)))
        })
        .child(lead)
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
                                .child(title),
                        )
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
                .children(subtitle.into_iter().map(Line::into_element)),
        )
}

/// One line under a row's title: the author or kind, or the message text of at most two lines.
struct Line {
    text: String,
    color: u32,
    clamp: usize,
}

impl Line {
    fn muted(text: &'static str) -> Self {
        Self {
            text: text.into(),
            color: MUTED,
            clamp: 1,
        }
    }

    fn accent(text: String) -> Self {
        Self {
            text,
            color: ACCENT,
            clamp: 1,
        }
    }

    fn text(text: String) -> Self {
        Self {
            text,
            color: MUTED,
            clamp: 2,
        }
    }

    fn into_element(self) -> Div {
        div()
            .min_w_0()
            .text_size(px(14.))
            .text_color(rgb(self.color))
            .line_clamp(self.clamp)
            .child(self.text)
    }
}

#[cfg(test)]
mod tests {
    use super::{Row, SearchView, StatusLine, view};
    use crate::format::Clock;
    use crate::store::{SearchState, SearchStatus};
    use bt_chat_core::model::{
        ChatHit, ChatId, ChatKind, MessageHit, PersonHit, SearchFailure, SearchHits, SearchId,
        UserId,
    };

    const HOUR: i64 = 3_600_000_000;

    fn clock() -> Clock {
        // Noon, UTC.
        Clock::fixed(12 * HOUR, 0)
    }

    fn id(s: &str) -> ChatId {
        ChatId(s.into())
    }

    fn chat_hit(s: &str) -> ChatHit {
        ChatHit {
            id: id(s),
            kind: ChatKind::Group,
            title: format!("title {s}"),
            avatar: None,
        }
    }

    fn person(guid: &str, chat: Option<&str>) -> PersonHit {
        PersonHit {
            user: UserId(guid.into()),
            name: format!("name {guid}"),
            avatar: None,
            chat: chat.map(id),
        }
    }

    fn message(chat: &str, ts: i64, text: &str) -> MessageHit {
        MessageHit {
            chat: id(chat),
            chat_title: format!("title {chat}"),
            timestamp: ts,
            author_name: Some("Анна".into()),
            text: text.into(),
        }
    }

    fn state(status: SearchStatus, hits: SearchHits) -> SearchState {
        SearchState {
            id: SearchId(1),
            query: "q".into(),
            status,
            hits,
        }
    }

    fn full() -> SearchHits {
        SearchHits {
            chats: vec![chat_hit("a"), chat_hit("b")],
            people: vec![person("u1", Some("p1")), person("u2", None)],
            messages: vec![message("a", 9 * HOUR, "line one\n  line   two")],
        }
    }

    fn titles(view: &SearchView) -> Vec<&'static str> {
        view.sections.iter().map(|s| s.title).collect()
    }

    #[test]
    fn sections_come_in_order() {
        let view = view(&state(SearchStatus::Ready, full()), &[], clock());
        assert_eq!(titles(&view), ["Чаты", "Люди", "Сообщения"]);
        assert_eq!(view.status, None);
        assert_eq!(view.sections[0].rows.len(), 2);
        assert_eq!(view.sections[1].rows.len(), 2);
        assert_eq!(view.sections[2].rows.len(), 1);
    }

    #[test]
    fn empty_sections_are_omitted() {
        let mut hits = full();
        hits.people.clear();
        assert_eq!(
            titles(&view(
                &state(SearchStatus::Ready, hits.clone()),
                &[],
                clock()
            )),
            ["Чаты", "Сообщения"]
        );
        hits.chats.clear();
        hits.messages.clear();
        hits.people.push(person("u1", None));
        assert_eq!(
            titles(&view(&state(SearchStatus::Ready, hits), &[], clock())),
            ["Люди"]
        );
    }

    #[test]
    fn chats_among_local_matches_are_dropped() {
        let view = view(&state(SearchStatus::Ready, full()), &[id("a")], clock());
        let Row::Chat { open, .. } = &view.sections[0].rows[0] else {
            panic!("chat row")
        };
        assert_eq!(*open, id("b"));
        assert_eq!(view.sections[0].rows.len(), 1);
        // Messages and people of a locally matched chat stay.
        assert_eq!(view.sections[2].rows.len(), 1);
    }

    #[test]
    fn a_section_of_only_local_chats_disappears_without_claiming_nothing_found() {
        let hits = SearchHits {
            chats: vec![chat_hit("a")],
            ..SearchHits::default()
        };
        let view = view(&state(SearchStatus::Ready, hits), &[id("a")], clock());
        assert!(view.sections.is_empty());
        assert_eq!(view.status, None);
    }

    #[test]
    fn a_person_is_clickable_only_with_a_private_chat() {
        let view = view(&state(SearchStatus::Ready, full()), &[], clock());
        let opens: Vec<Option<&ChatId>> = view.sections[1].rows.iter().map(Row::open).collect();
        assert_eq!(opens, [Some(&id("p1")), None]);
    }

    #[test]
    fn chat_and_message_rows_open_their_chat() {
        let view = view(&state(SearchStatus::Ready, full()), &[], clock());
        assert_eq!(view.sections[0].rows[1].open(), Some(&id("b")));
        assert_eq!(view.sections[2].rows[0].open(), Some(&id("a")));
    }

    #[test]
    fn a_message_row_shows_chat_author_time_and_one_line_text() {
        let view = view(&state(SearchStatus::Ready, full()), &[], clock());
        assert_eq!(
            view.sections[2].rows[0],
            Row::Message {
                open: id("a"),
                timestamp: 9 * HOUR,
                chat_title: "title a".into(),
                author: Some("Анна".into()),
                time: "09:00".into(),
                text: "line one line two".into(),
            }
        );
    }

    #[test]
    fn loading_keeps_the_previous_hits() {
        let view = view(&state(SearchStatus::Loading, full()), &[], clock());
        assert_eq!(view.status, Some(StatusLine::Loading));
        assert_eq!(titles(&view), ["Чаты", "Люди", "Сообщения"]);
        assert_eq!(StatusLine::Loading.text(), "Поиск…");
    }

    #[test]
    fn loading_without_hits_shows_only_the_status() {
        let view = view(
            &state(SearchStatus::Loading, SearchHits::default()),
            &[],
            clock(),
        );
        assert_eq!(
            view,
            SearchView {
                status: Some(StatusLine::Loading),
                sections: Vec::new()
            }
        );
    }

    #[test]
    fn offline_and_failure_have_their_own_text() {
        let offline = view(
            &state(
                SearchStatus::Failed(SearchFailure::Offline),
                SearchHits::default(),
            ),
            &[],
            clock(),
        );
        assert_eq!(offline.status, Some(StatusLine::Offline));
        assert_eq!(
            StatusLine::Offline.text(),
            "Для поиска по серверу нужно подключение"
        );
        let failed = view(
            &state(
                SearchStatus::Failed(SearchFailure::Failed),
                SearchHits::default(),
            ),
            &[],
            clock(),
        );
        assert_eq!(failed.status, Some(StatusLine::Failed));
        assert_eq!(StatusLine::Failed.text(), "Не удалось выполнить поиск");
        assert_ne!(StatusLine::Offline.text(), StatusLine::Failed.text());
    }

    #[test]
    fn a_failure_keeps_the_hits_already_shown() {
        let view = view(
            &state(SearchStatus::Failed(SearchFailure::Failed), full()),
            &[],
            clock(),
        );
        assert_eq!(view.status, Some(StatusLine::Failed));
        assert_eq!(titles(&view), ["Чаты", "Люди", "Сообщения"]);
    }

    #[test]
    fn an_empty_reply_says_nothing_found() {
        let view = view(
            &state(SearchStatus::Ready, SearchHits::default()),
            &[id("a")],
            clock(),
        );
        assert_eq!(
            view,
            SearchView {
                status: Some(StatusLine::NothingFound),
                sections: Vec::new()
            }
        );
        assert_eq!(
            StatusLine::NothingFound.text(),
            "На сервере ничего не найдено"
        );
    }

    mod rendering {
        use super::super::{SearchView, render, view};
        use crate::format::Clock;
        use crate::store::{ChatStore, SearchStatus};
        use bt_chat_core::model::{
            ChatHit, ChatId, ChatKind, ChatSummary, ClientEvent, Command, MessageHit, PersonHit,
            SearchHits, SearchId, UserId,
        };
        use gpui::{
            AppContext, Context, Entity, IntoElement, Modifiers, Render, TestAppContext,
            VisualTestContext, Window, px, size,
        };

        struct Host {
            store: Entity<ChatStore>,
            view: SearchView,
        }

        impl Render for Host {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                render(&self.view, &self.store, cx)
            }
        }

        fn private(id: &str, title: &str) -> ChatSummary {
            ChatSummary {
                id: ChatId(id.into()),
                kind: ChatKind::Private,
                title: title.into(),
                avatar: None,
                last_message: None,
                unread: 0,
                muted: false,
                pinned_rank: None,
                partner: None,
            }
        }

        fn hits() -> SearchHits {
            SearchHits {
                chats: vec![ChatHit {
                    id: ChatId("g".into()),
                    kind: ChatKind::Group,
                    title: "Команда".into(),
                    avatar: None,
                }],
                people: vec![
                    PersonHit {
                        user: UserId("u1".into()),
                        name: "Анна".into(),
                        avatar: None,
                        chat: Some(ChatId("p1".into())),
                    },
                    PersonHit {
                        user: UserId("u2".into()),
                        name: "Борис".into(),
                        avatar: None,
                        chat: None,
                    },
                ],
                messages: vec![MessageHit {
                    chat: ChatId("m".into()),
                    chat_title: "Релизы".into(),
                    timestamp: 7,
                    author_name: Some("Кристина".into()),
                    text: "пайпы падают".into(),
                }],
            }
        }

        /// A store with an answered search, and a window showing its results.
        fn show(
            cx: &mut TestAppContext,
        ) -> (
            Entity<ChatStore>,
            async_channel::Receiver<Command>,
            &mut VisualTestContext,
        ) {
            let (events, event_rx) = async_channel::unbounded::<ClientEvent>();
            let (command_tx, commands) = async_channel::unbounded();
            let store = cx.new(|cx| ChatStore::new(event_rx, command_tx, cx));
            store.update(cx, |s, cx| {
                s.apply(ClientEvent::ChatsReplaced(vec![private("p1", "Анна")]), cx);
                s.search("ab".into(), cx);
                s.apply(
                    ClientEvent::SearchResults {
                        id: SearchId(1),
                        hits: hits(),
                    },
                    cx,
                );
            });
            let state = store.read_with(cx, |s, _| s.search_state().cloned().unwrap());
            assert_eq!(state.status, SearchStatus::Ready);
            let view = view(&state, &[], Clock::fixed(0, 0));
            let (_, vcx) = cx.add_window_view({
                let store = store.clone();
                move |_, _| Host { store, view }
            });
            vcx.simulate_resize(size(px(400.), px(900.)));
            // The sender must outlive the store's pump; leak it for the test's lifetime.
            std::mem::forget(events);
            while commands.try_recv().is_ok() {}
            (store, commands, vcx)
        }

        fn click(cx: &mut VisualTestContext, selector: &'static str) {
            let bounds = cx
                .debug_bounds(selector)
                .unwrap_or_else(|| panic!("{selector} is not on screen"));
            cx.simulate_click(bounds.center(), Modifiers::none());
        }

        #[gpui::test]
        fn sections_and_rows_are_drawn(cx: &mut TestAppContext) {
            let (_store, _commands, vcx) = show(cx);
            for selector in [
                "search-section-Чаты",
                "search-section-Люди",
                "search-section-Сообщения",
                "search-chat-g-row",
                "search-chat-g-avatar-initials",
                "search-person-u1-row",
                "search-person-u2-row",
                "search-message-m-7-row",
            ] {
                assert!(
                    vcx.debug_bounds(selector).is_some(),
                    "{selector} is not on screen"
                );
            }
            assert!(
                vcx.debug_bounds("search-status").is_none(),
                "a ready reply with rows has no status line"
            );
        }

        #[gpui::test]
        fn clicking_a_chat_a_message_or_a_person_with_a_chat_selects_it(cx: &mut TestAppContext) {
            let (store, commands, vcx) = show(cx);
            click(vcx, "search-chat-g-row");
            assert_eq!(
                commands.try_recv().ok(),
                Some(Command::OpenChat(ChatId("g".into())))
            );
            click(vcx, "search-message-m-7-row");
            assert_eq!(
                commands.try_recv().ok(),
                Some(Command::OpenChat(ChatId("m".into())))
            );
            click(vcx, "search-person-u1-row");
            assert_eq!(
                commands.try_recv().ok(),
                Some(Command::OpenChat(ChatId("p1".into())))
            );
            assert!(
                commands.try_recv().is_err(),
                "choosing a result sends nothing else, in particular no read marker"
            );
            assert_eq!(
                store.read_with(vcx, |s, _| s.selected().cloned()),
                Some(ChatId("p1".into()))
            );
        }

        #[gpui::test]
        fn a_person_without_a_chat_does_nothing_on_click(cx: &mut TestAppContext) {
            let (store, commands, vcx) = show(cx);
            click(vcx, "search-person-u2-row");
            assert!(commands.try_recv().is_err());
            assert_eq!(store.read_with(vcx, |s, _| s.selected().cloned()), None);
        }
    }
}
