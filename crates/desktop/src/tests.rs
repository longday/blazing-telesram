//! Headless UI tests: the real `ChatStore` and `Desktop` driven through channels, inspected via debug bounds.

use crate::desktop::{chat_preview, AccountMenu, Desktop, Dismiss, Preview, ToggleLog, ViewerNext, ViewerPrev};
use crate::format::{fit_thumbnail, Clock};
use crate::logging::{LogBuffer, LogLine};
use crate::store::{ChatFilter, ChatStore, DownloadState, SessionState};
use crate::timeline::gallery_columns;
use async_channel::{Receiver, Sender};
use bt_chat_core::model::*;
use gpui::{
    point, px, size, AppContext, Bounds, Context, Entity, KeyBinding, ListState, Modifiers, MouseButton, Pixels,
    ScrollDelta, ScrollWheelEvent, TestAppContext, VisualTestContext,
};
use std::path::PathBuf;
use std::time::Duration;

const PNG_1X1: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00,
    0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49,
    0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0xf0, 0x1f, 0x00, 0x05, 0x00, 0x01, 0xff, 0x89, 0x99, 0x3d,
    0x1d, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
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
        last_message: last.map(|i| MessagePreview { author: None, text: format!("превью {i}"), timestamp: ts(i) }),
        unread: 0,
        muted: false,
        pinned_rank: None,
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
    }
}

fn text(chat_id: &str, i: i64) -> Message {
    message(chat_id, i, "alice", false, MessageBody::Text(format!("сообщение {i}")))
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
    MediaRef { id: id.into(), kind }
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
    (Harness { events: events_tx, commands, store, desktop }, vcx)
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
        self.desktop.read_with(cx, |d, _| d.list_state().expect("a chat is open").clone())
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
    f32::from(bounds(cx, selector).unwrap_or_else(|| panic!("{selector} is not on screen")).origin.y)
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
    h.store.read_with(cx, |s, _| s.timeline(&ChatId(chat.into())).unwrap().loading_older)
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
    h.feed(cx, [ClientEvent::ChatsReplaced(vec![a.clone(), chat("b", ChatKind::Group, "Группа", None)])]);
    h.feed(cx, [ClientEvent::ChatUpdated(a), ClientEvent::Connection(ConnectionState::Online)]);
    h.store.read_with(cx, |s, _| {
        assert!(s.chats_loaded());
        assert_eq!(s.chat_count(), 2);
        assert_eq!(s.connection(), ConnectionState::Online);
    });
    assert_eq!(h.commands(), [Command::FetchMedia(media("av-a", MediaKind::Avatar))], "avatar requested once");

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
            ClientEvent::MessageDeleted { chat: ChatId("b".into()), timestamp: ts(1) },
            // Never opened: ignored.
            ClientEvent::MessageUpserted(text("a", 9)),
        ],
    );
    h.store.read_with(cx, |s, _| {
        let tl = s.timeline(&ChatId("b".into())).unwrap();
        assert_eq!(tl.messages.iter().map(|m| m.timestamp).collect::<Vec<_>>(), [1, 2, 3, 4]);
        assert_eq!(tl.messages[0].body, MessageBody::Deleted);
        assert!(tl.messages[1].edited);
        assert!(s.timeline(&ChatId("a".into())).is_none());
    });

    let path = PathBuf::from("/tmp/av-a.png");
    h.feed(cx, [ClientEvent::MediaReady { media: media("av-a", MediaKind::Avatar), path: path.clone() }]);
    h.store.read_with(cx, |s, _| {
        assert_eq!(s.media_path(&media("av-a", MediaKind::Avatar)), Some(path.as_path()));
    });

    h.feed(cx, [ClientEvent::SessionExpired]);
    h.store.read_with(cx, |s, _| assert_eq!(s.session(), SessionState::Expired));
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
    assert!(h.commands().is_empty(), "no retry loop: waits for the next refresh");
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
            s.timeline(&ChatId("b".into())).unwrap().messages.iter().map(|m| m.timestamp).collect::<Vec<_>>()
        })
    };
    // Messages 2 and 3 were deleted while offline; the core replaced everything from 1 on.
    let catch_up = |messages, replaces| {
        ClientEvent::History(HistoryChunk { chat: ChatId("b".into()), messages, reached_start: false, replaces, payloads: Vec::new() })
    };
    h.feed(cx, [catch_up(vec![text("b", 1)], Some((Some(ts(1)), None)))]);
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
    assert!(h.commands().iter().all(|c| !matches!(c, Command::LoadOlder { .. })), "start known: no paging");
    // The core dropped the cached history: the new chunk does not reach the start, so the
    // short timeline pages backwards again.
    h.feed(cx, [ClientEvent::History(HistoryChunk {
        chat: b.clone(),
        messages: vec![text("b", 5), text("b", 6)],
        reached_start: false,
        replaces: Some((None, None)),
        payloads: Vec::new(),
    })]);
    assert!(h.commands().contains(&Command::LoadOlder { chat: b, before: ts(5) }));
}

// ---- 6.2 chat list ----

#[gpui::test]
fn empty_account_shows_empty_state(cx: &mut TestAppContext) {
    let list_or_empty = |cx: &mut TestAppContext, events: Vec<ClientEvent>| {
        let (_h, cx) = harness_with(cx, events, |_, _| {});
        (shown(cx, "chat-list"), shown(cx, "chat-list-empty"))
    };
    assert_eq!(list_or_empty(cx, vec![]), (false, true), "placeholder while loading");
    assert_eq!(list_or_empty(cx, vec![ClientEvent::ChatsReplaced(vec![])]), (false, true));
    let one = vec![ClientEvent::ChatsReplaced(vec![chat("a", ChatKind::Private, "Анна", Some(1))])];
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
    h.feed(cx, [ClientEvent::MediaReady { media: media("av-loud", MediaKind::Avatar), path }]);
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

    h.feed(cx, [ClientEvent::ChatUpdated(chat("a", ChatKind::Private, "A", Some(40)))]);
    assert_eq!(order(cx), "dcab", "newer activity moves an unpinned chat up, but not above pinned ones");
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
    let rows = [("p1", row!("p1")), ("p2", row!("p2")), ("g", row!("g")), ("ch", row!("ch"))]
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
    assert_eq!(rendered_rows(cx, "", All), (vec!["p1", "p2", "g", "ch"], false));
    assert_eq!(rendered_rows(cx, "ПЕТР", All).0, ["p1"], "case-insensitive substring match");
    assert_eq!(rendered_rows(cx, "нет такого", All), (vec![], true));
    assert_eq!(rendered_rows(cx, "", Unread).0, ["g"]);
    assert_eq!(rendered_rows(cx, "", Private).0, ["p1", "p2"]);
    assert_eq!(rendered_rows(cx, "", Groups).0, ["g"]);
    assert_eq!(rendered_rows(cx, "", Channels).0, ["ch"]);
    assert_eq!(rendered_rows(cx, "команда", Private), (vec![], true), "search and filter combine");
}

