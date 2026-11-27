//! Headless UI tests: the real `ChatStore` and `Desktop` driven through channels, inspected via debug bounds.

use crate::desktop::{
    AccountMenu, Desktop, Dismiss, Preview, Search, ToggleLog, ViewerNext, ViewerPrev, chat_preview,
};
use crate::format::{Clock, fit_thumbnail};
use crate::logging::{LogBuffer, LogLine};
use crate::search_results::StatusLine;
use crate::store::{
    ChatFilter, ChatStore, DownloadState, MESSAGE_GONE, REACTORS_REFRESH_DELAY, ReactorsState,
    SEARCH_DEBOUNCE, SearchStatus, SessionState,
};
use crate::timeline::gallery_columns;
use async_channel::{Receiver, Sender};
use bt_chat_core::model::*;
use bt_chat_core::reactions::{KNOWN, ReactionCode, ReactionCount, Reactions};
use gpui::{
    AppContext, Bounds, Context, Entity, KeyBinding, ListState, Modifiers, MouseButton, Pixels,
    ScrollDelta, ScrollWheelEvent, TestAppContext, VisualTestContext, point, px, size,
};
use std::path::PathBuf;
use std::time::Duration;

const PNG_1X1: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0xf0,
    0x1f, 0x00, 0x05, 0x00, 0x01, 0xff, 0x89, 0x99, 0x3d, 0x1d, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

/// Fixture message `i` has timestamp `i` microseconds, so its debug selectors (`msg-<ts>`) can be literals:
/// `debug_bounds` only takes `&'static str`.
fn ts(i: i64) -> MessageTs {
    i
}

macro_rules! msg {
    ($ts:literal) => {
        concat!("msg-", $ts)
    };
    ($ts:literal, $part:literal) => {
        concat!("msg-", $ts, "-", $part)
    };
}

macro_rules! row {
    ($id:literal) => {
        concat!("chat-", $id, "-row")
    };
}

macro_rules! part {
    ($id:literal, $part:literal) => {
        concat!("chat-", $id, "-", $part)
    };
}

fn chat(id: &str, kind: ChatKind, title: &str, last: Option<i64>) -> ChatSummary {
    ChatSummary {
        id: ChatId(id.into()),
        kind,
        title: title.into(),
        avatar: None,
        last_message: last.map(|i| MessagePreview {
            author: None,
            text: format!("превью {i}"),
            timestamp: ts(i),
        }),
        unread: 0,
        muted: false,
        pinned_rank: None,
        partner: None,
        pinned_message: None,
        pin_action_ts: None,
    }
}

fn message(chat: &str, i: i64, author: &str, outgoing: bool, body: MessageBody) -> Message {
    Message {
        chat: ChatId(chat.into()),
        timestamp: ts(i),
        author: UserId(author.into()),
        author_name: Some(author.to_uppercase()),
        author_avatar: None,
        outgoing,
        body,
        edited: false,
        reply_to: None,
        forwarded_from: None,
        delivery: bt_chat_core::model::Delivery::Sent,
        reactions: Default::default(),
        important: false,
    }
}

fn text(chat_id: &str, i: i64) -> Message {
    message(
        chat_id,
        i,
        "alice",
        false,
        MessageBody::Text(format!("сообщение {i}")),
    )
}

fn chunk(chat_id: &str, range: std::ops::RangeInclusive<i64>, reached_start: bool) -> ClientEvent {
    ClientEvent::History(HistoryChunk {
        chat: ChatId(chat_id.into()),
        messages: range.map(|i| text(chat_id, i)).collect(),
        reached_start,
        replaces: None,
        payloads: Vec::new(),
    })
}

fn history(chat_id: &str, messages: Vec<Message>, reached_start: bool) -> ClientEvent {
    ClientEvent::History(HistoryChunk {
        chat: ChatId(chat_id.into()),
        messages,
        reached_start,
        replaces: None,
        payloads: Vec::new(),
    })
}

fn media(id: &str, kind: MediaKind) -> MediaRef {
    MediaRef {
        id: id.into(),
        kind,
    }
}

struct Harness {
    events: Sender<ClientEvent>,
    commands: Receiver<Command>,
    store: Entity<ChatStore>,
    desktop: Entity<Desktop>,
}

fn harness(cx: &mut TestAppContext) -> (Harness, &mut VisualTestContext) {
    harness_with(cx, vec![], |_, _| {})
}

/// Marks that the app's key bindings are installed in this test app.
struct KeysBound;

impl gpui::Global for KeysBound {}

/// `debug_bounds` entries are never removed once painted, so "not shown" is only meaningful in a window
/// that never painted the element: tests of absence start from `events` and `setup` applied before the first frame.
fn harness_with(
    cx: &mut TestAppContext,
    events: Vec<ClientEvent>,
    setup: impl FnOnce(&mut Desktop, &mut Context<Desktop>) + 'static,
) -> (Harness, &mut VisualTestContext) {
    cx.update(|cx| {
        if !cx.has_global::<KeysBound>() {
            crate::input::bind(cx);
            cx.bind_keys([
                KeyBinding::new("escape", Dismiss, Some("BlazingTelesram")),
                KeyBinding::new("left", ViewerPrev, Some("BlazingTelesram")),
                KeyBinding::new("right", ViewerNext, Some("BlazingTelesram")),
            ]);
            cx.set_global(KeysBound);
        }
    });
    let (events_tx, event_rx) = async_channel::unbounded();
    let (command_tx, commands) = async_channel::unbounded();
    let store = cx.new(|cx| ChatStore::new(event_rx, command_tx, cx));
    for event in events {
        store.update(cx, |s, cx| s.apply(event, cx));
    }
    let clock = Clock::fixed(ts(10_000), 0);
    let (desktop, vcx) = cx.add_window_view({
        let store = store.clone();
        move |window, cx| {
            let mut desktop = Desktop::new(store, clock, window, cx);
            setup(&mut desktop, cx);
            desktop
        }
    });
    vcx.simulate_resize(size(px(1200.), px(1400.)));
    (
        Harness {
            events: events_tx,
            commands,
            store,
            desktop,
        },
        vcx,
    )
}

impl Harness {
    fn feed(&self, cx: &mut VisualTestContext, events: impl IntoIterator<Item = ClientEvent>) {
        for event in events {
            self.events.try_send(event).unwrap();
        }
        cx.run_until_parked();
    }

    fn commands(&self) -> Vec<Command> {
        std::iter::from_fn(|| self.commands.try_recv().ok()).collect()
    }

    fn list(&self, cx: &mut VisualTestContext) -> ListState {
        self.desktop
            .read_with(cx, |d, _| d.list_state().expect("a chat is open").clone())
    }
}

fn bounds(cx: &mut VisualTestContext, selector: &'static str) -> Option<Bounds<Pixels>> {
    cx.debug_bounds(selector)
}

fn shown(cx: &mut VisualTestContext, selector: &'static str) -> bool {
    bounds(cx, selector).is_some()
}

fn click(cx: &mut VisualTestContext, selector: &'static str) {
    let b = bounds(cx, selector).unwrap_or_else(|| panic!("{selector} is not on screen"));
    cx.simulate_click(b.center(), Modifiers::none());
}

fn scroll(cx: &mut VisualTestContext, dy: f32) {
    let b = bounds(cx, "timeline").expect("timeline is on screen");
    cx.simulate_event(ScrollWheelEvent {
        position: b.center(),
        delta: ScrollDelta::Pixels(point(px(0.), px(dy))),
        ..Default::default()
    });
}

fn y(cx: &mut VisualTestContext, selector: &'static str) -> f32 {
    f32::from(
        bounds(cx, selector)
            .unwrap_or_else(|| panic!("{selector} is not on screen"))
            .origin
            .y,
    )
}

fn scroll_to_top(h: &Harness, cx: &mut VisualTestContext) {
    // Unmeasured items above the viewport have no height yet, so one wheel burst may stop short.
    for _ in 0..10 {
        scroll(cx, 1_000_000.);
        cx.run_until_parked();
        if h.list(cx).logical_scroll_top().item_ix == 0 {
            return;
        }
    }
    panic!("could not reach the top of the timeline");
}

fn loading_older(h: &Harness, cx: &mut VisualTestContext, chat: &str) -> bool {
    h.store.read_with(cx, |s, _| {
        s.timeline(&ChatId(chat.into())).unwrap().loading_older
    })
}

// ---- 6.1 store ----

#[gpui::test]
fn store_applies_events_and_issues_commands(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    h.store.read_with(cx, |s, _| {
        assert!(!s.chats_loaded());
        assert_eq!(s.connection(), ConnectionState::Connecting);
    });

    let mut a = chat("a", ChatKind::Private, "Анна", Some(1));
    a.avatar = Some(media("av-a", MediaKind::Avatar));
    h.feed(
        cx,
        [ClientEvent::ChatsReplaced(vec![
            a.clone(),
            chat("b", ChatKind::Group, "Группа", None),
        ])],
    );
    h.feed(
        cx,
        [
            ClientEvent::ChatUpdated(a),
            ClientEvent::Connection(ConnectionState::Online),
        ],
    );
    h.store.read_with(cx, |s, _| {
        assert!(s.chats_loaded());
        assert_eq!(s.chat_count(), 2);
        assert_eq!(s.connection(), ConnectionState::Online);
    });
    assert_eq!(
        h.commands(),
        [Command::FetchMedia(media("av-a", MediaKind::Avatar))],
        "avatar requested once"
    );

    h.store.update(cx, |s, cx| s.select(ChatId("b".into()), cx));
    h.store.update(cx, |s, cx| s.select(ChatId("b".into()), cx));
    assert_eq!(h.commands(), [Command::OpenChat(ChatId("b".into()))]);

    h.feed(cx, [chunk("b", 1..=3, true)]);
    let mut edited = text("b", 2);
    edited.edited = true;
    h.feed(
        cx,
        [
            ClientEvent::MessageUpserted(text("b", 4)),
            ClientEvent::MessageUpserted(edited),
            ClientEvent::MessageDeleted {
                chat: ChatId("b".into()),
                timestamp: ts(1),
            },
            // Never opened: ignored.
            ClientEvent::MessageUpserted(text("a", 9)),
        ],
    );
    h.store.read_with(cx, |s, _| {
        let tl = s.timeline(&ChatId("b".into())).unwrap();
        assert_eq!(
            tl.messages.iter().map(|m| m.timestamp).collect::<Vec<_>>(),
            [1, 2, 3, 4]
        );
        assert_eq!(tl.messages[0].body, MessageBody::Deleted);
        assert!(tl.messages[1].edited);
        assert!(s.timeline(&ChatId("a".into())).is_none());
    });

    let path = PathBuf::from("/tmp/av-a.png");
    h.feed(
        cx,
        [ClientEvent::MediaReady {
            media: media("av-a", MediaKind::Avatar),
            path: path.clone(),
        }],
    );
    h.store.read_with(cx, |s, _| {
        assert_eq!(
            s.media_path(&media("av-a", MediaKind::Avatar)),
            Some(path.as_path())
        );
    });

    h.feed(cx, [ClientEvent::SessionExpired]);
    h.store
        .read_with(cx, |s, _| assert_eq!(s.session(), SessionState::Expired));
    h.feed(cx, [ClientEvent::SignedOut]);
    h.store.read_with(cx, |s, _| {
        assert_eq!(s.session(), SessionState::SignedOut);
        assert_eq!(s.chat_count(), 0);
        assert!(s.selected().is_none());
    });
    assert!(h.commands().is_empty(), "nothing else was sent");
}

#[gpui::test]
fn unavailable_media_is_requested_again_on_the_next_refresh(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    let avatar = media("deleted_user", MediaKind::Avatar);
    let mut a = chat("a", ChatKind::Private, "Анна", None);
    a.avatar = Some(avatar.clone());
    h.feed(cx, [ClientEvent::ChatsReplaced(vec![a.clone()])]);
    assert_eq!(h.commands(), [Command::FetchMedia(avatar.clone())]);
    h.feed(cx, [ClientEvent::ChatsReplaced(vec![a.clone()])]);
    assert!(h.commands().is_empty(), "still in flight");

    h.feed(cx, [ClientEvent::MediaUnavailable(avatar.clone())]);
    assert!(
        h.commands().is_empty(),
        "no retry loop: waits for the next refresh"
    );
    h.feed(cx, [ClientEvent::ChatUpdated(a)]);
    assert_eq!(h.commands(), [Command::FetchMedia(avatar)]);
}

#[gpui::test]
fn catch_up_drops_messages_deleted_beyond_the_returned_span(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    h.store.update(cx, |s, cx| s.select(ChatId("b".into()), cx));
    h.feed(cx, [chunk("b", 1..=3, true)]);
    let stamps = |h: &Harness, cx: &mut VisualTestContext| {
        h.store.read_with(cx, |s, _| {
            s.timeline(&ChatId("b".into()))
                .unwrap()
                .messages
                .iter()
                .map(|m| m.timestamp)
                .collect::<Vec<_>>()
        })
    };
    // Messages 2 and 3 were deleted while offline; the core replaced everything from 1 on.
    let catch_up = |messages, replaces| {
        ClientEvent::History(HistoryChunk {
            chat: ChatId("b".into()),
            messages,
            reached_start: false,
            replaces,
            payloads: Vec::new(),
        })
    };
    h.feed(
        cx,
        [catch_up(vec![text("b", 1)], Some((Some(ts(1)), None)))],
    );
    assert_eq!(stamps(&h, cx), [ts(1)]);
    // An empty chunk that replaced the whole history clears it.
    h.feed(cx, [catch_up(vec![], Some((None, None)))]);
    assert!(stamps(&h, cx).is_empty());
}

#[gpui::test]
fn replacement_over_a_gap_reopens_older_loading(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    let b = ChatId("b".into());
    h.store.update(cx, |s, cx| s.select(b.clone(), cx));
    h.feed(cx, [chunk("b", 1..=2, true)]);
    assert!(
        h.commands()
            .iter()
            .all(|c| !matches!(c, Command::LoadOlder { .. })),
        "start known: no paging"
    );
    // The core dropped the cached history: the new chunk does not reach the start, so the
    // short timeline pages backwards again.
    h.feed(
        cx,
        [ClientEvent::History(HistoryChunk {
            chat: b.clone(),
            messages: vec![text("b", 5), text("b", 6)],
            reached_start: false,
            replaces: Some((None, None)),
            payloads: Vec::new(),
        })],
    );
    assert!(h.commands().contains(&Command::LoadOlder {
        chat: b,
        before: ts(5)
    }));
}

// ---- 6.2 chat list ----

#[gpui::test]
fn empty_account_shows_empty_state(cx: &mut TestAppContext) {
    let list_or_empty = |cx: &mut TestAppContext, events: Vec<ClientEvent>| {
        let (_h, cx) = harness_with(cx, events, |_, _| {});
        (shown(cx, "chat-list"), shown(cx, "chat-list-empty"))
    };
    assert_eq!(
        list_or_empty(cx, vec![]),
        (false, true),
        "placeholder while loading"
    );
    assert_eq!(
        list_or_empty(cx, vec![ClientEvent::ChatsReplaced(vec![])]),
        (false, true)
    );
    let one = vec![ClientEvent::ChatsReplaced(vec![chat(
        "a",
        ChatKind::Private,
        "Анна",
        Some(1),
    )])];
    assert_eq!(list_or_empty(cx, one), (true, false));
}

#[gpui::test]
fn rows_show_avatar_badges_and_markers(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    let mut loud = chat("loud", ChatKind::Private, "Анна Петрова", Some(2));
    loud.unread = 3;
    loud.avatar = Some(media("av-loud", MediaKind::Avatar));
    let mut quiet = chat("quiet", ChatKind::Group, "Команда", Some(1));
    quiet.unread = 120;
    quiet.muted = true;
    quiet.pinned_rank = Some(0);
    let plain = chat("plain", ChatKind::Channel, "Новости", None);
    h.feed(cx, [ClientEvent::ChatsReplaced(vec![loud, quiet, plain])]);

    assert!(shown(cx, part!("loud", "badge")));
    assert!(!shown(cx, part!("loud", "badge-muted")));
    assert!(!shown(cx, part!("loud", "muted")));
    assert!(!shown(cx, part!("loud", "pinned")));
    assert!(shown(cx, part!("quiet", "badge-muted")));
    assert!(shown(cx, part!("quiet", "muted")));
    assert!(shown(cx, part!("quiet", "pinned")));
    assert!(!shown(cx, part!("plain", "badge")));
    assert!(!shown(cx, part!("plain", "badge-muted")));

    // Initials until the image arrives.
    assert!(shown(cx, part!("loud", "avatar-initials")));
    assert!(shown(cx, part!("plain", "avatar-initials")));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("loud.png");
    std::fs::write(&path, PNG_1X1).unwrap();
    h.feed(
        cx,
        [ClientEvent::MediaReady {
            media: media("av-loud", MediaKind::Avatar),
            path,
        }],
    );
    assert!(shown(cx, part!("loud", "avatar-image")));
    assert!(!shown(cx, part!("loud", "avatar-initials")));
    assert!(shown(cx, part!("plain", "avatar-initials")));
}

#[gpui::test]
fn rows_are_ordered_and_reorder_on_activity(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    let mut pinned_second = chat("c", ChatKind::Private, "C", Some(20));
    pinned_second.pinned_rank = Some(1);
    let mut pinned_first = chat("d", ChatKind::Private, "D", Some(5));
    pinned_first.pinned_rank = Some(0);
    h.feed(
        cx,
        [ClientEvent::ChatsReplaced(vec![
            chat("a", ChatKind::Private, "A", Some(10)),
            chat("b", ChatKind::Private, "B", Some(30)),
            pinned_second,
            pinned_first,
        ])],
    );
    let order = |cx: &mut VisualTestContext| {
        let mut rows = [
            ('a', y(cx, row!("a"))),
            ('b', y(cx, row!("b"))),
            ('c', y(cx, row!("c"))),
            ('d', y(cx, row!("d"))),
        ];
        rows.sort_by(|l, r| l.1.total_cmp(&r.1));
        rows.into_iter().map(|(id, _)| id).collect::<String>()
    };
    assert_eq!(order(cx), "dcba");

    h.feed(
        cx,
        [ClientEvent::ChatUpdated(chat(
            "a",
            ChatKind::Private,
            "A",
            Some(40),
        ))],
    );
    assert_eq!(
        order(cx),
        "dcab",
        "newer activity moves an unpinned chat up, but not above pinned ones"
    );
}

fn rendered_rows(
    cx: &mut TestAppContext,
    query: &'static str,
    filter: ChatFilter,
) -> (Vec<&'static str>, bool) {
    let mut unread = chat("g", ChatKind::Group, "Команда", Some(3));
    unread.unread = 1;
    let chats = vec![
        chat("p1", ChatKind::Private, "Анна Петрова", Some(1)),
        chat("p2", ChatKind::Private, "Пётр", Some(2)),
        unread,
        chat("ch", ChatKind::Channel, "Новости", Some(4)),
    ];
    let (_h, cx) = harness_with(cx, vec![ClientEvent::ChatsReplaced(chats)], move |d, cx| {
        d.filter = filter;
        d.search.update(cx, |input, cx| input.set_text(query, cx));
    });
    let rows = [
        ("p1", row!("p1")),
        ("p2", row!("p2")),
        ("g", row!("g")),
        ("ch", row!("ch")),
    ]
    .into_iter()
    .filter(|(_, selector)| shown(cx, *selector))
    .map(|(id, _)| id)
    .collect();
    (rows, shown(cx, "chat-list-empty"))
}

#[gpui::test]
fn search_and_filters_narrow_the_list(cx: &mut TestAppContext) {
    use ChatFilter::*;
    // Rows are reported in the fixed order p1, p2, g, ch (not screen order).
    assert_eq!(
        rendered_rows(cx, "", All),
        (vec!["p1", "p2", "g", "ch"], false)
    );
    assert_eq!(
        rendered_rows(cx, "ПЕТР", All).0,
        ["p1"],
        "case-insensitive substring match"
    );
    assert_eq!(rendered_rows(cx, "нет такого", All), (vec![], true));
    assert_eq!(rendered_rows(cx, "", Unread).0, ["g"]);
    assert_eq!(rendered_rows(cx, "", Private).0, ["p1", "p2"]);
    assert_eq!(rendered_rows(cx, "", Groups).0, ["g"]);
    assert_eq!(rendered_rows(cx, "", Channels).0, ["ch"]);
    assert_eq!(
        rendered_rows(cx, "команда", Private),
        (vec![], true),
        "search and filter combine"
    );
}

#[gpui::test]
fn filter_chips_select_one_filter(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    h.feed(
        cx,
        [ClientEvent::ChatsReplaced(vec![chat(
            "a",
            ChatKind::Private,
            "Анна",
            Some(1),
        )])],
    );
    let filter = |cx: &mut VisualTestContext| h.desktop.read_with(cx, |d, _| d.filter);
    assert_eq!(filter(cx), ChatFilter::All);
    click(cx, "filter-unread");
    assert_eq!(filter(cx), ChatFilter::Unread);
    click(cx, "filter-private");
    assert_eq!(filter(cx), ChatFilter::Private);
    click(cx, "filter-groups");
    assert_eq!(filter(cx), ChatFilter::Groups);
    click(cx, "filter-channels");
    assert_eq!(filter(cx), ChatFilter::Channels);
    click(cx, "filter-all");
    assert_eq!(filter(cx), ChatFilter::All);
}

#[gpui::test]
fn clicking_a_row_opens_the_chat_without_marking_it_read(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    let mut c = chat("c", ChatKind::Private, "Анна", Some(1));
    c.unread = 2;
    h.feed(cx, [ClientEvent::ChatsReplaced(vec![c])]);
    assert!(!shown(cx, "chat-header"));
    assert!(shown(cx, "no-chat"));
    assert!(!shown(cx, "composer"));

    click(cx, row!("c"));
    assert_eq!(h.commands(), [Command::OpenChat(ChatId("c".into()))]);
    assert!(shown(cx, "chat-header"));
    assert!(shown(cx, "timeline-loading-initial"));
    h.feed(cx, [chunk("c", 1..=2, true)]);
    assert!(shown(cx, "timeline"));
    assert!(shown(cx, part!("c", "badge")), "unread counter stays");
    h.store.read_with(cx, |s, _| {
        assert_eq!(s.chat(&ChatId("c".into())).unwrap().unread, 2)
    });
    assert!(h.commands().is_empty());
}

// ---- global search ----

fn search_chats() -> Vec<ChatSummary> {
    vec![
        chat("g", ChatKind::Group, "Команда", Some(1)),
        chat("a", ChatKind::Private, "Анна", Some(2)),
    ]
}

fn search_hits() -> SearchHits {
    SearchHits {
        chats: vec![ChatHit {
            id: ChatId("srv".into()),
            kind: ChatKind::Group,
            title: "Сервер".into(),
            avatar: None,
        }],
        people: vec![PersonHit {
            user: UserId("u1".into()),
            name: "Борис".into(),
            avatar: None,
            chat: None,
        }],
        messages: vec![MessageHit {
            chat: ChatId("g".into()),
            chat_title: "Команда".into(),
            timestamp: ts(7),
            author_name: Some("Анна".into()),
            text: "найденный текст".into(),
        }],
    }
}

fn search_command(id: u64, query: &str) -> Command {
    Command::Search {
        id: SearchId(id),
        query: query.into(),
    }
}

/// Types into the search field: the `Search` action focuses it first.
fn type_search(cx: &mut VisualTestContext, text: &str) {
    cx.dispatch_action(Search);
    cx.simulate_input(text);
}

fn pass(cx: &mut VisualTestContext, time: Duration) {
    cx.executor().advance_clock(time);
    cx.run_until_parked();
}

fn search_status(h: &Harness, cx: &mut VisualTestContext) -> Option<StatusLine> {
    h.desktop.read_with(cx, |d, cx| {
        d.search_listing(cx).and_then(|view| view.status)
    })
}

fn search_sections(h: &Harness, cx: &mut VisualTestContext) -> Vec<&'static str> {
    h.desktop.read_with(cx, |d, cx| {
        d.search_listing(cx)
            .map(|view| view.sections.iter().map(|s| s.title).collect())
            .unwrap_or_default()
    })
}

fn online_with_chats(cx: &mut TestAppContext) -> (Harness, &mut VisualTestContext) {
    let (h, cx) = harness(cx);
    h.feed(
        cx,
        [
            ClientEvent::ChatsReplaced(search_chats()),
            ClientEvent::Connection(ConnectionState::Online),
        ],
    );
    (h, cx)
}

#[gpui::test]
fn a_short_query_sends_nothing_and_two_characters_send_one_search_after_the_pause(
    cx: &mut TestAppContext,
) {
    let (h, cx) = online_with_chats(cx);
    type_search(cx, "a");
    pass(cx, SEARCH_DEBOUNCE * 3);
    assert!(
        h.commands().is_empty(),
        "one character is below the minimum"
    );
    type_search(cx, "   ");
    pass(cx, SEARCH_DEBOUNCE * 3);
    assert!(
        h.commands().is_empty(),
        "the minimum counts the trimmed text"
    );

    // Keystrokes inside the pause restart it: only the last text is sent, once, 300 ms after the last keystroke.
    cx.simulate_keystrokes("cmd-a backspace");
    type_search(cx, "a");
    pass(cx, Duration::from_millis(200));
    type_search(cx, "b");
    pass(cx, Duration::from_millis(200));
    type_search(cx, "c");
    pass(cx, SEARCH_DEBOUNCE - Duration::from_millis(1));
    assert!(h.commands().is_empty(), "still inside the pause");
    pass(cx, Duration::from_millis(1));
    assert_eq!(h.commands(), [search_command(1, "abc")]);
    pass(cx, SEARCH_DEBOUNCE * 3);
    assert!(h.commands().is_empty(), "exactly one request");
    h.store.read_with(cx, |s, _| {
        assert_eq!(
            s.search_state().map(|s| s.status),
            Some(SearchStatus::Loading)
        )
    });
    assert_eq!(search_status(&h, cx), Some(StatusLine::Loading));
}

#[gpui::test]
fn the_query_is_sent_trimmed(cx: &mut TestAppContext) {
    let (h, cx) = online_with_chats(cx);
    type_search(cx, "  ab ");
    pass(cx, SEARCH_DEBOUNCE);
    assert_eq!(h.commands(), [search_command(1, "ab")]);
}

#[gpui::test]
fn results_list_below_the_local_rows_and_clearing_cancels_the_search(cx: &mut TestAppContext) {
    let (h, cx) = online_with_chats(cx);
    type_search(cx, "ан");
    pass(cx, SEARCH_DEBOUNCE);
    assert_eq!(h.commands(), [search_command(1, "ан")]);
    h.feed(
        cx,
        [ClientEvent::SearchResults {
            id: SearchId(1),
            hits: search_hits(),
        }],
    );
    assert!(shown(cx, row!("a")), "the local name match stays listed");
    assert!(shown(cx, "search-section-Чаты"));
    assert!(shown(cx, "search-section-Люди"));
    assert!(shown(cx, "search-section-Сообщения"));
    assert!(
        y(cx, "search-section-Сообщения") > y(cx, row!("a")),
        "server results sit below the local rows"
    );
    assert_eq!(search_sections(&h, cx), ["Чаты", "Люди", "Сообщения"]);
    assert_eq!(search_status(&h, cx), None);

    cx.simulate_keystrokes("cmd-a backspace");
    assert_eq!(
        h.commands(),
        [Command::CancelSearch],
        "an empty query cancels at once, without waiting"
    );
    assert_eq!(
        search_sections(&h, cx),
        Vec::<&str>::new(),
        "the sections are gone"
    );
    h.store
        .read_with(cx, |s, _| assert!(s.search_state().is_none()));
    pass(cx, SEARCH_DEBOUNCE * 3);
    assert!(h.commands().is_empty());
}

#[gpui::test]
fn shortening_the_query_cancels_the_search_and_a_pending_timer(cx: &mut TestAppContext) {
    let (h, cx) = online_with_chats(cx);
    type_search(cx, "ab");
    pass(cx, SEARCH_DEBOUNCE);
    assert_eq!(h.commands(), [search_command(1, "ab")]);
    h.feed(
        cx,
        [ClientEvent::SearchResults {
            id: SearchId(1),
            hits: search_hits(),
        }],
    );

    type_search(cx, "c");
    cx.simulate_keystrokes("backspace backspace");
    assert_eq!(h.commands(), [Command::CancelSearch]);
    pass(cx, SEARCH_DEBOUNCE * 3);
    assert!(
        h.commands().is_empty(),
        "the timer of the longer text never fires"
    );
    assert_eq!(search_sections(&h, cx), Vec::<&str>::new());
}

#[gpui::test]
fn offline_sends_nothing_and_says_a_connection_is_needed(cx: &mut TestAppContext) {
    let (h, cx) = harness_with(
        cx,
        vec![
            ClientEvent::ChatsReplaced(search_chats()),
            ClientEvent::Connection(ConnectionState::Offline),
        ],
        |d, cx| d.search.update(cx, |input, cx| input.set_text("анн", cx)),
    );
    pass(cx, SEARCH_DEBOUNCE * 3);
    assert!(h.commands().is_empty(), "nothing is sent while offline");
    assert_eq!(search_status(&h, cx), Some(StatusLine::Offline));
    assert!(shown(cx, "search-status"));
    assert!(shown(cx, row!("a")), "local name matches still filter");
    assert!(!shown(cx, row!("g")));
}

#[gpui::test]
fn going_online_sends_the_query_that_waited_for_a_connection(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    h.feed(
        cx,
        [
            ClientEvent::ChatsReplaced(search_chats()),
            ClientEvent::Connection(ConnectionState::Offline),
        ],
    );
    type_search(cx, "ab");
    pass(cx, SEARCH_DEBOUNCE);
    assert!(h.commands().is_empty());
    assert_eq!(search_status(&h, cx), Some(StatusLine::Offline));

    h.feed(cx, [ClientEvent::Connection(ConnectionState::Online)]);
    assert_eq!(h.commands(), [search_command(1, "ab")]);
    assert_eq!(search_status(&h, cx), Some(StatusLine::Loading));

    // A connection event while the answer is awaited, or after it, does not repeat the request.
    h.feed(cx, [ClientEvent::Connection(ConnectionState::Online)]);
    assert!(h.commands().is_empty());
}

#[gpui::test]
fn a_stale_result_after_a_newer_query_changes_nothing(cx: &mut TestAppContext) {
    let (h, cx) = online_with_chats(cx);
    type_search(cx, "ab");
    pass(cx, SEARCH_DEBOUNCE);
    type_search(cx, "c");
    pass(cx, SEARCH_DEBOUNCE);
    assert_eq!(
        h.commands(),
        [search_command(1, "ab"), search_command(2, "abc")]
    );

    h.feed(
        cx,
        [ClientEvent::SearchResults {
            id: SearchId(1),
            hits: search_hits(),
        }],
    );
    h.store.read_with(cx, |s, _| {
        let state = s.search_state().unwrap();
        assert_eq!(
            (state.id, state.status, state.hits.clone()),
            (SearchId(2), SearchStatus::Loading, SearchHits::default())
        );
    });
    assert_eq!(search_sections(&h, cx), Vec::<&str>::new());

    h.feed(
        cx,
        [ClientEvent::SearchResults {
            id: SearchId(2),
            hits: search_hits(),
        }],
    );
    assert_eq!(search_sections(&h, cx), ["Чаты", "Люди", "Сообщения"]);
}

#[gpui::test]
fn clicking_a_message_hit_opens_its_chat_without_marking_it_read(cx: &mut TestAppContext) {
    let (h, cx) = online_with_chats(cx);
    type_search(cx, "ab");
    pass(cx, SEARCH_DEBOUNCE);
    h.feed(
        cx,
        [ClientEvent::SearchResults {
            id: SearchId(1),
            hits: search_hits(),
        }],
    );
    h.commands();

    click(cx, "search-message-g-7-row");
    assert_eq!(
        h.commands(),
        [Command::OpenChat(ChatId("g".into()))],
        "opening the chat is the only effect"
    );
    h.store.read_with(cx, |s, _| {
        assert_eq!(s.selected(), Some(&ChatId("g".into())))
    });
    assert!(shown(cx, "chat-header"));
}

#[gpui::test]
fn a_failed_search_keeps_the_local_rows(cx: &mut TestAppContext) {
    let (h, cx) = online_with_chats(cx);
    type_search(cx, "ан");
    pass(cx, SEARCH_DEBOUNCE);
    h.feed(
        cx,
        [ClientEvent::SearchFailed {
            id: SearchId(1),
            reason: SearchFailure::Failed,
        }],
    );
    assert_eq!(search_status(&h, cx), Some(StatusLine::Failed));
    assert!(shown(cx, "search-status"));
    assert!(shown(cx, row!("a")), "the local match is still listed");
    assert_eq!(
        h.commands(),
        [search_command(1, "ан")],
        "a failure is not retried by itself"
    );
}

#[gpui::test]
fn every_local_match_stays_above_the_server_results(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    let chats = (0..8)
        .map(|i| {
            chat(
                &format!("c{i}"),
                ChatKind::Group,
                &format!("Чат {i}"),
                Some(i + 1),
            )
        })
        .collect();
    h.feed(
        cx,
        [
            ClientEvent::ChatsReplaced(chats),
            ClientEvent::Connection(ConnectionState::Online),
        ],
    );
    type_search(cx, "чат");
    pass(cx, SEARCH_DEBOUNCE);
    h.feed(
        cx,
        [ClientEvent::SearchResults {
            id: SearchId(1),
            hits: search_hits(),
        }],
    );
    for row in [
        row!("c0"),
        row!("c1"),
        row!("c2"),
        row!("c3"),
        row!("c4"),
        row!("c5"),
        row!("c6"),
        row!("c7"),
    ] {
        assert!(shown(cx, row), "{row} is listed");
        assert!(
            y(cx, row) < y(cx, "search-section-Сообщения"),
            "{row} is above the server sections"
        );
    }
}

#[gpui::test]
fn a_reply_arriving_while_the_next_query_waits_for_its_pause_is_dropped(cx: &mut TestAppContext) {
    let (h, cx) = online_with_chats(cx);
    type_search(cx, "ab");
    pass(cx, SEARCH_DEBOUNCE);
    h.feed(
        cx,
        [ClientEvent::SearchResults {
            id: SearchId(1),
            hits: search_hits(),
        }],
    );
    type_search(cx, "c");
    pass(cx, SEARCH_DEBOUNCE);
    assert_eq!(
        h.commands(),
        [search_command(1, "ab"), search_command(2, "abc")]
    );

    // The reply to "abc" is in flight; the text changes again and the new request is still waiting.
    type_search(cx, "d");
    let late = SearchHits {
        messages: search_hits().messages,
        ..SearchHits::default()
    };
    h.feed(
        cx,
        [ClientEvent::SearchResults {
            id: SearchId(2),
            hits: late.clone(),
        }],
    );
    h.store.read_with(cx, |s, _| {
        let state = s.search_state().unwrap();
        assert_eq!(
            (state.status, state.hits.clone()),
            (SearchStatus::Loading, search_hits()),
            "the old reply changed nothing"
        );
    });
    assert_eq!(
        search_sections(&h, cx),
        ["Чаты", "Люди", "Сообщения"],
        "what was shown stays"
    );

    pass(cx, SEARCH_DEBOUNCE);
    let commands = h.commands();
    let [Command::Search { id, query }] = commands.as_slice() else {
        panic!("one request expected: {commands:?}")
    };
    assert_eq!(query, "abcd");
    h.feed(
        cx,
        [ClientEvent::SearchResults {
            id: *id,
            hits: late,
        }],
    );
    assert_eq!(
        search_sections(&h, cx),
        ["Сообщения"],
        "the reply to the current text is shown"
    );
}

// ---- 6.3 timeline ----

