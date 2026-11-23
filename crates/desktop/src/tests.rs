//! Headless UI tests: the real `ChatStore` and `Desktop` driven through channels, inspected via debug bounds.

use crate::desktop::{AccountMenu, Desktop, ToggleLog};
use crate::format::{fit_thumbnail, Clock};
use crate::logging::{LogBuffer, LogLine};
use crate::store::{ChatFilter, ChatStore, SessionState};
use async_channel::{Receiver, Sender};
use bt_chat_core::model::*;
use gpui::{
    point, px, size, AppContext, Bounds, Context, Entity, KeyBinding, ListState, Modifiers, Pixels, ScrollDelta,
    ScrollWheelEvent, TestAppContext, VisualTestContext,
};
use std::path::PathBuf;
use std::time::Duration;

const PNG_1X1: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00,
    0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49,
    0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xff, 0xff, 0x3f, 0x00, 0x05, 0xfe, 0x02, 0xfe, 0xdc, 0xcc, 0x59, 0xe7,
    0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
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
    })
}

fn history(chat_id: &str, messages: Vec<Message>, reached_start: bool) -> ClientEvent {
    ClientEvent::History(HistoryChunk { chat: ChatId(chat_id.into()), messages, reached_start, replaces: None })
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

/// `debug_bounds` entries are never removed once painted, so "not shown" is only meaningful in a window
/// that never painted the element: tests of absence start from `events` and `setup` applied before the first frame.
fn harness_with(
    cx: &mut TestAppContext,
    events: Vec<ClientEvent>,
    setup: impl FnOnce(&mut Desktop, &mut Context<Desktop>) + 'static,
) -> (Harness, &mut VisualTestContext) {
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
        ClientEvent::History(HistoryChunk { chat: ChatId("b".into()), messages, reached_start: false, replaces })
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
        d.search.update(cx, |input, cx| input.set(query.to_string(), cx));
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
    h.feed(cx, [chunk("g", 1..=0, false)]);
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
                    MessageBody::Image { media: landscape.clone(), width: 1600, height: 800, caption: Some("подпись".into()) },
                ),
                message(
                    "g",
                    2,
                    "bob",
                    true,
                    MessageBody::Image { media: portrait.clone(), width: 600, height: 1200, caption: None },
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

// ---- 6.5 read-only, connection, sign-out ----

#[gpui::test]
fn composer_is_disabled_with_a_notice(cx: &mut TestAppContext) {
    let (h, cx) = harness(cx);
    open_group(&h, cx);
    h.feed(cx, [chunk("g", 1..=3, true)]);
    assert!(shown(cx, "composer"));
    assert!(shown(cx, "composer-disabled"));
    assert!(shown(cx, "composer-notice"));
    assert!(crate::desktop::READ_ONLY_NOTICE.contains("только для чтения"));

    click(cx, "composer-disabled");
    cx.simulate_input("привет");
    let typed = h.desktop.read_with(cx, |d, cx| d.search.read(cx).content.to_string());
    assert!(typed.is_empty(), "no input field takes the text");
    assert!(h.commands().is_empty(), "nothing is sent");
}

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