#[gpui::test]
fn filter_chips_select_one_filter(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    h.feed(cx, [ClientEvent::ChatsReplaced(vec![chat("a", ChatKind::Private, "Анна", Some(1))])]);
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
    h.store.read_with(cx, |s, _| assert_eq!(s.chat(&ChatId("c".into())).unwrap().unread, 2));
    assert!(h.commands().is_empty());
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
    assert!(h.commands().is_empty(), "no loading while the top is off screen");

    scroll_to_top(&h, cx);
    assert_eq!(h.commands(), [Command::LoadOlder { chat: ChatId("g".into()), before: ts(41) }]);
    assert!(shown(cx, "timeline-loading"));
    assert!(loading_older(&h, cx, "g"));
    // Staying at the top must not request the same page again.
    scroll(cx, 10.);
    assert!(h.commands().is_empty());

    h.feed(cx, [chunk("g", 21..=40, false)]);
    assert!(!loading_older(&h, cx, "g"));
    let list = h.list(cx);
    assert_eq!(list.item_count(), 80);
    assert_eq!(list.logical_scroll_top().item_ix, 20, "the message that was on top is still the anchor");
    let anchor = list.bounds_for_item(20).expect("anchor message is laid out");
    let top = bounds(cx, "timeline").unwrap().origin.y;
    assert_eq!(anchor.origin.y, top, "and stays at the same place on screen");
    assert!(h.commands().is_empty());

    scroll_to_top(&h, cx);
    assert_eq!(h.commands(), [Command::LoadOlder { chat: ChatId("g".into()), before: ts(21) }]);
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
    h.store.read_with(cx, |s, _| assert!(s.timeline(&ChatId("g".into())).unwrap().reached_start));
}

#[gpui::test]
fn short_history_asks_for_more_until_the_start(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_group(&h, cx);
    h.feed(cx, [chunk("g", 10..=12, false)]);
    assert_eq!(
        h.commands(),
        [Command::LoadOlder { chat: ChatId("g".into()), before: ts(10) }],
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
    reply.reply_to = Some(ReplyQuote { author: Some("BOB".into()), text: "исходное".into(), timestamp: Some(ts(0)) });
    reply.forwarded_from = Some("Новости".into());
    let mut edited = message("g", 3, "bob", false, MessageBody::Text("правка".into()));
    edited.edited = true;
    h.feed(
        cx,
        [history(
            "g",
            vec![
                message("g", 1, "alice", false, MessageBody::Text("первая\nвторая строка".into())),
                reply,
                edited,
                message("g", 4, "me", true, MessageBody::Text("мой ответ".into())),
                message("g", 5, "me", true, MessageBody::Deleted),
                message("g", 6, "bob", false, MessageBody::Unsupported("sticker".into())),
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
    assert!(shown(cx, msg!(1, "day")), "date separator before the first message of a day");
    assert!(!shown(cx, msg!(2, "day")));
}

#[gpui::test]
fn long_text_wraps_inside_its_own_row(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_group(&h, cx);
    let url = format!("https://gitlab.com/signax/hub/-/merge_requests/{}", "a1b2c3".repeat(60));
    let words = "длинное сообщение из обычных слов, которое не помещается в одну строку пузыря ".repeat(6);
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

    let bubbles = [msg!(1), msg!(2), msg!(3), msg!(4)].map(|s| bounds(cx, s).unwrap_or_else(|| panic!("{s} on screen")));
    for pair in bubbles.windows(2) {
        assert!(pair[0].bottom() <= pair[1].top(), "bubble {:?} overlaps the next one {:?}", pair[0], pair[1]);
    }
    assert!(bubbles[0].size.height > px(80.), "an unbreakable URL wraps over several lines: {:?}", bubbles[0]);
    assert!(bubbles[1].size.height > px(80.), "long text wraps at word boundaries: {:?}", bubbles[1]);
    assert!(bubbles[0].size.width <= px(480.), "the URL bubble keeps the max width: {:?}", bubbles[0]);
    assert!(bubbles[3].size.width < px(120.), "a short message keeps a narrow bubble: {:?}", bubbles[3]);
}

#[gpui::test]
fn private_chats_and_channels_do_not_label_authors(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_group(&h, cx);

    click(cx, row!("p"));
    h.feed(cx, [history("p", vec![message("p", 11, "alice", false, MessageBody::Text("привет".into()))], true)]);
    assert!(shown(cx, msg!(11)));
    assert!(!shown(cx, msg!(11, "author")));
    assert!(!shown(cx, msg!(11, "avatar")));

    click(cx, row!("ch"));
    h.feed(cx, [history("ch", vec![message("ch", 12, "alice", false, MessageBody::Text("новость".into()))], true)]);
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
    h.feed(cx, [ClientEvent::MessageDeleted { chat: ChatId("g".into()), timestamp: ts(3) }]);
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
    h.feed(cx, [ClientEvent::MessageUpserted(text("p", 50)), ClientEvent::ChatUpdated(updated)]);
    assert!(shown(cx, part!("p", "badge")));
    assert!(y(cx, row!("p")) < y(cx, row!("g")), "moved above the older chat");
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
                    MessageBody::Image { media: landscape.clone(), width: 1600, height: 800, caption: Some("подпись".into()), name: None },
                ),
                message(
                    "g",
                    2,
                    "bob",
                    true,
                    MessageBody::Image { media: portrait.clone(), width: 600, height: 1200, caption: None, name: None },
                ),
                with_avatar,
                message(
                    "g",
                    4,
                    "bob",
                    false,
                    MessageBody::File { media: media("doc", MediaKind::File), name: "отчёт.pdf".into(), size: 3_500_000 },
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
    assert_eq!(size_of(cx, msg!(1, "image")), fit_thumbnail(1600, 800, 320.0));
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
            ClientEvent::MediaReady { media: landscape, path: write("wide.png") },
            ClientEvent::MediaReady { media: media("av-alice", MediaKind::Avatar), path: write("alice.png") },
        ],
    );
    assert!(shown(cx, msg!(1, "image-loaded")));
    assert!(!shown(cx, msg!(2, "image-loaded")), "portrait is still downloading");
    assert_eq!(size_of(cx, msg!(1, "image")), (320.0, 160.0), "row size does not change when the file arrives");
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
    assert!(y(vcx, "menu-button") < top_offline, "the banner is gone and the header moved up");

    assert!(indicator_shown(cx, ConnectionState::Connecting));
    assert!(indicator_shown(cx, ConnectionState::Offline));
    assert!(!indicator_shown(cx, ConnectionState::Online));
}

#[gpui::test]
fn sign_out_asks_for_confirmation_then_sends_the_command(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    h.feed(cx, [ClientEvent::ChatsReplaced(vec![chat("a", ChatKind::Private, "Анна", Some(1))])]);
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
    h.feed(cx, [ClientEvent::ChatsReplaced(vec![chat("a", ChatKind::Private, "Анна", Some(1))])]);
    h.feed(cx, [ClientEvent::SessionExpired]);
    assert!(shown(cx, "session-expired"));
    click(cx, row!("a"));
    assert!(h.commands().is_empty(), "the overlay swallows clicks on the list");
    click(cx, "expired-sign-out");
    assert_eq!(h.commands(), [Command::SignOut]);
}

fn log_line(text: &str) -> LogLine {
    LogLine { time_us: ts(1_000), level: tracing::Level::WARN, target: "yandex_proto::xiva".into(), text: text.into() }
}

#[gpui::test]
fn log_window_opens_by_shortcut_follows_new_records_and_copies(cx: &mut TestAppContext) {
    let buffer = LogBuffer::default();
    buffer.push(log_line("disconnected reason=closed"));
    cx.update(|cx| {
        cx.set_global(buffer.clone());
        cx.bind_keys([KeyBinding::new("cmd-shift-l", ToggleLog, Some("BlazingTelesram"))]);
    });
    let (h, cx) = harness(cx);
    let text = |cx: &mut VisualTestContext| h.desktop.read_with(cx, |d, _| d.log_text());
    assert_eq!(text(cx), None);

    cx.simulate_keystrokes("cmd-shift-l");
    assert!(shown(cx, "log-view"));
    assert_eq!(text(cx).unwrap(), "00:00:00.001 WARN  yandex_proto::xiva disconnected reason=closed\n");

    buffer.push(log_line("connecting"));
    cx.executor().advance_clock(Duration::from_millis(300));
    cx.run_until_parked();
    assert!(text(cx).unwrap().ends_with("yandex_proto::xiva connecting\n"), "picked up without reopening");

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
    m.delivery = Delivery::Failed { payload: PayloadId(payload.into()), reason: reason.into() };
    m
}

fn send_command(body: &str, reply_to: Option<ReplyTarget>) -> Command {
    Command::Send { chat: chat_g(), text: body.into(), reply_to }
}

/// Everything the UI asked of the core except typing notices, which every composer edit produces.
fn writes(h: &Harness) -> Vec<Command> {
    h.commands().into_iter().filter(|c| !matches!(c, Command::Typing(_))).collect()
}

/// Opens the group chat "g" and loads `messages` as its newest chunk; pending commands are dropped.
fn open_with(h: &Harness, cx: &mut VisualTestContext, messages: Vec<Message>) {
    open_group(h, cx);
    h.feed(cx, [history("g", messages, true)]);
    h.commands();
}

fn composer_text(h: &Harness, cx: &mut VisualTestContext) -> String {
    h.desktop.read_with(cx, |d, cx| d.composer_input().read(cx).content.to_string())
}

fn composer_reply(h: &Harness, cx: &mut VisualTestContext) -> Option<ReplyTarget> {
    h.desktop.read_with(cx, |d, _| d.composer_reply().cloned())
}

fn composer_cursor(h: &Harness, cx: &mut VisualTestContext) -> usize {
    h.desktop.read_with(cx, |d, cx| d.composer_input().read(cx).cursor())
}

fn composer_height(cx: &mut VisualTestContext) -> Pixels {
    bounds(cx, "composer-input").expect("composer is on screen").size.height
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
    h.store.read_with(cx, |s, _| s.timeline(&chat_g()).unwrap().messages.iter().map(|m| m.timestamp).collect())
}

fn delivery_of(h: &Harness, cx: &mut VisualTestContext, i: i64) -> Delivery {
    h.store.read_with(cx, |s, _| {
        s.timeline(&chat_g()).unwrap().messages.iter().find(|m| m.timestamp == ts(i)).unwrap().delivery.clone()
    })
}

fn reply_target_2() -> ReplyTarget {
    ReplyTarget { timestamp: ts(2), author: Some("ALICE".into()), text: "сообщение 2".into() }
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
    assert_eq!(writes(&h), [send_command("раз\nдва", None)], "line breaks inside the text are kept");
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
    assert!(wrapped > one_row && wrapped < px(22. * 8.), "wrapped text takes a few rows: {wrapped:?}");
    assert_eq!(f32::from(wrapped) % 22., 0., "whole rows");

    cx.simulate_keystrokes("cmd-a backspace");
    cx.simulate_input("x");
    for _ in 0..19 {
        cx.simulate_keystrokes("shift-enter");
        cx.simulate_input("x");
    }
    cx.run_until_parked();
    assert_eq!(composer_height(cx), px(22. * 8.), "twenty rows are capped and scroll inside");

    // Backspace until empty: the field shrinks back to one row.
    cx.simulate_keystrokes("cmd-a backspace");
    cx.run_until_parked();
    assert_eq!(composer_height(cx), one_row);
}

#[gpui::test]
fn clicks_and_vertical_keys_move_the_caret_between_rows(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1)]);
    h.desktop.update(cx, |d, cx| d.composer_input().update(cx, |input, cx| input.set_text("aaa\naa", cx)));
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
    cx.simulate_click(point(field.left() + px(1.), field.top() + px(22. + 11.)), Modifiers::none());
    assert_eq!(composer_cursor(&h, cx), 4, "a click on the second row lands in it");
    cx.simulate_click(point(field.right() - px(1.), field.top() + px(11.)), Modifiers::none());
    assert_eq!(composer_cursor(&h, cx), 3, "a click past the end of the first row lands at its end");
}