fn open_group(h: &Harness, cx: &mut VisualTestContext) {
    h.feed(
        cx,
        [ClientEvent::ChatsReplaced(vec![
            chat("g", ChatKind::Group, "Команда", Some(1)),
            chat("p", ChatKind::Private, "Анна", Some(1)),
            chat("ch", ChatKind::Channel, "Новости", Some(1)),
        ])],
    );
    click(cx, row!("g"));
    h.commands();
}

#[gpui::test]
fn opens_at_newest_message(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_group(&h, cx);
    h.feed(cx, [chunk("g", 41..=100, false)]);
    assert!(shown(cx, msg!(100)));
    assert!(!shown(cx, msg!(41)), "older messages are off screen");
    assert!(y(cx, msg!(100)) > y(cx, msg!(99)));
    let list = h.list(cx);
    assert_eq!(list.item_count(), 60);
    assert_eq!(list.logical_scroll_top().item_ix, 60, "pinned to the end");
}

#[gpui::test]
fn reaching_the_top_loads_older_and_keeps_the_anchor(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_group(&h, cx);
    h.feed(cx, [chunk("g", 41..=100, false)]);
    assert!(
        h.commands().is_empty(),
        "no loading while the top is off screen"
    );

    scroll_to_top(&h, cx);
    assert_eq!(
        h.commands(),
        [Command::LoadOlder {
            chat: ChatId("g".into()),
            before: ts(41)
        }]
    );
    assert!(shown(cx, "timeline-loading"));
    assert!(loading_older(&h, cx, "g"));
    // Staying at the top must not request the same page again.
    scroll(cx, 10.);
    assert!(h.commands().is_empty());

    h.feed(cx, [chunk("g", 21..=40, false)]);
    assert!(!loading_older(&h, cx, "g"));
    let list = h.list(cx);
    assert_eq!(list.item_count(), 80);
    assert_eq!(
        list.logical_scroll_top().item_ix,
        20,
        "the message that was on top is still the anchor"
    );
    let anchor = list
        .bounds_for_item(20)
        .expect("anchor message is laid out");
    let top = bounds(cx, "timeline").unwrap().origin.y;
    assert_eq!(
        anchor.origin.y, top,
        "and stays at the same place on screen"
    );
    assert!(h.commands().is_empty());

    scroll_to_top(&h, cx);
    assert_eq!(
        h.commands(),
        [Command::LoadOlder {
            chat: ChatId("g".into()),
            before: ts(21)
        }]
    );
}

#[gpui::test]
fn start_of_history_stops_loading(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_group(&h, cx);
    h.feed(cx, [chunk("g", 21..=80, false)]);
    scroll_to_top(&h, cx);
    assert_eq!(h.commands().len(), 1);
    h.feed(cx, [chunk("g", 1..=20, true)]);
    assert!(!loading_older(&h, cx, "g"));

    scroll_to_top(&h, cx);
    assert!(shown(cx, msg!(1)), "first message is on screen");
    assert!(h.commands().is_empty(), "nothing left to load");
    assert!(!loading_older(&h, cx, "g"), "no loading indicator remains");
    h.store.read_with(cx, |s, _| {
        assert!(s.timeline(&ChatId("g".into())).unwrap().reached_start)
    });
}

#[gpui::test]
fn short_history_asks_for_more_until_the_start(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_group(&h, cx);
    h.feed(cx, [chunk("g", 10..=12, false)]);
    assert_eq!(
        h.commands(),
        [Command::LoadOlder {
            chat: ChatId("g".into()),
            before: ts(10)
        }],
        "everything fits on screen, so the top is already in view"
    );
    // An empty page means there is nothing older.
    h.feed(cx, [history("g", Vec::new(), false)]);
    assert!(h.commands().is_empty());
    assert!(!shown(cx, "timeline-loading"));
}

#[gpui::test]
fn author_lines_avatars_and_bubble_sides(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_group(&h, cx);
    let mut reply = message("g", 2, "alice", false, MessageBody::Text("ответ".into()));
    reply.reply_to = Some(ReplyQuote {
        author: Some("BOB".into()),
        text: "исходное".into(),
        timestamp: Some(ts(0)),
    });
    reply.forwarded_from = Some("Новости".into());
    let mut edited = message("g", 3, "bob", false, MessageBody::Text("правка".into()));
    edited.edited = true;
    h.feed(
        cx,
        [history(
            "g",
            vec![
                message(
                    "g",
                    1,
                    "alice",
                    false,
                    MessageBody::Text("первая\nвторая строка".into()),
                ),
                reply,
                edited,
                message("g", 4, "me", true, MessageBody::Text("мой ответ".into())),
                message("g", 5, "me", true, MessageBody::Deleted),
                message(
                    "g",
                    6,
                    "bob",
                    false,
                    MessageBody::Unsupported("sticker".into()),
                ),
            ],
            true,
        )],
    );

    // Group of alice's two messages: name on the first, avatar on the last.
    assert!(shown(cx, msg!(1, "author")));
    assert!(!shown(cx, msg!(2, "author")));
    assert!(!shown(cx, msg!(1, "avatar")));
    assert!(shown(cx, msg!(2, "avatar-initials")));
    // A different author starts a new group.
    assert!(shown(cx, msg!(3, "author")));
    assert!(shown(cx, msg!(3, "avatar")));
    // Own messages: no author, no avatar, right side.
    assert!(!shown(cx, msg!(4, "author")));
    assert!(!shown(cx, msg!(4, "avatar")));
    let timeline = bounds(cx, "timeline").unwrap();
    let own = bounds(cx, msg!(4)).unwrap();
    let theirs = bounds(cx, msg!(3)).unwrap();
    assert_eq!(own.right(), timeline.right() - px(16.));
    assert!(theirs.left() < own.left());
    assert!(theirs.right() < timeline.right() - px(100.));

    assert!(shown(cx, msg!(2, "reply")));
    assert!(shown(cx, msg!(2, "forwarded")));
    assert!(!shown(cx, msg!(1, "reply")) && !shown(cx, msg!(1, "forwarded")));
    assert!(shown(cx, msg!(3, "edited")));
    assert!(!shown(cx, msg!(1, "edited")));
    assert!(shown(cx, msg!(5, "deleted")));
    assert!(shown(cx, msg!(6, "unsupported")));
    assert!(
        shown(cx, msg!(1, "day")),
        "date separator before the first message of a day"
    );
    assert!(!shown(cx, msg!(2, "day")));
}

#[gpui::test]
fn long_text_wraps_inside_its_own_row(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_group(&h, cx);
    let url = format!(
        "https://gitlab.com/signax/hub/-/merge_requests/{}",
        "a1b2c3".repeat(60)
    );
    let words =
        "длинное сообщение из обычных слов, которое не помещается в одну строку пузыря ".repeat(6);
    let image = MessageBody::Image {
        media: media("m1", MediaKind::Thumbnail),
        width: 800,
        height: 600,
        caption: Some(words.clone()),
        name: None,
    };
    h.feed(
        cx,
        [history(
            "g",
            vec![
                message("g", 1, "alice", false, MessageBody::Text(url)),
                message("g", 2, "bob", false, MessageBody::Text(words)),
                message("g", 3, "alice", false, image),
                message("g", 4, "bob", false, MessageBody::Text("после".into())),
            ],
            true,
        )],
    );

    let bubbles = [msg!(1), msg!(2), msg!(3), msg!(4)]
        .map(|s| bounds(cx, s).unwrap_or_else(|| panic!("{s} on screen")));
    for pair in bubbles.windows(2) {
        assert!(
            pair[0].bottom() <= pair[1].top(),
            "bubble {:?} overlaps the next one {:?}",
            pair[0],
            pair[1]
        );
    }
    assert!(
        bubbles[0].size.height > px(80.),
        "an unbreakable URL wraps over several lines: {:?}",
        bubbles[0]
    );
    assert!(
        bubbles[1].size.height > px(80.),
        "long text wraps at word boundaries: {:?}",
        bubbles[1]
    );
    assert!(
        bubbles[0].size.width <= px(480.),
        "the URL bubble keeps the max width: {:?}",
        bubbles[0]
    );
    assert!(
        bubbles[3].size.width < px(120.),
        "a short message keeps a narrow bubble: {:?}",
        bubbles[3]
    );
}

#[gpui::test]
fn private_chats_and_channels_do_not_label_authors(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_group(&h, cx);

    click(cx, row!("p"));
    h.feed(
        cx,
        [history(
            "p",
            vec![message(
                "p",
                11,
                "alice",
                false,
                MessageBody::Text("привет".into()),
            )],
            true,
        )],
    );
    assert!(shown(cx, msg!(11)));
    assert!(!shown(cx, msg!(11, "author")));
    assert!(!shown(cx, msg!(11, "avatar")));

    click(cx, row!("ch"));
    h.feed(
        cx,
        [history(
            "ch",
            vec![message(
                "ch",
                12,
                "alice",
                false,
                MessageBody::Text("новость".into()),
            )],
            true,
        )],
    );
    assert!(shown(cx, msg!(12)));
    assert!(!shown(cx, msg!(12, "author")));
    assert!(!shown(cx, msg!(12, "avatar")));
}

#[gpui::test]
fn live_updates_follow_only_at_the_bottom(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_group(&h, cx);
    h.feed(cx, [chunk("g", 1..=3, true)]);

    h.feed(cx, [ClientEvent::MessageUpserted(text("g", 4))]);
    assert!(shown(cx, msg!(4)), "new message appears at the bottom");
    let mut edited = text("g", 2);
    edited.body = MessageBody::Text("новый текст".into());
    edited.edited = true;
    h.feed(cx, [ClientEvent::MessageUpserted(edited)]);
    assert!(shown(cx, msg!(2, "edited")));
    h.feed(
        cx,
        [ClientEvent::MessageDeleted {
            chat: ChatId("g".into()),
            timestamp: ts(3),
        }],
    );
    assert!(shown(cx, msg!(3, "deleted")));
    assert!(h.commands().is_empty());

    // Long timeline, scrolled away from the bottom: the view stays put.
    h.feed(cx, [chunk("g", 5..=80, true)]);
    scroll(cx, 800.);
    let list = h.list(cx);
    let before = list.logical_scroll_top();
    assert!(before.item_ix < list.item_count(), "scrolled up");
    h.feed(cx, [ClientEvent::MessageUpserted(text("g", 81))]);
    assert!(!shown(cx, msg!(81)));
    assert_eq!(h.list(cx).logical_scroll_top().item_ix, before.item_ix);
    scroll(cx, -1_000_000.);
    assert!(shown(cx, msg!(81)));
}

#[gpui::test]
fn activity_in_another_chat_updates_its_row_only(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_group(&h, cx);
    h.feed(cx, [chunk("g", 1..=3, true)]);
    let mut updated = chat("p", ChatKind::Private, "Анна", Some(50));
    updated.unread = 1;
    assert!(!shown(cx, part!("p", "badge")));
    h.feed(
        cx,
        [
            ClientEvent::MessageUpserted(text("p", 50)),
            ClientEvent::ChatUpdated(updated),
        ],
    );
    assert!(shown(cx, part!("p", "badge")));
    assert!(
        y(cx, row!("p")) < y(cx, row!("g")),
        "moved above the older chat"
    );
    assert!(!shown(cx, msg!(50)), "the open timeline is untouched");
}

// ---- 6.4 media ----

#[gpui::test]
fn image_thumbnails_keep_aspect_ratio_and_load_lazily(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_group(&h, cx);
    let landscape = media("img-wide", MediaKind::Thumbnail);
    let portrait = media("img-tall", MediaKind::Thumbnail);
    let mut with_avatar = text("g", 3);
    with_avatar.author_avatar = Some(media("av-alice", MediaKind::Avatar));
    h.feed(
        cx,
        [history(
            "g",
            vec![
                message(
                    "g",
                    1,
                    "bob",
                    false,
                    MessageBody::Image {
                        media: landscape.clone(),
                        width: 1600,
                        height: 800,
                        caption: Some("подпись".into()),
                        name: None,
                    },
                ),
                message(
                    "g",
                    2,
                    "bob",
                    true,
                    MessageBody::Image {
                        media: portrait.clone(),
                        width: 600,
                        height: 1200,
                        caption: None,
                        name: None,
                    },
                ),
                with_avatar,
                message(
                    "g",
                    4,
                    "bob",
                    false,
                    MessageBody::File {
                        media: media("doc", MediaKind::File),
                        name: "отчёт.pdf".into(),
                        size: 3_500_000,
                    },
                ),
            ],
            true,
        )],
    );

    let commands = h.commands();
    assert!(commands.contains(&Command::FetchMedia(landscape.clone())));
    assert!(commands.contains(&Command::FetchMedia(portrait.clone())));
    assert!(commands.contains(&Command::FetchMedia(media("av-alice", MediaKind::Avatar))));
    assert_eq!(commands.len(), 3, "files are not downloaded");

    let size_of = |cx: &mut VisualTestContext, selector: &'static str| {
        let b = bounds(cx, selector).unwrap_or_else(|| panic!("{selector} missing"));
        (f32::from(b.size.width), f32::from(b.size.height))
    };
    assert_eq!(
        size_of(cx, msg!(1, "image")),
        fit_thumbnail(1600, 800, 320.0)
    );
    assert_eq!(size_of(cx, msg!(1, "image")), (320.0, 160.0));
    assert_eq!(size_of(cx, msg!(2, "image")), (160.0, 320.0));
    assert!(!shown(cx, msg!(1, "image-loaded")));
    assert!(shown(cx, msg!(4, "file")));
    assert!(!shown(cx, msg!(4, "image")));

    let dir = tempfile::tempdir().unwrap();
    let write = |name: &str| {
        let path = dir.path().join(name);
        std::fs::write(&path, PNG_1X1).unwrap();
        path
    };
    h.feed(
        cx,
        [
            ClientEvent::MediaReady {
                media: landscape,
                path: write("wide.png"),
            },
            ClientEvent::MediaReady {
                media: media("av-alice", MediaKind::Avatar),
                path: write("alice.png"),
            },
        ],
    );
    assert!(shown(cx, msg!(1, "image-loaded")));
    assert!(
        !shown(cx, msg!(2, "image-loaded")),
        "portrait is still downloading"
    );
    assert_eq!(
        size_of(cx, msg!(1, "image")),
        (320.0, 160.0),
        "row size does not change when the file arrives"
    );
    assert!(shown(cx, msg!(3, "avatar-image")));
}

// ---- 6.5 connection, sign-out ----

fn indicator_shown(cx: &mut TestAppContext, state: ConnectionState) -> bool {
    let (_h, cx) = harness_with(cx, vec![ClientEvent::Connection(state)], |_, _| {});
    shown(cx, "connection-indicator")
}

#[gpui::test]
fn connection_indicator_follows_state(cx: &mut TestAppContext) {
    let (h, vcx) = harness(cx);
    assert!(shown(vcx, "connection-indicator"), "connecting at startup");
    h.feed(vcx, [ClientEvent::Connection(ConnectionState::Offline)]);
    assert!(shown(vcx, "connection-indicator"));
    let top_offline = y(vcx, "menu-button");
    h.feed(vcx, [ClientEvent::Connection(ConnectionState::Online)]);
    assert!(
        y(vcx, "menu-button") < top_offline,
        "the banner is gone and the header moved up"
    );

    assert!(indicator_shown(cx, ConnectionState::Connecting));
    assert!(indicator_shown(cx, ConnectionState::Offline));
    assert!(!indicator_shown(cx, ConnectionState::Online));
}

#[gpui::test]
fn sign_out_asks_for_confirmation_then_sends_the_command(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    h.feed(
        cx,
        [ClientEvent::ChatsReplaced(vec![chat(
            "a",
            ChatKind::Private,
            "Анна",
            Some(1),
        )])],
    );
    assert!(!shown(cx, "menu-sign-out"));
    click(cx, "menu-button");
    assert!(shown(cx, "menu-sign-out"));
    click(cx, "menu-sign-out");
    assert!(shown(cx, "sign-out-dialog"));
    assert!(h.commands().is_empty(), "confirmation first");

    click(cx, "cancel-sign-out");
    assert_eq!(h.desktop.read_with(cx, |d, _| d.menu), AccountMenu::Closed);
    assert!(h.commands().is_empty());

    click(cx, "menu-button");
    click(cx, "menu-sign-out");
    click(cx, "confirm-sign-out");
    assert_eq!(h.commands(), [Command::SignOut]);
    assert_eq!(h.desktop.read_with(cx, |d, _| d.menu), AccountMenu::Closed);

    h.feed(cx, [ClientEvent::SignedOut]);
    assert!(shown(cx, "signed-out"));
    h.store.read_with(cx, |s, _| assert_eq!(s.chat_count(), 0));
}

#[gpui::test]
fn expired_session_blocks_the_window_and_offers_sign_out(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    h.feed(
        cx,
        [ClientEvent::ChatsReplaced(vec![chat(
            "a",
            ChatKind::Private,
            "Анна",
            Some(1),
        )])],
    );
    h.feed(cx, [ClientEvent::SessionExpired]);
    assert!(shown(cx, "session-expired"));
    click(cx, row!("a"));
    assert!(
        h.commands().is_empty(),
        "the overlay swallows clicks on the list"
    );
    click(cx, "expired-sign-out");
    assert_eq!(h.commands(), [Command::SignOut]);
}

fn log_line(text: &str) -> LogLine {
    LogLine {
        time_us: ts(1_000),
        level: tracing::Level::WARN,
        target: "yandex_proto::xiva".into(),
        text: text.into(),
    }
}

#[gpui::test]
fn log_window_opens_by_shortcut_follows_new_records_and_copies(cx: &mut TestAppContext) {
    let buffer = LogBuffer::default();
    buffer.push(log_line("disconnected reason=closed"));
    cx.update(|cx| {
        cx.set_global(buffer.clone());
        cx.bind_keys([KeyBinding::new(
            "cmd-shift-l",
            ToggleLog,
            Some("BlazingTelesram"),
        )]);
    });
    let (h, cx) = harness(cx);
    let text = |cx: &mut VisualTestContext| h.desktop.read_with(cx, |d, _| d.log_text());
    assert_eq!(text(cx), None);

    cx.simulate_keystrokes("cmd-shift-l");
    assert!(shown(cx, "log-view"));
    assert_eq!(
        text(cx).unwrap(),
        "00:00:00.001 WARN  yandex_proto::xiva disconnected reason=closed\n"
    );

    buffer.push(log_line("connecting"));
    cx.executor().advance_clock(Duration::from_millis(300));
    cx.run_until_parked();
    assert!(
        text(cx)
            .unwrap()
            .ends_with("yandex_proto::xiva connecting\n"),
        "picked up without reopening"
    );

    click(cx, "log-copy");
    let copied = cx.read_from_clipboard().and_then(|item| item.text());
    assert_eq!(copied, text(cx));

    click(cx, "log-close");
    assert_eq!(text(cx), None);
}

// ---- add-sending helpers ----

fn chat_g() -> ChatId {
    ChatId("g".into())
}

fn own(chat_id: &str, i: i64, body: &str) -> Message {
    message(chat_id, i, "me", true, MessageBody::Text(body.into()))
}

fn pending(chat_id: &str, i: i64, payload: &str, body: &str) -> Message {
    let mut m = own(chat_id, i, body);
    m.delivery = Delivery::Pending(PayloadId(payload.into()));
    m
}

fn failed(chat_id: &str, i: i64, payload: &str, body: &str, reason: &str) -> Message {
    let mut m = own(chat_id, i, body);
    m.delivery = Delivery::Failed {
        payload: PayloadId(payload.into()),
        reason: reason.into(),
    };
    m
}

fn send_command(body: &str, reply_to: Option<ReplyTarget>) -> Command {
    Command::Send {
        chat: chat_g(),
        text: body.into(),
        reply_to,
    }
}

/// Everything the UI asked of the core except typing notices, which every composer edit produces.
fn writes(h: &Harness) -> Vec<Command> {
    h.commands()
        .into_iter()
        .filter(|c| !matches!(c, Command::Typing(_)))
        .collect()
}

/// Opens the group chat "g" and loads `messages` as its newest chunk; pending commands are dropped.
fn open_with(h: &Harness, cx: &mut VisualTestContext, messages: Vec<Message>) {
    open_group(h, cx);
    h.feed(cx, [history("g", messages, true)]);
    h.commands();
}

fn composer_text(h: &Harness, cx: &mut VisualTestContext) -> String {
    h.desktop
        .read_with(cx, |d, cx| d.composer_input().read(cx).content.to_string())
}

fn composer_reply(h: &Harness, cx: &mut VisualTestContext) -> Option<ReplyTarget> {
    h.desktop.read_with(cx, |d, _| d.composer_reply().cloned())
}

fn composer_cursor(h: &Harness, cx: &mut VisualTestContext) -> usize {
    h.desktop
        .read_with(cx, |d, cx| d.composer_input().read(cx).cursor())
}

fn composer_height(cx: &mut VisualTestContext) -> Pixels {
    bounds(cx, "composer-input")
        .expect("composer is on screen")
        .size
        .height
}

fn right_click(cx: &mut VisualTestContext, selector: &'static str) {
    let b = bounds(cx, selector).unwrap_or_else(|| panic!("{selector} is not on screen"));
    cx.simulate_mouse_down(b.center(), MouseButton::Right, Modifiers::none());
    cx.simulate_mouse_up(b.center(), MouseButton::Right, Modifiers::none());
}

fn activate(cx: &mut VisualTestContext) {
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
}

fn stamps(h: &Harness, cx: &mut VisualTestContext) -> Vec<i64> {
    h.store.read_with(cx, |s, _| {
        s.timeline(&chat_g())
            .unwrap()
            .messages
            .iter()
            .map(|m| m.timestamp)
            .collect()
    })
}

fn delivery_of(h: &Harness, cx: &mut VisualTestContext, i: i64) -> Delivery {
    h.store.read_with(cx, |s, _| {
        s.timeline(&chat_g())
            .unwrap()
            .messages
            .iter()
            .find(|m| m.timestamp == ts(i))
            .unwrap()
            .delivery
            .clone()
    })
}

fn reply_target_2() -> ReplyTarget {
    ReplyTarget {
        timestamp: ts(2),
        author: Some("ALICE".into()),
        text: "сообщение 2".into(),
    }
}

fn choose_reply_to_2(cx: &mut VisualTestContext) {
    right_click(cx, msg!(2));
    click(cx, "menu-reply");
}

// ---- 5.1 composer ----

#[gpui::test]
fn enter_sends_the_typed_text_and_empties_the_composer(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1)]);
    cx.simulate_input("привет");
    assert_eq!(composer_text(&h, cx), "привет");
    cx.simulate_keystrokes("enter");
    assert_eq!(writes(&h), [send_command("привет", None)]);
    assert_eq!(composer_text(&h, cx), "");
}

#[gpui::test]
fn the_send_button_sends_like_enter(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1)]);
    click(cx, "composer-send");
    assert!(writes(&h).is_empty(), "nothing to send yet");
    cx.simulate_input("привет");
    click(cx, "composer-send");
    assert_eq!(writes(&h), [send_command("привет", None)]);
    assert_eq!(composer_text(&h, cx), "");
}

#[gpui::test]
fn shift_enter_inserts_a_line_break_and_sends_nothing(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1)]);
    cx.simulate_input("раз");
    cx.simulate_keystrokes("shift-enter");
    cx.simulate_input("два");
    assert_eq!(composer_text(&h, cx), "раз\nдва");
    assert!(writes(&h).is_empty());
    cx.simulate_keystrokes("enter");
    assert_eq!(
        writes(&h),
        [send_command("раз\nдва", None)],
        "line breaks inside the text are kept"
    );
}

#[gpui::test]
fn blank_text_is_not_sent_and_the_rest_is_trimmed(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1)]);
    cx.simulate_input("   ");
    cx.simulate_keystrokes("shift-enter enter");
    assert!(writes(&h).is_empty(), "spaces and line breaks only");
    assert_eq!(composer_text(&h, cx), "   \n");

    cx.simulate_keystrokes("cmd-a backspace");
    cx.simulate_input("  привет \n");
    cx.simulate_keystrokes("enter");
    assert_eq!(writes(&h), [send_command("привет", None)]);
}

#[gpui::test]
fn composer_wraps_and_grows_up_to_eight_rows(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1)]);
    let one_row = composer_height(cx);
    assert_eq!(one_row, px(22.));

    cx.simulate_input(&"слово ".repeat(60));
    cx.run_until_parked();
    let wrapped = composer_height(cx);
    assert!(
        wrapped > one_row && wrapped < px(22. * 8.),
        "wrapped text takes a few rows: {wrapped:?}"
    );
    assert_eq!(f32::from(wrapped) % 22., 0., "whole rows");

    cx.simulate_keystrokes("cmd-a backspace");
    cx.simulate_input("x");
    for _ in 0..19 {
        cx.simulate_keystrokes("shift-enter");
        cx.simulate_input("x");
    }
    cx.run_until_parked();
    assert_eq!(
        composer_height(cx),
        px(22. * 8.),
        "twenty rows are capped and scroll inside"
    );

    // Backspace until empty: the field shrinks back to one row.
    cx.simulate_keystrokes("cmd-a backspace");
    cx.run_until_parked();
    assert_eq!(composer_height(cx), one_row);
}

#[gpui::test]
fn clicks_and_vertical_keys_move_the_caret_between_rows(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1)]);
    h.desktop.update(cx, |d, cx| {
        d.composer_input()
            .update(cx, |input, cx| input.set_text("aaa\naa", cx))
    });
    cx.run_until_parked();
    assert_eq!(composer_cursor(&h, cx), 6);

    cx.simulate_keystrokes("up");
    assert_eq!(composer_cursor(&h, cx), 2, "same column one row up");
    cx.simulate_keystrokes("down");
    assert_eq!(composer_cursor(&h, cx), 6);
    cx.simulate_keystrokes("home");
    assert_eq!(composer_cursor(&h, cx), 4, "row start");
    cx.simulate_keystrokes("up end");
    assert_eq!(composer_cursor(&h, cx), 3, "row end");

    let field = bounds(cx, "composer-input").unwrap();
    cx.simulate_click(
        point(field.left() + px(1.), field.top() + px(22. + 11.)),
        Modifiers::none(),
    );
    assert_eq!(
        composer_cursor(&h, cx),
        4,
        "a click on the second row lands in it"
    );
    cx.simulate_click(
        point(field.right() - px(1.), field.top() + px(11.)),
        Modifiers::none(),
    );
    assert_eq!(
        composer_cursor(&h, cx),
        3,
        "a click past the end of the first row lands at its end"
    );
}

// ---- 5.2 delivery states ----

#[gpui::test]
fn a_sent_message_is_pending_then_confirmed_exactly_once(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1), text("g", 2)]);
    h.feed(
        cx,
        [ClientEvent::MessageUpserted(pending(
            "g",
            50,
            "p1",
            "привет",
        ))],
    );
    assert!(shown(cx, msg!(50, "pending")));
    assert!(
        y(cx, msg!(50)) > y(cx, msg!(2)),
        "at the bottom of the timeline at once"
    );
    assert!(h.commands().is_empty());

    let settled = ClientEvent::PendingSettled {
        chat: chat_g(),
        payload: PayloadId("p1".into()),
    };
    let server = ClientEvent::MessageUpserted(own("g", 60, "привет"));
    h.feed(cx, [settled.clone(), server.clone()]);
    assert!(shown(cx, msg!(60, "sent")));
    assert_eq!(
        stamps(&h, cx),
        [1, 2, 60],
        "one copy: the pending one is gone"
    );

    h.feed(cx, [settled, server]);
    assert_eq!(
        stamps(&h, cx),
        [1, 2, 60],
        "a repeated signal changes nothing"
    );
}

#[gpui::test]
fn the_echo_may_come_first_and_still_leaves_one_copy(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1)]);
    h.feed(
        cx,
        [ClientEvent::MessageUpserted(pending(
            "g",
            50,
            "p1",
            "привет",
        ))],
    );
    h.feed(
        cx,
        [
            ClientEvent::PendingSettled {
                chat: chat_g(),
                payload: PayloadId("p1".into()),
            },
            ClientEvent::MessageUpserted(own("g", 40, "привет")),
        ],
    );
    assert_eq!(
        stamps(&h, cx),
        [1, 40],
        "the server copy may sort before the local time"
    );
    assert_eq!(delivery_of(&h, cx, 40), Delivery::Sent);
    assert!(shown(cx, msg!(40, "sent")));
}

#[gpui::test]
fn a_new_pending_message_scrolls_into_view(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, (1..=80).map(|i| text("g", i)).collect());
    scroll(cx, 800.);
    let list = h.list(cx);
    assert!(
        list.logical_scroll_top().item_ix < list.item_count(),
        "scrolled up"
    );
    h.feed(
        cx,
        [ClientEvent::MessageUpserted(pending(
            "g",
            90,
            "p1",
            "привет",
        ))],
    );
    assert!(
        shown(cx, msg!(90, "pending")),
        "the user's own message is brought into view"
    );
}

#[gpui::test]
fn a_rejected_message_shows_the_reason_and_retry_sends_it_again(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1)]);
    h.feed(
        cx,
        [ClientEvent::MessageUpserted(pending(
            "g",
            50,
            "p1",
            "привет",
        ))],
    );
    h.feed(
        cx,
        [ClientEvent::MessageUpserted(failed(
            "g",
            50,
            "p1",
            "привет",
            "нет прав",
        ))],
    );
    assert!(shown(cx, msg!(50, "failed")));
    assert!(shown(cx, msg!(50, "failed-reason")));
    assert_eq!(
        stamps(&h, cx),
        [1, 50],
        "the failed message replaces the pending one in place"
    );

    click(cx, msg!(50, "retry"));
    assert_eq!(h.commands(), [Command::Retry(PayloadId("p1".into()))]);
}

// ---- 5.3 reply ----

#[gpui::test]
fn reply_quotes_the_chosen_message_in_the_sent_one(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1), text("g", 2)]);
    choose_reply_to_2(cx);
    assert!(shown(cx, "reply-bar"));
    assert_eq!(composer_reply(&h, cx), Some(reply_target_2()));

    cx.simulate_input("ответ");
    cx.simulate_keystrokes("enter");
    assert_eq!(writes(&h), [send_command("ответ", Some(reply_target_2()))]);
    assert_eq!(
        composer_reply(&h, cx),
        None,
        "the bar goes away once the message is sent"
    );
}

#[gpui::test]
fn escape_and_the_close_button_cancel_the_reply(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1), text("g", 2)]);
    choose_reply_to_2(cx);
    cx.simulate_keystrokes("escape");
    assert_eq!(composer_reply(&h, cx), None);
    cx.simulate_input("без цитаты");
    cx.simulate_keystrokes("enter");
    assert_eq!(writes(&h), [send_command("без цитаты", None)]);

    choose_reply_to_2(cx);
    click(cx, "reply-bar-close");
    assert_eq!(composer_reply(&h, cx), None);
    cx.simulate_input("после закрытия");
    cx.simulate_keystrokes("enter");
    assert_eq!(
        writes(&h),
        [send_command("после закрытия", None)],
        "the composer keeps focus after the click"
    );
}

// ---- 5.4 drafts ----

#[gpui::test]
fn text_and_reply_target_are_restored_after_a_chat_switch(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1), text("g", 2)]);
    choose_reply_to_2(cx);
    cx.simulate_input("черновик");

    click(cx, row!("p"));
    let commands = h.commands();
    let draft = Draft {
        chat: chat_g(),
        text: "черновик".into(),
        reply_to: Some(reply_target_2()),
    };
    assert!(
        commands.contains(&Command::SaveDraft(draft)),
        "saved when the chat is left: {commands:?}"
    );
    assert_eq!(
        composer_text(&h, cx),
        "",
        "the other chat starts with an empty composer"
    );
    assert_eq!(composer_reply(&h, cx), None);

    click(cx, row!("g"));
    assert_eq!(composer_text(&h, cx), "черновик");
    assert_eq!(composer_reply(&h, cx), Some(reply_target_2()));
}

#[gpui::test]
fn a_chat_with_a_draft_shows_it_instead_of_the_last_message(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1)]);
    cx.simulate_input("привет\nмир");
    click(cx, row!("p"));
    assert!(shown(cx, part!("g", "draft")));
    h.store.read_with(cx, |s, _| {
        let summary = s.chat(&chat_g()).unwrap();
        assert_eq!(
            chat_preview(s, summary),
            Preview::Draft("привет мир".into())
        );
    });

    click(cx, row!("g"));
    cx.simulate_keystrokes("cmd-a backspace");
    click(cx, row!("p"));
    h.store.read_with(cx, |s, _| {
        let summary = s.chat(&chat_g()).unwrap();
        assert_eq!(
            chat_preview(s, summary),
            Preview::Message("превью 1".into()),
            "an emptied draft is gone"
        );
    });
}

#[gpui::test]
fn a_draft_is_saved_one_second_after_the_last_keystroke(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1)]);
    let saved = |h: &Harness| {
        writes(h).into_iter().find_map(|c| {
            if let Command::SaveDraft(d) = c {
                Some(d.text)
            } else {
                None
            }
        })
    };

    cx.simulate_input("а");
    cx.executor().advance_clock(Duration::from_millis(900));
    cx.run_until_parked();
    assert_eq!(saved(&h), None);
    cx.simulate_input("б");
    cx.executor().advance_clock(Duration::from_millis(900));
    cx.run_until_parked();
    assert_eq!(saved(&h), None, "every keystroke restarts the wait");
    cx.executor().advance_clock(Duration::from_millis(200));
    cx.run_until_parked();
    assert_eq!(saved(&h), Some("аб".into()));
}

#[gpui::test]
fn sending_discards_the_draft_without_a_second_command(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1)]);
    cx.simulate_input("привет");
    cx.executor().advance_clock(Duration::from_secs(2));
    cx.run_until_parked();
    h.commands();
    cx.simulate_keystrokes("enter");
    assert_eq!(writes(&h), [send_command("привет", None)]);
    h.store
        .read_with(cx, |s, _| assert!(s.draft(&chat_g()).is_none()));
    cx.executor().advance_clock(Duration::from_secs(2));
    cx.run_until_parked();
    assert!(
        writes(&h).is_empty(),
        "the core deletes the draft when it takes the message"
    );
}

#[gpui::test]
fn persisted_drafts_fill_the_composer_and_the_chat_row(cx: &mut TestAppContext) {
    let draft = Draft {
        chat: chat_g(),
        text: "из прошлого".into(),
        reply_to: None,
    };
    let events = vec![
        ClientEvent::ChatsReplaced(vec![chat("g", ChatKind::Group, "Команда", Some(1))]),
        ClientEvent::Drafts(vec![draft]),
    ];
    let (h, cx) = harness_with(cx, events, |_, _| {});
    assert!(shown(cx, part!("g", "draft")));
    click(cx, row!("g"));
    assert_eq!(composer_text(&h, cx), "из прошлого");
}

#[gpui::test]
fn a_draft_that_arrives_late_fills_an_untouched_composer_only(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1)]);
    let draft = |text: &str| {
        ClientEvent::Drafts(vec![Draft {
            chat: chat_g(),
            text: text.into(),
            reply_to: None,
        }])
    };
    h.feed(cx, [draft("пришёл позже")]);
    assert_eq!(composer_text(&h, cx), "пришёл позже");

    cx.simulate_keystrokes("cmd-a backspace");
    cx.simulate_input("мой текст");
    h.feed(cx, [draft("чужой")]);
    assert_eq!(
        composer_text(&h, cx),
        "мой текст",
        "typed text is never overwritten"
    );
}

// ---- 5.5 message menu, edit, delete ----

#[gpui::test]
fn own_message_menu_offers_reply_edit_copy_and_delete(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![own("g", 1, "мой")]);
    right_click(cx, msg!(1));
    for item in ["menu-reply", "menu-copy", "menu-edit", "menu-delete"] {
        assert!(shown(cx, item), "{item}");
    }
}

#[gpui::test]
fn other_users_message_menu_offers_reply_and_copy_only(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1)]);
    right_click(cx, msg!(1));
    assert!(shown(cx, "menu-reply") && shown(cx, "menu-copy"));
    assert!(!shown(cx, "menu-edit") && !shown(cx, "menu-delete"));
}