// ---- 5.2 delivery states ----

#[gpui::test]
fn a_sent_message_is_pending_then_confirmed_exactly_once(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1), text("g", 2)]);
    h.feed(cx, [ClientEvent::MessageUpserted(pending("g", 50, "p1", "привет"))]);
    assert!(shown(cx, msg!(50, "pending")));
    assert!(y(cx, msg!(50)) > y(cx, msg!(2)), "at the bottom of the timeline at once");
    assert!(h.commands().is_empty());

    let settled = ClientEvent::PendingSettled { chat: chat_g(), payload: PayloadId("p1".into()) };
    let server = ClientEvent::MessageUpserted(own("g", 60, "привет"));
    h.feed(cx, [settled.clone(), server.clone()]);
    assert!(shown(cx, msg!(60, "sent")));
    assert_eq!(stamps(&h, cx), [1, 2, 60], "one copy: the pending one is gone");

    h.feed(cx, [settled, server]);
    assert_eq!(stamps(&h, cx), [1, 2, 60], "a repeated signal changes nothing");
}

#[gpui::test]
fn the_echo_may_come_first_and_still_leaves_one_copy(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1)]);
    h.feed(cx, [ClientEvent::MessageUpserted(pending("g", 50, "p1", "привет"))]);
    h.feed(
        cx,
        [
            ClientEvent::PendingSettled { chat: chat_g(), payload: PayloadId("p1".into()) },
            ClientEvent::MessageUpserted(own("g", 40, "привет")),
        ],
    );
    assert_eq!(stamps(&h, cx), [1, 40], "the server copy may sort before the local time");
    assert_eq!(delivery_of(&h, cx, 40), Delivery::Sent);
    assert!(shown(cx, msg!(40, "sent")));
}

#[gpui::test]
fn a_new_pending_message_scrolls_into_view(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, (1..=80).map(|i| text("g", i)).collect());
    scroll(cx, 800.);
    let list = h.list(cx);
    assert!(list.logical_scroll_top().item_ix < list.item_count(), "scrolled up");
    h.feed(cx, [ClientEvent::MessageUpserted(pending("g", 90, "p1", "привет"))]);
    assert!(shown(cx, msg!(90, "pending")), "the user's own message is brought into view");
}

#[gpui::test]
fn a_rejected_message_shows_the_reason_and_retry_sends_it_again(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1)]);
    h.feed(cx, [ClientEvent::MessageUpserted(pending("g", 50, "p1", "привет"))]);
    h.feed(cx, [ClientEvent::MessageUpserted(failed("g", 50, "p1", "привет", "нет прав"))]);
    assert!(shown(cx, msg!(50, "failed")));
    assert!(shown(cx, msg!(50, "failed-reason")));
    assert_eq!(stamps(&h, cx), [1, 50], "the failed message replaces the pending one in place");

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
    assert_eq!(composer_reply(&h, cx), None, "the bar goes away once the message is sent");
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
    assert_eq!(writes(&h), [send_command("после закрытия", None)], "the composer keeps focus after the click");
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
    let draft = Draft { chat: chat_g(), text: "черновик".into(), reply_to: Some(reply_target_2()) };
    assert!(commands.contains(&Command::SaveDraft(draft)), "saved when the chat is left: {commands:?}");
    assert_eq!(composer_text(&h, cx), "", "the other chat starts with an empty composer");
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
        assert_eq!(chat_preview(s, summary), Preview::Draft("привет мир".into()));
    });

    click(cx, row!("g"));
    cx.simulate_keystrokes("cmd-a backspace");
    click(cx, row!("p"));
    h.store.read_with(cx, |s, _| {
        let summary = s.chat(&chat_g()).unwrap();
        assert_eq!(chat_preview(s, summary), Preview::Message("превью 1".into()), "an emptied draft is gone");
    });
}

#[gpui::test]
fn a_draft_is_saved_one_second_after_the_last_keystroke(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1)]);
    let saved = |h: &Harness| writes(h).into_iter().find_map(|c| if let Command::SaveDraft(d) = c { Some(d.text) } else { None });

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
    h.store.read_with(cx, |s, _| assert!(s.draft(&chat_g()).is_none()));
    cx.executor().advance_clock(Duration::from_secs(2));
    cx.run_until_parked();
    assert!(writes(&h).is_empty(), "the core deletes the draft when it takes the message");
}

#[gpui::test]
fn persisted_drafts_fill_the_composer_and_the_chat_row(cx: &mut TestAppContext) {
    let draft = Draft { chat: chat_g(), text: "из прошлого".into(), reply_to: None };
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
    let draft = |text: &str| ClientEvent::Drafts(vec![Draft { chat: chat_g(), text: text.into(), reply_to: None }]);
    h.feed(cx, [draft("пришёл позже")]);
    assert_eq!(composer_text(&h, cx), "пришёл позже");

    cx.simulate_keystrokes("cmd-a backspace");
    cx.simulate_input("мой текст");
    h.feed(cx, [draft("чужой")]);
    assert_eq!(composer_text(&h, cx), "мой текст", "typed text is never overwritten");
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
    assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()), Some("сообщение 1".into()));
    assert!(!h.desktop.read_with(cx, |d, _| d.menu_is_open()));
}

#[gpui::test]
fn escape_and_an_outside_click_close_the_menu(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1)]);
    let open = |h: &Harness, cx: &mut VisualTestContext| h.desktop.read_with(cx, |d, _| d.menu_is_open());
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
    assert_eq!(h.desktop.read_with(cx, |d, _| d.composer_editing()), Some(ts(2)));
    assert_eq!(composer_text(&h, cx), "старый");

    cx.simulate_keystrokes("cmd-a");
    cx.simulate_input("новый");
    assert!(h.commands().is_empty(), "no typing notice while editing");
    cx.simulate_keystrokes("enter");
    assert_eq!(h.commands(), [Command::Edit { chat: chat_g(), timestamp: ts(2), text: "новый".into() }]);
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
    let messages = vec![own("g", 2, "первое"), text("g", 3), own("g", 4, "второе"), pending("g", 9, "p1", "жду")];
    open_with(&h, cx, messages);
    cx.simulate_keystrokes("up");
    assert_eq!(h.desktop.read_with(cx, |d, _| d.composer_editing()), Some(ts(4)), "pending messages are skipped");
    assert_eq!(composer_text(&h, cx), "второе");
    assert!(shown(cx, "edit-bar"));

    cx.simulate_keystrokes("escape");
    cx.simulate_input("а");
    cx.simulate_keystrokes("up");
    assert_eq!(h.desktop.read_with(cx, |d, _| d.composer_editing()), None, "with text in the composer Up only moves the caret");
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
    assert!(!h.commands().iter().any(|c| matches!(c, Command::Edit { .. })), "the message stays unchanged");
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
    assert_eq!(h.commands(), [Command::Delete { chat: chat_g(), timestamp: ts(2) }]);
    assert!(!h.desktop.read_with(cx, |d, _| d.delete_is_pending()));

    h.feed(cx, [ClientEvent::MessageDeleted { chat: chat_g(), timestamp: ts(2) }]);
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
    assert!(!h.desktop.read_with(cx, |d, _| d.delete_is_pending()), "Escape also cancels");
}

#[gpui::test]
fn a_rejected_edit_or_deletion_shows_the_reason_for_a_while(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![own("g", 2, "старый")]);
    h.feed(cx, [ClientEvent::ActionFailed { chat: chat_g(), reason: "слишком поздно".into() }]);
    assert!(shown(cx, "toast"));
    assert_eq!(h.desktop.read_with(cx, |d, _| d.toast_text().map(str::to_string)), Some("слишком поздно".into()));
    assert_eq!(stamps(&h, cx), [2]);
    assert_eq!(h.store.read_with(cx, |s, _| s.timeline(&chat_g()).unwrap().messages[0].body.clone()), MessageBody::Text("старый".into()));

    cx.executor().advance_clock(Duration::from_secs(6));
    cx.run_until_parked();
    assert_eq!(h.desktop.read_with(cx, |d, _| d.toast_text().map(str::to_string)), None);
}

// ---- 6.1 read markers ----

#[gpui::test]
fn the_newest_visible_message_is_reported_once_while_the_window_is_active(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_group(&h, cx);
    h.feed(cx, [chunk("g", 1..=3, true)]);
    assert!(h.commands().is_empty(), "an inactive window reports nothing");

    activate(cx);
    assert_eq!(h.commands(), [Command::MarkRead { chat: chat_g(), timestamp: ts(3) }]);

    h.feed(cx, [ClientEvent::MessageUpserted(text("g", 4))]);
    assert_eq!(h.commands(), [Command::MarkRead { chat: chat_g(), timestamp: ts(4) }]);
    let mut edited = text("g", 4);
    edited.edited = true;
    h.feed(cx, [ClientEvent::MessageUpserted(edited)]);
    assert!(h.commands().is_empty(), "the same position is not reported twice");

    cx.deactivate_window();
    h.feed(cx, [ClientEvent::MessageUpserted(text("g", 5))]);
    assert!(h.commands().is_empty(), "nothing is reported while the window is in the background");
    activate(cx);
    assert_eq!(h.commands(), [Command::MarkRead { chat: chat_g(), timestamp: ts(5) }]);
}

#[gpui::test]
fn only_messages_in_view_are_reported_when_scrolled_up(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_group(&h, cx);
    h.feed(cx, [chunk("g", 1..=80, true)]);
    scroll(cx, 800.);
    activate(cx);
    let marks = h.commands();
    let [Command::MarkRead { timestamp, .. }] = marks.as_slice() else { panic!("one marker expected: {marks:?}") };
    assert!((1..80).contains(timestamp), "a message above the newest one: {timestamp}");

    scroll(cx, -1_000_000.);
    cx.run_until_parked();
    assert_eq!(h.commands(), [Command::MarkRead { chat: chat_g(), timestamp: ts(80) }]);
}