#[gpui::test]
fn a_pending_message_menu_offers_copy_only(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![pending("g", 5, "p1", "жду")]);
    right_click(cx, msg!(5));
    assert!(shown(cx, "menu-copy"));
    assert!(!shown(cx, "menu-reply") && !shown(cx, "menu-edit") && !shown(cx, "menu-delete"));
}

#[gpui::test]
fn copy_text_puts_the_message_on_the_clipboard(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1)]);
    right_click(cx, msg!(1));
    click(cx, "menu-copy");
    assert_eq!(
        cx.read_from_clipboard().and_then(|item| item.text()),
        Some("сообщение 1".into())
    );
    assert!(!h.desktop.read_with(cx, |d, _| d.menu_is_open()));
}

#[gpui::test]
fn escape_and_an_outside_click_close_the_menu(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1)]);
    let open =
        |h: &Harness, cx: &mut VisualTestContext| h.desktop.read_with(cx, |d, _| d.menu_is_open());
    right_click(cx, msg!(1));
    assert!(open(&h, cx));
    cx.simulate_keystrokes("escape");
    assert!(!open(&h, cx));

    right_click(cx, msg!(1));
    assert!(open(&h, cx));
    click(cx, "chat-header");
    assert!(!open(&h, cx));
    assert!(h.commands().is_empty(), "closing the menu changes nothing");
}

#[gpui::test]
fn edit_loads_the_message_and_enter_saves_the_new_text(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1), own("g", 2, "старый")]);
    right_click(cx, msg!(2));
    click(cx, "menu-edit");
    assert!(shown(cx, "edit-bar"));
    assert_eq!(
        h.desktop.read_with(cx, |d, _| d.composer_editing()),
        Some(ts(2))
    );
    assert_eq!(composer_text(&h, cx), "старый");

    cx.simulate_keystrokes("cmd-a");
    cx.simulate_input("новый");
    assert!(h.commands().is_empty(), "no typing notice while editing");
    cx.simulate_keystrokes("enter");
    assert_eq!(
        h.commands(),
        [Command::Edit {
            chat: chat_g(),
            timestamp: ts(2),
            text: "новый".into()
        }]
    );
    assert_eq!(h.desktop.read_with(cx, |d, _| d.composer_editing()), None);
    assert_eq!(composer_text(&h, cx), "");

    let mut edited = own("g", 2, "новый");
    edited.edited = true;
    h.feed(cx, [ClientEvent::MessageUpserted(edited)]);
    assert!(shown(cx, msg!(2, "edited")));
}

#[gpui::test]
fn saving_an_unchanged_text_sends_no_edit(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![own("g", 2, "как было")]);
    right_click(cx, msg!(2));
    click(cx, "menu-edit");
    cx.simulate_keystrokes("enter");
    assert!(h.commands().is_empty());
    assert_eq!(h.desktop.read_with(cx, |d, _| d.composer_editing()), None);
}

#[gpui::test]
fn up_in_an_empty_composer_edits_the_newest_own_sent_message(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    let messages = vec![
        own("g", 2, "первое"),
        text("g", 3),
        own("g", 4, "второе"),
        pending("g", 9, "p1", "жду"),
    ];
    open_with(&h, cx, messages);
    cx.simulate_keystrokes("up");
    assert_eq!(
        h.desktop.read_with(cx, |d, _| d.composer_editing()),
        Some(ts(4)),
        "pending messages are skipped"
    );
    assert_eq!(composer_text(&h, cx), "второе");
    assert!(shown(cx, "edit-bar"));

    cx.simulate_keystrokes("escape");
    cx.simulate_input("а");
    cx.simulate_keystrokes("up");
    assert_eq!(
        h.desktop.read_with(cx, |d, _| d.composer_editing()),
        None,
        "with text in the composer Up only moves the caret"
    );
}

#[gpui::test]
fn escape_cancels_an_edit_and_brings_back_the_draft(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![own("g", 2, "старый")]);
    cx.simulate_input("черновик");
    right_click(cx, msg!(2));
    click(cx, "menu-edit");
    assert_eq!(composer_text(&h, cx), "старый");

    cx.simulate_keystrokes("escape");
    assert_eq!(composer_text(&h, cx), "черновик");
    assert_eq!(h.desktop.read_with(cx, |d, _| d.composer_editing()), None);
    assert!(
        !h.commands()
            .iter()
            .any(|c| matches!(c, Command::Edit { .. })),
        "the message stays unchanged"
    );
}

#[gpui::test]
fn delete_asks_for_confirmation_and_then_deletes(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1), own("g", 2, "мой")]);
    right_click(cx, msg!(2));
    click(cx, "menu-delete");
    assert!(shown(cx, "delete-dialog"));
    assert!(h.commands().is_empty(), "confirmation first");

    click(cx, "confirm-delete");
    assert_eq!(
        h.commands(),
        [Command::Delete {
            chat: chat_g(),
            timestamp: ts(2)
        }]
    );
    assert!(!h.desktop.read_with(cx, |d, _| d.delete_is_pending()));

    h.feed(
        cx,
        [ClientEvent::MessageDeleted {
            chat: chat_g(),
            timestamp: ts(2),
        }],
    );
    assert!(shown(cx, msg!(2, "deleted")));
}

#[gpui::test]
fn cancelling_the_confirmation_keeps_the_message(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![own("g", 2, "мой")]);
    right_click(cx, msg!(2));
    click(cx, "menu-delete");
    click(cx, "cancel-delete");
    assert!(!h.desktop.read_with(cx, |d, _| d.delete_is_pending()));
    assert!(h.commands().is_empty());
    assert_eq!(stamps(&h, cx), [2]);

    right_click(cx, msg!(2));
    click(cx, "menu-delete");
    cx.simulate_keystrokes("escape");
    assert!(
        !h.desktop.read_with(cx, |d, _| d.delete_is_pending()),
        "Escape also cancels"
    );
}

#[gpui::test]
fn a_rejected_edit_or_deletion_shows_the_reason_for_a_while(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![own("g", 2, "старый")]);
    h.feed(
        cx,
        [ClientEvent::ActionFailed {
            chat: chat_g(),
            reason: "слишком поздно".into(),
        }],
    );
    assert!(shown(cx, "toast"));
    assert_eq!(
        h.desktop
            .read_with(cx, |d, _| d.toast_text().map(str::to_string)),
        Some("слишком поздно".into())
    );
    assert_eq!(stamps(&h, cx), [2]);
    assert_eq!(
        h.store
            .read_with(cx, |s, _| s.timeline(&chat_g()).unwrap().messages[0]
                .body
                .clone()),
        MessageBody::Text("старый".into())
    );

    cx.executor().advance_clock(Duration::from_secs(6));
    cx.run_until_parked();
    assert_eq!(
        h.desktop
            .read_with(cx, |d, _| d.toast_text().map(str::to_string)),
        None
    );
}

// ---- 6.1 read markers ----

#[gpui::test]
fn the_newest_visible_message_is_reported_once_while_the_window_is_active(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_group(&h, cx);
    h.feed(cx, [chunk("g", 1..=3, true)]);
    assert!(
        h.commands().is_empty(),
        "an inactive window reports nothing"
    );

    activate(cx);
    assert_eq!(
        h.commands(),
        [Command::MarkRead {
            chat: chat_g(),
            timestamp: ts(3)
        }]
    );

    h.feed(cx, [ClientEvent::MessageUpserted(text("g", 4))]);
    assert_eq!(
        h.commands(),
        [Command::MarkRead {
            chat: chat_g(),
            timestamp: ts(4)
        }]
    );
    let mut edited = text("g", 4);
    edited.edited = true;
    h.feed(cx, [ClientEvent::MessageUpserted(edited)]);
    assert!(
        h.commands().is_empty(),
        "the same position is not reported twice"
    );

    cx.deactivate_window();
    h.feed(cx, [ClientEvent::MessageUpserted(text("g", 5))]);
    assert!(
        h.commands().is_empty(),
        "nothing is reported while the window is in the background"
    );
    activate(cx);
    assert_eq!(
        h.commands(),
        [Command::MarkRead {
            chat: chat_g(),
            timestamp: ts(5)
        }]
    );
}

#[gpui::test]
fn only_messages_in_view_are_reported_when_scrolled_up(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_group(&h, cx);
    h.feed(cx, [chunk("g", 1..=80, true)]);
    scroll(cx, 800.);
    activate(cx);
    let marks = h.commands();
    let [Command::MarkRead { timestamp, .. }] = marks.as_slice() else {
        panic!("one marker expected: {marks:?}")
    };
    assert!(
        (1..80).contains(timestamp),
        "a message above the newest one: {timestamp}"
    );

    scroll(cx, -1_000_000.);
    cx.run_until_parked();
    assert_eq!(
        h.commands(),
        [Command::MarkRead {
            chat: chat_g(),
            timestamp: ts(80)
        }]
    );
}

#[gpui::test]
fn pending_and_failed_messages_are_never_reported(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_group(&h, cx);
    h.feed(
        cx,
        [history(
            "g",
            vec![
                text("g", 1),
                pending("g", 9, "p1", "жду"),
                failed("g", 10, "p2", "не вышло", "нет прав"),
            ],
            true,
        )],
    );
    activate(cx);
    assert_eq!(
        h.commands(),
        [Command::MarkRead {
            chat: chat_g(),
            timestamp: ts(1)
        }]
    );
}

// ---- 6.2 typing ----

fn typing_event(
    cx: &mut VisualTestContext,
    chat_id: &str,
    user: &str,
    name: Option<&str>,
    secs: u64,
) -> ClientEvent {
    ClientEvent::Typing {
        chat: ChatId(chat_id.into()),
        user: UserId(user.into()),
        name: name.map(str::to_string),
        until: cx.executor().now() + Duration::from_secs(secs),
    }
}

fn typing_text(h: &Harness, cx: &mut VisualTestContext, chat_id: &str) -> Option<String> {
    h.store
        .read_with(cx, |s, _| s.typing_text(&ChatId(chat_id.into())))
}

#[gpui::test]
fn every_composer_edit_notifies_the_core(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1)]);
    cx.simulate_input("а");
    assert_eq!(h.commands(), [Command::Typing(chat_g())]);
    cx.simulate_keystrokes("backspace");
    assert_eq!(h.commands(), [Command::Typing(chat_g())]);
}

#[gpui::test]
fn typing_is_withheld_while_newer_messages_are_unseen(cx: &mut TestAppContext) {
    // The server treats an outgoing typing as reading the chat, so it must not go out over unseen messages.
    let (h, cx) = harness(cx);
    open_group(&h, cx);
    h.feed(cx, [chunk("g", 1..=80, true)]);
    scroll_to_top(&h, cx);
    // What the core reports once messages 79 and 80 arrived while the user was up here.
    let mut unseen = chat("g", ChatKind::Group, "Команда", Some(80));
    unseen.unread = 2;
    h.feed(cx, [ClientEvent::ChatUpdated(unseen)]);
    activate(cx);
    h.commands();

    cx.simulate_input("а");
    cx.simulate_keystrokes("backspace");
    assert!(h.commands().is_empty(), "no typing over unseen messages");
    assert_eq!(composer_text(&h, cx), "");

    cx.simulate_input("привет");
    cx.executor().advance_clock(Duration::from_secs(2));
    cx.run_until_parked();
    let draft = Draft {
        chat: chat_g(),
        text: "привет".into(),
        reply_to: None,
    };
    assert_eq!(
        h.commands(),
        [Command::SaveDraft(draft)],
        "the draft is still saved"
    );

    scroll(cx, -1_000_000.);
    cx.run_until_parked();
    assert_eq!(
        h.commands(),
        [Command::MarkRead {
            chat: chat_g(),
            timestamp: ts(80)
        }]
    );
    h.feed(
        cx,
        [ClientEvent::ChatUpdated(chat(
            "g",
            ChatKind::Group,
            "Команда",
            Some(80),
        ))],
    );
    cx.simulate_input("!");
    assert_eq!(
        h.commands(),
        [Command::Typing(chat_g())],
        "typing resumes once the newest message is read"
    );
}

#[gpui::test]
fn typing_shows_in_the_header_and_the_chat_row_and_then_expires(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1)]);
    let event = typing_event(cx, "g", "alice", Some("Алиса"), 5);
    h.feed(cx, [event]);
    assert!(shown(cx, "header-typing"));
    assert!(shown(cx, part!("g", "typing")));
    assert_eq!(typing_text(&h, cx, "g"), Some("Алиса печатает…".into()));

    cx.executor().advance_clock(Duration::from_secs(4));
    cx.run_until_parked();
    assert_eq!(
        typing_text(&h, cx, "g"),
        Some("Алиса печатает…".into()),
        "still typing before the timeout"
    );
    cx.executor().advance_clock(Duration::from_secs(2));
    cx.run_until_parked();
    assert_eq!(typing_text(&h, cx, "g"), None);
    h.store.read_with(cx, |s, _| {
        assert_eq!(
            chat_preview(s, s.chat(&chat_g()).unwrap()),
            Preview::Message("превью 1".into())
        );
    });
}

#[gpui::test]
fn typing_text_names_the_typists(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_group(&h, cx);
    let alice = typing_event(cx, "g", "alice", Some("Алиса"), 5);
    let bob = typing_event(cx, "g", "bob", Some("Боб"), 5);
    let nameless = typing_event(cx, "ch", "carol", None, 5);
    let private = typing_event(cx, "p", "dave", Some("Дэйв"), 5);
    h.feed(cx, [alice, bob, nameless, private]);
    assert_eq!(
        typing_text(&h, cx, "g"),
        Some("Алиса и ещё 1 печатают…".into())
    );
    assert_eq!(typing_text(&h, cx, "ch"), Some("Кто-то печатает…".into()));
    assert_eq!(
        typing_text(&h, cx, "p"),
        Some("печатает…".into()),
        "a private chat needs no name"
    );
}

#[gpui::test]
fn a_message_from_the_typist_ends_the_indicator(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1)]);
    let alice = typing_event(cx, "g", "alice", Some("Алиса"), 5);
    let bob = typing_event(cx, "g", "bob", Some("Боб"), 5);
    h.feed(cx, [alice, bob]);
    h.feed(cx, [ClientEvent::MessageUpserted(text("g", 2))]);
    assert_eq!(
        typing_text(&h, cx, "g"),
        Some("Боб печатает…".into()),
        "only alice stopped"
    );
}

#[gpui::test]
fn an_already_expired_typing_event_is_ignored(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_group(&h, cx);
    let event = typing_event(cx, "g", "alice", Some("Алиса"), 0);
    h.feed(cx, [event]);
    assert_eq!(typing_text(&h, cx, "g"), None);
}

// ---- review fixes: overlays, refused sends, read markers, editable rules ----

/// Installs the log buffer and the Cmd+Shift+L binding the real app has.
fn with_log_shortcut(cx: &mut TestAppContext) {
    cx.update(|cx| {
        cx.set_global(LogBuffer::default());
        cx.bind_keys([KeyBinding::new(
            "cmd-shift-l",
            ToggleLog,
            Some("BlazingTelesram"),
        )]);
    });
}

fn log_is_open(h: &Harness, cx: &mut VisualTestContext) -> bool {
    h.desktop.read_with(cx, |d, _| d.log_text().is_some())
}

#[gpui::test]
fn the_log_view_takes_the_keyboard_from_the_composer(cx: &mut TestAppContext) {
    with_log_shortcut(cx);
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1)]);
    cx.simulate_input("привет");
    h.commands();

    cx.simulate_keystrokes("cmd-shift-l");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("ещё");
    assert!(
        h.commands().is_empty(),
        "neither a send nor a typing notice while the log is open"
    );
    assert_eq!(composer_text(&h, cx), "привет");

    click(cx, "log-close");
    cx.simulate_input("!");
    assert_eq!(
        composer_text(&h, cx),
        "привет!",
        "the composer has the keyboard again"
    );
    assert_eq!(h.commands(), [Command::Typing(chat_g())]);
}

#[gpui::test]
fn escape_closes_the_log_before_touching_the_reply(cx: &mut TestAppContext) {
    with_log_shortcut(cx);
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1), text("g", 2)]);
    choose_reply_to_2(cx);
    cx.simulate_keystrokes("cmd-shift-l");
    assert!(log_is_open(&h, cx));
    cx.simulate_keystrokes("escape");
    assert!(!log_is_open(&h, cx));
    assert_eq!(composer_reply(&h, cx), Some(reply_target_2()));
}

#[gpui::test]
fn confirmation_dialogs_take_the_keyboard_from_the_composer(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![own("g", 2, "мой")]);
    cx.simulate_input("привет");
    h.commands();

    right_click(cx, msg!(2));
    click(cx, "menu-delete");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("ещё");
    assert!(h.commands().is_empty());
    assert_eq!(composer_text(&h, cx), "привет");
    click(cx, "cancel-delete");
    cx.simulate_input("!");
    assert_eq!(composer_text(&h, cx), "привет!");
    h.commands();

    click(cx, "menu-button");
    click(cx, "menu-sign-out");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("ещё");
    assert!(h.commands().is_empty());
    assert_eq!(composer_text(&h, cx), "привет!");
    click(cx, "cancel-sign-out");
    cx.simulate_input("?");
    assert_eq!(composer_text(&h, cx), "привет!?");
}

fn refused(body: &str, reply_to: Option<ReplyTarget>) -> ClientEvent {
    ClientEvent::SendRefused {
        draft: Draft {
            chat: chat_g(),
            text: body.into(),
            reply_to,
        },
        reason: "диск переполнен".into(),
    }
}

fn toast(h: &Harness, cx: &mut VisualTestContext) -> Option<String> {
    h.desktop
        .read_with(cx, |d, _| d.toast_text().map(str::to_string))
}

#[gpui::test]
fn a_refused_send_puts_text_and_reply_back_in_an_empty_composer(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1), text("g", 2)]);
    choose_reply_to_2(cx);
    cx.simulate_input("привет");
    cx.simulate_keystrokes("enter");
    assert_eq!(writes(&h), [send_command("привет", Some(reply_target_2()))]);
    assert_eq!(composer_text(&h, cx), "");
    assert_eq!(composer_reply(&h, cx), None);

    h.feed(cx, [refused("привет", Some(reply_target_2()))]);
    assert_eq!(composer_text(&h, cx), "привет");
    assert_eq!(composer_reply(&h, cx), Some(reply_target_2()));
    assert_eq!(toast(&h, cx), Some("диск переполнен".into()));
}

#[gpui::test]
fn a_refused_send_goes_in_front_of_what_was_typed_since(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1), text("g", 2)]);
    choose_reply_to_2(cx);
    cx.simulate_input("первое");
    cx.simulate_keystrokes("enter");
    h.commands();
    cx.simulate_input("второе");
    h.commands();

    h.feed(cx, [refused("первое", Some(reply_target_2()))]);
    assert_eq!(composer_text(&h, cx), "первое\nвторое");
    assert_eq!(
        composer_reply(&h, cx),
        Some(reply_target_2()),
        "no reply was chosen since, so the refused one returns"
    );
    assert_eq!(toast(&h, cx), Some("диск переполнен".into()));
    cx.executor().advance_clock(Duration::from_secs(2));
    cx.run_until_parked();
    let draft = Draft {
        chat: chat_g(),
        text: "первое\nвторое".into(),
        reply_to: Some(reply_target_2()),
    };
    assert_eq!(
        writes(&h),
        [Command::SaveDraft(draft)],
        "both texts survive the autosave"
    );
}

#[gpui::test]
fn a_refused_send_keeps_the_reply_target_chosen_since(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1), text("g", 2)]);
    cx.simulate_input("первое");
    cx.simulate_keystrokes("enter");
    choose_reply_to_2(cx);
    cx.simulate_input("второе");

    let other = ReplyTarget {
        timestamp: ts(1),
        author: Some("ALICE".into()),
        text: "сообщение 1".into(),
    };
    h.feed(cx, [refused("первое", Some(other))]);
    assert_eq!(composer_text(&h, cx), "первое\nвторое");
    assert_eq!(composer_reply(&h, cx), Some(reply_target_2()));
}

#[gpui::test]
fn a_refused_send_joins_the_saved_draft_of_a_chat_that_is_not_open(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1)]);
    cx.simulate_input("первое");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("второе");
    click(cx, row!("p"));
    h.commands();

    h.feed(cx, [refused("первое", None)]);
    let draft = Draft {
        chat: chat_g(),
        text: "первое\nвторое".into(),
        reply_to: None,
    };
    h.store
        .read_with(cx, |s, _| assert_eq!(s.draft(&chat_g()), Some(&draft)));
    assert_eq!(writes(&h), [Command::SaveDraft(draft)]);
    assert_eq!(composer_text(&h, cx), "", "the open chat is untouched");

    click(cx, row!("g"));
    assert_eq!(composer_text(&h, cx), "первое\nвторое");
}

#[gpui::test]
fn a_refused_send_for_a_chat_that_is_not_open_becomes_its_draft(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1), text("g", 2)]);
    cx.simulate_input("первое");
    cx.simulate_keystrokes("enter");
    click(cx, row!("p"));
    h.commands();

    h.feed(cx, [refused("первое", Some(reply_target_2()))]);
    let draft = Draft {
        chat: chat_g(),
        text: "первое".into(),
        reply_to: Some(reply_target_2()),
    };
    assert_eq!(writes(&h), [Command::SaveDraft(draft)]);
}

#[gpui::test]
fn a_refused_send_during_an_edit_comes_back_when_the_edit_ends(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![own("g", 2, "старый")]);
    cx.simulate_input("первое");
    cx.simulate_keystrokes("enter");
    cx.simulate_keystrokes("up");
    assert_eq!(
        h.desktop.read_with(cx, |d, _| d.composer_editing()),
        Some(ts(2))
    );

    h.feed(cx, [refused("первое", None)]);
    assert_eq!(composer_text(&h, cx), "старый", "the edit is not disturbed");
    cx.simulate_keystrokes("escape");
    assert_eq!(composer_text(&h, cx), "первое");
}

#[gpui::test]
fn a_refused_send_during_an_edit_joins_the_draft_saved_before_it(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![own("g", 2, "старый")]);
    cx.simulate_input("второе");
    right_click(cx, msg!(2));
    click(cx, "menu-edit");
    assert_eq!(
        h.desktop.read_with(cx, |d, _| d.composer_editing()),
        Some(ts(2))
    );

    h.feed(cx, [refused("первое", None)]);
    assert_eq!(composer_text(&h, cx), "старый", "the edit is not disturbed");
    cx.simulate_keystrokes("escape");
    assert_eq!(composer_text(&h, cx), "первое\nвторое");
}

#[gpui::test]
fn a_refused_send_during_an_edit_saves_the_merged_pre_edit_draft_at_once(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![own("g", 2, "старый")]);
    cx.simulate_input("второе");
    right_click(cx, msg!(2));
    click(cx, "menu-edit");
    assert_eq!(
        h.desktop.read_with(cx, |d, _| d.composer_editing()),
        Some(ts(2))
    );
    h.commands();

    h.feed(cx, [refused("первое", None)]);
    let draft = Draft {
        chat: chat_g(),
        text: "первое\nвторое".into(),
        reply_to: None,
    };
    assert_eq!(
        writes(&h),
        [Command::SaveDraft(draft.clone())],
        "persisted without further typing"
    );
    h.store
        .read_with(cx, |s, _| assert_eq!(s.draft(&chat_g()), Some(&draft)));
    assert_eq!(composer_text(&h, cx), "старый", "the edit is not disturbed");
}

fn tall_own(i: i64) -> Message {
    own(
        "g",
        i,
        &(0..150)
            .map(|n| format!("строка {n}"))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

#[gpui::test]
fn a_message_taller_than_the_viewport_counts_as_seen_once_its_bottom_is_visible(
    cx: &mut TestAppContext,
) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![tall_own(2)]);
    scroll(cx, 2_000.);
    cx.run_until_parked();
    activate(cx);
    assert!(
        h.commands().is_empty(),
        "only the top of the message is visible"
    );

    scroll(cx, -1_000_000.);
    cx.run_until_parked();
    assert_eq!(
        h.commands(),
        [Command::MarkRead {
            chat: chat_g(),
            timestamp: ts(2)
        }]
    );
}

#[gpui::test]
fn a_row_clipped_at_the_top_counts_as_seen_while_its_bottom_is_visible(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    let mut messages = vec![own("g", 1, "первое")];
    messages.extend((10..90).map(|i| pending("g", i, &format!("p{i}"), "жду")));
    open_with(&h, cx, messages);
    scroll_to_top(&h, cx);
    scroll(cx, -10.);
    cx.run_until_parked();
    let list = h.list(cx);
    let row = list.bounds_for_item(0).expect("the first row is laid out");
    let viewport = list.viewport_bounds();
    assert!(
        row.top() < viewport.top() && row.bottom() > viewport.top(),
        "clipped at the top: {row:?} in {viewport:?}"
    );

    activate(cx);
    assert_eq!(
        h.commands(),
        [Command::MarkRead {
            chat: chat_g(),
            timestamp: ts(1)
        }]
    );
}

#[gpui::test]
fn opening_the_log_closes_an_open_message_menu(cx: &mut TestAppContext) {
    with_log_shortcut(cx);
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1), own("g", 2, "мой")]);
    cx.simulate_input("черновик");
    h.commands();
    right_click(cx, msg!(2));
    assert!(h.desktop.read_with(cx, |d, _| d.menu_is_open()));

    cx.simulate_keystrokes("cmd-shift-l");
    assert!(log_is_open(&h, cx));
    assert!(
        !h.desktop.read_with(cx, |d, _| d.menu_is_open()),
        "no menu above the log"
    );
    cx.simulate_keystrokes("enter");
    cx.simulate_input("ещё");
    assert!(h.commands().is_empty());
    assert_eq!(composer_text(&h, cx), "черновик");
    assert_eq!(h.desktop.read_with(cx, |d, _| d.composer_editing()), None);

    click(cx, "log-close");
    assert!(
        !h.desktop.read_with(cx, |d, _| d.menu_is_open()),
        "the menu does not come back"
    );
}

#[gpui::test]
fn a_sent_message_above_a_viewport_of_pending_ones_is_not_reported(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_group(&h, cx);
    let mut messages = vec![text("g", 1)];
    messages.extend((10..90).map(|i| pending("g", i, &format!("p{i}"), "жду")));
    h.feed(cx, [history("g", messages, true)]);
    h.commands();
    activate(cx);
    assert!(
        h.commands().is_empty(),
        "the only sent message is far above the viewport"
    );

    scroll_to_top(&h, cx);
    assert_eq!(
        h.commands(),
        [Command::MarkRead {
            chat: chat_g(),
            timestamp: ts(1)
        }]
    );
}

#[gpui::test]
fn nothing_is_reported_while_the_log_covers_the_chat(cx: &mut TestAppContext) {
    with_log_shortcut(cx);
    let (h, cx) = harness(cx);
    open_group(&h, cx);
    h.feed(cx, [chunk("g", 1..=3, true)]);
    activate(cx);
    assert_eq!(
        h.commands(),
        [Command::MarkRead {
            chat: chat_g(),
            timestamp: ts(3)
        }]
    );

    cx.simulate_keystrokes("cmd-shift-l");
    h.feed(cx, [ClientEvent::MessageUpserted(text("g", 4))]);
    assert!(
        h.commands().is_empty(),
        "the message arrived behind the log"
    );

    click(cx, "log-close");
    assert_eq!(
        h.commands(),
        [Command::MarkRead {
            chat: chat_g(),
            timestamp: ts(4)
        }]
    );
}

#[gpui::test]
fn a_history_chunk_carrying_the_payload_replaces_the_local_copy(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1), pending("g", 9, "p1", "жду")]);
    h.feed(
        cx,
        [ClientEvent::History(HistoryChunk {
            chat: chat_g(),
            messages: vec![text("g", 1), own("g", 5, "жду")],
            reached_start: true,
            replaces: None,
            payloads: vec![PayloadId("p1".into())],
        })],
    );
    assert_eq!(stamps(&h, cx), [1, 5]);
}

fn forwarded_own(i: i64) -> Message {
    let mut m = own("g", i, "пересланное");
    m.forwarded_from = Some("Анна".into());
    m
}

fn reply_without_id() -> Message {
    let mut m = own("g", 1, "ответ");
    m.reply_to = Some(ReplyQuote {
        author: None,
        text: "цитата".into(),
        timestamp: None,
    });
    m
}

#[gpui::test]
fn a_forwarded_own_message_cannot_be_edited(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![forwarded_own(1)]);
    right_click(cx, msg!(1));
    assert!(shown(cx, "menu-delete") && shown(cx, "menu-copy"));
    assert!(!shown(cx, "menu-edit"));
}

#[gpui::test]
fn a_reply_without_a_quoted_message_id_cannot_be_edited(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![reply_without_id()]);
    right_click(cx, msg!(1));
    assert!(shown(cx, "menu-delete"));
    assert!(!shown(cx, "menu-edit"));
}

#[gpui::test]
fn a_reply_with_a_quoted_message_id_can_be_edited(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    let mut reply = reply_without_id();
    reply.reply_to = Some(ReplyQuote {
        author: None,
        text: "цитата".into(),
        timestamp: Some(ts(1)),
    });
    reply.timestamp = ts(2);
    open_with(&h, cx, vec![text("g", 1), reply]);
    right_click(cx, msg!(2));
    assert!(shown(cx, "menu-edit"));
}

#[gpui::test]
fn up_skips_own_messages_that_cannot_be_edited(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    let mut unquotable = reply_without_id();
    unquotable.timestamp = ts(5);
    open_with(
        &h,
        cx,
        vec![own("g", 2, "первое"), forwarded_own(4), unquotable],
    );
    cx.simulate_keystrokes("up");
    assert_eq!(
        h.desktop.read_with(cx, |d, _| d.composer_editing()),
        Some(ts(2))
    );
    assert_eq!(composer_text(&h, cx), "первое");
}

// ---- download (store) ----

fn original(id: &str) -> MediaRef {
    media(id, MediaKind::File)
}

fn download_state(
    h: &Harness,
    cx: &mut VisualTestContext,
    original: &MediaRef,
) -> Option<DownloadState> {
    h.store
        .read_with(cx, |s, _| s.download_state(original).cloned())
}

#[gpui::test]
fn download_sends_one_command_and_shows_running(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    h.feed(cx, [ClientEvent::Connection(ConnectionState::Online)]);
    let file = original("disk/1");
    h.store.update(cx, |s, cx| {
        s.download(file.clone(), Some("report.pdf".into()), cx)
    });
    assert_eq!(
        h.commands(),
        [Command::Download {
            media: file.clone(),
            name: Some("report.pdf".into())
        }]
    );
    assert_eq!(
        download_state(&h, cx, &file),
        Some(DownloadState::Running {
            received: 0,
            total: None
        })
    );

    h.store.update(cx, |s, cx| {
        s.download(file.clone(), Some("report.pdf".into()), cx)
    });
    assert!(
        h.commands().is_empty(),
        "a running download is not started twice"
    );
}

#[gpui::test]
fn download_events_update_the_state(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    h.feed(cx, [ClientEvent::Connection(ConnectionState::Online)]);
    let file = original("disk/1");
    let other = original("disk/2");
    h.store
        .update(cx, |s, cx| s.download(file.clone(), None, cx));
    h.commands();

    h.feed(
        cx,
        [ClientEvent::DownloadProgress {
            media: file.clone(),
            received: 10,
            total: Some(40),
        }],
    );
    assert_eq!(
        download_state(&h, cx, &file),
        Some(DownloadState::Running {
            received: 10,
            total: Some(40)
        })
    );
    assert_eq!(
        download_state(&h, cx, &other),
        None,
        "other files are untouched"
    );

    let saved = PathBuf::from("/tmp/Downloads/report (1).pdf");
    h.feed(
        cx,
        [ClientEvent::Downloaded {
            media: file.clone(),
            path: saved.clone(),
        }],
    );
    assert_eq!(
        download_state(&h, cx, &file),
        Some(DownloadState::Saved(saved))
    );

    h.feed(
        cx,
        [ClientEvent::DownloadFailed {
            media: file.clone(),
            reason: DownloadFailure::Disk,
        }],
    );
    assert_eq!(
        download_state(&h, cx, &file),
        Some(DownloadState::Failed(DownloadFailure::Disk))
    );

    // A retry after a failure starts again from zero.
    h.store
        .update(cx, |s, cx| s.download(file.clone(), None, cx));
    assert_eq!(
        h.commands(),
        [Command::Download {
            media: file.clone(),
            name: None
        }]
    );
    assert_eq!(
        download_state(&h, cx, &file),
        Some(DownloadState::Running {
            received: 0,
            total: None
        })
    );
    // Saved files can be fetched again.
    h.feed(
        cx,
        [ClientEvent::Downloaded {
            media: file.clone(),
            path: PathBuf::from("/tmp/a"),
        }],
    );
    h.store
        .update(cx, |s, cx| s.download(file.clone(), None, cx));
    assert_eq!(h.commands().len(), 1);
}

#[gpui::test]
fn download_while_not_online_fails_without_a_command(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    let file = original("disk/1");
    for state in [ConnectionState::Connecting, ConnectionState::Offline] {
        h.feed(cx, [ClientEvent::Connection(state)]);
        h.store
            .update(cx, |s, cx| s.download(file.clone(), None, cx));
        assert!(h.commands().is_empty(), "{state:?}: nothing is sent");
        assert_eq!(
            download_state(&h, cx, &file),
            Some(DownloadState::Failed(DownloadFailure::Offline))
        );
        h.feed(cx, [ClientEvent::DownloadCancelled(file.clone())]);
    }
}

#[gpui::test]
fn cancel_download_asks_the_core_and_waits_for_its_answer(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    h.feed(cx, [ClientEvent::Connection(ConnectionState::Online)]);
    let file = original("disk/1");
    h.store
        .update(cx, |s, cx| s.download(file.clone(), None, cx));
    h.commands();

    h.store.read_with(cx, |s, _| s.cancel_download(&file));
    assert_eq!(h.commands(), [Command::CancelDownload(file.clone())]);
    assert_eq!(
        download_state(&h, cx, &file),
        Some(DownloadState::Running {
            received: 0,
            total: None
        }),
        "cleared by the answer"
    );
    h.feed(cx, [ClientEvent::DownloadCancelled(file.clone())]);
    assert_eq!(download_state(&h, cx, &file), None);

    h.store.read_with(cx, |s, _| s.cancel_download(&file));
    assert!(h.commands().is_empty(), "nothing runs, nothing to cancel");
}

#[gpui::test]
fn sign_out_clears_download_states(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    h.feed(cx, [ClientEvent::Connection(ConnectionState::Online)]);
    let file = original("disk/1");
    h.store
        .update(cx, |s, cx| s.download(file.clone(), None, cx));
    h.feed(
        cx,
        [
            ClientEvent::Downloaded {
                media: original("disk/2"),
                path: PathBuf::from("/tmp/b"),
            },
            ClientEvent::SignedOut,
        ],
    );
    assert_eq!(download_state(&h, cx, &file), None);
    assert_eq!(download_state(&h, cx, &original("disk/2")), None);
}