#[gpui::test]
fn pending_and_failed_messages_are_never_reported(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_group(&h, cx);
    h.feed(cx, [history("g", vec![text("g", 1), pending("g", 9, "p1", "жду"), failed("g", 10, "p2", "не вышло", "нет прав")], true)]);
    activate(cx);
    assert_eq!(h.commands(), [Command::MarkRead { chat: chat_g(), timestamp: ts(1) }]);
}

// ---- 6.2 typing ----

fn typing_event(cx: &mut VisualTestContext, chat_id: &str, user: &str, name: Option<&str>, secs: u64) -> ClientEvent {
    ClientEvent::Typing {
        chat: ChatId(chat_id.into()),
        user: UserId(user.into()),
        name: name.map(str::to_string),
        until: cx.executor().now() + Duration::from_secs(secs),
    }
}

fn typing_text(h: &Harness, cx: &mut VisualTestContext, chat_id: &str) -> Option<String> {
    h.store.read_with(cx, |s, _| s.typing_text(&ChatId(chat_id.into())))
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
    let draft = Draft { chat: chat_g(), text: "привет".into(), reply_to: None };
    assert_eq!(h.commands(), [Command::SaveDraft(draft)], "the draft is still saved");

    scroll(cx, -1_000_000.);
    cx.run_until_parked();
    assert_eq!(h.commands(), [Command::MarkRead { chat: chat_g(), timestamp: ts(80) }]);
    h.feed(cx, [ClientEvent::ChatUpdated(chat("g", ChatKind::Group, "Команда", Some(80)))]);
    cx.simulate_input("!");
    assert_eq!(h.commands(), [Command::Typing(chat_g())], "typing resumes once the newest message is read");
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
    assert_eq!(typing_text(&h, cx, "g"), Some("Алиса печатает…".into()), "still typing before the timeout");
    cx.executor().advance_clock(Duration::from_secs(2));
    cx.run_until_parked();
    assert_eq!(typing_text(&h, cx, "g"), None);
    h.store.read_with(cx, |s, _| {
        assert_eq!(chat_preview(s, s.chat(&chat_g()).unwrap()), Preview::Message("превью 1".into()));
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
    assert_eq!(typing_text(&h, cx, "g"), Some("Алиса и ещё 1 печатают…".into()));
    assert_eq!(typing_text(&h, cx, "ch"), Some("Кто-то печатает…".into()));
    assert_eq!(typing_text(&h, cx, "p"), Some("печатает…".into()), "a private chat needs no name");
}

#[gpui::test]
fn a_message_from_the_typist_ends_the_indicator(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1)]);
    let alice = typing_event(cx, "g", "alice", Some("Алиса"), 5);
    let bob = typing_event(cx, "g", "bob", Some("Боб"), 5);
    h.feed(cx, [alice, bob]);
    h.feed(cx, [ClientEvent::MessageUpserted(text("g", 2))]);
    assert_eq!(typing_text(&h, cx, "g"), Some("Боб печатает…".into()), "only alice stopped");
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
        cx.bind_keys([KeyBinding::new("cmd-shift-l", ToggleLog, Some("BlazingTelesram"))]);
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
    assert!(h.commands().is_empty(), "neither a send nor a typing notice while the log is open");
    assert_eq!(composer_text(&h, cx), "привет");

    click(cx, "log-close");
    cx.simulate_input("!");
    assert_eq!(composer_text(&h, cx), "привет!", "the composer has the keyboard again");
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
        draft: Draft { chat: chat_g(), text: body.into(), reply_to },
        reason: "диск переполнен".into(),
    }
}

fn toast(h: &Harness, cx: &mut VisualTestContext) -> Option<String> {
    h.desktop.read_with(cx, |d, _| d.toast_text().map(str::to_string))
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
    assert_eq!(composer_reply(&h, cx), Some(reply_target_2()), "no reply was chosen since, so the refused one returns");
    assert_eq!(toast(&h, cx), Some("диск переполнен".into()));
    cx.executor().advance_clock(Duration::from_secs(2));
    cx.run_until_parked();
    let draft = Draft { chat: chat_g(), text: "первое\nвторое".into(), reply_to: Some(reply_target_2()) };
    assert_eq!(writes(&h), [Command::SaveDraft(draft)], "both texts survive the autosave");
}

#[gpui::test]
fn a_refused_send_keeps_the_reply_target_chosen_since(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1), text("g", 2)]);
    cx.simulate_input("первое");
    cx.simulate_keystrokes("enter");
    choose_reply_to_2(cx);
    cx.simulate_input("второе");

    let other = ReplyTarget { timestamp: ts(1), author: Some("ALICE".into()), text: "сообщение 1".into() };
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
    let draft = Draft { chat: chat_g(), text: "первое\nвторое".into(), reply_to: None };
    h.store.read_with(cx, |s, _| assert_eq!(s.draft(&chat_g()), Some(&draft)));
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
    let draft = Draft { chat: chat_g(), text: "первое".into(), reply_to: Some(reply_target_2()) };
    assert_eq!(writes(&h), [Command::SaveDraft(draft)]);
}

#[gpui::test]
fn a_refused_send_during_an_edit_comes_back_when_the_edit_ends(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![own("g", 2, "старый")]);
    cx.simulate_input("первое");
    cx.simulate_keystrokes("enter");
    cx.simulate_keystrokes("up");
    assert_eq!(h.desktop.read_with(cx, |d, _| d.composer_editing()), Some(ts(2)));

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
    assert_eq!(h.desktop.read_with(cx, |d, _| d.composer_editing()), Some(ts(2)));

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
    assert_eq!(h.desktop.read_with(cx, |d, _| d.composer_editing()), Some(ts(2)));
    h.commands();

    h.feed(cx, [refused("первое", None)]);
    let draft = Draft { chat: chat_g(), text: "первое\nвторое".into(), reply_to: None };
    assert_eq!(writes(&h), [Command::SaveDraft(draft.clone())], "persisted without further typing");
    h.store.read_with(cx, |s, _| assert_eq!(s.draft(&chat_g()), Some(&draft)));
    assert_eq!(composer_text(&h, cx), "старый", "the edit is not disturbed");
}

fn tall_own(i: i64) -> Message {
    own("g", i, &(0..150).map(|n| format!("строка {n}")).collect::<Vec<_>>().join("\n"))
}