#[gpui::test]
fn a_gallery_requests_every_thumbnail_once_and_never_an_original(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    let item = |id: &str| GalleryItem {
        media: media(id, MediaKind::Thumbnail),
        width: 4,
        height: 3,
        name: Some(format!("{id}.jpg")),
    };
    let gallery = MessageBody::Gallery {
        items: vec![item("g/1"), item("g/2"), item("g/3")],
        caption: None,
    };
    let single = MessageBody::Image {
        media: media("s/1", MediaKind::Thumbnail),
        width: 4,
        height: 3,
        caption: None,
        name: None,
    };
    let file = MessageBody::File {
        media: original("f/1"),
        name: "a.zip".into(),
        size: 5,
    };
    let messages = vec![
        message("a", 1, "alice", false, gallery.clone()),
        message("a", 2, "alice", false, single),
        message("a", 3, "alice", false, file),
    ];
    h.feed(cx, [history("a", messages, true)]);
    assert_eq!(
        h.commands(),
        [
            Command::FetchMedia(media("g/1", MediaKind::Thumbnail)),
            Command::FetchMedia(media("g/2", MediaKind::Thumbnail)),
            Command::FetchMedia(media("g/3", MediaKind::Thumbnail)),
            Command::FetchMedia(media("s/1", MediaKind::Thumbnail)),
        ]
    );

    // The same gallery arriving again (a live edit or a refresh) asks for nothing new.
    h.feed(
        cx,
        [ClientEvent::MessageUpserted(message(
            "a", 1, "alice", false, gallery,
        ))],
    );
    assert!(h.commands().is_empty());
}

// ---- image viewer ----

fn picture(id: &str) -> GalleryItem {
    GalleryItem {
        media: media(id, MediaKind::Thumbnail),
        width: 1600,
        height: 800,
        name: Some(format!("{id}.jpg")),
    }
}

/// Message 2 of chat "g": four pictures `v/1` … `v/4`.
fn four_pictures() -> Message {
    let items = ["v/1", "v/2", "v/3", "v/4"].map(picture).to_vec();
    message(
        "g",
        2,
        "alice",
        false,
        MessageBody::Gallery {
            items,
            caption: None,
        },
    )
}

fn open_viewer(h: &Harness, cx: &mut VisualTestContext, ts: MessageTs, index: usize) {
    h.desktop
        .update(cx, |d, cx| d.open_viewer(chat_g(), ts, index, cx));
    cx.run_until_parked();
}

fn viewer_at(h: &Harness, cx: &mut VisualTestContext) -> Option<(MessageTs, usize)> {
    h.desktop.read_with(cx, |d, _| d.viewer_at())
}

fn blocked(h: &Harness, cx: &mut VisualTestContext) -> bool {
    h.desktop.read_with(cx, |d, cx| d.blocked(cx))
}

#[gpui::test]
fn the_viewer_opens_at_the_chosen_picture_with_its_position(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1), four_pictures()]);
    assert_eq!(viewer_at(&h, cx), None);
    open_viewer(&h, cx, ts(2), 2);
    assert_eq!(viewer_at(&h, cx), Some((ts(2), 2)));
    assert!(shown(cx, "viewer"));
    assert!(shown(cx, "viewer-pos-3-4"));
    assert!(shown(cx, "viewer-prev") && shown(cx, "viewer-next"));
    assert!(
        h.commands().is_empty(),
        "opening the viewer downloads nothing and asks for nothing"
    );
}

fn inside(outer: Bounds<Pixels>, inner: Bounds<Pixels>) -> bool {
    let slack = px(0.5);
    inner.left() >= outer.left() - slack
        && inner.right() <= outer.right() + slack
        && inner.top() >= outer.top() - slack
        && inner.bottom() <= outer.bottom() + slack
}

#[gpui::test]
fn the_viewer_fits_the_smallest_window(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    cx.simulate_resize(size(px(850.), px(560.)));
    let sized = |id: &str, width, height| GalleryItem {
        width,
        height,
        ..picture(id)
    };
    let items = vec![
        sized("v/1", 3000, 4000),
        sized("v/2", 4000, 1000),
        sized("v/3", 4000, 4000),
    ];
    open_with(
        &h,
        cx,
        vec![message(
            "g",
            2,
            "alice",
            false,
            MessageBody::Gallery {
                items,
                caption: None,
            },
        )],
    );
    open_viewer(&h, cx, ts(2), 0);

    for index in 0..3 {
        let overlay = bounds(cx, "viewer").expect("the viewer is on screen");
        assert_eq!(
            overlay.size,
            size(px(850.), px(560.)),
            "the overlay covers the window"
        );
        let picture = bounds(cx, "viewer-image").unwrap();
        for selector in [
            "viewer-image",
            "viewer-prev",
            "viewer-next",
            "viewer-close",
            "viewer-download",
        ] {
            let part =
                bounds(cx, selector).unwrap_or_else(|| panic!("{selector} is not on screen"));
            assert!(
                inside(overlay, part),
                "picture {index}: {selector} {part:?} leaves the window {overlay:?}"
            );
        }
        let (prev, next) = (
            bounds(cx, "viewer-prev").unwrap(),
            bounds(cx, "viewer-next").unwrap(),
        );
        assert!(
            prev.right() <= picture.left() && picture.right() <= next.left(),
            "picture {index} overlaps an arrow"
        );
        let (close, download) = (
            bounds(cx, "viewer-close").unwrap(),
            bounds(cx, "viewer-download").unwrap(),
        );
        assert!(
            close.bottom() <= picture.top() && picture.bottom() <= download.top(),
            "picture {index} overlaps a bar"
        );
        cx.simulate_keystrokes("right");
    }
}

#[gpui::test]
fn the_viewer_picture_follows_the_window_size(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![four_pictures()]);
    open_viewer(&h, cx, ts(2), 0);
    let width =
        |cx: &mut VisualTestContext| f32::from(bounds(cx, "viewer-image").unwrap().size.width);
    assert!((width(cx) - 800.0).abs() < 0.5);
    cx.simulate_resize(size(px(850.), px(560.)));
    cx.run_until_parked();
    let expected = 850.0 - 32.0 - 2.0 * 64.0; // what the padding and the two arrows leave
    assert!(
        (width(cx) - expected).abs() < 0.5,
        "{} != {expected}",
        width(cx)
    );
}

#[gpui::test]
fn the_viewer_ignores_what_it_cannot_show(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1), four_pictures()]);
    open_viewer(&h, cx, ts(2), 4);
    open_viewer(&h, cx, ts(1), 0);
    open_viewer(&h, cx, ts(99), 0);
    h.desktop.update(cx, |d, cx| {
        d.open_viewer(ChatId("other".into()), ts(2), 0, cx)
    });
    assert_eq!(viewer_at(&h, cx), None);
}

#[gpui::test]
fn right_and_left_move_between_pictures_and_stop_at_the_ends(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![four_pictures()]);
    open_viewer(&h, cx, ts(2), 2);

    cx.simulate_keystrokes("right");
    assert_eq!(viewer_at(&h, cx), Some((ts(2), 3)));
    assert!(shown(cx, "viewer-pos-4-4"));
    cx.simulate_keystrokes("right");
    assert_eq!(viewer_at(&h, cx), Some((ts(2), 3)), "the end does not wrap");

    for _ in 0..5 {
        cx.simulate_keystrokes("left");
    }
    assert_eq!(
        viewer_at(&h, cx),
        Some((ts(2), 0)),
        "the start does not wrap"
    );
    assert!(shown(cx, "viewer-pos-1-4"));
}

#[gpui::test]
fn the_arrow_controls_step_through_the_pictures(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![four_pictures()]);
    open_viewer(&h, cx, ts(2), 1);
    click(cx, "viewer-next");
    assert_eq!(viewer_at(&h, cx), Some((ts(2), 2)));
    click(cx, "viewer-prev");
    click(cx, "viewer-prev");
    assert_eq!(viewer_at(&h, cx), Some((ts(2), 0)));
    click(cx, "viewer-prev");
    assert_eq!(viewer_at(&h, cx), Some((ts(2), 0)), "inert at the start");
}

#[gpui::test]
fn escape_and_the_close_control_close_the_viewer(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![four_pictures()]);
    open_viewer(&h, cx, ts(2), 0);
    cx.simulate_keystrokes("escape");
    assert_eq!(viewer_at(&h, cx), None);
    assert!(!blocked(&h, cx));

    open_viewer(&h, cx, ts(2), 1);
    click(cx, "viewer-close");
    assert_eq!(viewer_at(&h, cx), None);
}

#[gpui::test]
fn escape_closes_the_viewer_before_touching_the_reply(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1), text("g", 2)]);
    choose_reply_to_2(cx);
    h.feed(cx, [ClientEvent::MessageUpserted(four_pictures_at(3))]);
    open_viewer(&h, cx, ts(3), 0);
    cx.simulate_keystrokes("escape");
    assert_eq!(viewer_at(&h, cx), None);
    assert_eq!(composer_reply(&h, cx), Some(reply_target_2()));
}

fn four_pictures_at(i: i64) -> Message {
    let mut m = four_pictures();
    m.timestamp = ts(i);
    m
}

#[gpui::test]
fn a_single_image_has_no_previous_or_next(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    let single = MessageBody::Image {
        media: media("s/1", MediaKind::Thumbnail),
        width: 600,
        height: 1200,
        caption: None,
        name: None,
    };
    open_with(&h, cx, vec![message("g", 2, "alice", false, single)]);
    open_viewer(&h, cx, ts(2), 0);
    assert!(shown(cx, "viewer-pos-1-1"));
    assert!(shown(cx, "viewer-image"));
    assert!(!shown(cx, "viewer-prev") && !shown(cx, "viewer-next"));
    cx.simulate_keystrokes("right");
    assert_eq!(viewer_at(&h, cx), Some((ts(2), 0)));
}

#[gpui::test]
fn the_preview_replaces_the_placeholder_when_it_arrives(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![four_pictures()]);
    open_viewer(&h, cx, ts(2), 1);
    let b = bounds(cx, "viewer-image").expect("the placeholder is shown");
    assert_eq!(
        (f32::from(b.size.width), f32::from(b.size.height)),
        (800.0, 400.0)
    );
    assert!(!shown(cx, "viewer-image-loaded"));

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v2.png");
    std::fs::write(&path, PNG_1X1).unwrap();
    h.feed(
        cx,
        [ClientEvent::MediaReady {
            media: media("v/2", MediaKind::Thumbnail),
            path,
        }],
    );
    assert!(shown(cx, "viewer-image-loaded"));
    let b = bounds(cx, "viewer-image").unwrap();
    assert_eq!(
        (f32::from(b.size.width), f32::from(b.size.height)),
        (800.0, 400.0),
        "the box does not change"
    );
}

#[gpui::test]
fn deleting_the_message_closes_the_viewer(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1), four_pictures()]);
    open_viewer(&h, cx, ts(2), 1);
    h.feed(
        cx,
        [ClientEvent::MessageDeleted {
            chat: chat_g(),
            timestamp: ts(2),
        }],
    );
    assert_eq!(viewer_at(&h, cx), None);
    assert!(!blocked(&h, cx));
}

#[gpui::test]
fn a_body_without_pictures_closes_the_viewer_and_fewer_pictures_step_back(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![four_pictures()]);
    open_viewer(&h, cx, ts(2), 3);
    let two = MessageBody::Gallery {
        items: vec![picture("v/1"), picture("v/2")],
        caption: None,
    };
    h.feed(
        cx,
        [ClientEvent::MessageUpserted(message(
            "g", 2, "alice", false, two,
        ))],
    );
    assert_eq!(
        viewer_at(&h, cx),
        Some((ts(2), 1)),
        "the last picture that is left"
    );
    assert!(shown(cx, "viewer-pos-2-2"));

    h.feed(
        cx,
        [ClientEvent::MessageUpserted(message(
            "g",
            2,
            "alice",
            false,
            MessageBody::Deleted,
        ))],
    );
    assert_eq!(viewer_at(&h, cx), None);
}

#[gpui::test]
fn an_open_viewer_blocks_the_composer_and_read_markers(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1), four_pictures()]);
    cx.simulate_input("привет");
    h.commands();

    open_viewer(&h, cx, ts(2), 0);
    assert!(blocked(&h, cx));
    cx.simulate_keystrokes("enter");
    cx.simulate_input("ещё");
    activate(cx);
    assert!(
        h.commands().is_empty(),
        "no send, no typing notice, nothing read while the viewer is open"
    );
    assert_eq!(composer_text(&h, cx), "привет");

    cx.simulate_keystrokes("escape");
    assert_eq!(
        h.commands(),
        [Command::MarkRead {
            chat: chat_g(),
            timestamp: ts(2)
        }],
        "reading resumes after it closes"
    );
    cx.simulate_input("!");
    assert_eq!(
        composer_text(&h, cx),
        "привет!",
        "the composer has the keyboard again"
    );
}

#[gpui::test]
fn the_download_button_saves_the_original_under_the_items_name(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    h.feed(cx, [ClientEvent::Connection(ConnectionState::Online)]);
    open_with(&h, cx, vec![four_pictures()]);
    open_viewer(&h, cx, ts(2), 2);
    let file = original("v/3");

    click(cx, "viewer-download");
    assert_eq!(
        h.commands(),
        [Command::Download {
            media: file.clone(),
            name: Some("v/3.jpg".into())
        }]
    );
    assert_eq!(
        download_state(&h, cx, &file),
        Some(DownloadState::Running {
            received: 0,
            total: None
        })
    );

    h.feed(
        cx,
        [ClientEvent::DownloadProgress {
            media: file.clone(),
            received: 10,
            total: Some(40),
        }],
    );
    click(cx, "viewer-cancel");
    assert_eq!(h.commands(), [Command::CancelDownload(file.clone())]);
    h.feed(cx, [ClientEvent::DownloadCancelled(file.clone())]);
    assert_eq!(download_state(&h, cx, &file), None);
}

#[gpui::test]
fn the_viewer_shows_saved_and_failed_downloads_of_its_item(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    h.feed(cx, [ClientEvent::Connection(ConnectionState::Online)]);
    open_with(&h, cx, vec![four_pictures()]);
    open_viewer(&h, cx, ts(2), 0);
    let file = original("v/1");

    h.feed(
        cx,
        [ClientEvent::DownloadFailed {
            media: file.clone(),
            reason: DownloadFailure::Network,
        }],
    );
    assert!(shown(cx, "viewer-retry"));
    click(cx, "viewer-retry");
    assert_eq!(
        h.commands(),
        [Command::Download {
            media: file.clone(),
            name: Some("v/1.jpg".into())
        }]
    );

    h.feed(
        cx,
        [ClientEvent::Downloaded {
            media: file.clone(),
            path: PathBuf::from("/tmp/Downloads/v (1).jpg"),
        }],
    );
    assert!(shown(cx, "viewer-saved"));
    click(cx, "viewer-download");
    assert_eq!(
        h.commands().len(),
        1,
        "a saved picture can be fetched again"
    );

    h.feed(
        cx,
        [ClientEvent::DownloadProgress {
            media: original("v/2"),
            received: 1,
            total: None,
        }],
    );
    assert_eq!(
        h.commands(),
        [],
        "another picture's download does not touch this one"
    );
}

#[gpui::test]
fn the_download_button_is_disabled_without_a_connection(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    h.feed(cx, [ClientEvent::Connection(ConnectionState::Offline)]);
    open_with(&h, cx, vec![four_pictures()]);
    open_viewer(&h, cx, ts(2), 0);
    assert!(shown(cx, "viewer-download"));
    assert!(shown(cx, "viewer-status"));
    click(cx, "viewer-download");
    assert!(h.commands().is_empty());
    assert_eq!(download_state(&h, cx, &original("v/1")), None);
}

// ---- file card and gallery grid ----

fn file_message(i: i64) -> Message {
    message(
        "g",
        i,
        "bob",
        false,
        MessageBody::File {
            media: original("doc"),
            name: "отчёт.pdf".into(),
            size: 3_500_000,
        },
    )
}

fn dims(cx: &mut VisualTestContext, selector: &'static str) -> (f32, f32) {
    let b = bounds(cx, selector).unwrap_or_else(|| panic!("{selector} is not on screen"));
    (f32::from(b.size.width), f32::from(b.size.height))
}

fn origin(cx: &mut VisualTestContext, selector: &'static str) -> (f32, f32) {
    let b = bounds(cx, selector).unwrap_or_else(|| panic!("{selector} is not on screen"));
    (f32::from(b.origin.x), f32::from(b.origin.y))
}

/// Message 2 of chat "g": `n` pictures `v/1` … `v/n`.
fn pictures(n: usize) -> Message {
    let items = (1..=n).map(|i| picture(&format!("v/{i}"))).collect();
    message(
        "g",
        2,
        "alice",
        false,
        MessageBody::Gallery {
            items,
            caption: None,
        },
    )
}

const CELLS: [&str; 5] = [
    "msg-2-image-0",
    "msg-2-image-1",
    "msg-2-image-2",
    "msg-2-image-3",
    "msg-2-image-4",
];

#[test]
fn the_grid_has_one_two_or_three_columns() {
    let columns: Vec<usize> = (1..=10).map(gallery_columns).collect();
    assert_eq!(columns, [1, 2, 3, 2, 3, 3, 3, 3, 3, 3]);
}

#[gpui::test]
fn the_file_card_offers_download_and_follows_the_transfer(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    h.feed(cx, [ClientEvent::Connection(ConnectionState::Online)]);
    open_with(&h, cx, vec![file_message(5)]);
    let file = original("doc");
    assert!(shown(cx, msg!(5, "download")));
    let idle = dims(cx, msg!(5, "file"));

    click(cx, msg!(5, "download"));
    assert_eq!(
        h.commands(),
        [Command::Download {
            media: file.clone(),
            name: Some("отчёт.pdf".into())
        }]
    );
    assert_eq!(
        download_state(&h, cx, &file),
        Some(DownloadState::Running {
            received: 0,
            total: None
        })
    );
    assert!(
        shown(cx, msg!(5, "cancel")),
        "a running download offers Cancel"
    );

    h.feed(
        cx,
        [ClientEvent::DownloadProgress {
            media: file.clone(),
            received: 10,
            total: Some(40),
        }],
    );
    assert!(
        shown(cx, msg!(5, "status")),
        "the progress is written next to the button"
    );
    click(cx, msg!(5, "cancel"));
    assert_eq!(h.commands(), [Command::CancelDownload(file.clone())]);
    h.feed(cx, [ClientEvent::DownloadCancelled(file.clone())]);
    assert_eq!(download_state(&h, cx, &file), None);

    h.feed(
        cx,
        [ClientEvent::Downloaded {
            media: file.clone(),
            path: PathBuf::from("/tmp/Downloads/отчёт (1).pdf"),
        }],
    );
    assert!(shown(cx, msg!(5, "saved")), "the saved name is shown");
    click(cx, msg!(5, "download"));
    assert_eq!(h.commands().len(), 1, "a saved file can be fetched again");

    h.feed(
        cx,
        [ClientEvent::DownloadFailed {
            media: file.clone(),
            reason: DownloadFailure::Network,
        }],
    );
    assert!(shown(cx, msg!(5, "retry")));
    click(cx, msg!(5, "retry"));
    assert_eq!(
        h.commands(),
        [Command::Download {
            media: file.clone(),
            name: Some("отчёт.pdf".into())
        }]
    );
    assert_eq!(
        download_state(&h, cx, &file),
        Some(DownloadState::Running {
            received: 0,
            total: None
        })
    );

    assert_eq!(
        dims(cx, msg!(5, "file")),
        idle,
        "the card keeps its size in every state"
    );
}

#[gpui::test]
fn the_file_card_cannot_start_a_download_without_a_connection(cx: &mut TestAppContext) {
    let (h, cx) = harness_with(
        cx,
        vec![ClientEvent::Connection(ConnectionState::Offline)],
        |_, _| {},
    );
    open_with(&h, cx, vec![file_message(5)]);
    assert!(shown(cx, msg!(5, "download")));
    assert!(
        shown(cx, msg!(5, "status")),
        "the reason is written next to the button"
    );
    click(cx, msg!(5, "download"));
    assert!(h.commands().is_empty());
    assert_eq!(download_state(&h, cx, &original("doc")), None);
}

#[gpui::test]
fn the_file_card_shows_the_reason_a_download_failed(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    h.feed(cx, [ClientEvent::Connection(ConnectionState::Online)]);
    open_with(&h, cx, vec![file_message(5)]);
    h.feed(
        cx,
        [ClientEvent::DownloadFailed {
            media: original("doc"),
            reason: DownloadFailure::NotFound,
        }],
    );
    assert!(shown(cx, msg!(5, "retry")));
    assert!(shown(cx, msg!(5, "status")));
    assert!(!shown(cx, msg!(5, "saved")));
}

#[gpui::test]
fn a_gallery_renders_square_cells_in_order_and_keeps_its_height(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![pictures(4)]);
    assert!(
        !shown(cx, msg!(2, "image")),
        "a gallery is not a single image"
    );
    let side = (456.0 - 4.0) / 2.0;
    for cell in &CELLS[..4] {
        assert_eq!(dims(cx, cell), (side, side), "{cell}");
    }
    let at: Vec<_> = CELLS[..4].iter().map(|c| origin(cx, c)).collect();
    assert_eq!(at[0].1, at[1].1, "first row");
    assert!(at[0].0 < at[1].0, "reading order");
    assert_eq!(at[2].1, at[3].1, "second row");
    assert!(at[2].1 > at[0].1);
    assert_eq!((at[2].0, at[3].0), (at[0].0, at[1].0), "two columns");
    assert!(!shown(cx, CELLS[4]));

    let before = dims(cx, msg!(2));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v2.png");
    std::fs::write(&path, PNG_1X1).unwrap();
    h.feed(
        cx,
        [ClientEvent::MediaReady {
            media: media("v/2", MediaKind::Thumbnail),
            path,
        }],
    );
    assert!(shown(cx, "msg-2-image-1-loaded"));
    assert!(!shown(cx, "msg-2-image-0-loaded"));
    assert_eq!(
        dims(cx, msg!(2)),
        before,
        "the row height does not change when a preview arrives"
    );
    assert_eq!(dims(cx, CELLS[1]), (side, side));
}

#[gpui::test]
fn the_last_row_of_a_grid_is_left_aligned(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![pictures(5)]);
    let side = (456.0 - 8.0) / 3.0;
    for cell in &CELLS {
        let (w, h) = dims(cx, cell);
        assert!(
            (w - side).abs() < 1.0 && (h - side).abs() < 1.0,
            "{cell}: {w}x{h}"
        );
    }
    let at: Vec<_> = CELLS.iter().map(|c| origin(cx, c)).collect();
    assert_eq!((at[1].1, at[2].1), (at[0].1, at[0].1), "first row");
    assert_eq!(at[4].1, at[3].1, "second row");
    assert!(at[3].1 > at[0].1);
    assert_eq!(
        (at[3].0, at[4].0),
        (at[0].0, at[1].0),
        "the short row starts at the left edge"
    );
}

#[gpui::test]
fn a_lone_picture_keeps_its_aspect_ratio_and_the_caption_goes_below(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    let one = MessageBody::Gallery {
        items: vec![picture("v/1")],
        caption: Some("подпись".into()),
    };
    open_with(&h, cx, vec![message("g", 2, "alice", false, one)]);
    assert_eq!(dims(cx, msg!(2, "image")), fit_thumbnail(1600, 800, 320.0));
    assert!(!shown(cx, CELLS[0]));
    assert!(
        y(cx, msg!(2, "caption")) >= y(cx, msg!(2, "image")) + 160.0,
        "the caption sits below the picture"
    );
}

#[gpui::test]
fn the_caption_of_a_gallery_sits_below_the_last_row(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    let items = ["v/1", "v/2", "v/3", "v/4"].map(picture).to_vec();
    let captioned = MessageBody::Gallery {
        items,
        caption: Some("подпись".into()),
    };
    open_with(&h, cx, vec![message("g", 2, "alice", false, captioned)]);
    assert!(
        y(cx, msg!(2, "caption")) >= y(cx, CELLS[2]) + 226.0,
        "the caption sits below the last row"
    );
}

#[gpui::test]
fn clicking_a_cell_or_the_single_image_opens_the_viewer_at_it(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    let single = MessageBody::Image {
        media: media("s/1", MediaKind::Thumbnail),
        width: 600,
        height: 1200,
        caption: None,
        name: None,
    };
    open_with(
        &h,
        cx,
        vec![message("g", 1, "alice", false, single), pictures(4)],
    );
    assert_eq!(viewer_at(&h, cx), None);

    click(cx, CELLS[2]);
    assert_eq!(viewer_at(&h, cx), Some((ts(2), 2)));
    assert!(shown(cx, "viewer-pos-3-4"));
    cx.simulate_keystrokes("escape");
    click(cx, CELLS[0]);
    assert_eq!(viewer_at(&h, cx), Some((ts(2), 0)));
    cx.simulate_keystrokes("escape");

    click(cx, msg!(1, "image"));
    assert_eq!(viewer_at(&h, cx), Some((ts(1), 0)));
    assert!(shown(cx, "viewer-pos-1-1"));
    assert!(
        h.commands().is_empty(),
        "opening the viewer asks for nothing"
    );
}

// ---- add-message-reactions 5.4: chips, menu row, refusals ----
//
// Debug bounds say where an element is, not what it is filled with or which glyph it shows. The own
// reaction (chip and menu button fill) is therefore asserted through the store state both views read
// (`Reactions::mine`), and the neutral glyph through the chip being there and clickable for a code
// outside `KNOWN`. Bounds are also never removed once painted, so every "no such element" case runs in
// a window that never painted it.

const LIKE: i64 = 100102;
const FIRE: i64 = 100104;
const HEART: i64 = 100109;
const NOT_IN_TABLE: i64 = 424_242;

/// One selector per `KNOWN` type, in picker order; `debug_bounds` only takes `&'static str`.
const MENU_REACTIONS: [&str; 8] = [
    "menu-reaction-100109",
    "menu-reaction-100104",
    "menu-reaction-100102",
    "menu-reaction-100103",
    "menu-reaction-100106",
    "menu-reaction-100105",
    "menu-reaction-100110",
    "menu-reaction-100101",
];

fn reactions_of(counts: &[(i64, u32)], mine: Option<i64>) -> Reactions {
    let counts = counts
        .iter()
        .map(|&(code, count)| ReactionCount {
            code: ReactionCode(code),
            count,
        })
        .collect();
    Reactions {
        counts,
        mine: mine.map(ReactionCode),
        version: Some(1),
    }
}

/// An incoming message with reactions.
fn reacted(i: i64, counts: &[(i64, u32)], mine: Option<i64>) -> Message {
    let mut m = text("g", i);
    m.reactions = reactions_of(counts, mine);
    m
}

fn toggle_command(i: i64, code: i64) -> Command {
    Command::ToggleReaction {
        chat: chat_g(),
        timestamp: ts(i),
        code: ReactionCode(code),
    }
}

fn reactions_in_store(h: &Harness, cx: &mut VisualTestContext, i: i64) -> Reactions {
    h.store.read_with(cx, |s, _| {
        s.timeline(&chat_g())
            .unwrap()
            .messages
            .iter()
            .find(|m| m.timestamp == ts(i))
            .unwrap()
            .reactions
            .clone()
    })
}

fn menu_is_open(h: &Harness, cx: &mut VisualTestContext) -> bool {
    h.desktop.read_with(cx, |d, _| d.menu_is_open())
}

/// The menu surface is repainted (its bounds overwritten) every time a menu opens, unlike its entries
/// that stay in the debug map after they are closed.
fn menu_height(cx: &mut VisualTestContext) -> f32 {
    f32::from(
        bounds(cx, "message-menu")
            .expect("a menu was painted")
            .size
            .height,
    )
}

#[gpui::test]
fn chips_show_one_chip_per_type_in_the_server_order_inside_the_bubble(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    let counts = [(LIKE, 3), (FIRE, 1000), (NOT_IN_TABLE, 2)];
    open_with(&h, cx, vec![text("g", 1), reacted(2, &counts, Some(FIRE))]);

    let chips = [
        msg!(2, "reaction-100102"),
        msg!(2, "reaction-100104"),
        msg!(2, "reaction-424242"),
    ];
    let bubble = bounds(cx, msg!(2)).unwrap();
    for chip in chips {
        assert!(
            inside(
                bubble,
                bounds(cx, chip).unwrap_or_else(|| panic!("{chip} is not on screen"))
            ),
            "{chip} sits in the bubble"
        );
    }
    assert!(
        origin(cx, chips[0]).0 < origin(cx, chips[1]).0
            && origin(cx, chips[1]).0 < origin(cx, chips[2]).0,
        "server order"
    );
    assert!(
        !shown(cx, msg!(2, "reaction-100109")),
        "a type nobody chose has no chip"
    );
    assert!(
        !shown(cx, msg!(1, "reaction-100102")),
        "a message without reactions has no chips"
    );
    assert!(
        dims(cx, chips[1]).0 > dims(cx, chips[0]).0 + 10.,
        "the count is part of the chip: 1000 is wider than 3"
    );
    assert!(
        h.commands().is_empty(),
        "showing reactions asks for nothing"
    );
}

#[gpui::test]
fn a_type_outside_the_table_is_still_a_clickable_chip(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![reacted(2, &[(NOT_IN_TABLE, 1)], None)]);
    assert!(
        bt_chat_core::reactions::emoji(ReactionCode(NOT_IN_TABLE)).is_none(),
        "drawn with the neutral glyph"
    );
    click(cx, msg!(2, "reaction-424242"));
    assert_eq!(h.commands(), [toggle_command(2, NOT_IN_TABLE)]);
}

#[gpui::test]
fn clicking_a_chip_sends_a_toggle_with_its_code_and_opens_no_menu(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(
        &h,
        cx,
        vec![reacted(2, &[(LIKE, 3), (FIRE, 1)], Some(FIRE))],
    );
    click(cx, msg!(2, "reaction-100102"));
    assert_eq!(
        h.commands(),
        [toggle_command(2, LIKE)],
        "another type replaces the own one"
    );
    click(cx, msg!(2, "reaction-100104"));
    assert_eq!(
        h.commands(),
        [toggle_command(2, FIRE)],
        "the own type goes through the same command: the core removes it"
    );
    assert!(!menu_is_open(&h, cx));
    assert_eq!(
        reactions_in_store(&h, cx, 2),
        reactions_of(&[(LIKE, 3), (FIRE, 1)], Some(FIRE)),
        "the UI waits for the core's overlay"
    );
}

#[gpui::test]
fn the_own_type_follows_the_account_reaction_in_the_store(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(
        &h,
        cx,
        vec![reacted(2, &[(LIKE, 3), (FIRE, 1)], Some(FIRE))],
    );
    // The chip and the menu button of a type are filled when `mine` is that type (see the header note).
    assert_eq!(reactions_in_store(&h, cx, 2).mine, Some(ReactionCode(FIRE)));

    let moved = Reactions {
        version: Some(2),
        ..reactions_of(&[(LIKE, 4)], Some(LIKE))
    };
    h.feed(
        cx,
        [ClientEvent::ReactionsChanged {
            chat: chat_g(),
            timestamp: ts(2),
            reactions: moved,
        }],
    );
    assert_eq!(reactions_in_store(&h, cx, 2).mine, Some(ReactionCode(LIKE)));

    let removed = Reactions {
        version: Some(3),
        ..reactions_of(&[(LIKE, 3)], None)
    };
    h.feed(
        cx,
        [ClientEvent::ReactionsChanged {
            chat: chat_g(),
            timestamp: ts(2),
            reactions: removed,
        }],
    );
    assert_eq!(reactions_in_store(&h, cx, 2).mine, None);
    assert!(h.commands().is_empty());
}

#[gpui::test]
fn a_server_update_adds_chips_in_place(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1), text("g", 2)]);
    assert!(!shown(cx, msg!(2, "reaction-100104")));

    h.feed(
        cx,
        [ClientEvent::ReactionsChanged {
            chat: chat_g(),
            timestamp: ts(2),
            reactions: reactions_of(&[(FIRE, 2)], None),
        }],
    );
    assert!(shown(cx, msg!(2, "reaction-100104")));
    assert_eq!(
        stamps(&h, cx),
        [1, 2],
        "no message was added or moved in the list"
    );
    assert!(h.commands().is_empty());
}

#[gpui::test]
fn deleted_pending_and_failed_messages_show_no_chips(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    let busy = reactions_of(&[(LIKE, 2)], None);
    let mut deleted = message("g", 2, "alice", false, MessageBody::Deleted);
    deleted.reactions = busy.clone();
    let mut waiting = pending("g", 3, "p1", "жду");
    waiting.reactions = busy.clone();
    let mut broken = failed("g", 4, "p2", "не вышло", "нет прав");
    broken.reactions = busy;
    open_with(
        &h,
        cx,
        vec![reacted(1, &[(LIKE, 2)], None), deleted, waiting, broken],
    );

    assert!(
        shown(cx, msg!(1, "reaction-100102")),
        "the control message shows its chip"
    );
    assert!(
        shown(cx, msg!(2)) && shown(cx, msg!(3)) && shown(cx, msg!(4)),
        "the other rows are on screen"
    );
    assert!(!shown(cx, msg!(2, "reaction-100102")), "deleted");
    assert!(!shown(cx, msg!(3, "reaction-100102")), "pending");
    assert!(!shown(cx, msg!(4, "reaction-100102")), "failed");
}

#[gpui::test]
fn the_menu_row_has_one_button_per_known_type_in_one_line_above_the_entries(
    cx: &mut TestAppContext,
) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![own("g", 1, "мой")]);
    // The non-empty table is the 3.3 outcome these cases are written for.
    assert!(!KNOWN.is_empty());
    assert_eq!(KNOWN.len(), MENU_REACTIONS.len());
    for (known, selector) in KNOWN.iter().zip(MENU_REACTIONS) {
        assert_eq!(
            selector,
            format!("menu-reaction-{}", known.code.0),
            "the selector table follows KNOWN"
        );
    }

    right_click(cx, msg!(1));
    assert!(shown(cx, "menu-reactions"));
    let row_y = y(cx, MENU_REACTIONS[0]);
    let mut left = f32::MIN;
    for selector in MENU_REACTIONS {
        assert_eq!(y(cx, selector), row_y, "{selector} is on the first line");
        assert!(
            origin(cx, selector).0 > left,
            "{selector} follows the previous button"
        );
        left = origin(cx, selector).0;
        assert!(
            inside(
                bounds(cx, "message-menu").unwrap(),
                bounds(cx, selector).unwrap()
            ),
            "{selector} fits the menu"
        );
    }
    assert!(row_y < y(cx, "menu-reply"), "the row is above the entries");
    assert!(h.commands().is_empty());
}

#[gpui::test]
fn choosing_a_type_in_the_menu_toggles_it_and_closes_the_menu(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(
        &h,
        cx,
        vec![text("g", 1), reacted(2, &[(LIKE, 1)], Some(LIKE))],
    );
    right_click(cx, msg!(2));
    click(cx, "menu-reaction-100104");
    assert_eq!(h.commands(), [toggle_command(2, FIRE)]);
    assert!(!menu_is_open(&h, cx));

    // The own type is marked from the state; choosing it again is the same command, the core removes it.
    assert_eq!(reactions_in_store(&h, cx, 2).mine, Some(ReactionCode(LIKE)));
    right_click(cx, msg!(2));
    click(cx, "menu-reaction-100102");
    assert_eq!(h.commands(), [toggle_command(2, LIKE)]);
    assert!(!menu_is_open(&h, cx));

    right_click(cx, msg!(1));
    click(cx, "menu-reaction-100109");
    assert_eq!(
        h.commands(),
        [toggle_command(1, HEART)],
        "an incoming message without reactions can be reacted to"
    );
}

#[gpui::test]
fn a_message_with_only_a_reaction_row_still_opens_the_menu(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(
        &h,
        cx,
        vec![message(
            "g",
            1,
            "bob",
            false,
            MessageBody::Unsupported("sticker".into()),
        )],
    );
    right_click(cx, msg!(1));
    assert!(menu_is_open(&h, cx));
    assert!(shown(cx, "menu-reactions"));
    for entry in ["menu-reply", "menu-copy", "menu-edit", "menu-delete"] {
        assert!(!shown(cx, entry), "{entry}: nothing else applies to it");
    }
    click(cx, "menu-reaction-100101");
    assert_eq!(h.commands(), [toggle_command(1, 100101)]);
}

#[gpui::test]
fn pending_and_failed_menus_have_no_reaction_row_and_a_deleted_message_has_no_menu(
    cx: &mut TestAppContext,
) {
    let (h, cx) = harness(cx);
    let deleted = message("g", 2, "alice", false, MessageBody::Deleted);
    let mut own_deleted = own("g", 6, "x");
    own_deleted.body = MessageBody::Deleted;
    open_with(
        &h,
        cx,
        vec![
            pending("g", 1, "p1", "жду"),
            deleted,
            failed("g", 3, "p2", "не вышло", "нет прав"),
            own_deleted,
        ],
    );

    right_click(cx, msg!(1));
    assert!(menu_is_open(&h, cx));
    assert!(shown(cx, "menu-copy"));
    cx.simulate_keystrokes("escape");
    right_click(cx, msg!(3));
    assert!(menu_is_open(&h, cx));
    cx.simulate_keystrokes("escape");
    assert!(
        !shown(cx, "menu-reactions"),
        "neither a pending nor a failed message offers reactions"
    );

    right_click(cx, msg!(2));
    assert!(!menu_is_open(&h, cx), "a deleted message has no menu");
    right_click(cx, msg!(6));
    assert!(!menu_is_open(&h, cx), "not even its own author's");
    assert!(!shown(cx, "menu-reactions"));
    assert!(h.commands().is_empty());
}

#[gpui::test]
fn a_toggle_for_a_chat_that_is_not_open_or_while_blocked_is_ignored(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![reacted(2, &[(LIKE, 1)], None)]);
    let ask = |h: &Harness, cx: &mut VisualTestContext, chat: &str| {
        h.desktop.update(cx, |d, cx| {
            d.toggle_reaction(ChatId(chat.into()), ts(2), ReactionCode(LIKE), cx)
        });
    };
    ask(&h, cx, "p");
    assert!(h.commands().is_empty(), "the chat is not the open one");
    ask(&h, cx, "g");
    assert_eq!(h.commands(), [toggle_command(2, LIKE)]);

    h.feed(cx, [ClientEvent::SessionExpired]);
    assert!(blocked(&h, cx));
    ask(&h, cx, "g");
    assert!(
        h.commands().is_empty(),
        "an expired session asks for nothing"
    );
}

#[gpui::test]
fn a_refused_reaction_rolls_back_the_overlay_and_shows_the_reason_for_a_while(
    cx: &mut TestAppContext,
) {
    let (h, cx) = harness(cx);
    let confirmed = reactions_of(&[(LIKE, 3)], None);
    open_with(&h, cx, vec![text("g", 1), reacted(2, &[(LIKE, 3)], None)]);

    click(cx, msg!(2, "reaction-100102"));
    assert_eq!(h.commands(), [toggle_command(2, LIKE)]);
    // What the core answers: the overlay at once, then the rollback with the reason when the server refuses.
    let overlay = reactions_of(&[(LIKE, 4)], Some(LIKE));
    h.feed(
        cx,
        [ClientEvent::ReactionsChanged {
            chat: chat_g(),
            timestamp: ts(2),
            reactions: overlay.clone(),
        }],
    );
    assert_eq!(reactions_in_store(&h, cx, 2), overlay);
    assert!(!shown(cx, "toast"));

    h.feed(
        cx,
        [
            ClientEvent::ReactionsChanged {
                chat: chat_g(),
                timestamp: ts(2),
                reactions: confirmed.clone(),
            },
            ClientEvent::ActionFailed {
                chat: chat_g(),
                reason: "NO_PERMISSION".into(),
            },
        ],
    );
    assert_eq!(
        reactions_in_store(&h, cx, 2),
        confirmed,
        "back to the confirmed state"
    );
    assert!(shown(cx, "toast"));
    assert_eq!(
        h.desktop
            .read_with(cx, |d, _| d.toast_text().map(str::to_string)),
        Some("NO_PERMISSION".into())
    );

    cx.executor().advance_clock(Duration::from_secs(6));
    cx.run_until_parked();
    assert_eq!(
        h.desktop
            .read_with(cx, |d, _| d.toast_text().map(str::to_string)),
        None
    );
}

/// Observation from 5.3: right-clicking a pending message right after closing another message's menu
/// seemed to open the menu "as if Sent". Not a bug, two test-side effects: the entries of a closed menu stay
/// in the debug map (`shown("menu-reply")` is true from the earlier menu whatever is open now), and the
/// open menu (192 px below the click) covers the message below it, so a right click there lands on the
/// menu or its overlay and closes it instead of opening a second one. The check therefore goes through
/// what every opening repaints, the height of the menu surface, and through what a click does; the
/// pending message sits below the menu's reach.
#[gpui::test]
fn a_menu_opened_after_another_one_closed_offers_what_its_own_message_offers(
    cx: &mut TestAppContext,
) {
    let (h, cx) = harness(cx);
    let mut messages = vec![own("g", 1, "мой")];
    messages.extend((2..=9).map(|i| text("g", i)));
    messages.push(pending("g", 10, "p1", "жду"));
    open_with(&h, cx, messages);

    right_click(cx, msg!(10));
    let copy_only = menu_height(cx);
    cx.simulate_keystrokes("escape");
    right_click(cx, msg!(1));
    let full = menu_height(cx);
    assert!(
        full > copy_only * 2.,
        "the sent message's menu is the long one: {full} against {copy_only}"
    );
    assert!(
        y(cx, msg!(10)) > y(cx, "message-menu") + full,
        "the pending message is outside the open menu"
    );

    // Closed with Escape, then the pending message.
    cx.simulate_keystrokes("escape");
    right_click(cx, msg!(10));
    assert!(menu_is_open(&h, cx));
    assert_eq!(menu_height(cx), copy_only, "after Escape");
    cx.simulate_keystrokes("escape");

    // Closed by an outside click.
    right_click(cx, msg!(1));
    click(cx, "chat-header");
    right_click(cx, msg!(10));
    assert_eq!(menu_height(cx), copy_only, "after an outside click");
    cx.simulate_keystrokes("escape");

    // A right click on another message while a menu is open only closes that menu.
    right_click(cx, msg!(1));
    right_click(cx, msg!(10));
    assert!(!menu_is_open(&h, cx));
    right_click(cx, msg!(10));
    assert!(menu_is_open(&h, cx));
    assert_eq!(
        menu_height(cx),
        copy_only,
        "after a right click on the occluded message"
    );

    click(cx, "menu-copy");
    assert_eq!(
        cx.read_from_clipboard().and_then(|item| item.text()),
        Some("жду".into())
    );
    assert!(
        h.commands().is_empty(),
        "no reply, edit, delete or reaction came out of it"
    );
    assert_eq!(composer_reply(&h, cx), None);
    assert_eq!(h.desktop.read_with(cx, |d, _| d.composer_editing()), None);
}

// ---- chat info panel (CM 4.1) ----

fn member(guid: &str, name: &str, role: MemberRole, me: bool) -> Member {
    Member {
        user: UserId(guid.into()),
        name: Some(name.into()),
        avatar: None,
        role,
        me,
    }
}

fn members_of(chat_id: &str, users: Vec<Member>, partial: bool) -> ClientEvent {
    ClientEvent::Members(ChatMembers {
        chat: ChatId(chat_id.into()),
        users,
        partial,
    })
}

fn members_failed(chat_id: &str, reason: &str, offline: bool) -> ClientEvent {
    ClientEvent::MembersFailed {
        chat: ChatId(chat_id.into()),
        reason: reason.into(),
        offline,
    }
}

/// Brings the client online the way the core does: a state is announced only when it changes.
fn go_online(h: &Harness, cx: &mut VisualTestContext) {
    if h.store.read_with(cx, |s, _| s.connection()) != ConnectionState::Online {
        h.feed(cx, [ClientEvent::Connection(ConnectionState::Online)]);
    }
}

/// Opens the group "g" with one message and its info panel; the drained commands are the member request.
fn open_info(h: &Harness, cx: &mut VisualTestContext) {
    go_online(h, cx);
    open_with(h, cx, vec![text("g", 1)]);
    click(cx, "chat-header");
    assert_eq!(
        h.commands(),
        [Command::LoadMembers(chat_g())],
        "opening asks for the members, nothing else"
    );
}

fn info_open(h: &Harness, cx: &mut VisualTestContext) -> bool {
    h.desktop.read_with(cx, |d, _| d.info_is_open())
}

#[gpui::test]
fn the_panel_lists_the_users_of_a_group_and_marks_admins_only(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_info(&h, cx);
    assert!(shown(cx, "info-panel"));
    assert!(
        shown(cx, "info-loading"),
        "a loading state, not an empty list"
    );
    assert!(!shown(cx, "info-count"));
    assert!(!shown(cx, "member-a"));

    h.feed(
        cx,
        [members_of(
            "g",
            vec![
                member("b", "Борис", MemberRole::Member, false),
                member("a", "Алиса", MemberRole::Admin, false),
                member("m", "Я", MemberRole::Member, true),
            ],
            false,
        )],
    );
    assert!(shown(cx, "info-count"));
    assert!(!shown(cx, "info-partial"));
    for row in ["member-a", "member-b", "member-m"] {
        assert!(shown(cx, row), "{row}");
    }
    assert!(shown(cx, "member-a-avatar"));
    assert!(shown(cx, "member-a-admin"), "the admin role is marked");
    assert!(!shown(cx, "member-b-admin"), "a plain member is not");
    assert!(!shown(cx, "member-m-admin"));
    assert!(
        y(cx, "member-b") < y(cx, "member-a"),
        "the order of the list is the server's, admins are not moved up"
    );
    assert!(h.commands().is_empty(), "viewing sends no other request");
}

#[gpui::test]
fn a_failed_load_shows_the_reason_and_retries(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_info(&h, cx);
    h.feed(cx, [members_failed("g", "сервер ответил ошибкой", false)]);
    assert!(shown(cx, "info-error"));
    assert!(shown(cx, "info-error-reason"));
    assert!(!shown(cx, "info-count"), "no list is presented");
    assert!(!shown(cx, "info-empty"));
    assert!(!shown(cx, "info-offline"));

    click(cx, "info-retry");
    assert_eq!(h.commands(), [Command::LoadMembers(chat_g())]);
    let state = |h: &Harness, cx: &mut VisualTestContext| {
        h.store
            .read_with(cx, |s, _| s.members_state(&chat_g()).cloned())
    };
    assert_eq!(state(&h, cx), Some(crate::store::MembersState::Loading));
    h.feed(
        cx,
        [members_of(
            "g",
            vec![member("a", "Алиса", MemberRole::Member, false)],
            false,
        )],
    );
    assert!(shown(cx, "member-a"));
    assert!(
        info_open(&h, cx),
        "the chat itself keeps working: the panel stays and the timeline is there"
    );
    assert!(shown(cx, msg!(1)));
}

#[gpui::test]
fn a_refused_list_says_so_and_is_not_an_empty_list(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_group(&h, cx);
    click(cx, row!("ch"));
    h.feed(cx, [chunk("ch", 1..=2, true)]);
    h.commands();
    click(cx, "chat-header");
    assert_eq!(h.commands(), [Command::LoadMembers(ChatId("ch".into()))]);
    h.feed(
        cx,
        [members_failed(
            "ch",
            "список участников недоступен для этой учётной записи",
            false,
        )],
    );
    assert!(shown(cx, "info-error-reason"));
    assert!(!shown(cx, "info-empty"));
    assert!(!shown(cx, "info-count"));
    assert!(shown(cx, "timeline"), "the chat keeps working");
}

#[gpui::test]
fn offline_the_panel_says_the_list_is_unavailable(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_info(&h, cx);
    h.feed(cx, [members_failed("g", "Нет соединения", true)]);
    assert!(shown(cx, "info-offline"));
    assert!(!shown(cx, "info-error"), "an offline notice, not an error");
    assert!(!shown(cx, "info-count"));
}

#[gpui::test]
fn a_loaded_list_gives_way_to_the_offline_notice_when_the_connection_drops(
    cx: &mut TestAppContext,
) {
    let (h, cx) = harness(cx);
    open_info(&h, cx);
    h.feed(
        cx,
        [members_of(
            "g",
            vec![member("a", "Алиса", MemberRole::Member, false)],
            false,
        )],
    );
    assert!(shown(cx, "info-count"));
    assert!(shown(cx, "member-a"));
    assert!(!shown(cx, "info-offline"));

    h.feed(cx, [ClientEvent::Connection(ConnectionState::Offline)]);
    assert!(
        shown(cx, "info-offline"),
        "an offline notice instead of the stale list"
    );
    assert!(!shown(cx, "info-error"));
    h.store.read_with(cx, |s, _| {
        let body = crate::info_panel::body(s.members_state(&chat_g()), s.connection());
        assert_eq!(body, crate::info_panel::Body::Offline);
    });
    assert!(h.commands().is_empty(), "going offline sends nothing");
}

#[gpui::test]
fn a_list_that_arrives_after_the_connection_dropped_is_not_shown(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_info(&h, cx);
    h.feed(cx, [ClientEvent::Connection(ConnectionState::Offline)]);
    h.feed(
        cx,
        [members_of(
            "g",
            vec![member("a", "Алиса", MemberRole::Member, false)],
            false,
        )],
    );
    assert!(
        shown(cx, "info-offline"),
        "the late answer does not repaint the list"
    );
    assert!(!shown(cx, "info-count"));
    assert!(!shown(cx, "member-a"));
}

#[gpui::test]
fn coming_back_online_asks_for_the_list_again(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_info(&h, cx);
    h.feed(
        cx,
        [members_of(
            "g",
            vec![member("a", "Алиса", MemberRole::Member, false)],
            false,
        )],
    );
    h.feed(cx, [ClientEvent::Connection(ConnectionState::Offline)]);
    assert!(h.commands().is_empty());

    h.feed(cx, [ClientEvent::Connection(ConnectionState::Connecting)]);
    assert!(
        h.commands().is_empty(),
        "nothing is asked while still connecting"
    );
    h.feed(cx, [ClientEvent::Connection(ConnectionState::Online)]);
    assert_eq!(
        h.commands(),
        [Command::LoadMembers(chat_g())],
        "the list from before the drop is not shown as current"
    );
    assert!(shown(cx, "info-loading"));
    h.feed(
        cx,
        [members_of(
            "g",
            vec![member("a", "Алиса", MemberRole::Member, false)],
            false,
        )],
    );
    assert!(shown(cx, "info-count"));
}

#[test]
fn the_panel_body_follows_the_state_and_the_connection() {
    use crate::info_panel::{Body, body};
    use crate::store::MembersState;
    let users = vec![member("a", "А", MemberRole::Member, false)];
    let ready = MembersState::Ready(ChatMembers {
        chat: chat_g(),
        users,
        partial: true,
    });
    assert_eq!(
        body(Some(&ready), ConnectionState::Online),
        Body::Ready {
            shown: 1,
            partial: true
        }
    );
    assert_eq!(body(Some(&ready), ConnectionState::Offline), Body::Offline);
    assert_eq!(
        body(Some(&ready), ConnectionState::Connecting),
        Body::Offline
    );
    assert_eq!(
        body(Some(&MembersState::Loading), ConnectionState::Offline),
        Body::Loading
    );
    let failed = MembersState::Failed {
        reason: "r".into(),
        offline: false,
    };
    assert_eq!(
        body(Some(&failed), ConnectionState::Offline),
        Body::Failed("r".into())
    );
}

#[gpui::test]
fn a_bounded_list_says_how_many_are_shown_and_that_more_may_exist(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_info(&h, cx);
    h.feed(
        cx,
        [members_of(
            "g",
            vec![member("a", "Алиса", MemberRole::Member, false)],
            true,
        )],
    );
    assert!(shown(cx, "info-partial"));
    assert!(
        !shown(cx, "info-count"),
        "a bounded list is not presented as the full one"
    );
    assert!(shown(cx, "member-a"));

    assert_eq!(
        crate::info_panel::count_text(1000, true),
        "Показано участников: 1000. Список может быть неполным"
    );
    assert_eq!(crate::info_panel::count_text(58, false), "Участников: 58");
}

#[gpui::test]
fn a_private_chat_shows_its_partner_without_group_parts(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    go_online(&h, cx);
    open_group(&h, cx);
    click(cx, row!("p"));
    h.feed(cx, [chunk("p", 1..=2, true)]);
    h.commands();
    click(cx, "chat-header");
    let private = ChatId("p".into());
    assert_eq!(
        h.commands(),
        [Command::LoadMembers(private)],
        "the core answers from the chat row, the panel sends nothing else"
    );
    h.feed(
        cx,
        [members_of(
            "p",
            vec![member("anna", "Анна", MemberRole::Member, false)],
            false,
        )],
    );
    assert!(shown(cx, "member-anna"));
    assert!(
        !shown(cx, "member-anna-admin"),
        "no role information, no marker"
    );
    assert!(!shown(cx, "info-partial"));
    assert!(!shown(cx, "info-empty"));
}

#[gpui::test]
fn a_private_chat_without_a_named_partner_lists_nobody(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    go_online(&h, cx);
    open_group(&h, cx);
    click(cx, row!("p"));
    h.feed(cx, [chunk("p", 1..=2, true)]);
    h.commands();
    click(cx, "chat-header");
    h.feed(cx, [members_of("p", Vec::new(), false)]);
    assert!(shown(cx, "info-empty"));
    assert!(
        !shown(cx, "info-members"),
        "no participant rows, none invented"
    );
}

#[gpui::test]
fn the_panel_closes_by_header_button_escape_and_chat_switch(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_info(&h, cx);
    assert!(info_open(&h, cx));
    h.feed(
        cx,
        [members_of(
            "g",
            vec![member("a", "Алиса", MemberRole::Member, false)],
            false,
        )],
    );

    click(cx, "chat-header");
    assert!(!info_open(&h, cx), "the header toggles");
    assert!(h.commands().is_empty(), "closing sends nothing");

    click(cx, "chat-header");
    assert_eq!(
        h.commands(),
        [Command::LoadMembers(chat_g())],
        "a re-open asks again"
    );
    click(cx, "info-close");
    assert!(!info_open(&h, cx), "the close button");

    click(cx, "chat-header");
    h.commands();
    cx.simulate_keystrokes("escape");
    assert!(!info_open(&h, cx), "Escape");

    click(cx, "chat-header");
    h.commands();
    assert!(info_open(&h, cx));
    click(cx, row!("p"));
    assert!(!info_open(&h, cx), "another chat closes the panel");
    assert!(
        h.commands()
            .iter()
            .all(|c| !matches!(c, Command::LoadMembers(_))),
        "and asks for nothing"
    );
}

#[gpui::test]
fn closing_the_panel_keeps_the_timeline_scroll(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, (1..=80).map(|i| text("g", i)).collect());
    scroll(cx, 800.);
    let before = h.list(cx).logical_scroll_top();
    assert!(before.item_ix < h.list(cx).item_count(), "scrolled up");

    click(cx, "chat-header");
    h.feed(
        cx,
        [members_of(
            "g",
            vec![member("a", "Алиса", MemberRole::Admin, false)],
            false,
        )],
    );
    assert!(shown(cx, "info-panel"));
    click(cx, "info-close");
    cx.run_until_parked();
    let after = h.list(cx).logical_scroll_top();
    assert_eq!(
        (after.item_ix, after.offset_in_item),
        (before.item_ix, before.offset_in_item)
    );
}

#[gpui::test]
fn a_covered_chat_does_not_open_the_panel(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1)]);
    h.desktop.update(cx, |d, cx| d.toggle_log(cx));
    h.desktop.update(cx, |d, cx| d.toggle_info(cx));
    assert!(!info_open(&h, cx));
    assert!(h.commands().is_empty());
}

// ---- read-only profile (CM 4.2) ----

fn profile_of(name: Option<&str>, status: Option<&str>) -> Profile {
    Profile {
        user: UserId("me".into()),
        name: name.map(Into::into),
        avatar: None,
        status_text: status.map(Into::into),
    }
}

fn profile_state(h: &Harness, cx: &mut VisualTestContext) -> Option<crate::store::ProfileState> {
    h.store.read_with(cx, |s, _| s.profile_state().cloned())
}

fn profile_open(h: &Harness, cx: &mut VisualTestContext) -> bool {
    h.desktop.read_with(cx, |d, _| d.profile_is_open())
}

/// Opens the profile through the account menu; the drained commands are the profile request.
fn open_profile(h: &Harness, cx: &mut VisualTestContext) {
    go_online(h, cx);
    click(cx, "menu-button");
    click(cx, "menu-profile");
    assert_eq!(
        h.commands(),
        [Command::LoadProfile],
        "opening asks for the profile, nothing else"
    );
}

#[gpui::test]
fn the_account_menu_opens_the_profile_with_name_avatar_and_status(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    go_online(&h, cx);
    assert!(!shown(cx, "profile-view"));
    click(cx, "menu-button");
    assert!(shown(cx, "menu-profile"));
    click(cx, "menu-profile");
    assert_eq!(h.commands(), [Command::LoadProfile]);
    assert!(profile_open(&h, cx));
    assert_eq!(
        h.desktop.read_with(cx, |d, _| d.menu),
        AccountMenu::Closed,
        "the menu closes"
    );
    assert!(shown(cx, "profile-view"));
    assert!(
        shown(cx, "profile-loading"),
        "a loading state, not an empty profile"
    );
    assert!(!shown(cx, "profile-name"));

    h.feed(
        cx,
        [ClientEvent::Profile(profile_of(
            Some("Иван Петров"),
            Some("в отпуске до пятницы"),
        ))],
    );
    assert!(shown(cx, "profile-name"));
    assert!(shown(cx, "profile-avatar"));
    assert!(
        shown(cx, "profile-avatar-initials"),
        "initials while there is no picture"
    );
    assert!(
        shown(cx, "profile-status"),
        "the status text is shown when the server gave one"
    );
    assert!(!shown(cx, "profile-error"));
    assert!(!shown(cx, "profile-offline"));
    assert!(h.commands().is_empty(), "viewing sends no other request");
}

#[gpui::test]
fn the_profile_avatar_is_the_downloaded_picture(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_profile(&h, cx);
    let mut profile = profile_of(Some("Иван"), None);
    profile.avatar = Some(media("me-av", MediaKind::Avatar));
    h.feed(cx, [ClientEvent::Profile(profile)]);
    assert_eq!(
        h.commands(),
        [Command::FetchMedia(media("me-av", MediaKind::Avatar))]
    );
    assert!(shown(cx, "profile-avatar-initials"));
    h.feed(
        cx,
        [ClientEvent::MediaReady {
            media: media("me-av", MediaKind::Avatar),
            path: PathBuf::from("/tmp/me-av.png"),
        }],
    );
    assert!(shown(cx, "profile-avatar-image"));
}

#[gpui::test]
fn no_status_information_shows_no_status_line(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_profile(&h, cx);
    h.feed(
        cx,
        [ClientEvent::Profile(profile_of(Some("Иван Петров"), None))],
    );
    assert!(shown(cx, "profile-name"));
    assert!(!shown(cx, "profile-status"), "an empty status is no line");
}

#[gpui::test]
fn a_failed_profile_load_says_so_and_retries(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_profile(&h, cx);
    h.feed(
        cx,
        [ClientEvent::ProfileFailed {
            reason: "сервер ответил ошибкой".into(),
            offline: false,
        }],
    );
    assert!(shown(cx, "profile-error"));
    assert!(shown(cx, "profile-error-reason"));
    assert!(shown(cx, "profile-retry"));
    assert!(!shown(cx, "profile-offline"));
    assert!(
        !shown(cx, "profile-name"),
        "no name, avatar or status from anywhere else"
    );
    assert!(!shown(cx, "profile-avatar"));
    assert!(!shown(cx, "profile-status"));

    click(cx, "profile-retry");
    assert_eq!(h.commands(), [Command::LoadProfile]);
    assert_eq!(
        profile_state(&h, cx),
        Some(crate::store::ProfileState::Loading)
    );
    h.feed(cx, [ClientEvent::Profile(profile_of(Some("Иван"), None))]);
    assert!(shown(cx, "profile-name"));
    assert!(profile_open(&h, cx));
}

#[gpui::test]
fn offline_the_profile_says_it_could_not_be_loaded(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_profile(&h, cx);
    h.feed(
        cx,
        [ClientEvent::ProfileFailed {
            reason: "Нет соединения".into(),
            offline: true,
        }],
    );
    assert!(shown(cx, "profile-offline"));
    assert!(
        !shown(cx, "profile-error"),
        "an offline notice, not an error"
    );
    assert!(!shown(cx, "profile-name"));
    assert!(!shown(cx, "profile-avatar"));
    assert!(!shown(cx, "profile-status"));
}

#[gpui::test]
fn a_loaded_profile_gives_way_to_the_offline_notice_when_the_connection_drops(
    cx: &mut TestAppContext,
) {
    let (h, cx) = harness(cx);
    open_profile(&h, cx);
    h.feed(
        cx,
        [ClientEvent::Profile(profile_of(
            Some("Иван"),
            Some("на связи"),
        ))],
    );
    assert!(shown(cx, "profile-name"));
    assert!(!shown(cx, "profile-offline"));

    h.feed(cx, [ClientEvent::Connection(ConnectionState::Offline)]);
    assert!(
        shown(cx, "profile-offline"),
        "an offline notice instead of the stale profile"
    );
    assert!(!shown(cx, "profile-error"));
    h.store.read_with(cx, |s, _| {
        assert_eq!(
            crate::profile::body(s.profile_state(), s.connection()),
            crate::profile::Body::Offline
        );
    });
    assert!(h.commands().is_empty(), "going offline sends nothing");

    // Back online: the profile is asked for again, the old one is not shown as current.
    h.feed(cx, [ClientEvent::Connection(ConnectionState::Online)]);
    assert_eq!(h.commands(), [Command::LoadProfile]);
    assert_eq!(
        profile_state(&h, cx),
        Some(crate::store::ProfileState::Loading)
    );
    assert!(shown(cx, "profile-loading"));
}

#[gpui::test]
fn a_profile_that_arrives_after_the_connection_dropped_is_not_shown(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_profile(&h, cx);
    h.feed(cx, [ClientEvent::Connection(ConnectionState::Offline)]);
    h.feed(
        cx,
        [ClientEvent::Profile(profile_of(
            Some("Иван"),
            Some("на связи"),
        ))],
    );
    assert!(
        shown(cx, "profile-offline"),
        "the late answer does not repaint the profile"
    );
    assert!(!shown(cx, "profile-name"));
    assert!(!shown(cx, "profile-status"));
}

#[gpui::test]
fn every_open_asks_again_and_shows_nothing_from_before(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_profile(&h, cx);
    h.feed(
        cx,
        [ClientEvent::Profile(profile_of(
            Some("Иван"),
            Some("на связи"),
        ))],
    );
    click(cx, "profile-close");
    assert!(!profile_open(&h, cx));
    assert!(h.commands().is_empty(), "closing sends nothing");

    open_profile(&h, cx);
    assert_eq!(
        profile_state(&h, cx),
        Some(crate::store::ProfileState::Loading),
        "the old profile is not current"
    );
    h.store.read_with(cx, |s, _| {
        assert_eq!(
            crate::profile::body(s.profile_state(), s.connection()),
            crate::profile::Body::Loading
        );
    });
}

#[test]
fn the_profile_body_follows_the_state() {
    use crate::profile::{Body, body};
    use crate::store::ProfileState;
    let online = ConnectionState::Online;
    assert_eq!(body(None, online), Body::Loading);
    assert_eq!(body(Some(&ProfileState::Loading), online), Body::Loading);
    assert_eq!(
        body(
            Some(&ProfileState::Failed {
                reason: "r".into(),
                offline: true
            }),
            online
        ),
        Body::Offline
    );
    assert_eq!(
        body(
            Some(&ProfileState::Failed {
                reason: "r".into(),
                offline: false
            }),
            online
        ),
        Body::Failed("r".into())
    );
    assert_eq!(
        body(Some(&ProfileState::Ready(profile_of(None, None))), online),
        Body::Ready {
            name: "Без имени".into(),
            status: None
        }
    );
    assert_eq!(
        body(
            Some(&ProfileState::Ready(profile_of(
                Some("Иван"),
                Some("занят")
            ))),
            online
        ),
        Body::Ready {
            name: "Иван".into(),
            status: Some("занят".into())
        }
    );
    let ready = ProfileState::Ready(profile_of(Some("Иван"), None));
    assert_eq!(body(Some(&ready), ConnectionState::Offline), Body::Offline);
    assert_eq!(
        body(Some(&ready), ConnectionState::Connecting),
        Body::Offline
    );
    assert_eq!(
        body(Some(&ProfileState::Loading), ConnectionState::Offline),
        Body::Loading
    );
}

#[gpui::test]
fn the_profile_has_no_input_and_changes_nothing(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_profile(&h, cx);
    h.feed(
        cx,
        [ClientEvent::Profile(profile_of(
            Some("Иван"),
            Some("на связи"),
        ))],
    );
    for selector in [
        "profile-edit",
        "profile-name-input",
        "profile-status-input",
        "profile-avatar-edit",
    ] {
        assert!(!shown(cx, selector), "{selector}");
    }
    // Clicking the fields and typing reaches no input: nothing changes and nothing is sent.
    click(cx, "profile-name");
    click(cx, "profile-status");
    cx.simulate_input("zzz");
    assert_eq!(composer_text(&h, cx), "");
    assert_eq!(
        h.desktop
            .read_with(cx, |d, cx| d.search.read(cx).content.to_string()),
        ""
    );
    assert_eq!(
        profile_state(&h, cx),
        Some(crate::store::ProfileState::Ready(profile_of(
            Some("Иван"),
            Some("на связи")
        )))
    );
    cx.run_until_parked();
    assert!(h.commands().is_empty());
}

#[gpui::test]
fn the_profile_blocks_the_chat_and_escape_closes_it(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1)]);
    open_profile(&h, cx);
    assert!(
        h.desktop.read_with(cx, |d, cx| d.blocked(cx)),
        "blocks the chat like other overlays"
    );
    h.desktop.update(cx, |d, cx| d.toggle_info(cx));
    assert!(
        !info_open(&h, cx),
        "the info panel does not open under the profile"
    );
    assert!(h.commands().is_empty());

    h.feed(cx, [ClientEvent::Profile(profile_of(Some("Иван"), None))]);
    cx.simulate_keystrokes("escape");
    assert!(!profile_open(&h, cx), "Escape");
    assert!(!h.desktop.read_with(cx, |d, cx| d.blocked(cx)));
    assert!(shown(cx, msg!(1)), "the chat is as it was");

    open_profile(&h, cx);
    click(cx, "profile-close");
    assert!(!profile_open(&h, cx), "the close button");
}

#[gpui::test]
fn an_ended_session_closes_the_profile(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_profile(&h, cx);
    h.feed(cx, [ClientEvent::SessionExpired]);
    assert!(!profile_open(&h, cx));
    assert!(shown(cx, "session-expired"));
}

// ---- new chat flow (CM 4.3) ----

fn person_chat(id: &str, title: &str, partner: &str) -> ChatSummary {
    let mut chat = chat(id, ChatKind::Private, title, Some(1));
    chat.partner = Some(UserId(partner.into()));
    chat
}

/// The group "g", a private chat with Анна (`anna`) and the self chat (partner `me`).
fn people_chats() -> Vec<ChatSummary> {
    vec![
        chat("g", ChatKind::Group, "Команда", Some(1)),
        person_chat("p", "Анна", "anna"),
        person_chat("s", "Избранное", "me"),
    ]
}

/// Online, the group "g" open with one message; the drained commands are the usual opening ones.
fn open_people(h: &Harness, cx: &mut VisualTestContext) {
    h.feed(
        cx,
        [
            ClientEvent::ChatsReplaced(people_chats()),
            ClientEvent::Connection(ConnectionState::Online),
        ],
    );
    click(cx, row!("g"));
    h.feed(cx, [history("g", vec![text("g", 1)], true)]);
    h.commands();
}

/// As `open_people`, with the info panel of "g" showing Алиса, Борис, Анна and the current user.
fn open_people_panel(h: &Harness, cx: &mut VisualTestContext) {
    open_people(h, cx);
    click(cx, "chat-header");
    h.commands();
    h.feed(
        cx,
        [members_of(
            "g",
            vec![
                member("alice", "Алиса", MemberRole::Member, false),
                member("boris", "Борис", MemberRole::Admin, false),
                member("anna", "Анна", MemberRole::Member, false),
                member("me", "Я", MemberRole::Member, true),
            ],
            false,
        )],
    );
}

fn dialog_open(h: &Harness, cx: &mut VisualTestContext) -> bool {
    h.desktop.read_with(cx, |d, _| d.new_chat_is_open())
}

/// Presses "Новый чат"; the dialog asks for the profile once (to know who "me" is), nothing else.
fn open_dialog(h: &Harness, cx: &mut VisualTestContext) {
    click(cx, "new-chat-button");
    cx.run_until_parked();
    assert!(dialog_open(h, cx));
    assert_eq!(
        h.commands(),
        [Command::LoadProfile],
        "opening reads the profile, creates nothing"
    );
}

fn chosen_person(h: &Harness, cx: &mut VisualTestContext) -> Option<String> {
    h.desktop
        .read_with(cx, |d, _| d.new_chat_choice().map(|c| c.user.0.clone()))
}

fn creation(h: &Harness, cx: &mut VisualTestContext) -> Option<crate::store::CreationState> {
    h.store.read_with(cx, |s, _| s.creation_state().cloned())
}

fn selected_chat(h: &Harness, cx: &mut VisualTestContext) -> Option<ChatId> {
    h.store.read_with(cx, |s, _| s.selected().cloned())
}

fn create_alice() -> Command {
    Command::CreatePrivateChat {
        user: UserId("alice".into()),
    }
}

fn creations(commands: &[Command]) -> usize {
    commands
        .iter()
        .filter(|c| matches!(c, Command::CreatePrivateChat { .. }))
        .count()
}

/// The dialog with Алиса (no chat with her yet) chosen from the open panel, ready to confirm.
fn choose_alice(h: &Harness, cx: &mut VisualTestContext) {
    open_people_panel(h, cx);
    open_dialog(h, cx);
    click(cx, "candidate-alice");
    assert_eq!(chosen_person(h, cx).as_deref(), Some("alice"));
    assert!(h.commands().is_empty(), "choosing a person sends nothing");
}

#[gpui::test]
fn new_chat_is_disabled_offline_and_opens_the_dialog_online(cx: &mut TestAppContext) {
    let (h, cx) = harness_with(
        cx,
        vec![
            ClientEvent::ChatsReplaced(people_chats()),
            ClientEvent::Connection(ConnectionState::Offline),
        ],
        |_, _| {},
    );
    assert!(
        shown(cx, "new-chat-button"),
        "the action is beside the search field"
    );
    let (search, button) = (
        bounds(cx, "search").unwrap(),
        bounds(cx, "new-chat-button").unwrap(),
    );
    assert!(
        search.right() <= button.left(),
        "to the right of the search field"
    );
    assert!(
        button.center().y > search.top() && button.center().y < search.bottom(),
        "on its line"
    );
    click(cx, "new-chat-button");
    cx.run_until_parked();
    assert!(!dialog_open(&h, cx), "offline the action does nothing");
    assert!(!shown(cx, "new-chat-view"));
    assert!(h.commands().is_empty());

    h.feed(cx, [ClientEvent::Connection(ConnectionState::Online)]);
    open_dialog(&h, cx);
    assert!(shown(cx, "new-chat-view"));
    assert!(shown(cx, "new-chat-search"));
    assert!(shown(cx, row!("p")), "the chat list is unchanged");
    assert_eq!(h.store.read_with(cx, |s, _| s.chat_count()), 3);
}