#[gpui::test]
fn a_message_taller_than_the_viewport_counts_as_seen_once_its_bottom_is_visible(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![tall_own(2)]);
    scroll(cx, 2_000.);
    cx.run_until_parked();
    activate(cx);
    assert!(h.commands().is_empty(), "only the top of the message is visible");

    scroll(cx, -1_000_000.);
    cx.run_until_parked();
    assert_eq!(h.commands(), [Command::MarkRead { chat: chat_g(), timestamp: ts(2) }]);
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
    assert!(row.top() < viewport.top() && row.bottom() > viewport.top(), "clipped at the top: {row:?} in {viewport:?}");

    activate(cx);
    assert_eq!(h.commands(), [Command::MarkRead { chat: chat_g(), timestamp: ts(1) }]);
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
    assert!(!h.desktop.read_with(cx, |d, _| d.menu_is_open()), "no menu above the log");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("ещё");
    assert!(h.commands().is_empty());
    assert_eq!(composer_text(&h, cx), "черновик");
    assert_eq!(h.desktop.read_with(cx, |d, _| d.composer_editing()), None);

    click(cx, "log-close");
    assert!(!h.desktop.read_with(cx, |d, _| d.menu_is_open()), "the menu does not come back");
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
    assert!(h.commands().is_empty(), "the only sent message is far above the viewport");

    scroll_to_top(&h, cx);
    assert_eq!(h.commands(), [Command::MarkRead { chat: chat_g(), timestamp: ts(1) }]);
}

#[gpui::test]
fn nothing_is_reported_while_the_log_covers_the_chat(cx: &mut TestAppContext) {
    with_log_shortcut(cx);
    let (h, cx) = harness(cx);
    open_group(&h, cx);
    h.feed(cx, [chunk("g", 1..=3, true)]);
    activate(cx);
    assert_eq!(h.commands(), [Command::MarkRead { chat: chat_g(), timestamp: ts(3) }]);

    cx.simulate_keystrokes("cmd-shift-l");
    h.feed(cx, [ClientEvent::MessageUpserted(text("g", 4))]);
    assert!(h.commands().is_empty(), "the message arrived behind the log");

    click(cx, "log-close");
    assert_eq!(h.commands(), [Command::MarkRead { chat: chat_g(), timestamp: ts(4) }]);
}

#[gpui::test]
fn a_history_chunk_carrying_the_payload_replaces_the_local_copy(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1), pending("g", 9, "p1", "жду")]);
    h.feed(cx, [ClientEvent::History(HistoryChunk {
        chat: chat_g(),
        messages: vec![text("g", 1), own("g", 5, "жду")],
        reached_start: true,
        replaces: None,
        payloads: vec![PayloadId("p1".into())],
    })]);
    assert_eq!(stamps(&h, cx), [1, 5]);
}

fn forwarded_own(i: i64) -> Message {
    let mut m = own("g", i, "пересланное");
    m.forwarded_from = Some("Анна".into());
    m
}

fn reply_without_id() -> Message {
    let mut m = own("g", 1, "ответ");
    m.reply_to = Some(ReplyQuote { author: None, text: "цитата".into(), timestamp: None });
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
    reply.reply_to = Some(ReplyQuote { author: None, text: "цитата".into(), timestamp: Some(ts(1)) });
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
    open_with(&h, cx, vec![own("g", 2, "первое"), forwarded_own(4), unquotable]);
    cx.simulate_keystrokes("up");
    assert_eq!(h.desktop.read_with(cx, |d, _| d.composer_editing()), Some(ts(2)));
    assert_eq!(composer_text(&h, cx), "первое");
}

// ---- download (store) ----

fn original(id: &str) -> MediaRef {
    media(id, MediaKind::File)
}

fn download_state(h: &Harness, cx: &mut VisualTestContext, original: &MediaRef) -> Option<DownloadState> {
    h.store.read_with(cx, |s, _| s.download_state(original).cloned())
}

#[gpui::test]
fn download_sends_one_command_and_shows_running(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    h.feed(cx, [ClientEvent::Connection(ConnectionState::Online)]);
    let file = original("disk/1");
    h.store.update(cx, |s, cx| s.download(file.clone(), Some("report.pdf".into()), cx));
    assert_eq!(h.commands(), [Command::Download { media: file.clone(), name: Some("report.pdf".into()) }]);
    assert_eq!(download_state(&h, cx, &file), Some(DownloadState::Running { received: 0, total: None }));

    h.store.update(cx, |s, cx| s.download(file.clone(), Some("report.pdf".into()), cx));
    assert!(h.commands().is_empty(), "a running download is not started twice");
}

#[gpui::test]
fn download_events_update_the_state(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    h.feed(cx, [ClientEvent::Connection(ConnectionState::Online)]);
    let file = original("disk/1");
    let other = original("disk/2");
    h.store.update(cx, |s, cx| s.download(file.clone(), None, cx));
    h.commands();

    h.feed(cx, [ClientEvent::DownloadProgress { media: file.clone(), received: 10, total: Some(40) }]);
    assert_eq!(download_state(&h, cx, &file), Some(DownloadState::Running { received: 10, total: Some(40) }));
    assert_eq!(download_state(&h, cx, &other), None, "other files are untouched");

    let saved = PathBuf::from("/tmp/Downloads/report (1).pdf");
    h.feed(cx, [ClientEvent::Downloaded { media: file.clone(), path: saved.clone() }]);
    assert_eq!(download_state(&h, cx, &file), Some(DownloadState::Saved(saved)));

    h.feed(cx, [ClientEvent::DownloadFailed { media: file.clone(), reason: DownloadFailure::Disk }]);
    assert_eq!(download_state(&h, cx, &file), Some(DownloadState::Failed(DownloadFailure::Disk)));

    // A retry after a failure starts again from zero.
    h.store.update(cx, |s, cx| s.download(file.clone(), None, cx));
    assert_eq!(h.commands(), [Command::Download { media: file.clone(), name: None }]);
    assert_eq!(download_state(&h, cx, &file), Some(DownloadState::Running { received: 0, total: None }));
    // Saved files can be fetched again.
    h.feed(cx, [ClientEvent::Downloaded { media: file.clone(), path: PathBuf::from("/tmp/a") }]);
    h.store.update(cx, |s, cx| s.download(file.clone(), None, cx));
    assert_eq!(h.commands().len(), 1);
}

#[gpui::test]
fn download_while_not_online_fails_without_a_command(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    let file = original("disk/1");
    for state in [ConnectionState::Connecting, ConnectionState::Offline] {
        h.feed(cx, [ClientEvent::Connection(state)]);
        h.store.update(cx, |s, cx| s.download(file.clone(), None, cx));
        assert!(h.commands().is_empty(), "{state:?}: nothing is sent");
        assert_eq!(download_state(&h, cx, &file), Some(DownloadState::Failed(DownloadFailure::Offline)));
        h.feed(cx, [ClientEvent::DownloadCancelled(file.clone())]);
    }
}

#[gpui::test]
fn cancel_download_asks_the_core_and_waits_for_its_answer(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    h.feed(cx, [ClientEvent::Connection(ConnectionState::Online)]);
    let file = original("disk/1");
    h.store.update(cx, |s, cx| s.download(file.clone(), None, cx));
    h.commands();

    h.store.read_with(cx, |s, _| s.cancel_download(&file));
    assert_eq!(h.commands(), [Command::CancelDownload(file.clone())]);
    assert_eq!(download_state(&h, cx, &file), Some(DownloadState::Running { received: 0, total: None }), "cleared by the answer");
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
    h.store.update(cx, |s, cx| s.download(file.clone(), None, cx));
    h.feed(cx, [ClientEvent::Downloaded { media: original("disk/2"), path: PathBuf::from("/tmp/b") }, ClientEvent::SignedOut]);
    assert_eq!(download_state(&h, cx, &file), None);
    assert_eq!(download_state(&h, cx, &original("disk/2")), None);
}

#[gpui::test]
fn a_gallery_requests_every_thumbnail_once_and_never_an_original(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    let item = |id: &str| GalleryItem { media: media(id, MediaKind::Thumbnail), width: 4, height: 3, name: Some(format!("{id}.jpg")) };
    let gallery = MessageBody::Gallery { items: vec![item("g/1"), item("g/2"), item("g/3")], caption: None };
    let single = MessageBody::Image { media: media("s/1", MediaKind::Thumbnail), width: 4, height: 3, caption: None, name: None };
    let file = MessageBody::File { media: original("f/1"), name: "a.zip".into(), size: 5 };
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
    h.feed(cx, [ClientEvent::MessageUpserted(message("a", 1, "alice", false, gallery))]);
    assert!(h.commands().is_empty());
}

// ---- image viewer ----

fn picture(id: &str) -> GalleryItem {
    GalleryItem { media: media(id, MediaKind::Thumbnail), width: 1600, height: 800, name: Some(format!("{id}.jpg")) }
}

/// Message 2 of chat "g": four pictures `v/1` … `v/4`.
fn four_pictures() -> Message {
    let items = ["v/1", "v/2", "v/3", "v/4"].map(picture).to_vec();
    message("g", 2, "alice", false, MessageBody::Gallery { items, caption: None })
}

fn open_viewer(h: &Harness, cx: &mut VisualTestContext, ts: MessageTs, index: usize) {
    h.desktop.update(cx, |d, cx| d.open_viewer(chat_g(), ts, index, cx));
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
    assert!(h.commands().is_empty(), "opening the viewer downloads nothing and asks for nothing");
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
    let sized = |id: &str, width, height| GalleryItem { width, height, ..picture(id) };
    let items = vec![sized("v/1", 3000, 4000), sized("v/2", 4000, 1000), sized("v/3", 4000, 4000)];
    open_with(&h, cx, vec![message("g", 2, "alice", false, MessageBody::Gallery { items, caption: None })]);
    open_viewer(&h, cx, ts(2), 0);

    for index in 0..3 {
        let overlay = bounds(cx, "viewer").expect("the viewer is on screen");
        assert_eq!(overlay.size, size(px(850.), px(560.)), "the overlay covers the window");
        let picture = bounds(cx, "viewer-image").unwrap();
        for selector in ["viewer-image", "viewer-prev", "viewer-next", "viewer-close", "viewer-download"] {
            let part = bounds(cx, selector).unwrap_or_else(|| panic!("{selector} is not on screen"));
            assert!(inside(overlay, part), "picture {index}: {selector} {part:?} leaves the window {overlay:?}");
        }
        let (prev, next) = (bounds(cx, "viewer-prev").unwrap(), bounds(cx, "viewer-next").unwrap());
        assert!(prev.right() <= picture.left() && picture.right() <= next.left(), "picture {index} overlaps an arrow");
        let (close, download) = (bounds(cx, "viewer-close").unwrap(), bounds(cx, "viewer-download").unwrap());
        assert!(close.bottom() <= picture.top() && picture.bottom() <= download.top(), "picture {index} overlaps a bar");
        cx.simulate_keystrokes("right");
    }
}

#[gpui::test]
fn the_viewer_picture_follows_the_window_size(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![four_pictures()]);
    open_viewer(&h, cx, ts(2), 0);
    let width = |cx: &mut VisualTestContext| f32::from(bounds(cx, "viewer-image").unwrap().size.width);
    assert!((width(cx) - 800.0).abs() < 0.5);
    cx.simulate_resize(size(px(850.), px(560.)));
    cx.run_until_parked();
    let expected = 850.0 - 32.0 - 2.0 * 64.0; // what the padding and the two arrows leave
    assert!((width(cx) - expected).abs() < 0.5, "{} != {expected}", width(cx));
}

#[gpui::test]
fn the_viewer_ignores_what_it_cannot_show(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1), four_pictures()]);
    open_viewer(&h, cx, ts(2), 4);
    open_viewer(&h, cx, ts(1), 0);
    open_viewer(&h, cx, ts(99), 0);
    h.desktop.update(cx, |d, cx| d.open_viewer(ChatId("other".into()), ts(2), 0, cx));
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
    assert_eq!(viewer_at(&h, cx), Some((ts(2), 0)), "the start does not wrap");
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
    let single = MessageBody::Image { media: media("s/1", MediaKind::Thumbnail), width: 600, height: 1200, caption: None, name: None };
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
    assert_eq!((f32::from(b.size.width), f32::from(b.size.height)), (800.0, 400.0));
    assert!(!shown(cx, "viewer-image-loaded"));

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v2.png");
    std::fs::write(&path, PNG_1X1).unwrap();
    h.feed(cx, [ClientEvent::MediaReady { media: media("v/2", MediaKind::Thumbnail), path }]);
    assert!(shown(cx, "viewer-image-loaded"));
    let b = bounds(cx, "viewer-image").unwrap();
    assert_eq!((f32::from(b.size.width), f32::from(b.size.height)), (800.0, 400.0), "the box does not change");
}

#[gpui::test]
fn deleting_the_message_closes_the_viewer(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![text("g", 1), four_pictures()]);
    open_viewer(&h, cx, ts(2), 1);
    h.feed(cx, [ClientEvent::MessageDeleted { chat: chat_g(), timestamp: ts(2) }]);
    assert_eq!(viewer_at(&h, cx), None);
    assert!(!blocked(&h, cx));
}

#[gpui::test]
fn a_body_without_pictures_closes_the_viewer_and_fewer_pictures_step_back(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![four_pictures()]);
    open_viewer(&h, cx, ts(2), 3);
    let two = MessageBody::Gallery { items: vec![picture("v/1"), picture("v/2")], caption: None };
    h.feed(cx, [ClientEvent::MessageUpserted(message("g", 2, "alice", false, two))]);
    assert_eq!(viewer_at(&h, cx), Some((ts(2), 1)), "the last picture that is left");
    assert!(shown(cx, "viewer-pos-2-2"));

    h.feed(cx, [ClientEvent::MessageUpserted(message("g", 2, "alice", false, MessageBody::Deleted))]);
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
    assert!(h.commands().is_empty(), "no send, no typing notice, nothing read while the viewer is open");
    assert_eq!(composer_text(&h, cx), "привет");

    cx.simulate_keystrokes("escape");
    assert_eq!(h.commands(), [Command::MarkRead { chat: chat_g(), timestamp: ts(2) }], "reading resumes after it closes");
    cx.simulate_input("!");
    assert_eq!(composer_text(&h, cx), "привет!", "the composer has the keyboard again");
}