#[gpui::test]
fn the_dialog_offers_known_people_without_the_current_user_or_duplicates(cx: &mut TestAppContext) {
    use crate::new_chat::candidates;
    let (h, cx) = harness(cx);
    open_people_panel(&h, cx);
    let names = |h: &Harness, cx: &mut VisualTestContext, panel: Option<ChatId>| -> Vec<String> {
        h.store.read_with(cx, |s, _| {
            candidates(s, panel.as_ref())
                .into_iter()
                .map(|c| c.name)
                .collect()
        })
    };
    assert_eq!(
        names(&h, cx, Some(chat_g())),
        ["Алиса", "Анна", "Борис"],
        "private partners and the open panel's members, once each; not the self chat, not the `me` member"
    );
    assert_eq!(
        names(&h, cx, None),
        ["Анна"],
        "without the panel only the partners of private chats are known"
    );
    assert_eq!(
        names(&h, cx, Some(ChatId("p".into()))),
        ["Анна"],
        "a panel without a loaded list adds nobody"
    );

    open_dialog(&h, cx);
    for row in ["candidate-alice", "candidate-boris", "candidate-anna"] {
        assert!(shown(cx, row), "{row}");
    }
    assert!(!shown(cx, "candidate-me"), "never the current user");

    // The loaded profile names the current user even when no member list does.
    h.feed(cx, [ClientEvent::Profile(profile_of(Some("Я"), None))]);
    h.store
        .read_with(cx, |s, _| assert_eq!(candidates(s, None).len(), 1));
}

#[gpui::test]
fn a_closed_panel_adds_no_candidates(cx: &mut TestAppContext) {
    use crate::new_chat::candidates;
    let (h, cx) = harness(cx);
    open_people_panel(&h, cx);
    click(cx, "info-close");
    open_dialog(&h, cx);
    assert!(shown(cx, "candidate-anna"));
    assert!(
        !shown(cx, "candidate-alice"),
        "members of a chat whose panel is closed are not offered"
    );
    h.store
        .read_with(cx, |s, _| assert_eq!(candidates(s, None).len(), 1));
}

#[gpui::test]
fn typing_narrows_the_people_and_enter_creates_nothing(cx: &mut TestAppContext) {
    use crate::new_chat::{candidates, matching};
    let (h, cx) = harness(cx);
    open_people_panel(&h, cx);
    open_dialog(&h, cx);
    cx.simulate_input("бор");
    assert_eq!(
        h.desktop
            .read_with(cx, |d, cx| d.new_chat_query(cx))
            .as_deref(),
        Some("бор")
    );
    h.store.read_with(cx, |s, _| {
        let all = candidates(s, Some(&chat_g()));
        assert_eq!(
            matching(&all, " БОР ")
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            ["Борис"]
        );
        assert_eq!(matching(&all, "").len(), 3);
    });
    assert_eq!(composer_text(&h, cx), "", "the text went to the dialog");

    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(
        h.commands().is_empty(),
        "Enter in the search field sends nothing"
    );
    assert!(dialog_open(&h, cx));
    assert_eq!(creation(&h, cx), None);

    // Also with a person chosen: only the button creates.
    click(cx, "candidate-boris");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(chosen_person(&h, cx).as_deref(), Some("boris"));
    assert!(h.commands().is_empty());
    assert_eq!(creation(&h, cx), None);

    cx.simulate_input("яяя");
    assert!(
        shown(cx, "new-chat-nomatch"),
        "a query nobody matches says so"
    );
}

#[gpui::test]
fn an_account_with_nobody_known_says_so(cx: &mut TestAppContext) {
    let (h, cx) = harness_with(
        cx,
        vec![
            ClientEvent::ChatsReplaced(vec![chat("g", ChatKind::Group, "Команда", Some(1))]),
            ClientEvent::Connection(ConnectionState::Online),
        ],
        |_, _| {},
    );
    open_dialog(&h, cx);
    h.feed(cx, [ClientEvent::Profile(profile_of(Some("Я"), None))]);
    assert!(
        shown(cx, "new-chat-empty"),
        "the identity is known now, and nobody else is"
    );
    assert!(!shown(cx, "new-chat-participant"));
}

#[gpui::test]
fn the_self_chat_is_not_offered_before_the_identity_is_known(cx: &mut TestAppContext) {
    let (h, cx) = harness_with(
        cx,
        vec![
            ClientEvent::ChatsReplaced(people_chats()),
            ClientEvent::Connection(ConnectionState::Online),
        ],
        |_, _| {},
    );
    // Cold start: no profile and no member list, so nobody can tell which partner is the current user.
    open_dialog(&h, cx);
    assert!(
        shown(cx, "new-chat-identity"),
        "the dialog says it is still loading"
    );
    for row in ["candidate-me", "candidate-anna"] {
        assert!(!shown(cx, row), "{row} waits for the identity");
    }
    h.store.read_with(cx, |s, _| {
        assert!(crate::new_chat::candidates(s, None).is_empty())
    });

    // The profile fails: the self chat stays out and the dialog says why.
    h.feed(
        cx,
        [ClientEvent::ProfileFailed {
            reason: "Нет соединения".into(),
            offline: true,
        }],
    );
    assert!(shown(cx, "new-chat-identity"));
    assert!(!shown(cx, "candidate-me") && !shown(cx, "candidate-anna"));
    h.store.read_with(cx, |s, _| {
        assert!(matches!(
            s.profile_state(),
            Some(crate::store::ProfileState::Failed { .. })
        ));
        assert!(!crate::new_chat::identity_known(s));
        assert!(
            crate::new_chat::candidates(s, None).is_empty(),
            "a failed profile leaves the self chat out"
        );
    });
    assert_eq!(h.commands(), [], "nothing is created or retried by itself");

    // The profile arrives on another try: the others are offered, the current user never.
    h.store.update(cx, |s, cx| s.load_profile(cx));
    h.feed(cx, [ClientEvent::Profile(profile_of(Some("Я"), None))]);
    h.store
        .read_with(cx, |s, _| assert!(crate::new_chat::identity_known(s)));
    assert!(shown(cx, "candidate-anna"));
    assert!(!shown(cx, "candidate-me"));
}

#[gpui::test]
fn the_confirmation_lists_the_participant_and_one_confirm_sends_one_command(
    cx: &mut TestAppContext,
) {
    let (h, cx) = harness(cx);
    open_people_panel(&h, cx);
    open_dialog(&h, cx);
    assert!(!shown(cx, "new-chat-participant"), "nobody is chosen yet");
    assert!(shown(cx, "new-chat-create") && shown(cx, "new-chat-cancel"));
    click(cx, "new-chat-create");
    assert!(
        h.commands().is_empty(),
        "nothing to create before a person is chosen"
    );

    click(cx, "candidate-alice");
    assert!(
        shown(cx, "new-chat-participant"),
        "the participant is listed"
    );
    assert!(shown(cx, "new-chat-participant-avatar"));
    assert_eq!(creation(&h, cx), None);
    assert!(h.commands().is_empty());

    click(cx, "new-chat-create");
    click(cx, "new-chat-create");
    cx.run_until_parked();
    assert_eq!(
        h.commands(),
        [create_alice()],
        "one command per confirm, also on a double click"
    );
    assert_eq!(
        creation(&h, cx),
        Some(crate::store::CreationState::InFlight)
    );
    assert!(shown(cx, "new-chat-sending"));
    h.store.read_with(cx, |s, _| {
        assert!(
            !crate::new_chat::can_create(true, s.creation_state()),
            "the button is disabled while in flight"
        );
    });
    click(cx, "candidate-boris");
    assert_eq!(
        chosen_person(&h, cx).as_deref(),
        Some("alice"),
        "the choice is locked while in flight"
    );
    assert!(h.commands().is_empty());
}

#[gpui::test]
fn a_created_chat_closes_the_dialog_and_is_opened(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    choose_alice(&h, cx);
    click(cx, "new-chat-create");
    assert_eq!(h.commands(), [create_alice()]);
    let mut chats = people_chats();
    chats.push(person_chat("n", "Алиса", "alice"));
    h.feed(
        cx,
        [
            ClientEvent::ChatsReplaced(chats),
            ClientEvent::ChatCreated {
                chat: Some(ChatId("n".into())),
                existing: false,
            },
        ],
    );
    assert!(!dialog_open(&h, cx));
    assert_eq!(creation(&h, cx), None);
    assert_eq!(
        selected_chat(&h, cx),
        Some(ChatId("n".into())),
        "the created chat is opened"
    );
    assert_eq!(
        h.commands(),
        [Command::OpenChat(ChatId("n".into()))],
        "nothing is created a second time"
    );
    assert!(shown(cx, row!("n")), "it is in the list");
}

#[gpui::test]
fn a_created_chat_the_list_does_not_name_closes_the_dialog_and_is_opened_once_listed(
    cx: &mut TestAppContext,
) {
    let (h, cx) = harness(cx);
    choose_alice(&h, cx);
    click(cx, "new-chat-create");
    h.commands();
    h.feed(
        cx,
        [ClientEvent::ChatCreated {
            chat: None,
            existing: false,
        }],
    );
    assert!(!dialog_open(&h, cx));
    assert_eq!(
        selected_chat(&h, cx),
        Some(chat_g()),
        "nothing to open yet, the open chat stays"
    );
    assert!(h.commands().is_empty());

    // The list later shows the private chat with that person: it is opened (spec "New private chat").
    let mut chats = people_chats();
    chats.push(person_chat("n", "Алиса", "alice"));
    h.feed(cx, [ClientEvent::ChatsReplaced(chats)]);
    assert_eq!(selected_chat(&h, cx), Some(ChatId("n".into())));
    assert_eq!(h.commands(), [Command::OpenChat(ChatId("n".into()))]);
}

#[gpui::test]
fn a_person_with_a_chat_opens_that_chat_without_a_command(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_people_panel(&h, cx);
    open_dialog(&h, cx);
    click(cx, "candidate-anna");
    assert!(!dialog_open(&h, cx), "no confirmation: nothing is created");
    assert_eq!(selected_chat(&h, cx), Some(ChatId("p".into())));
    let commands = h.commands();
    assert_eq!(creations(&commands), 0);
    assert_eq!(commands, [Command::OpenChat(ChatId("p".into()))]);
    assert_eq!(creation(&h, cx), None);
}

#[gpui::test]
fn a_chat_that_appeared_meanwhile_is_opened_instead_of_created(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    choose_alice(&h, cx);
    let mut chats = people_chats();
    chats.push(person_chat("n", "Алиса", "alice"));
    h.feed(cx, [ClientEvent::ChatsReplaced(chats)]);
    click(cx, "new-chat-create");
    assert!(!dialog_open(&h, cx));
    assert_eq!(selected_chat(&h, cx), Some(ChatId("n".into())));
    assert_eq!(
        creations(&h.commands()),
        0,
        "the existing chat is opened, no request goes out"
    );
}

#[gpui::test]
fn a_rejection_shows_the_reason_keeps_the_data_and_never_retries(cx: &mut TestAppContext) {
    use crate::new_chat::{Notice, notice};
    let (h, cx) = harness(cx);
    choose_alice(&h, cx);
    cx.simulate_input("али");
    click(cx, "new-chat-create");
    assert_eq!(h.commands(), [create_alice()]);
    h.feed(
        cx,
        [ClientEvent::CreationFailed {
            reason: "сервер отказал: нельзя создать чат".into(),
            unknown: false,
        }],
    );
    assert!(shown(cx, "new-chat-error"));
    assert!(!shown(cx, "new-chat-unknown"));
    assert!(dialog_open(&h, cx), "the dialog stays open");
    assert_eq!(
        chosen_person(&h, cx).as_deref(),
        Some("alice"),
        "the choice is kept"
    );
    assert_eq!(
        h.desktop
            .read_with(cx, |d, cx| d.new_chat_query(cx))
            .as_deref(),
        Some("али"),
        "so is the typed text"
    );
    h.store.read_with(cx, |s, _| {
        assert_eq!(
            notice(s.creation_state()),
            Notice::Failed("сервер отказал: нельзя создать чат".into())
        );
        assert!(
            crate::new_chat::can_create(true, s.creation_state()),
            "another explicit attempt is possible"
        );
    });
    cx.run_until_parked();
    assert!(h.commands().is_empty(), "nothing is sent by itself");

    click(cx, "new-chat-create");
    assert_eq!(
        h.commands(),
        [create_alice()],
        "only the user's press tries again"
    );
    assert_eq!(
        creation(&h, cx),
        Some(crate::store::CreationState::InFlight)
    );
}

#[gpui::test]
fn an_unknown_result_is_said_honestly_and_not_retried(cx: &mut TestAppContext) {
    use crate::new_chat::{Notice, notice};
    let (h, cx) = harness(cx);
    choose_alice(&h, cx);
    click(cx, "new-chat-create");
    h.commands();
    h.feed(
        cx,
        [
            ClientEvent::ChatsReplaced(people_chats()),
            ClientEvent::CreationFailed {
                reason: "Результат неизвестен: чат мог быть создан".into(),
                unknown: true,
            },
        ],
    );
    assert!(shown(cx, "new-chat-unknown"));
    assert!(
        !shown(cx, "new-chat-error"),
        "not presented as a plain failure"
    );
    assert!(dialog_open(&h, cx));
    assert_eq!(chosen_person(&h, cx).as_deref(), Some("alice"));
    h.store.read_with(cx, |s, _| {
        assert_eq!(
            notice(s.creation_state()),
            Notice::Unknown("Результат неизвестен: чат мог быть создан".into())
        );
    });
    cx.run_until_parked();
    assert!(h.commands().is_empty(), "no second request");
}

#[gpui::test]
fn an_unverified_result_says_the_list_could_not_be_read_and_the_next_attempt_is_the_users(
    cx: &mut TestAppContext,
) {
    use crate::new_chat::{Notice, notice};
    let (h, cx) = harness(cx);
    choose_alice(&h, cx);
    click(cx, "new-chat-create");
    h.commands();
    h.feed(
        cx,
        [ClientEvent::CreationUnverified {
            reason: "Результат неизвестен: список не обновлён".into(),
        }],
    );
    assert!(shown(cx, "new-chat-unverified"));
    assert!(
        !shown(cx, "new-chat-unknown"),
        "the list was not refreshed, the dialog must not say it was"
    );
    assert!(!shown(cx, "new-chat-error"));
    assert!(dialog_open(&h, cx));
    assert_eq!(chosen_person(&h, cx).as_deref(), Some("alice"));
    h.store.read_with(cx, |s, _| {
        assert_eq!(
            notice(s.creation_state()),
            Notice::Unverified("Результат неизвестен: список не обновлён".into())
        );
        assert!(crate::new_chat::can_create(true, s.creation_state()));
    });
    cx.run_until_parked();
    assert!(h.commands().is_empty(), "no second request by itself");

    click(cx, "new-chat-create");
    assert_eq!(
        h.commands(),
        [create_alice()],
        "the core owes the list check; the press is the user's"
    );
    assert_eq!(
        creation(&h, cx),
        Some(crate::store::CreationState::InFlight)
    );
}

#[gpui::test]
fn a_chat_that_appeared_after_an_unknown_result_is_opened(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    choose_alice(&h, cx);
    click(cx, "new-chat-create");
    h.commands();
    let mut chats = people_chats();
    chats.push(person_chat("n", "Алиса", "alice"));
    h.feed(
        cx,
        [
            ClientEvent::ChatsReplaced(chats),
            ClientEvent::ChatCreated {
                chat: Some(ChatId("n".into())),
                existing: true,
            },
        ],
    );
    assert!(!dialog_open(&h, cx));
    assert_eq!(selected_chat(&h, cx), Some(ChatId("n".into())));
    assert_eq!(creations(&h.commands()), 0);
}

#[gpui::test]
fn offline_the_refusal_is_shown_and_the_data_kept(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    choose_alice(&h, cx);
    cx.simulate_input("али");
    h.feed(cx, [ClientEvent::Connection(ConnectionState::Offline)]);
    assert!(
        dialog_open(&h, cx),
        "losing the connection does not close the dialog"
    );
    click(cx, "new-chat-create");
    // The core refuses at once, without a request to the server; the dialog says so.
    h.feed(
        cx,
        [ClientEvent::CreationFailed {
            reason: "Нет соединения".into(),
            unknown: false,
        }],
    );
    assert!(shown(cx, "new-chat-error"));
    assert!(dialog_open(&h, cx));
    assert_eq!(chosen_person(&h, cx).as_deref(), Some("alice"));
    assert_eq!(
        h.desktop
            .read_with(cx, |d, cx| d.new_chat_query(cx))
            .as_deref(),
        Some("али")
    );
    assert_eq!(
        creation(&h, cx),
        Some(crate::store::CreationState::Failed {
            reason: "Нет соединения".into(),
            unknown: false
        })
    );
    assert_eq!(
        creations(&h.commands()),
        1,
        "the one command of the press; the core answered it offline"
    );
}

#[gpui::test]
fn cancel_and_escape_close_the_dialog_without_sending(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    choose_alice(&h, cx);
    click(cx, "new-chat-cancel");
    assert!(!dialog_open(&h, cx));
    assert!(h.commands().is_empty());
    assert_eq!(creation(&h, cx), None);

    open_dialog_again(&h, cx);
    click(cx, "candidate-alice");
    cx.simulate_keystrokes("escape");
    assert!(!dialog_open(&h, cx), "Escape");
    assert!(h.commands().is_empty());
    assert!(
        info_open(&h, cx),
        "Escape closed the dialog, the panel under it stays"
    );

    // A failure is forgotten with the dialog.
    open_dialog_again(&h, cx);
    click(cx, "candidate-alice");
    click(cx, "new-chat-create");
    h.commands();
    h.feed(
        cx,
        [ClientEvent::CreationFailed {
            reason: "нельзя".into(),
            unknown: false,
        }],
    );
    click(cx, "new-chat-cancel");
    assert!(!dialog_open(&h, cx));
    assert_eq!(creation(&h, cx), None);
}

/// A second opening in the same window: the profile is known by now, so nothing is requested.
fn open_dialog_again(h: &Harness, cx: &mut VisualTestContext) {
    h.feed(cx, [ClientEvent::Profile(profile_of(Some("Я"), None))]);
    click(cx, "new-chat-button");
    cx.run_until_parked();
    assert!(dialog_open(h, cx));
}

#[gpui::test]
fn a_creation_in_flight_keeps_the_dialog_until_its_outcome_is_shown(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    choose_alice(&h, cx);
    click(cx, "new-chat-create");
    h.commands();
    click(cx, "new-chat-cancel");
    cx.simulate_keystrokes("escape");
    assert!(
        dialog_open(&h, cx),
        "a request that is out cannot be recalled; its outcome has to reach the user"
    );
    h.feed(
        cx,
        [ClientEvent::CreationFailed {
            reason: "нельзя".into(),
            unknown: false,
        }],
    );
    assert!(shown(cx, "new-chat-error"));
    cx.simulate_keystrokes("escape");
    assert!(!dialog_open(&h, cx));
}

fn create_group(title: &str, users: &[&str]) -> Command {
    Command::CreateGroupChat {
        title: title.into(),
        users: users.iter().map(|u| UserId((*u).into())).collect(),
    }
}

fn group_creations(commands: &[Command]) -> usize {
    commands
        .iter()
        .filter(|c| matches!(c, Command::CreateGroupChat { .. }))
        .count()
}

fn members_chosen(h: &Harness, cx: &mut VisualTestContext) -> Vec<String> {
    h.desktop.read_with(cx, |d, _| d.new_chat_members())
}

fn group_title(h: &Harness, cx: &mut VisualTestContext) -> String {
    h.desktop
        .read_with(cx, |d, cx| d.new_chat_title(cx))
        .unwrap_or_default()
}

/// The dialog in its group variant over the panel of "g" (Алиса, Борис, Анна offered); the title field has
/// the focus. Nothing was sent.
fn open_group_dialog(h: &Harness, cx: &mut VisualTestContext) {
    open_people_panel(h, cx);
    open_dialog(h, cx);
    click(cx, "new-chat-mode-group");
    assert!(
        h.commands().is_empty(),
        "switching the variant sends nothing"
    );
}

/// The group dialog with a title and Алиса and Борис picked, ready to confirm.
fn fill_group(h: &Harness, cx: &mut VisualTestContext) {
    open_group_dialog(h, cx);
    cx.simulate_input("Дружба");
    click(cx, "candidate-alice");
    click(cx, "candidate-boris");
    assert_eq!(group_title(h, cx), "Дружба");
    assert_eq!(members_chosen(h, cx), ["alice", "boris"]);
}

#[gpui::test]
fn the_group_variant_shows_the_title_field_and_leaves_the_private_one_intact(
    cx: &mut TestAppContext,
) {
    let (h, cx) = harness(cx);
    open_people_panel(&h, cx);
    open_dialog(&h, cx);
    assert!(shown(cx, "new-chat-mode-private") && shown(cx, "new-chat-mode-group"));
    assert!(!shown(cx, "new-chat-title"), "a private chat has no title");
    assert!(!shown(cx, "new-chat-members-empty"));

    click(cx, "candidate-alice");
    assert_eq!(chosen_person(&h, cx).as_deref(), Some("alice"));
    click(cx, "new-chat-mode-group");
    assert!(shown(cx, "new-chat-title") && shown(cx, "new-chat-search"));
    assert!(shown(cx, "new-chat-members-empty"), "nobody is in yet");
    assert!(members_chosen(&h, cx).is_empty());
    cx.simulate_input("Команда 2");
    assert_eq!(group_title(&h, cx), "Команда 2", "the title has the focus");
    assert_eq!(composer_text(&h, cx), "");

    // Back: the private choice and its row are as they were left.
    click(cx, "new-chat-mode-private");
    assert_eq!(chosen_person(&h, cx).as_deref(), Some("alice"));
    click(cx, "new-chat-mode-group");
    assert_eq!(group_title(&h, cx), "Команда 2", "the title is kept too");
    assert!(h.commands().is_empty());
}

#[gpui::test]
fn a_click_selects_and_deselects_and_a_cross_removes_a_participant(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_group_dialog(&h, cx);
    for row in ["candidate-alice", "candidate-boris", "candidate-anna"] {
        assert!(shown(cx, row), "{row}");
    }
    assert!(!shown(cx, "candidate-alice-check"));

    click(cx, "candidate-alice");
    assert_eq!(members_chosen(&h, cx), ["alice"]);
    assert!(shown(cx, "candidate-alice-check"), "the row is marked");
    assert!(shown(cx, "new-chat-member-alice"), "and listed");
    assert!(shown(cx, "new-chat-member-alice-avatar"));
    assert!(dialog_open(&h, cx), "choosing never closes the dialog");

    click(cx, "candidate-alice");
    assert!(
        members_chosen(&h, cx).is_empty(),
        "a second click takes them out"
    );
    assert!(shown(cx, "new-chat-members-empty"));

    click(cx, "candidate-boris");
    click(cx, "candidate-alice");
    click(cx, "candidate-anna");
    assert_eq!(
        members_chosen(&h, cx),
        ["boris", "alice", "anna"],
        "in the order they were picked"
    );
    click(cx, "new-chat-remove-alice");
    assert_eq!(members_chosen(&h, cx), ["boris", "anna"]);
    assert!(shown(cx, "candidate-boris-check") && shown(cx, "candidate-anna-check"));
    assert_eq!(
        chosen_person(&h, cx),
        None,
        "the private choice is separate"
    );
    assert!(
        h.commands().is_empty(),
        "selecting and removing send nothing"
    );
}

#[gpui::test]
fn a_known_private_chat_is_not_opened_from_the_group_variant(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_group_dialog(&h, cx);
    click(cx, "candidate-anna");
    assert!(
        dialog_open(&h, cx),
        "a person with a private chat is just a participant"
    );
    assert_eq!(members_chosen(&h, cx), ["anna"]);
    assert_eq!(selected_chat(&h, cx), Some(chat_g()));
    assert!(h.commands().is_empty(), "no chat is opened");

    cx.simulate_input("Свои");
    click(cx, "new-chat-create");
    assert_eq!(
        h.commands(),
        [create_group("Свои", &["anna"])],
        "a group is created even with someone a private chat exists with"
    );
}

#[gpui::test]
fn create_is_disabled_without_a_usable_title_or_a_participant(cx: &mut TestAppContext) {
    use crate::new_chat::{can_create_group, title_ok};
    let (h, cx) = harness(cx);
    open_group_dialog(&h, cx);
    assert!(shown(cx, "new-chat-create"));

    // Nothing at all.
    click(cx, "new-chat-create");
    assert!(h.commands().is_empty());

    // A participant, no title.
    click(cx, "candidate-alice");
    click(cx, "new-chat-create");
    assert!(h.commands().is_empty(), "no title");

    // A blank title is no title.
    cx.simulate_input("   ");
    assert_eq!(group_title(&h, cx), "   ");
    click(cx, "new-chat-create");
    assert!(h.commands().is_empty(), "a title of spaces");
    assert_eq!(creation(&h, cx), None);

    // A title, no participant.
    cx.simulate_input("Дружба");
    click(cx, "new-chat-remove-alice");
    assert!(members_chosen(&h, cx).is_empty());
    click(cx, "new-chat-create");
    assert!(h.commands().is_empty(), "nobody else in the group");

    // Too long a title.
    click(cx, "candidate-alice");
    let long = "я".repeat(251);
    h.desktop.update(cx, |d, cx| {
        d.new_chat_set_title(&long, cx);
    });
    assert!(shown(cx, "new-chat-title-long"), "the dialog says why");
    click(cx, "new-chat-create");
    assert!(h.commands().is_empty(), "over the limit");
    h.desktop
        .update(cx, |d, cx| d.new_chat_set_title(&"я".repeat(250), cx));
    click(cx, "new-chat-create");
    assert_eq!(group_creations(&h.commands()), 1, "250 characters are fine");

    // The pure rule behind the button.
    let idle = None;
    assert!(can_create_group("Дружба", 1, idle));
    assert!(!can_create_group("Дружба", 0, idle));
    assert!(!can_create_group("", 1, idle));
    assert!(!can_create_group(" \t ", 1, idle));
    assert!(!can_create_group(
        "Дружба",
        1,
        Some(&crate::store::CreationState::InFlight)
    ));
    assert!(title_ok(&"я".repeat(250)) && !title_ok(&"я".repeat(251)));
}

#[gpui::test]
fn enter_in_the_title_or_search_field_never_creates_a_group(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    fill_group(&h, cx);
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(h.commands().is_empty(), "Enter in the title sends nothing");
    assert_eq!(
        group_title(&h, cx).trim(),
        "Дружба",
        "at most the platform's line break text is added, which the title rule trims"
    );
    assert_eq!(creation(&h, cx), None);

    click(cx, "new-chat-search");
    cx.simulate_input("бор");
    assert_eq!(
        h.desktop
            .read_with(cx, |d, cx| d.new_chat_query(cx))
            .as_deref(),
        Some("бор"),
        "the search field has the focus now"
    );
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(
        h.commands().is_empty(),
        "Enter in the search field sends nothing"
    );
    assert!(dialog_open(&h, cx));
    assert_eq!(members_chosen(&h, cx), ["alice", "boris"]);
    assert_eq!(creation(&h, cx), None);

    // Only the button creates, and the title goes out trimmed.
    click(cx, "new-chat-create");
    assert_eq!(h.commands(), [create_group("Дружба", &["alice", "boris"])]);
}

#[gpui::test]
fn one_confirm_sends_one_group_command_with_the_trimmed_title_and_the_participants_in_order(
    cx: &mut TestAppContext,
) {
    let (h, cx) = harness(cx);
    open_group_dialog(&h, cx);
    cx.simulate_input("  Дружба  ");
    click(cx, "candidate-boris");
    click(cx, "candidate-alice");
    assert!(h.commands().is_empty());

    click(cx, "new-chat-create");
    click(cx, "new-chat-create");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(
        h.commands(),
        [create_group("Дружба", &["boris", "alice"])],
        "one command per confirm, also on a double click"
    );
    assert_eq!(
        creation(&h, cx),
        Some(crate::store::CreationState::InFlight)
    );
    assert!(shown(cx, "new-chat-sending"));

    // Everything is locked until the outcome is shown.
    click(cx, "candidate-anna");
    click(cx, "new-chat-remove-alice");
    click(cx, "new-chat-mode-private");
    assert_eq!(members_chosen(&h, cx), ["boris", "alice"]);
    assert!(h.desktop.read_with(cx, |d, _| d.new_chat_is_group()));
    click(cx, "new-chat-cancel");
    cx.simulate_keystrokes("escape");
    assert!(dialog_open(&h, cx));
    assert!(h.commands().is_empty());
}

#[gpui::test]
fn a_created_group_closes_the_dialog_and_is_opened(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    fill_group(&h, cx);
    click(cx, "new-chat-create");
    assert_eq!(h.commands(), [create_group("Дружба", &["alice", "boris"])]);
    let mut chats = people_chats();
    chats.push(chat("n", ChatKind::Group, "Дружба", Some(1)));
    h.feed(
        cx,
        [
            ClientEvent::ChatsReplaced(chats),
            ClientEvent::ChatCreated {
                chat: Some(ChatId("n".into())),
                existing: false,
            },
        ],
    );
    assert!(!dialog_open(&h, cx));
    assert_eq!(creation(&h, cx), None);
    assert_eq!(selected_chat(&h, cx), Some(ChatId("n".into())));
    assert_eq!(h.commands(), [Command::OpenChat(ChatId("n".into()))]);
    assert!(shown(cx, row!("n")));
}

#[gpui::test]
fn a_failed_group_keeps_the_title_and_participants_and_only_the_user_retries(
    cx: &mut TestAppContext,
) {
    let (h, cx) = harness(cx);
    fill_group(&h, cx);
    click(cx, "new-chat-create");
    h.commands();
    h.feed(
        cx,
        [ClientEvent::CreationFailed {
            reason: "сервер отказал".into(),
            unknown: false,
        }],
    );
    assert!(shown(cx, "new-chat-error"));
    assert!(dialog_open(&h, cx));
    assert_eq!(group_title(&h, cx), "Дружба");
    assert_eq!(members_chosen(&h, cx), ["alice", "boris"]);
    cx.run_until_parked();
    assert!(h.commands().is_empty(), "nothing is sent by itself");

    // Changing the group forgets the failure; the next press is a new attempt.
    click(cx, "new-chat-remove-boris");
    assert_eq!(creation(&h, cx), None);
    click(cx, "new-chat-create");
    assert_eq!(h.commands(), [create_group("Дружба", &["alice"])]);
}

#[gpui::test]
fn an_unknown_or_unverified_group_result_is_said_honestly_and_not_retried(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    fill_group(&h, cx);
    click(cx, "new-chat-create");
    h.commands();
    h.feed(
        cx,
        [ClientEvent::CreationFailed {
            reason: "Результат неизвестен: группа могла быть создана".into(),
            unknown: true,
        }],
    );
    assert!(shown(cx, "new-chat-unknown") && !shown(cx, "new-chat-error"));
    assert!(dialog_open(&h, cx));
    assert_eq!(group_title(&h, cx), "Дружба");
    assert_eq!(members_chosen(&h, cx), ["alice", "boris"]);
    cx.run_until_parked();
    assert!(h.commands().is_empty(), "no second request by itself");

    click(cx, "new-chat-create");
    assert_eq!(group_creations(&h.commands()), 1, "the press is the user's");
    h.feed(
        cx,
        [ClientEvent::CreationUnverified {
            reason: "Результат неизвестен: список не обновлён".into(),
        }],
    );
    assert!(shown(cx, "new-chat-unverified"));
    assert!(!shown(cx, "new-chat-error"));
    assert!(matches!(
        creation(&h, cx),
        Some(crate::store::CreationState::Unverified { .. })
    ));
    assert!(dialog_open(&h, cx));
    assert_eq!(members_chosen(&h, cx), ["alice", "boris"]);
    cx.run_until_parked();
    assert!(h.commands().is_empty());
}

#[gpui::test]
fn a_group_created_but_not_named_by_the_list_closes_the_dialog_with_a_toast(
    cx: &mut TestAppContext,
) {
    let (h, cx) = harness(cx);
    fill_group(&h, cx);
    click(cx, "new-chat-create");
    h.commands();
    h.feed(
        cx,
        [ClientEvent::ChatCreated {
            chat: None,
            existing: false,
        }],
    );
    assert!(!dialog_open(&h, cx));
    assert_eq!(selected_chat(&h, cx), Some(chat_g()), "nothing to open yet");
    assert!(
        shown(cx, "toast"),
        "the user is told it will show up in the list"
    );
    assert!(h.commands().is_empty());
}

#[gpui::test]
fn the_group_dialog_fits_the_smallest_window_with_several_participants(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    cx.simulate_resize(size(px(850.), px(560.)));
    open_people_panel(&h, cx);
    open_dialog(&h, cx);
    click(cx, "new-chat-mode-group");
    cx.simulate_input("Дружба");
    for row in ["candidate-alice", "candidate-boris", "candidate-anna"] {
        click(cx, row);
    }
    assert_eq!(members_chosen(&h, cx), ["alice", "boris", "anna"]);

    let window = px(560.);
    let card = bounds(cx, "new-chat-card").expect("the card is on screen");
    assert!(
        card.top() >= px(0.) && card.bottom() <= window - px(16.),
        "the card is capped to the window with a margin: {card:?}"
    );
    for selector in ["new-chat-create", "new-chat-cancel", "new-chat-mode-group"] {
        let b = bounds(cx, selector).unwrap();
        assert!(
            b.top() >= px(0.) && b.bottom() <= window,
            "{selector} is inside the window: {b:?}"
        );
        assert!(b.bottom() <= card.bottom(), "{selector} is inside the card");
    }

    // It can still be confirmed.
    click(cx, "new-chat-create");
    assert_eq!(
        h.commands(),
        [create_group("Дружба", &["alice", "boris", "anna"])]
    );
}

#[gpui::test]
fn cancel_before_confirming_a_group_sends_nothing_and_the_next_dialog_starts_empty(
    cx: &mut TestAppContext,
) {
    let (h, cx) = harness(cx);
    fill_group(&h, cx);
    click(cx, "new-chat-cancel");
    assert!(!dialog_open(&h, cx));
    assert!(h.commands().is_empty(), "cancel creates nothing");
    assert_eq!(creation(&h, cx), None);

    // Escape does the same, from the group variant.
    open_dialog_again(&h, cx);
    click(cx, "new-chat-mode-group");
    click(cx, "candidate-alice");
    cx.simulate_keystrokes("escape");
    assert!(!dialog_open(&h, cx));
    assert!(h.commands().is_empty());

    // Nothing of the earlier dialog is left in the next one.
    open_dialog_again(&h, cx);
    assert!(!h.desktop.read_with(cx, |d, _| d.new_chat_is_group()));
    click(cx, "new-chat-mode-group");
    assert!(members_chosen(&h, cx).is_empty());
    assert_eq!(group_title(&h, cx), "");
}

#[gpui::test]
fn offline_a_group_refusal_is_shown_and_the_title_and_participants_kept(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    fill_group(&h, cx);
    h.feed(cx, [ClientEvent::Connection(ConnectionState::Offline)]);
    assert!(
        dialog_open(&h, cx),
        "losing the connection does not close the dialog"
    );
    click(cx, "new-chat-create");
    assert_eq!(
        group_creations(&h.commands()),
        1,
        "the one command of the press; the core answers it offline"
    );
    // The core refuses at once, without a request to the server; the dialog says so.
    h.feed(
        cx,
        [ClientEvent::CreationFailed {
            reason: "Нет соединения".into(),
            unknown: false,
        }],
    );
    assert!(shown(cx, "new-chat-error"));
    assert!(dialog_open(&h, cx));
    assert_eq!(group_title(&h, cx), "Дружба");
    assert_eq!(members_chosen(&h, cx), ["alice", "boris"]);
    assert_eq!(
        creation(&h, cx),
        Some(crate::store::CreationState::Failed {
            reason: "Нет соединения".into(),
            unknown: false
        })
    );
    assert_eq!(h.commands().len(), 0, "nothing is sent by itself");
}

#[gpui::test]
fn the_focused_field_stays_in_view_while_the_body_scrolls_in_the_smallest_window(
    cx: &mut TestAppContext,
) {
    let (h, cx) = harness(cx);
    cx.simulate_resize(size(px(850.), px(560.)));
    open_people_panel(&h, cx);
    open_dialog(&h, cx);
    click(cx, "new-chat-mode-group");
    cx.simulate_input("Дружба");
    for row in ["candidate-alice", "candidate-boris", "candidate-anna"] {
        click(cx, row);
    }
    let (title, search) = (
        bounds(cx, "new-chat-title").unwrap(),
        bounds(cx, "new-chat-search").unwrap(),
    );
    let body = bounds(cx, "new-chat-body").unwrap();
    let card = bounds(cx, "new-chat-card").unwrap();
    assert!(
        title.top() >= card.top() && search.bottom() <= body.top(),
        "both fields lie above the scrolling part, inside the card: {title:?} {search:?} {body:?}"
    );

    // Scroll the body to the bottom; the focused title is not moved out of view.
    for _ in 0..5 {
        cx.simulate_event(ScrollWheelEvent {
            position: point(body.center().x, body.bottom() - px(6.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(-400.))),
            ..Default::default()
        });
    }
    assert_eq!(bounds(cx, "new-chat-title").unwrap(), title);
    assert_eq!(bounds(cx, "new-chat-search").unwrap(), search);
    cx.simulate_input("!");
    assert_eq!(group_title(&h, cx), "Дружба!", "typing reaches the title");
    let title = bounds(cx, "new-chat-title").unwrap();
    assert!(
        title.top() >= card.top() && title.bottom() <= body.top(),
        "and the edited field is visible: {title:?}"
    );
}

#[gpui::test]
fn the_dialog_blocks_the_chat_and_yields_to_nothing_under_it(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_people_panel(&h, cx);
    open_dialog(&h, cx);
    assert!(
        h.desktop.read_with(cx, |d, cx| d.blocked(cx)),
        "blocks the chat like other overlays"
    );
    assert_eq!(composer_text(&h, cx), "");
    cx.simulate_input("zzz");
    assert_eq!(
        composer_text(&h, cx),
        "",
        "typing goes to the dialog, not the composer"
    );
    click(cx, "new-chat-cancel");
    assert!(!h.desktop.read_with(cx, |d, cx| d.blocked(cx)));

    h.feed(cx, [ClientEvent::Profile(profile_of(Some("Я"), None))]);
    open_profile(&h, cx);
    h.desktop.update(cx, |d, cx| d.open_new_chat(None, cx));
    assert!(
        !dialog_open(&h, cx),
        "the dialog does not open under another overlay"
    );
}

#[gpui::test]
fn an_ended_session_closes_the_dialog(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    choose_alice(&h, cx);
    h.feed(cx, [ClientEvent::SessionExpired]);
    assert!(!dialog_open(&h, cx));
    assert!(shown(cx, "session-expired"));
}

#[gpui::test]
fn write_on_a_member_row_opens_the_dialog_for_that_person(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_people_panel(&h, cx);
    for row in [
        "member-alice-write",
        "member-boris-write",
        "member-anna-write",
    ] {
        assert!(shown(cx, row), "{row}");
    }
    assert!(shown(cx, "member-me"), "the own row is listed");
    assert!(!shown(cx, "member-me-write"), "but offers no \"Написать\"");
    assert!(h.commands().is_empty(), "showing the action starts nothing");
    assert!(!dialog_open(&h, cx));

    click(cx, "member-alice-write");
    cx.run_until_parked();
    assert!(dialog_open(&h, cx));
    assert_eq!(chosen_person(&h, cx).as_deref(), Some("alice"));
    assert!(
        shown(cx, "new-chat-participant"),
        "the confirmation is for that person"
    );
    let commands = h.commands();
    assert_eq!(
        commands,
        [Command::LoadProfile],
        "the dialog opens, nothing is created"
    );

    click(cx, "new-chat-create");
    assert_eq!(h.commands(), [create_alice()]);
}

#[gpui::test]
fn write_on_a_member_with_a_chat_opens_it(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_people_panel(&h, cx);
    click(cx, "member-anna-write");
    assert!(!dialog_open(&h, cx));
    assert_eq!(selected_chat(&h, cx), Some(ChatId("p".into())));
    assert_eq!(
        h.commands(),
        [Command::OpenChat(ChatId("p".into()))],
        "no creation, no dialog"
    );
}

#[gpui::test]
fn escape_closes_the_dialog_before_the_panel_under_it(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_people_panel(&h, cx);
    click(cx, "member-boris-write");
    cx.run_until_parked();
    h.commands();
    assert!(dialog_open(&h, cx));
    cx.simulate_keystrokes("escape");
    assert!(!dialog_open(&h, cx));
    assert!(info_open(&h, cx));
    cx.simulate_keystrokes("escape");
    assert!(!info_open(&h, cx));
}

#[test]
fn the_dialog_notice_follows_the_creation_state() {
    use crate::new_chat::{Notice, can_create, notice};
    use crate::store::CreationState;
    assert_eq!(notice(None), Notice::Nothing);
    assert_eq!(notice(Some(&CreationState::InFlight)), Notice::Sending);
    assert_eq!(
        notice(Some(&CreationState::Failed {
            reason: "r".into(),
            unknown: false
        })),
        Notice::Failed("r".into())
    );
    assert_eq!(
        notice(Some(&CreationState::Failed {
            reason: "r".into(),
            unknown: true
        })),
        Notice::Unknown("r".into())
    );
    assert_eq!(
        notice(Some(&CreationState::Unverified { reason: "r".into() })),
        Notice::Unverified("r".into())
    );
    assert!(!can_create(false, None), "nobody chosen");
    assert!(can_create(true, None));
    assert!(!can_create(true, Some(&CreationState::InFlight)));
    assert!(can_create(
        true,
        Some(&CreationState::Failed {
            reason: "r".into(),
            unknown: true
        })
    ));
    assert!(can_create(
        true,
        Some(&CreationState::Unverified { reason: "r".into() })
    ));
}

// ---- add-reaction-list 6.3: «Кто отреагировал» in the message menu ----
//
// Bounds are never removed once painted, so every "not offered" case runs in a window that never painted
// the entry. The list's rows and states are 6.2's view and 6.7's tests; here the entry, the popover's
// place and closing, and the two buttons the host wires are checked.

fn with_reactions(mut m: Message, counts: &[(i64, u32)]) -> Message {
    m.reactions = reactions_of(counts, None);
    m
}

fn load_reactors(i: i64, before: Option<i64>, request: u64) -> Command {
    Command::LoadReactors {
        chat: chat_g(),
        timestamp: ts(i),
        before,
        request,
    }
}

fn reactors_popover_open(h: &Harness, cx: &mut VisualTestContext) -> bool {
    h.desktop.read_with(cx, |d, _| d.reactors_popover_is_open())
}

fn reactors_list_in_store(h: &Harness, cx: &mut VisualTestContext) -> bool {
    h.store.read_with(cx, |s, _| s.reactors().is_some())
}

/// Opens the popover of the incoming message 1 (two reactions) through the menu entry.
fn open_reactors_popover(h: &Harness, cx: &mut VisualTestContext) {
    open_with(h, cx, vec![reacted(1, &[(LIKE, 2)], None)]);
    right_click(cx, msg!(1));
    click(cx, "menu-reactors");
}

#[gpui::test]
fn the_menu_offers_who_reacted_for_a_sent_message_with_reactions(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![reacted(1, &[(LIKE, 2), (FIRE, 1)], None)]);
    right_click(cx, msg!(1));
    assert!(shown(cx, "menu-reactors"), "another user's message");
    assert!(shown(cx, "menu-reply") && shown(cx, "menu-copy"));
    assert!(!shown(cx, "menu-edit") && !shown(cx, "menu-delete"));
    assert!(
        y(cx, "menu-reactions") < y(cx, "menu-reactors")
            && y(cx, "menu-reactors") < y(cx, "menu-reply"),
        "between the reaction row and the other entries"
    );
    assert!(h.commands().is_empty(), "offering the entry sends nothing");
}

#[gpui::test]
fn the_menu_of_an_own_message_with_reactions_offers_who_reacted_too(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(
        &h,
        cx,
        vec![with_reactions(own("g", 2, "мой"), &[(HEART, 1)])],
    );
    right_click(cx, msg!(2));
    for entry in ["menu-reactors", "menu-reply", "menu-edit", "menu-delete"] {
        assert!(shown(cx, entry), "own message: {entry}");
    }
    assert!(h.commands().is_empty());
}

#[gpui::test]
fn a_message_without_reactions_has_no_who_reacted_entry(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1), own("g", 2, "мой")]);
    for i in [msg!(1), msg!(2)] {
        right_click(cx, i);
        assert!(menu_is_open(&h, cx));
        cx.simulate_keystrokes("escape");
    }
    assert!(!shown(cx, "menu-reactors"));
    assert!(h.commands().is_empty());
}

#[gpui::test]
fn pending_failed_and_deleted_messages_have_no_who_reacted_entry(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    let counts = [(LIKE, 1)];
    let mut deleted = own("g", 6, "x");
    deleted.body = MessageBody::Deleted;
    open_with(
        &h,
        cx,
        vec![
            with_reactions(pending("g", 1, "p1", "жду"), &counts),
            with_reactions(failed("g", 3, "p2", "не вышло", "нет прав"), &counts),
            with_reactions(deleted, &counts),
        ],
    );
    for i in [msg!(1), msg!(3)] {
        right_click(cx, i);
        assert!(menu_is_open(&h, cx), "copy is still offered");
        cx.simulate_keystrokes("escape");
    }
    right_click(cx, msg!(6));
    assert!(!menu_is_open(&h, cx), "a deleted message has no menu");
    assert!(!shown(cx, "menu-reactors"));
    assert!(h.commands().is_empty());
}

#[gpui::test]
fn the_entry_alone_still_opens_the_menu(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    let sticker = message(
        "g",
        1,
        "bob",
        false,
        MessageBody::Unsupported("sticker".into()),
    );
    open_with(&h, cx, vec![with_reactions(sticker, &[(LIKE, 1)])]);
    right_click(cx, msg!(1));
    assert!(menu_is_open(&h, cx));
    assert!(shown(cx, "menu-reactors"));
    for entry in ["menu-reply", "menu-copy", "menu-edit", "menu-delete"] {
        assert!(!shown(cx, entry), "{entry}: nothing else applies to it");
    }
}

#[gpui::test]
fn choosing_the_entry_closes_the_menu_and_opens_the_list_where_the_menu_was(
    cx: &mut TestAppContext,
) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![reacted(1, &[(LIKE, 2)], None)]);
    let anchor = bounds(cx, msg!(1))
        .expect("the message is on screen")
        .center();
    right_click(cx, msg!(1));
    let menu = bounds(cx, "message-menu").expect("the menu is on screen");
    assert!(!reactors_popover_open(&h, cx));
    click(cx, "menu-reactors");

    assert!(!menu_is_open(&h, cx));
    assert!(reactors_popover_open(&h, cx));
    assert!(shown(cx, "reactors-list") && shown(cx, "reactors-loading"));
    assert_eq!(h.commands(), [load_reactors(1, None, 1)]);
    let at = bounds(cx, "reactors-popover").expect("the popover is on screen");
    assert_eq!(
        at.origin.x, menu.origin.x,
        "at the menu's horizontal anchor"
    );
    assert!(
        at.origin.y <= anchor.y && anchor.y <= at.origin.y + at.size.height,
        "the popover covers the point the menu opened at"
    );
}

#[gpui::test]
fn the_list_is_drawn_exactly_at_the_given_anchor(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![reacted(1, &[(LIKE, 2)], None)]);
    h.desktop.update(cx, |d, cx| {
        d.open_reactors(chat_g(), ts(1), point(px(300.), px(200.)), cx)
    });
    cx.run_until_parked();
    let at = bounds(cx, "reactors-popover").expect("the popover is on screen");
    assert_eq!((at.origin.x, at.origin.y), (px(300.), px(200.)));
    assert_eq!(h.commands(), [load_reactors(1, None, 1)]);
}

#[gpui::test]
fn escape_and_an_outside_click_close_the_list_without_a_write(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_reactors_popover(&h, cx);
    h.commands();
    assert!(reactors_popover_open(&h, cx) && reactors_list_in_store(&h, cx));
    cx.simulate_keystrokes("escape");
    assert!(!reactors_popover_open(&h, cx));
    assert!(!reactors_list_in_store(&h, cx), "the store drops the list");

    right_click(cx, msg!(1));
    click(cx, "menu-reactors");
    assert!(reactors_popover_open(&h, cx));
    click(cx, "chat-header");
    assert!(!reactors_popover_open(&h, cx));
    assert!(!reactors_list_in_store(&h, cx));
    assert_eq!(
        h.commands(),
        [load_reactors(1, None, 2)],
        "only the second opening's read; closing and the outside click send nothing"
    );
}

#[gpui::test]
fn a_click_inside_the_list_keeps_it_open(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_reactors_popover(&h, cx);
    click(cx, "reactors-list");
    assert!(reactors_popover_open(&h, cx));
}

#[gpui::test]
fn a_chip_click_still_toggles_and_opens_no_list(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(
        &h,
        cx,
        vec![reacted(2, &[(LIKE, 3), (FIRE, 1)], Some(FIRE))],
    );
    click(cx, msg!(2, "reaction-100102"));
    assert_eq!(h.commands(), [toggle_command(2, LIKE)]);
    assert!(!reactors_popover_open(&h, cx) && !reactors_list_in_store(&h, cx));
    assert!(!menu_is_open(&h, cx));
}

#[gpui::test]
fn an_expired_session_closes_the_list(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_reactors_popover(&h, cx);
    h.feed(cx, [ClientEvent::SessionExpired]);
    assert!(!reactors_popover_open(&h, cx));
    assert!(!reactors_list_in_store(&h, cx));
}

#[gpui::test]
fn the_retry_button_asks_for_the_first_page_again(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_reactors_popover(&h, cx);
    h.commands();
    h.feed(
        cx,
        [ClientEvent::ReactorsFailed {
            chat: chat_g(),
            timestamp: ts(1),
            request: 1,
            reason: "нет прав".into(),
            offline: false,
        }],
    );
    assert!(shown(cx, "reactors-error"));
    click(cx, "reactors-retry");
    assert_eq!(h.commands(), [load_reactors(1, None, 2)]);
    assert!(reactors_popover_open(&h, cx), "retrying does not close it");
}

#[gpui::test]
fn the_more_button_asks_for_the_next_page_from_the_oldest_entry(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_reactors_popover(&h, cx);
    h.feed(cx, [ClientEvent::Connection(ConnectionState::Online)]);
    h.commands();
    h.feed(
        cx,
        [ClientEvent::Reactors(ReactorsPage {
            chat: chat_g(),
            timestamp: ts(1),
            request: 1,
            reactors: vec![Reactor {
                user: UserId("u1".into()),
                name: Some("Анна".into()),
                avatar: None,
                code: ReactionCode(LIKE),
                at: Some(ts(7)),
                me: false,
            }],
            skipped: 0,
            raw: bt_chat_core::client::REACTORS_PAGE,
            oldest: Some(ts(5)),
        })],
    );
    assert!(shown(cx, "reactors-more"));
    click(cx, "reactors-more");
    assert_eq!(h.commands(), [load_reactors(1, Some(5), 2)]);
    assert!(reactors_popover_open(&h, cx));
}

// ---- add-reaction-list 6.7: the list end to end ----
//
// 6.3's tests above cover the entry and the popover's place; these drive the list through the real
// store and window: rows, the loading, failed and «Нет соединения» states with their retry, «Показать
// ещё», the refresh after a reaction change (the fake clock stands in for the delay), late replies,
// and that nothing but `LoadReactors` is ever sent. Bounds are never removed once painted, so what
// went away is checked in the store and what appeared on screen.

/// A full first page (as far as the store can tell, there is a next one).
const FULL_PAGE: usize = bt_chat_core::client::REACTORS_PAGE;

fn who(guid: &str, name: Option<&str>, code: i64, at: Option<i64>, me: bool) -> Reactor {
    Reactor {
        user: UserId(guid.into()),
        name: name.map(Into::into),
        avatar: None,
        code: ReactionCode(code),
        at,
        me,
    }
}

fn page(request: u64, reactors: Vec<Reactor>, raw: usize, oldest: Option<i64>) -> ClientEvent {
    ClientEvent::Reactors(ReactorsPage {
        chat: chat_g(),
        timestamp: ts(1),
        request,
        reactors,
        skipped: 0,
        raw,
        oldest,
    })
}

fn page_failed(request: u64, reason: &str, offline: bool) -> ClientEvent {
    ClientEvent::ReactorsFailed {
        chat: chat_g(),
        timestamp: ts(1),
        request,
        reason: reason.into(),
        offline,
    }
}

fn two_people() -> Vec<Reactor> {
    vec![
        who("u1", Some("Анна"), LIKE, Some(1_700_000_000_000_000), false),
        who("u2", Some("Борис"), FIRE, Some(1_700_000_100_000_000), true),
    ]
}

fn reactors_state(h: &Harness, cx: &mut VisualTestContext) -> Option<ReactorsState> {
    h.store
        .read_with(cx, |s, _| s.reactors().map(|v| v.state.clone()))
}

/// The guids of the shown rows, in order; empty for a list that is not `Ready` or not open.
fn reactor_rows(h: &Harness, cx: &mut VisualTestContext) -> Vec<String> {
    match reactors_state(h, cx) {
        Some(ReactorsState::Ready { rows, .. }) => rows.into_iter().map(|r| r.user.0).collect(),
        _ => Vec::new(),
    }
}

/// Whether the popover or the store's list is still there.
fn reactors_open(h: &Harness, cx: &mut VisualTestContext) -> bool {
    reactors_popover_open(h, cx) || reactors_list_in_store(h, cx)
}

/// The group chat with messages 1 (three reactions, version 1) and 2 (one), the client online and the
/// list of message 1 opened through the menu. The first command is checked and consumed.
fn open_reactors_online(h: &Harness, cx: &mut VisualTestContext) {
    open_with(
        h,
        cx,
        vec![
            reacted(1, &[(LIKE, 2), (FIRE, 1)], None),
            reacted(2, &[(HEART, 1)], None),
        ],
    );
    h.feed(cx, [ClientEvent::Connection(ConnectionState::Online)]);
    right_click(cx, msg!(1));
    click(cx, "menu-reactors");
    assert_eq!(h.commands(), [load_reactors(1, None, 1)]);
    assert_eq!(reactors_state(h, cx), Some(ReactorsState::Loading));
}

/// Reactions of message 1 at `version`.
fn changed(version: i64, counts: &[(i64, u32)]) -> ClientEvent {
    ClientEvent::ReactionsChanged {
        chat: chat_g(),
        timestamp: ts(1),
        reactions: Reactions {
            version: Some(version),
            ..reactions_of(counts, None)
        },
    }
}

const ONE_MS: Duration = Duration::from_millis(1);

#[gpui::test]
fn the_list_shows_each_person_in_server_order_with_the_own_marker(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_reactors_online(&h, cx);
    h.feed(
        cx,
        [page(
            1,
            vec![
                who("u1", Some("Анна"), LIKE, Some(1_700_000_000_000_000), false),
                who("u2", Some("Борис"), FIRE, Some(1_700_000_100_000_000), true),
                who("u3", None, NOT_IN_TABLE, None, false),
            ],
            3,
            Some(1_700_000_000_000_000),
        )],
    );
    assert_eq!(reactor_rows(&h, cx), ["u1", "u2", "u3"]);
    for row in ["reactor-u1", "reactor-u2", "reactor-u3"] {
        assert!(shown(cx, row), "{row}");
    }
    assert!(y(cx, "reactor-u1") < y(cx, "reactor-u2") && y(cx, "reactor-u2") < y(cx, "reactor-u3"));
    assert!(shown(cx, "reactor-u2-own") && !shown(cx, "reactor-u1-own"));
    assert!(shown(cx, "reactor-u3-name"), "a placeholder, not a blank");
    assert!(shown(cx, "reactor-u1-time") && shown(cx, "reactor-u2-time"));
    assert!(!shown(cx, "reactor-u3-time"), "no time was told");
    assert!(shown(cx, "reactor-u1-emoji") && shown(cx, "reactor-u3-emoji"));
    assert!(!shown(cx, "reactors-more"), "a short page is the end");
    assert!(!shown(cx, "reactors-header"), "3 of the 3 known are shown");
    assert!(h.commands().is_empty());
}

#[gpui::test]
fn a_short_list_declares_the_entries_it_misses(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_reactors_online(&h, cx);
    h.feed(cx, [page(1, two_people(), 2, Some(5))]);
    assert!(shown(cx, "reactors-header"), "2 of the 3 known reactions");
    assert!(shown(cx, "reactor-u1") && shown(cx, "reactor-u2"));
    assert!(
        !shown(cx, "reactors-more"),
        "the page, not the count, ends the list"
    );
}

#[gpui::test]
fn a_refused_list_shows_the_reason_and_recovers_through_retry(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_reactors_online(&h, cx);
    h.feed(cx, [page_failed(1, "нет прав", false)]);
    assert!(shown(cx, "reactors-error") && shown(cx, "reactors-error-reason"));
    assert!(shown(cx, "reactors-retry"));
    assert_eq!(
        reactors_state(&h, cx),
        Some(ReactorsState::Failed {
            reason: "нет прав".into(),
            offline: false
        })
    );

    click(cx, "reactors-retry");
    assert_eq!(h.commands(), [load_reactors(1, None, 2)]);
    assert_eq!(reactors_state(&h, cx), Some(ReactorsState::Loading));
    assert!(reactors_popover_open(&h, cx), "retrying does not close it");

    h.feed(cx, [page_failed(1, "поздно", false)]);
    assert_eq!(
        reactors_state(&h, cx),
        Some(ReactorsState::Loading),
        "the first read's failure is superseded"
    );
    h.feed(cx, [page(2, two_people(), 2, Some(5))]);
    assert_eq!(reactor_rows(&h, cx), ["u1", "u2"]);
    assert!(shown(cx, "reactor-u1"));
    assert!(h.commands().is_empty());
}

#[gpui::test]
fn an_offline_failure_shows_no_connection_with_a_retry(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_reactors_online(&h, cx);
    h.feed(cx, [page_failed(1, "нет сети", true)]);
    assert!(shown(cx, "reactors-offline") && shown(cx, "reactors-retry"));
    assert!(!shown(cx, "reactors-error"), "not worded as a refusal");
    assert_eq!(
        reactors_state(&h, cx),
        Some(ReactorsState::Failed {
            reason: "нет сети".into(),
            offline: true
        })
    );

    click(cx, "reactors-retry");
    assert_eq!(h.commands(), [load_reactors(1, None, 2)]);
    h.feed(cx, [page(2, two_people(), 2, Some(5))]);
    assert_eq!(reactor_rows(&h, cx), ["u1", "u2"]);
}

#[gpui::test]
fn a_shown_list_reads_no_connection_while_the_client_is_offline(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_reactors_online(&h, cx);
    h.feed(cx, [page(1, two_people(), 2, Some(5))]);
    assert!(shown(cx, "reactor-u1"));
    assert!(!shown(cx, "reactors-offline"));

    h.feed(cx, [ClientEvent::Connection(ConnectionState::Offline)]);
    assert!(
        shown(cx, "reactors-offline") && shown(cx, "reactors-retry"),
        "rows from before the outage are not presented as current"
    );
    assert_eq!(
        reactor_rows(&h, cx),
        ["u1", "u2"],
        "kept until the re-read answers"
    );

    click(cx, "reactors-retry");
    assert_eq!(h.commands(), [load_reactors(1, None, 2)]);
    h.feed(
        cx,
        [
            ClientEvent::Connection(ConnectionState::Online),
            page(2, vec![two_people().remove(1)], 1, Some(6)),
        ],
    );
    assert_eq!(reactor_rows(&h, cx), ["u2"], "the answer replaces the rows");
    assert!(h.commands().is_empty());
}

#[gpui::test]
fn show_more_appends_the_next_page_without_a_person_twice(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_reactors_online(&h, cx);
    h.feed(cx, [page(1, two_people(), FULL_PAGE, Some(5))]);
    assert!(
        shown(cx, "reactors-more"),
        "a full page may have a next one"
    );

    click(cx, "reactors-more");
    assert_eq!(h.commands(), [load_reactors(1, Some(5), 2)]);
    assert_eq!(
        reactor_rows(&h, cx),
        ["u1", "u2"],
        "the rows stay meanwhile"
    );

    h.feed(
        cx,
        [page(
            2,
            vec![
                two_people().remove(1),
                who(
                    "u3",
                    Some("Вера"),
                    HEART,
                    Some(1_699_999_000_000_000),
                    false,
                ),
            ],
            2,
            Some(3),
        )],
    );
    assert_eq!(
        reactor_rows(&h, cx),
        ["u1", "u2", "u3"],
        "appended, u2 once"
    );
    assert!(shown(cx, "reactor-u3"));
    assert!(matches!(
        reactors_state(&h, cx),
        Some(ReactorsState::Ready {
            exhausted: true,
            loading_more: false,
            ..
        })
    ));
    assert!(h.commands().is_empty());
}

#[gpui::test]
fn a_failed_next_page_keeps_the_rows_and_retries_that_page(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_reactors_online(&h, cx);
    h.feed(cx, [page(1, two_people(), FULL_PAGE, Some(5))]);
    click(cx, "reactors-more");
    assert_eq!(h.commands(), [load_reactors(1, Some(5), 2)]);

    h.feed(cx, [page_failed(2, "сбой", false)]);
    assert_eq!(reactor_rows(&h, cx), ["u1", "u2"]);
    assert!(shown(cx, "reactors-more-error") && shown(cx, "reactors-retry"));
    assert!(!shown(cx, "reactors-error"), "the list itself did not fail");

    click(cx, "reactors-retry");
    assert_eq!(
        h.commands(),
        [load_reactors(1, Some(5), 3)],
        "that page again, fresh token"
    );
    h.feed(
        cx,
        [page(
            3,
            vec![who("u3", Some("Вера"), HEART, None, false)],
            1,
            Some(3),
        )],
    );
    assert_eq!(reactor_rows(&h, cx), ["u1", "u2", "u3"]);
}

#[gpui::test]
fn a_reaction_change_rereads_the_list_from_the_first_page_after_the_delay(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_reactors_online(&h, cx);
    h.feed(cx, [page(1, two_people(), FULL_PAGE, Some(5))]);
    click(cx, "reactors-more");
    assert_eq!(h.commands(), [load_reactors(1, Some(5), 2)]);
    h.feed(
        cx,
        [page(
            2,
            vec![who("u3", Some("Вера"), HEART, None, false)],
            FULL_PAGE,
            Some(3),
        )],
    );
    assert_eq!(reactor_rows(&h, cx), ["u1", "u2", "u3"], "a deep list");

    h.feed(cx, [changed(2, &[(LIKE, 2), (FIRE, 2)])]);
    pass(cx, REACTORS_REFRESH_DELAY / 2);
    h.feed(cx, [changed(3, &[(LIKE, 3), (FIRE, 2)])]);
    pass(cx, REACTORS_REFRESH_DELAY / 2);
    pass(cx, REACTORS_REFRESH_DELAY / 2 - ONE_MS);
    assert!(
        h.commands().is_empty(),
        "each change restarts the delay, nothing is read before it ran out"
    );

    pass(cx, ONE_MS);
    assert_eq!(
        h.commands(),
        [load_reactors(1, None, 3)],
        "one read of the first page for the whole burst"
    );
    assert_eq!(
        reactor_rows(&h, cx),
        ["u1", "u2", "u3"],
        "the rows stay shown until the answer"
    );

    h.feed(
        cx,
        [page(
            3,
            vec![
                who("u4", Some("Глеб"), FIRE, None, false),
                two_people().remove(0),
            ],
            2,
            Some(4),
        )],
    );
    assert_eq!(
        reactor_rows(&h, cx),
        ["u4", "u1"],
        "the answer replaces them"
    );
    assert!(matches!(
        reactors_state(&h, cx),
        Some(ReactorsState::Ready {
            exhausted: true,
            ..
        })
    ));
    pass(cx, REACTORS_REFRESH_DELAY * 3);
    assert!(h.commands().is_empty(), "nothing more is scheduled");
}

#[gpui::test]
fn a_refresh_supersedes_a_next_page_still_out(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_reactors_online(&h, cx);
    h.feed(cx, [page(1, two_people(), FULL_PAGE, Some(5))]);
    click(cx, "reactors-more");
    assert_eq!(h.commands(), [load_reactors(1, Some(5), 2)]);

    h.feed(cx, [changed(2, &[(LIKE, 3), (FIRE, 1)])]);
    pass(cx, REACTORS_REFRESH_DELAY);
    assert_eq!(h.commands(), [load_reactors(1, None, 3)]);

    let late = page(2, vec![who("u3", None, HEART, None, false)], 1, Some(3));
    h.feed(cx, [late.clone()]);
    assert_eq!(
        reactor_rows(&h, cx),
        ["u1", "u2"],
        "the late page changes nothing"
    );
    h.feed(cx, [page(3, vec![two_people().remove(1)], 1, Some(7))]);
    h.feed(cx, [late]);
    assert_eq!(
        reactor_rows(&h, cx),
        ["u2"],
        "nor does it after the refresh's answer"
    );
}

#[gpui::test]
fn a_change_that_does_not_concern_the_open_list_reads_nothing(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_reactors_online(&h, cx);
    h.feed(cx, [page(1, two_people(), 2, Some(5))]);

    h.feed(cx, [changed(1, &[(LIKE, 5)])]);
    h.feed(
        cx,
        [ClientEvent::ReactionsChanged {
            chat: chat_g(),
            timestamp: ts(2),
            reactions: Reactions {
                version: Some(9),
                ..reactions_of(&[(HEART, 2)], None)
            },
        }],
    );
    pass(cx, REACTORS_REFRESH_DELAY * 3);
    assert!(
        h.commands().is_empty(),
        "the version the list was read at, and another message"
    );

    h.feed(cx, [changed(4, &[(LIKE, 5)])]);
    cx.simulate_keystrokes("escape");
    assert!(!reactors_open(&h, cx));
    pass(cx, REACTORS_REFRESH_DELAY * 3);
    h.feed(cx, [changed(5, &[(LIKE, 6)])]);
    pass(cx, REACTORS_REFRESH_DELAY * 3);
    assert!(
        h.commands().is_empty(),
        "closing cancels a waiting refresh and a closed list is not refreshed"
    );
}

#[gpui::test]
fn a_deleted_message_turns_the_list_into_message_gone_and_cancels_the_refresh(
    cx: &mut TestAppContext,
) {
    let (h, cx) = harness(cx);
    open_reactors_online(&h, cx);
    h.feed(cx, [page(1, two_people(), 2, Some(5))]);
    h.feed(cx, [changed(2, &[(LIKE, 3)])]);
    h.feed(
        cx,
        [ClientEvent::MessageDeleted {
            chat: chat_g(),
            timestamp: ts(1),
        }],
    );
    assert!(shown(cx, "reactors-gone"));
    assert_eq!(
        reactors_state(&h, cx),
        Some(ReactorsState::Failed {
            reason: MESSAGE_GONE.into(),
            offline: false
        })
    );
    pass(cx, REACTORS_REFRESH_DELAY * 3);
    assert!(h.commands().is_empty());
}

#[gpui::test]
fn a_reply_after_the_list_closed_changes_nothing(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_reactors_online(&h, cx);
    cx.simulate_keystrokes("escape");
    assert!(!reactors_open(&h, cx));

    h.feed(cx, [page(1, two_people(), 2, Some(5))]);
    assert!(
        !reactors_open(&h, cx),
        "a page for a closed list is dropped"
    );
    h.feed(cx, [page_failed(1, "нет прав", false)]);
    assert!(!reactors_open(&h, cx), "so is a failure");
    assert!(h.commands().is_empty());

    // Reopened while the first read is still out: only the second one's answer counts.
    right_click(cx, msg!(1));
    click(cx, "menu-reactors");
    assert_eq!(h.commands(), [load_reactors(1, None, 2)]);
    h.feed(cx, [page(1, two_people(), 2, Some(5))]);
    assert_eq!(reactors_state(&h, cx), Some(ReactorsState::Loading));
    h.feed(cx, [page_failed(1, "нет прав", false)]);
    assert_eq!(reactors_state(&h, cx), Some(ReactorsState::Loading));
    h.feed(cx, [page(2, vec![two_people().remove(0)], 1, Some(5))]);
    assert_eq!(reactor_rows(&h, cx), ["u1"]);
    assert!(h.commands().is_empty());
}

#[gpui::test]
fn opening_paging_refreshing_and_closing_send_only_reads(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(
        &h,
        cx,
        vec![reacted(1, &[(LIKE, 2), (FIRE, 1)], Some(LIKE))],
    );
    h.feed(cx, [ClientEvent::Connection(ConnectionState::Online)]);
    let mut sent = Vec::new();

    right_click(cx, msg!(1));
    click(cx, "menu-reactors");
    sent.extend(h.commands());
    h.feed(cx, [page(1, two_people(), FULL_PAGE, Some(5))]);

    click(cx, "reactors-more");
    sent.extend(h.commands());
    h.feed(
        cx,
        [page(
            2,
            vec![who("u3", Some("Вера"), HEART, None, false)],
            FULL_PAGE,
            Some(3),
        )],
    );

    let before = sent.len();
    click(cx, "reactor-u1");
    click(cx, "reactor-u2");
    assert!(
        reactors_popover_open(&h, cx),
        "a click on an entry is not a gesture"
    );
    sent.extend(h.commands());
    assert_eq!(sent.len(), before, "a row sends nothing");

    h.feed(
        cx,
        [ClientEvent::ReactionsChanged {
            chat: chat_g(),
            timestamp: ts(1),
            reactions: Reactions {
                version: Some(2),
                ..reactions_of(&[(LIKE, 3), (FIRE, 1)], Some(LIKE))
            },
        }],
    );
    pass(cx, REACTORS_REFRESH_DELAY);
    sent.extend(h.commands());
    h.feed(cx, [page(3, two_people(), 2, Some(5))]);

    cx.simulate_keystrokes("escape");
    assert!(!reactors_open(&h, cx));
    sent.extend(h.commands());

    assert_eq!(
        sent,
        [
            load_reactors(1, None, 1),
            load_reactors(1, Some(5), 2),
            load_reactors(1, None, 3)
        ]
    );
    assert!(
        !sent
            .iter()
            .any(|c| matches!(c, Command::ToggleReaction { .. })),
        "the own reaction was never touched"
    );
    assert_eq!(
        reactions_in_store(&h, cx, 1).mine,
        Some(ReactionCode(LIKE)),
        "and the message's own reaction is as it was"
    );
}

// ---- reveal in Finder (REV 4.1) ----
//
// The tests observe the `StoreEvent::Reveal` the store emits; the platform call lives in `main.rs`, which
// the harness never runs, and the test platform cannot make it.

use crate::store::StoreEvent;
use std::cell::RefCell;
use std::rc::Rc;

/// Every event the store emits from now on, in order.
fn record_store_events(h: &Harness, cx: &mut VisualTestContext) -> Rc<RefCell<Vec<StoreEvent>>> {
    let log = Rc::new(RefCell::new(Vec::new()));
    let sink = log.clone();
    cx.update(|_, app| {
        app.subscribe(&h.store, move |_, event: &StoreEvent, _| {
            sink.borrow_mut().push(event.clone())
        })
        .detach()
    });
    cx.run_until_parked();
    log
}

fn take_events(log: &Rc<RefCell<Vec<StoreEvent>>>) -> Vec<StoreEvent> {
    std::mem::take(&mut *log.borrow_mut())
}

/// A real file in `dir`.
fn saved_file(dir: &tempfile::TempDir, name: &str) -> PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, b"saved").unwrap();
    path
}

fn saved(h: &Harness, cx: &mut VisualTestContext, file: &MediaRef, path: &std::path::Path) {
    h.feed(
        cx,
        [ClientEvent::Downloaded {
            media: file.clone(),
            path: path.to_path_buf(),
        }],
    );
}