#[gpui::test]
fn the_download_button_saves_the_original_under_the_items_name(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    h.feed(cx, [ClientEvent::Connection(ConnectionState::Online)]);
    open_with(&h, cx, vec![four_pictures()]);
    open_viewer(&h, cx, ts(2), 2);
    let file = original("v/3");

    click(cx, "viewer-download");
    assert_eq!(h.commands(), [Command::Download { media: file.clone(), name: Some("v/3.jpg".into()) }]);
    assert_eq!(download_state(&h, cx, &file), Some(DownloadState::Running { received: 0, total: None }));

    h.feed(cx, [ClientEvent::DownloadProgress { media: file.clone(), received: 10, total: Some(40) }]);
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

    h.feed(cx, [ClientEvent::DownloadFailed { media: file.clone(), reason: DownloadFailure::Network }]);
    assert!(shown(cx, "viewer-retry"));
    click(cx, "viewer-retry");
    assert_eq!(h.commands(), [Command::Download { media: file.clone(), name: Some("v/1.jpg".into()) }]);

    h.feed(cx, [ClientEvent::Downloaded { media: file.clone(), path: PathBuf::from("/tmp/Downloads/v (1).jpg") }]);
    assert!(shown(cx, "viewer-saved"));
    click(cx, "viewer-download");
    assert_eq!(h.commands().len(), 1, "a saved picture can be fetched again");

    h.feed(cx, [ClientEvent::DownloadProgress { media: original("v/2"), received: 1, total: None }]);
    assert_eq!(h.commands(), [], "another picture's download does not touch this one");
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
    message("g", i, "bob", false, MessageBody::File { media: original("doc"), name: "отчёт.pdf".into(), size: 3_500_000 })
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
    message("g", 2, "alice", false, MessageBody::Gallery { items, caption: None })
}

const CELLS: [&str; 5] = ["msg-2-image-0", "msg-2-image-1", "msg-2-image-2", "msg-2-image-3", "msg-2-image-4"];

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
    assert_eq!(h.commands(), [Command::Download { media: file.clone(), name: Some("отчёт.pdf".into()) }]);
    assert_eq!(download_state(&h, cx, &file), Some(DownloadState::Running { received: 0, total: None }));
    assert!(shown(cx, msg!(5, "cancel")), "a running download offers Cancel");

    h.feed(cx, [ClientEvent::DownloadProgress { media: file.clone(), received: 10, total: Some(40) }]);
    assert!(shown(cx, msg!(5, "status")), "the progress is written next to the button");
    click(cx, msg!(5, "cancel"));
    assert_eq!(h.commands(), [Command::CancelDownload(file.clone())]);
    h.feed(cx, [ClientEvent::DownloadCancelled(file.clone())]);
    assert_eq!(download_state(&h, cx, &file), None);

    h.feed(cx, [ClientEvent::Downloaded { media: file.clone(), path: PathBuf::from("/tmp/Downloads/отчёт (1).pdf") }]);
    assert!(shown(cx, msg!(5, "saved")), "the saved name is shown");
    click(cx, msg!(5, "download"));
    assert_eq!(h.commands().len(), 1, "a saved file can be fetched again");

    h.feed(cx, [ClientEvent::DownloadFailed { media: file.clone(), reason: DownloadFailure::Network }]);
    assert!(shown(cx, msg!(5, "retry")));
    click(cx, msg!(5, "retry"));
    assert_eq!(h.commands(), [Command::Download { media: file.clone(), name: Some("отчёт.pdf".into()) }]);
    assert_eq!(download_state(&h, cx, &file), Some(DownloadState::Running { received: 0, total: None }));

    assert_eq!(dims(cx, msg!(5, "file")), idle, "the card keeps its size in every state");
}

#[gpui::test]
fn the_file_card_cannot_start_a_download_without_a_connection(cx: &mut TestAppContext) {
    let (h, cx) = harness_with(cx, vec![ClientEvent::Connection(ConnectionState::Offline)], |_, _| {});
    open_with(&h, cx, vec![file_message(5)]);
    assert!(shown(cx, msg!(5, "download")));
    assert!(shown(cx, msg!(5, "status")), "the reason is written next to the button");
    click(cx, msg!(5, "download"));
    assert!(h.commands().is_empty());
    assert_eq!(download_state(&h, cx, &original("doc")), None);
}

#[gpui::test]
fn the_file_card_shows_the_reason_a_download_failed(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    h.feed(cx, [ClientEvent::Connection(ConnectionState::Online)]);
    open_with(&h, cx, vec![file_message(5)]);
    h.feed(cx, [ClientEvent::DownloadFailed { media: original("doc"), reason: DownloadFailure::NotFound }]);
    assert!(shown(cx, msg!(5, "retry")));
    assert!(shown(cx, msg!(5, "status")));
    assert!(!shown(cx, msg!(5, "saved")));
}

#[gpui::test]
fn a_gallery_renders_square_cells_in_order_and_keeps_its_height(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![pictures(4)]);
    assert!(!shown(cx, msg!(2, "image")), "a gallery is not a single image");
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
    h.feed(cx, [ClientEvent::MediaReady { media: media("v/2", MediaKind::Thumbnail), path }]);
    assert!(shown(cx, "msg-2-image-1-loaded"));
    assert!(!shown(cx, "msg-2-image-0-loaded"));
    assert_eq!(dims(cx, msg!(2)), before, "the row height does not change when a preview arrives");
    assert_eq!(dims(cx, CELLS[1]), (side, side));
}

#[gpui::test]
fn the_last_row_of_a_grid_is_left_aligned(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_with(&h, cx, vec![pictures(5)]);
    let side = (456.0 - 8.0) / 3.0;
    for cell in &CELLS {
        let (w, h) = dims(cx, cell);
        assert!((w - side).abs() < 1.0 && (h - side).abs() < 1.0, "{cell}: {w}x{h}");
    }
    let at: Vec<_> = CELLS.iter().map(|c| origin(cx, c)).collect();
    assert_eq!((at[1].1, at[2].1), (at[0].1, at[0].1), "first row");
    assert_eq!(at[4].1, at[3].1, "second row");
    assert!(at[3].1 > at[0].1);
    assert_eq!((at[3].0, at[4].0), (at[0].0, at[1].0), "the short row starts at the left edge");
}

#[gpui::test]
fn a_lone_picture_keeps_its_aspect_ratio_and_the_caption_goes_below(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    let one = MessageBody::Gallery { items: vec![picture("v/1")], caption: Some("подпись".into()) };
    open_with(&h, cx, vec![message("g", 2, "alice", false, one)]);
    assert_eq!(dims(cx, msg!(2, "image")), fit_thumbnail(1600, 800, 320.0));
    assert!(!shown(cx, CELLS[0]));
    assert!(y(cx, msg!(2, "caption")) >= y(cx, msg!(2, "image")) + 160.0, "the caption sits below the picture");
}

#[gpui::test]
fn the_caption_of_a_gallery_sits_below_the_last_row(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    let items = ["v/1", "v/2", "v/3", "v/4"].map(picture).to_vec();
    let captioned = MessageBody::Gallery { items, caption: Some("подпись".into()) };
    open_with(&h, cx, vec![message("g", 2, "alice", false, captioned)]);
    assert!(y(cx, msg!(2, "caption")) >= y(cx, CELLS[2]) + 226.0, "the caption sits below the last row");
}

#[gpui::test]
fn clicking_a_cell_or_the_single_image_opens_the_viewer_at_it(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    let single = MessageBody::Image { media: media("s/1", MediaKind::Thumbnail), width: 600, height: 1200, caption: None, name: None };
    open_with(&h, cx, vec![message("g", 1, "alice", false, single), pictures(4)]);
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
    assert!(h.commands().is_empty(), "opening the viewer asks for nothing");
}