fn reveal(h: &Harness, cx: &mut VisualTestContext, file: &MediaRef) {
    h.store.update(cx, |s, cx| s.reveal_download(file, cx));
    cx.run_until_parked();
}

#[gpui::test]
fn reveal_emits_the_saved_path_and_sends_no_command(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    h.feed(cx, [ClientEvent::Connection(ConnectionState::Online)]);
    let dir = tempfile::tempdir().unwrap();
    let (file, other) = (original("disk/1"), original("disk/2"));
    let (path, other_path) = (saved_file(&dir, "a.pdf"), saved_file(&dir, "b.pdf"));
    saved(&h, cx, &file, &path);
    saved(&h, cx, &other, &other_path);
    h.commands();
    let log = record_store_events(&h, cx);

    reveal(&h, cx, &file);
    assert_eq!(take_events(&log), [StoreEvent::Reveal(path.clone())]);
    assert!(h.commands().is_empty(), "revealing is local: no command");
    assert_eq!(
        download_state(&h, cx, &file),
        Some(DownloadState::Saved(path)),
        "the state is kept"
    );
    assert_eq!(
        download_state(&h, cx, &other),
        Some(DownloadState::Saved(other_path)),
        "another file is untouched"
    );
}

#[gpui::test]
fn reveal_of_a_deleted_file_clears_the_state_and_emits_no_reveal(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    h.feed(cx, [ClientEvent::Connection(ConnectionState::Online)]);
    let dir = tempfile::tempdir().unwrap();
    let (file, other) = (original("disk/1"), original("disk/2"));
    let (path, other_path) = (saved_file(&dir, "a.pdf"), saved_file(&dir, "b.pdf"));
    saved(&h, cx, &file, &path);
    saved(&h, cx, &other, &other_path);
    std::fs::remove_file(&path).unwrap();
    h.commands();
    let log = record_store_events(&h, cx);

    reveal(&h, cx, &file);
    assert_eq!(
        take_events(&log),
        [StoreEvent::Download(file.clone())],
        "only the card's state changes"
    );
    assert_eq!(download_state(&h, cx, &file), None);
    assert!(h.commands().is_empty());
    assert_eq!(
        download_state(&h, cx, &other),
        Some(DownloadState::Saved(other_path)),
        "another file is untouched"
    );

    reveal(&h, cx, &file);
    assert!(take_events(&log).is_empty(), "nothing is left to reveal");
}

#[gpui::test]
fn reveal_without_a_saved_state_emits_nothing_and_changes_nothing(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    h.feed(cx, [ClientEvent::Connection(ConnectionState::Online)]);
    let file = original("disk/1");
    let log = record_store_events(&h, cx);
    let failures = [
        DownloadFailure::Offline,
        DownloadFailure::NotFound,
        DownloadFailure::Destination,
        DownloadFailure::Disk,
        DownloadFailure::Network,
        DownloadFailure::Other,
    ];
    let mut states = vec![
        None,
        Some(DownloadState::Running {
            received: 10,
            total: Some(40),
        }),
    ];
    states.extend(failures.map(|f| Some(DownloadState::Failed(f))));

    for state in states {
        match &state {
            None => {}
            Some(DownloadState::Running { received, total }) => h.feed(
                cx,
                [ClientEvent::DownloadProgress {
                    media: file.clone(),
                    received: *received,
                    total: *total,
                }],
            ),
            Some(DownloadState::Failed(reason)) => h.feed(
                cx,
                [ClientEvent::DownloadFailed {
                    media: file.clone(),
                    reason: *reason,
                }],
            ),
            Some(DownloadState::Saved(_)) => unreachable!(),
        }
        h.commands();
        take_events(&log);

        reveal(&h, cx, &file);
        assert!(take_events(&log).is_empty(), "{state:?}: no event");
        assert!(h.commands().is_empty(), "{state:?}: no command");
        assert_eq!(download_state(&h, cx, &file), state, "{state:?}: unchanged");
    }
}

#[gpui::test]
fn reveal_works_without_a_connection(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    let dir = tempfile::tempdir().unwrap();
    let file = original("disk/1");
    let path = saved_file(&dir, "a.pdf");
    saved(&h, cx, &file, &path);
    let log = record_store_events(&h, cx);

    for state in [ConnectionState::Offline, ConnectionState::Connecting] {
        h.feed(cx, [ClientEvent::Connection(state)]);
        h.commands();
        take_events(&log);

        reveal(&h, cx, &file);
        assert_eq!(
            take_events(&log),
            [StoreEvent::Reveal(path.clone())],
            "{state:?}"
        );
        assert!(h.commands().is_empty(), "{state:?}: no command");
    }
}

#[gpui::test]
fn reveal_follows_the_latest_download_of_the_same_file(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    let dir = tempfile::tempdir().unwrap();
    let file = original("disk/1");
    let (first, second) = (saved_file(&dir, "a.pdf"), saved_file(&dir, "a (1).pdf"));
    saved(&h, cx, &file, &first);
    saved(&h, cx, &file, &second);
    let log = record_store_events(&h, cx);

    reveal(&h, cx, &file);
    assert_eq!(take_events(&log), [StoreEvent::Reveal(second)]);
}

#[gpui::test]
fn reveal_leaves_the_choice_to_the_platform_when_the_file_cannot_be_checked(
    cx: &mut TestAppContext,
) {
    let (h, cx) = harness(cx);
    let dir = tempfile::tempdir().unwrap();
    let file = original("disk/1");
    // A path below a regular file: the stat fails with "not a directory", which is not "does not exist".
    let path = saved_file(&dir, "a.pdf").join("b.pdf");
    assert!(path.try_exists().is_err());
    saved(&h, cx, &file, &path);
    let log = record_store_events(&h, cx);

    reveal(&h, cx, &file);
    assert_eq!(take_events(&log), [StoreEvent::Reveal(path.clone())]);
    assert_eq!(
        download_state(&h, cx, &file),
        Some(DownloadState::Saved(path))
    );
}

/// Whether the file card of `doc` (message 5) shows the reveal link once `state` is applied before the
/// first frame (`debug_bounds` entries of earlier frames would never disappear).
fn card_shows_reveal(cx: &mut TestAppContext, state: Option<ClientEvent>) -> bool {
    let (h, cx) = harness(cx);
    h.feed(cx, [ClientEvent::Connection(ConnectionState::Online)]);
    h.feed(cx, state);
    open_with(&h, cx, vec![file_message(5)]);
    assert!(shown(cx, msg!(5, "file")), "the card is on screen");
    shown(cx, msg!(5, "reveal"))
}

#[gpui::test]
fn the_file_card_has_no_reveal_link_before_a_download(cx: &mut TestAppContext) {
    assert!(!card_shows_reveal(cx, None));
}

#[gpui::test]
fn the_file_card_has_no_reveal_link_while_running(cx: &mut TestAppContext) {
    let running = ClientEvent::DownloadProgress {
        media: original("doc"),
        received: 10,
        total: Some(40),
    };
    assert!(!card_shows_reveal(cx, Some(running)));
}

#[gpui::test]
fn the_file_card_has_no_reveal_link_after_a_failure(cx: &mut TestAppContext) {
    let failed = ClientEvent::DownloadFailed {
        media: original("doc"),
        reason: DownloadFailure::Network,
    };
    assert!(!card_shows_reveal(cx, Some(failed)));
}

#[gpui::test]
fn the_file_cards_reveal_link_yields_the_event_in_every_connection_state(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    h.feed(cx, [ClientEvent::Connection(ConnectionState::Online)]);
    let dir = tempfile::tempdir().unwrap();
    let file = original("doc");
    let path = saved_file(&dir, "отчёт.pdf");
    saved(&h, cx, &file, &path);
    open_with(&h, cx, vec![file_message(5)]);
    assert!(shown(cx, msg!(5, "saved")));
    assert!(shown(cx, msg!(5, "reveal")));
    let log = record_store_events(&h, cx);

    click(cx, msg!(5, "reveal"));
    assert_eq!(take_events(&log), [StoreEvent::Reveal(path.clone())]);
    assert!(h.commands().is_empty(), "revealing is local: no command");

    for state in [ConnectionState::Offline, ConnectionState::Connecting] {
        h.feed(cx, [ClientEvent::Connection(state)]);
        h.commands();
        take_events(&log);
        assert!(shown(cx, msg!(5, "reveal")), "{state:?}: still offered");
        click(cx, msg!(5, "reveal"));
        assert_eq!(
            take_events(&log),
            [StoreEvent::Reveal(path.clone())],
            "{state:?}"
        );
        assert!(h.commands().is_empty(), "{state:?}: no command");
    }

    std::fs::remove_file(&path).unwrap();
    click(cx, msg!(5, "reveal"));
    assert_eq!(
        take_events(&log),
        [StoreEvent::Download(file.clone())],
        "a deleted file is not revealed"
    );
    assert_eq!(download_state(&h, cx, &file), None);
}

/// Whether the viewer of picture `v/1` shows the reveal button once `state` is applied before the first frame.
fn viewer_shows_reveal(cx: &mut TestAppContext, state: Option<ClientEvent>) -> bool {
    let (h, cx) = harness(cx);
    h.feed(cx, [ClientEvent::Connection(ConnectionState::Online)]);
    h.feed(cx, state);
    open_with(&h, cx, vec![four_pictures()]);
    open_viewer(&h, cx, ts(2), 0);
    assert!(shown(cx, "viewer-image"), "the viewer is on screen");
    shown(cx, "viewer-reveal")
}

#[gpui::test]
fn the_viewer_has_no_reveal_button_before_a_download(cx: &mut TestAppContext) {
    assert!(!viewer_shows_reveal(cx, None));
}

#[gpui::test]
fn the_viewer_has_no_reveal_button_while_running(cx: &mut TestAppContext) {
    let running = ClientEvent::DownloadProgress {
        media: original("v/1"),
        received: 10,
        total: Some(40),
    };
    assert!(!viewer_shows_reveal(cx, Some(running)));
}

#[gpui::test]
fn the_viewer_has_no_reveal_button_after_a_failure(cx: &mut TestAppContext) {
    let failed = ClientEvent::DownloadFailed {
        media: original("v/1"),
        reason: DownloadFailure::Network,
    };
    assert!(!viewer_shows_reveal(cx, Some(failed)));
}

#[gpui::test]
fn the_viewers_reveal_button_yields_the_event_of_the_saved_original(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    h.feed(cx, [ClientEvent::Connection(ConnectionState::Online)]);
    let dir = tempfile::tempdir().unwrap();
    let file = original("v/1");
    let path = saved_file(&dir, "v (1).jpg");
    saved(&h, cx, &file, &path);
    open_with(&h, cx, vec![four_pictures()]);
    open_viewer(&h, cx, ts(2), 0);
    assert!(shown(cx, "viewer-saved"));
    assert!(shown(cx, "viewer-reveal"));
    h.commands();
    let log = record_store_events(&h, cx);

    click(cx, "viewer-reveal");
    assert_eq!(take_events(&log), [StoreEvent::Reveal(path.clone())]);
    assert!(h.commands().is_empty(), "revealing is local: no command");

    h.feed(cx, [ClientEvent::Connection(ConnectionState::Offline)]);
    h.commands();
    take_events(&log);
    assert!(shown(cx, "viewer-reveal"), "still offered offline");
    click(cx, "viewer-reveal");
    assert_eq!(take_events(&log), [StoreEvent::Reveal(path)]);
    assert!(h.commands().is_empty());
}

// ---- PIN 4.4: pinned-message bar and important marker ----

fn pin_preview(i: i64, author: Option<&str>, text: &str) -> PinnedMessage {
    PinnedMessage {
        timestamp: ts(i),
        preview: Some(MessagePreview {
            author: author.map(str::to_string),
            text: text.into(),
            timestamp: ts(i),
        }),
    }
}

fn pin_changed(chat_id: &str, pinned: Option<PinnedMessage>) -> ClientEvent {
    ClientEvent::PinChanged {
        chat: ChatId(chat_id.into()),
        pinned,
    }
}

fn pin_in_store(h: &Harness, cx: &mut VisualTestContext, chat_id: &str) -> Option<PinnedMessage> {
    h.store.read_with(cx, |s, _| {
        s.chat(&ChatId(chat_id.into()))
            .and_then(|c| c.pinned_message.clone())
    })
}

/// What the bar of `chat_id` shows under its label, as the bar itself builds it from the store's pin.
fn pin_line(h: &Harness, cx: &mut VisualTestContext, chat_id: &str) -> Option<String> {
    h.store.read_with(cx, |s, _| {
        let chat = s.chat(&ChatId(chat_id.into()))?;
        let pinned = chat.pinned_message.as_ref()?;
        crate::desktop::pinned_message_text(chat.kind, pinned.preview.as_ref())
    })
}

fn summary_in_store(h: &Harness, cx: &mut VisualTestContext, chat_id: &str) -> ChatSummary {
    h.store.read_with(cx, |s, _| {
        s.chat(&ChatId(chat_id.into()))
            .cloned()
            .expect("the chat is listed")
    })
}

fn important_message(chat_id: &str, i: i64, outgoing: bool, body: MessageBody) -> Message {
    let mut m = message(
        chat_id,
        i,
        if outgoing { "me" } else { "alice" },
        outgoing,
        body,
    );
    m.important = true;
    m
}

/// Counts the store's notifications: a pin change that does not alter anything must not make one.
fn pin_notifications(
    h: &Harness,
    cx: &mut VisualTestContext,
) -> (std::rc::Rc<std::cell::Cell<usize>>, gpui::Subscription) {
    let count = std::rc::Rc::new(std::cell::Cell::new(0));
    let seen = count.clone();
    let subscription =
        cx.update(|_, app| app.observe(&h.store, move |_, _| seen.set(seen.get() + 1)));
    (count, subscription)
}

fn pin_offset(h: &Harness, cx: &mut VisualTestContext) -> (usize, Pixels) {
    let top = h.list(cx).logical_scroll_top();
    (top.item_ix, top.offset_in_item)
}

/// Where the item at the top of the viewport sits below the top edge of the timeline.
fn pin_anchor(h: &Harness, cx: &mut VisualTestContext) -> f32 {
    let list = h.list(cx);
    let item = list
        .bounds_for_item(list.logical_scroll_top().item_ix)
        .expect("the top item is laid out");
    f32::from(
        item.origin.y
            - bounds(cx, "timeline")
                .expect("timeline is on screen")
                .origin
                .y,
    )
}

fn pin_bar_height(cx: &mut VisualTestContext) -> f32 {
    f32::from(
        bounds(cx, "pinned-message")
            .expect("the bar was painted")
            .size
            .height,
    )
}

fn pin_width(cx: &mut VisualTestContext, selector: &'static str) -> f32 {
    f32::from(
        bounds(cx, selector)
            .unwrap_or_else(|| panic!("{selector} is not on screen"))
            .size
            .width,
    )
}

/// Starts with `summary` as the only chat, opens it and drops the commands of the opening.
fn pin_started(
    cx: &mut TestAppContext,
    summary: ChatSummary,
    extra: Vec<ClientEvent>,
) -> (Harness, &mut VisualTestContext) {
    let mut events = vec![ClientEvent::ChatsReplaced(vec![summary])];
    events.extend(extra);
    let (h, cx) = harness_with(cx, events, |_, _| {});
    click(cx, row!("g"));
    h.commands();
    (h, cx)
}

#[gpui::test]
fn a_pin_change_shows_replaces_and_removes_the_bar_without_scrolling(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_group(&h, cx);
    h.feed(cx, [chunk("g", 41..=100, false)]);
    scroll(cx, 600.);
    let offset = pin_offset(&h, cx);
    assert!(
        offset.0 + 1 < h.list(cx).item_count(),
        "scrolled away from the newest message"
    );
    let anchor = pin_anchor(&h, cx);
    let timeline_top = y(cx, "timeline");
    h.commands();
    assert!(!shown(cx, "pinned-message"), "no pin, no bar");
    let (renders, _subscription) = pin_notifications(&h, cx);

    // Another user pins a message.
    h.feed(
        cx,
        [pin_changed(
            "g",
            Some(pin_preview(50, Some("ALICE"), "сообщение 50")),
        )],
    );
    assert!(shown(cx, "pinned-message") && shown(cx, "pinned-message-text"));
    assert_eq!(pin_line(&h, cx, "g"), Some("ALICE: сообщение 50".into()));
    let full = pin_bar_height(cx);
    assert!(full > 0.);
    assert!(
        (y(cx, "timeline") - timeline_top - full).abs() < 0.5,
        "the bar sits between the header and the timeline"
    );
    assert_eq!(pin_offset(&h, cx), offset, "the timeline did not scroll");
    assert_eq!(pin_anchor(&h, cx), anchor);
    assert_eq!(renders.get(), 1);

    // The same state again changes nothing and wakes nobody; neither does a chat that is not listed.
    h.feed(
        cx,
        [
            pin_changed("g", Some(pin_preview(50, Some("ALICE"), "сообщение 50"))),
            pin_changed("нет-такого", Some(pin_preview(1, None, "x"))),
        ],
    );
    assert_eq!(renders.get(), 1);

    // Replaced by another message.
    h.feed(
        cx,
        [pin_changed(
            "g",
            Some(pin_preview(60, Some("ALICE"), "сообщение 60")),
        )],
    );
    assert_eq!(pin_in_store(&h, cx, "g").map(|p| p.timestamp), Some(ts(60)));
    assert_eq!(pin_line(&h, cx, "g"), Some("ALICE: сообщение 60".into()));
    assert_eq!(pin_bar_height(cx), full);
    assert_eq!(renders.get(), 2);

    // The text of the new pin is not known yet: the label stays alone, a shorter bar.
    h.feed(
        cx,
        [pin_changed(
            "g",
            Some(PinnedMessage {
                timestamp: ts(70),
                preview: None,
            }),
        )],
    );
    assert!(pin_bar_height(cx) < full, "the label alone is one line");
    assert_eq!(pin_line(&h, cx, "g"), None);
    assert_eq!(pin_offset(&h, cx), offset);
    assert_eq!(pin_anchor(&h, cx), anchor);

    // Unpinned: the timeline starts directly under the header again.
    h.feed(cx, [pin_changed("g", None)]);
    assert_eq!(pin_in_store(&h, cx, "g"), None);
    assert!(
        (y(cx, "timeline") - timeline_top).abs() < 0.5,
        "the bar is gone"
    );
    assert_eq!(pin_offset(&h, cx), offset);
    assert_eq!(pin_anchor(&h, cx), anchor);
    assert!(h.commands().is_empty(), "a pin change asks for nothing");
}

#[gpui::test]
fn a_message_pin_never_marks_or_moves_the_chat_row(cx: &mut TestAppContext) {
    let mut with_pin = chat("g", ChatKind::Group, "Команда", Some(20));
    with_pin.unread = 2;
    with_pin.pinned_message = Some(pin_preview(1, Some("ALICE"), "сообщение 1"));
    let mut pinned_chat = chat("c", ChatKind::Private, "C", Some(5));
    pinned_chat.pinned_rank = Some(0);
    let (h, cx) = harness_with(
        cx,
        vec![ClientEvent::ChatsReplaced(vec![
            chat("a", ChatKind::Private, "A", Some(10)),
            chat("b", ChatKind::Private, "B", Some(30)),
            with_pin,
            pinned_chat,
        ])],
        |_, _| {},
    );
    let order = |cx: &mut VisualTestContext| {
        let mut rows = [
            ('a', y(cx, row!("a"))),
            ('b', y(cx, row!("b"))),
            ('c', y(cx, row!("c"))),
            ('g', y(cx, row!("g"))),
        ];
        rows.sort_by(|l, r| l.1.total_cmp(&r.1));
        rows.into_iter().map(|(id, _)| id).collect::<String>()
    };
    // Only the chat's own pin is a marker and a place; a chat with a pinned message keeps its activity order.
    assert_eq!(order(cx), "cbga");
    assert!(shown(cx, part!("c", "pinned")));
    assert!(!shown(cx, part!("g", "pinned")));

    click(cx, row!("g"));
    h.commands();
    let strip = |mut c: ChatSummary| {
        c.pinned_message = None;
        c
    };
    let (g0, b0) = (summary_in_store(&h, cx, "g"), summary_in_store(&h, cx, "b"));
    let rows_y = |cx: &mut VisualTestContext| {
        ["a", "b", "c", "g"].map(|id| match id {
            "a" => y(cx, row!("a")),
            "b" => y(cx, row!("b")),
            "c" => y(cx, row!("c")),
            _ => y(cx, row!("g")),
        })
    };
    let places = rows_y(cx);

    // The open chat's pin is replaced and removed, then the pin of a chat that is not open changes.
    h.feed(
        cx,
        [
            pin_changed("g", Some(pin_preview(7, Some("ALICE"), "сообщение 7"))),
            pin_changed("b", Some(pin_preview(3, None, "чужое"))),
        ],
    );
    assert_eq!(order(cx), "cbga");
    assert_eq!(rows_y(cx), places, "rows did not move");
    h.feed(cx, [pin_changed("g", None), pin_changed("b", None)]);
    assert_eq!(order(cx), "cbga");
    assert_eq!(rows_y(cx), places);

    assert!(!shown(cx, part!("g", "pinned")) && !shown(cx, part!("b", "pinned")));
    assert_eq!(
        strip(summary_in_store(&h, cx, "g")),
        strip(g0),
        "preview, time, unread and the rest of the row are as they were"
    );
    assert_eq!(strip(summary_in_store(&h, cx, "b")), strip(b0));
    assert_eq!(summary_in_store(&h, cx, "c").pinned_rank, Some(0));
    assert!(h.commands().is_empty());
}

#[gpui::test]
fn a_pin_set_in_a_chat_that_is_not_open_shows_when_it_is_opened(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_group(&h, cx);
    h.feed(cx, [chunk("g", 1..=3, true)]);
    h.feed(
        cx,
        [pin_changed(
            "p",
            Some(pin_preview(11, None, "в личном чате")),
        )],
    );
    assert!(!shown(cx, "pinned-message"), "the open chat has no pin");

    click(cx, row!("p"));
    h.feed(cx, [chunk("p", 11..=12, true)]);
    assert!(shown(cx, "pinned-message") && shown(cx, "pinned-message-text"));
}

#[gpui::test]
fn the_bar_shows_the_placeholder_of_a_photo_and_of_a_deleted_message(cx: &mut TestAppContext) {
    let photo = MessageBody::Image {
        media: media("m1", MediaKind::Thumbnail),
        width: 800,
        height: 600,
        caption: None,
        name: None,
    };
    for (body, placeholder) in [(photo, "Фото"), (MessageBody::Deleted, "Сообщение удалено")]
    {
        let mut summary = chat("g", ChatKind::Group, "Команда", Some(2));
        summary.pinned_message = Some(PinnedMessage {
            timestamp: ts(1),
            preview: Some(MessagePreview {
                author: Some("ALICE".into()),
                text: body.preview_text(),
                timestamp: ts(1),
            }),
        });
        let (h, cx) = pin_started(cx, summary, vec![]);
        h.feed(
            cx,
            [history(
                "g",
                vec![message("g", 1, "alice", false, body), text("g", 2)],
                true,
            )],
        );
        assert!(shown(cx, "pinned-message"), "{placeholder}");
        assert!(shown(cx, "pinned-message-text"), "{placeholder}");
        assert_eq!(pin_line(&h, cx, "g"), Some(format!("ALICE: {placeholder}")));
    }
}

#[gpui::test]
fn a_pin_without_a_resolved_text_shows_the_label_alone(cx: &mut TestAppContext) {
    let mut summary = chat("g", ChatKind::Group, "Команда", Some(2));
    summary.pinned_message = Some(PinnedMessage {
        timestamp: ts(1),
        preview: None,
    });
    let (h, cx) = pin_started(cx, summary, vec![]);
    h.feed(cx, [chunk("g", 1..=2, true)]);
    assert!(shown(cx, "pinned-message"));
    assert!(!shown(cx, "pinned-message-text"));
}

#[gpui::test]
fn the_bar_click_scrolls_to_a_loaded_message_and_otherwise_does_nothing(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_group(&h, cx);
    h.feed(cx, [chunk("g", 41..=100, false)]);
    h.feed(
        cx,
        [pin_changed(
            "g",
            Some(pin_preview(10, Some("ALICE"), "старое")),
        )],
    );
    let offset = pin_offset(&h, cx);
    h.commands();

    // Older than the loaded history: no request, no loading state, no movement.
    click(cx, "pinned-message");
    cx.run_until_parked();
    assert_eq!(pin_offset(&h, cx), offset);
    assert!(h.commands().is_empty());
    assert!(!loading_older(&h, cx, "g"));

    // Loaded: the timeline goes to it (message 45 is the fifth item).
    h.feed(
        cx,
        [pin_changed(
            "g",
            Some(pin_preview(45, Some("ALICE"), "сообщение 45")),
        )],
    );
    click(cx, "pinned-message");
    cx.run_until_parked();
    assert_eq!(h.list(cx).logical_scroll_top().item_ix, 4);
    assert!(h.commands().is_empty());
}

#[gpui::test]
fn a_launch_offline_shows_the_cached_bar_and_marker_with_the_offline_indicator(
    cx: &mut TestAppContext,
) {
    let mut summary = chat("g", ChatKind::Group, "Команда", Some(2));
    summary.pinned_message = Some(pin_preview(1, Some("ALICE"), "сообщение 1"));
    let (h, cx) = harness_with(
        cx,
        vec![
            ClientEvent::ChatsReplaced(vec![summary]),
            ClientEvent::Connection(ConnectionState::Offline),
        ],
        |_, _| {},
    );
    assert!(shown(cx, "connection-indicator"));
    assert!(!shown(cx, "pinned-message"), "no chat is open yet");

    click(cx, row!("g"));
    assert!(
        shown(cx, "pinned-message") && shown(cx, "pinned-message-text"),
        "the bar comes from the cached summary, before any history"
    );
    assert_eq!(pin_line(&h, cx, "g"), Some("ALICE: сообщение 1".into()));
    h.feed(
        cx,
        [history(
            "g",
            vec![
                important_message("g", 1, false, MessageBody::Text("сообщение 1".into())),
                text("g", 2),
            ],
            true,
        )],
    );
    assert!(shown(cx, msg!(1, "important")), "the cached flag is shown");
    assert_eq!(
        pin_line(&h, cx, "g"),
        Some("ALICE: сообщение 1".into()),
        "the cached text is kept"
    );
    assert!(!shown(cx, msg!(2, "important")));
    assert!(shown(cx, "connection-indicator"), "still offline");
    assert_eq!(h.commands(), [Command::OpenChat(chat_g())]);
}

#[gpui::test]
fn the_marker_shows_on_incoming_and_own_messages_of_every_chat_kind_and_not_on_deleted(
    cx: &mut TestAppContext,
) {
    let (h, cx) = harness(cx);
    open_group(&h, cx);
    h.feed(
        cx,
        [history(
            "g",
            vec![
                important_message("g", 1, false, MessageBody::Text("чужое важное".into())),
                important_message("g", 2, true, MessageBody::Text("моё важное".into())),
                important_message("g", 3, false, MessageBody::Deleted),
                text("g", 4),
            ],
            true,
        )],
    );
    assert!(shown(cx, msg!(1, "important")), "incoming");
    assert!(shown(cx, msg!(2, "important")), "own");
    assert!(shown(cx, msg!(3)), "the deleted message is on screen");
    assert!(!shown(cx, msg!(3, "important")), "but has no marker");
    assert!(shown(cx, msg!(4)));
    assert!(!shown(cx, msg!(4, "important")), "an ordinary message");

    click(cx, row!("p"));
    h.feed(
        cx,
        [history(
            "p",
            vec![
                important_message("p", 11, false, MessageBody::Text("личное".into())),
                important_message("p", 12, true, MessageBody::Text("моё личное".into())),
            ],
            true,
        )],
    );
    assert!(shown(cx, msg!(11, "important")), "private chat, incoming");
    assert!(shown(cx, msg!(12, "important")), "private chat, own");

    click(cx, row!("ch"));
    h.feed(
        cx,
        [history(
            "ch",
            vec![important_message(
                "ch",
                21,
                false,
                MessageBody::Text("новость".into()),
            )],
            true,
        )],
    );
    assert!(shown(cx, msg!(21, "important")), "channel");
}

#[gpui::test]
fn a_live_importance_change_updates_the_marker_in_place_without_scrolling(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    // Two short own messages next to each other: their bubbles are as wide as their footers, so a marker
    // in the footer shows as a wider bubble (the debug map keeps the bounds of vanished elements).
    open_with(
        &h,
        cx,
        (41..=100)
            .map(|i| match i {
                89 | 90 => own("g", i, "а"),
                _ => text("g", i),
            })
            .collect(),
    );
    scroll(cx, 200.);
    assert!(shown(cx, msg!(89)) && shown(cx, msg!(90)));
    let ordinary = pin_width(cx, msg!(89));
    assert_eq!(pin_width(cx, msg!(90)), ordinary);
    let offset = pin_offset(&h, cx);
    assert!(offset.0 + 1 < h.list(cx).item_count(), "scrolled up");
    let anchor = pin_anchor(&h, cx);
    let row_top = y(cx, row!("g"));
    let before = summary_in_store(&h, cx, "g");
    let flag = |h: &Harness, cx: &mut VisualTestContext| {
        h.store.read_with(cx, |s, _| {
            s.timeline(&chat_g())
                .unwrap()
                .messages
                .iter()
                .find(|m| m.timestamp == ts(90))
                .unwrap()
                .important
        })
    };

    // Marked as important from another client.
    h.feed(
        cx,
        [ClientEvent::MessageUpserted(important_message(
            "g",
            90,
            true,
            MessageBody::Text("а".into()),
        ))],
    );
    assert!(flag(&h, cx));
    assert!(shown(cx, msg!(90, "important")));
    assert!(
        pin_width(cx, msg!(90)) > ordinary,
        "the marker is in the footer"
    );
    assert!(!shown(cx, msg!(89, "important")));
    assert_eq!(pin_offset(&h, cx), offset, "the view did not scroll");
    assert_eq!(pin_anchor(&h, cx), anchor);

    // The flag removed again.
    h.feed(cx, [ClientEvent::MessageUpserted(own("g", 90, "а"))]);
    assert!(!flag(&h, cx));
    assert_eq!(pin_width(cx, msg!(90)), ordinary, "the marker is gone");
    assert_eq!(pin_offset(&h, cx), offset);
    assert_eq!(pin_anchor(&h, cx), anchor);

    assert_eq!(y(cx, row!("g")), row_top);
    assert_eq!(
        summary_in_store(&h, cx, "g"),
        before,
        "the row is as it was"
    );
    assert!(h.commands().is_empty());
}

#[gpui::test]
fn pins_and_importance_are_display_only(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(
        &h,
        cx,
        vec![
            text("g", 1),
            important_message("g", 2, true, MessageBody::Text("важное".into())),
            own("g", 3, "обычное"),
            text("g", 4),
        ],
    );
    h.feed(
        cx,
        [pin_changed(
            "g",
            Some(pin_preview(1, Some("ALICE"), "сообщение 1")),
        )],
    );
    h.commands();

    // The menu of a pinned or important message is the menu of any message of its kind: same entries.
    right_click(cx, msg!(1));
    let pinned_incoming = menu_height(cx);
    cx.simulate_keystrokes("escape");
    right_click(cx, msg!(4));
    assert_eq!(menu_height(cx), pinned_incoming, "pinned, incoming");
    assert!(shown(cx, "menu-reply") && shown(cx, "menu-copy"));
    assert!(!shown(cx, "menu-edit") && !shown(cx, "menu-delete"));
    cx.simulate_keystrokes("escape");
    right_click(cx, msg!(2));
    let important_own = menu_height(cx);
    cx.simulate_keystrokes("escape");
    right_click(cx, msg!(3));
    assert_eq!(menu_height(cx), important_own, "important, own");
    for item in ["menu-reply", "menu-copy", "menu-edit", "menu-delete"] {
        assert!(shown(cx, item), "{item}");
    }
    for item in [
        "menu-pin",
        "menu-unpin",
        "menu-pinned",
        "menu-important",
        "menu-unimportant",
    ] {
        assert!(!shown(cx, item), "{item}");
    }
    cx.simulate_keystrokes("escape");
    assert!(!menu_is_open(&h, cx));

    // The bar is only a link to the message: no control, no menu, no request.
    click(cx, "pinned-message");
    right_click(cx, "pinned-message");
    assert!(!menu_is_open(&h, cx), "the bar has no menu");
    for control in [
        "pinned-message-unpin",
        "pinned-message-close",
        "pinned-message-menu",
    ] {
        assert!(!shown(cx, control), "{control}");
    }
    cx.run_until_parked();
    assert!(h.commands().is_empty(), "nothing is sent for a pin");
}

#[gpui::test]
fn a_cache_from_before_pins_and_importance_loads_without_bar_marker_or_error(
    cx: &mut TestAppContext,
) {
    // The JSON of rows cached before the fields existed: the keys are not there at all.
    let legacy_chat: ChatSummary = {
        let mut json =
            serde_json::to_value(chat("g", ChatKind::Group, "Команда", Some(1))).unwrap();
        let fields = json.as_object_mut().unwrap();
        fields.remove("pinned_message");
        fields.remove("pin_action_ts");
        serde_json::from_value(json).unwrap()
    };
    let legacy_message: Message = {
        let mut json = serde_json::to_value(text("g", 1)).unwrap();
        json.as_object_mut().unwrap().remove("important");
        serde_json::from_value(json).unwrap()
    };
    assert_eq!(legacy_chat.pinned_message, None);
    assert_eq!(legacy_chat.pin_action_ts, None);
    assert!(!legacy_message.important);

    let (h, cx) = pin_started(cx, legacy_chat, vec![]);
    h.feed(cx, [history("g", vec![legacy_message], true)]);
    assert!(shown(cx, msg!(1)), "the history is shown");
    assert!(!shown(cx, "pinned-message"));
    assert!(!shown(cx, msg!(1, "important")));
    assert_eq!(toast(&h, cx), None);
    assert!(h.commands().is_empty());
}

#[gpui::test]
fn the_bar_line_names_the_author_outside_private_chats_and_is_one_line(_cx: &mut TestAppContext) {
    use crate::desktop::pinned_message_text as line;
    let by = |author: Option<&str>, text: &str| MessagePreview {
        author: author.map(str::to_string),
        text: text.into(),
        timestamp: ts(1),
    };
    let said = by(Some("ALICE"), "привет");
    assert_eq!(
        line(ChatKind::Group, Some(&said)),
        Some("ALICE: привет".into())
    );
    assert_eq!(
        line(ChatKind::Channel, Some(&said)),
        Some("ALICE: привет".into())
    );
    assert_eq!(
        line(ChatKind::Private, Some(&said)),
        Some("привет".into()),
        "no author in a private chat"
    );
    assert_eq!(
        line(ChatKind::Group, Some(&by(None, "привет"))),
        Some("привет".into()),
        "an unknown author is not named"
    );
    assert_eq!(
        line(
            ChatKind::Group,
            Some(&by(Some("ALICE"), "раз\n\n  два   три\n"))
        ),
        Some("ALICE: раз два три".into())
    );

    // The placeholders of a message without text are the ones of the chat row.
    let photo = MessageBody::Image {
        media: media("m1", MediaKind::Thumbnail),
        width: 800,
        height: 600,
        caption: None,
        name: None,
    };
    let placeholder = |body: &MessageBody| by(Some("ALICE"), &body.preview_text());
    assert_eq!(
        line(ChatKind::Group, Some(&placeholder(&photo))),
        Some("ALICE: Фото".into())
    );
    assert_eq!(
        line(ChatKind::Private, Some(&placeholder(&MessageBody::Deleted))),
        Some("Сообщение удалено".into())
    );

    // No text yet: nothing under the label.
    assert_eq!(line(ChatKind::Group, None), None);
    assert_eq!(line(ChatKind::Private, None), None);
}
