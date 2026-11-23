use serde_json::Value;
use yandex_proto::wire::{
    DEFAULT_EXCLUDED_NAMESPACES, PushEvent, Relation, WireBody, WireChatKind, decode_push, parse_chat_list,
    parse_history, parse_users,
};

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn fixtures_hold_no_credentials() {
    let research = ["chat-list", "history-page", "users", "push-edit", "push-deleted"].map(|n| format!("research-{n}"));
    let live = ["message", "seen-marker", "typing", "reaction", "heartbeat"].map(|n| format!("live-push-{n}"));
    for name in research.iter().chain(&live) {
        let text = fixture(&format!("{name}.json")).to_string().to_lowercase();
        for needle in ["session_id", "sessionid2", "csrf", "cookie", "@yandex"] {
            assert!(!text.contains(needle), "{name} contains {needle}");
        }
    }
}

#[test]
fn chat_list_fixture() {
    let chats = parse_chat_list(&fixture("research-chat-list.json"), &DEFAULT_EXCLUDED_NAMESPACES).unwrap();
    assert_eq!(chats.len(), 3, "namespaces 95 and 5 are excluded");

    let private = &chats[0];
    assert_eq!(private.kind, WireChatKind::Private);
    assert_eq!(private.title.as_deref(), Some("Boris Test"));
    assert_eq!(private.avatar_id.as_deref(), Some("user_avatar/yapic/1000/abcdef"));
    assert_eq!(private.unread, 3);
    assert!(!private.muted);
    assert_eq!(private.last_activity, Some(1_784_117_592_261_029));
    let last = private.last_message.as_ref().unwrap();
    assert!(matches!(&last.body, WireBody::Text(t) if t == "See you at the call"));

    let group = &chats[1];
    assert_eq!(group.kind, WireChatKind::Group);
    assert_eq!(group.title.as_deref(), Some("Team chat"));
    assert!(group.muted);
    assert_eq!(group.unread, 0);
    assert!(group.last_message.is_none());

    let channel = &chats[2];
    assert_eq!(channel.kind, WireChatKind::Channel);
    assert_eq!(channel.unread, 3);
    let last = channel.last_message.as_ref().unwrap();
    assert!(matches!(last.body, WireBody::Image(_)));
    assert_eq!(last.from.as_ref().unwrap().display_name.as_deref(), Some("Announcements"));
    assert!(channel.raw.get("Messages").is_none(), "raw chat keeps no message rows");
}

#[test]
fn history_fixture_covers_every_message_type() {
    let chat = "0/0/33333333-3333-4333-8333-333333333333";
    let page = parse_history(&fixture("research-history-page.json"), chat, 50).unwrap();
    assert_eq!(page.messages.len(), 14);
    assert!(page.reached_start);
    assert_eq!(page.skipped, 0);
    assert!(page.messages.windows(2).all(|w| w[0].timestamp < w[1].timestamp));
    let by_ts = |n: i64| &page.messages[(n - 1001) as usize];

    assert!(matches!(&by_ts(1001).body, WireBody::Text(t) if t == "plain text"));
    assert_eq!(by_ts(1001).from.as_ref().unwrap().guid.as_deref(), Some("g-anna"));
    assert!(!by_ts(1001).edited);

    assert!(by_ts(1002).edited);
    assert_eq!(by_ts(1002).version, Some(2));

    assert_eq!(by_ts(1003).body, WireBody::Deleted);

    let Relation::Forward { originals } = &by_ts(1004).relation else { panic!("forward") };
    assert_eq!(by_ts(1004).body, WireBody::Empty);
    assert_eq!(originals[0].from.as_ref().unwrap().display_name.as_deref(), Some("Olga"));
    assert!(matches!(&originals[0].body, WireBody::Text(t) if t == "original of the forward"));

    let Relation::Reply { quote, original, reference } = &by_ts(1005).relation else { panic!("reply") };
    assert_eq!(quote.as_deref(), Some("plain text"));
    assert_eq!(original.as_ref().unwrap().from.as_ref().unwrap().display_name.as_deref(), Some("Anna"));
    assert_eq!(reference.as_ref().unwrap().timestamp, Some(1_784_000_000_001_001));
    assert!(matches!(&by_ts(1005).body, WireBody::Text(t) if t == "this is a reply"));

    let WireBody::Image(image) = &by_ts(1006).body else { panic!("image") };
    assert_eq!((image.file.id.as_str(), image.width, image.height), ("bucket-a/99999999-9999-4999-8999-999999999999", 1280, 720));

    let WireBody::File(file) = &by_ts(1007).body else { panic!("file") };
    assert_eq!((file.name.as_deref(), file.size), (Some("report.pdf"), Some(99000)));

    assert!(matches!(&by_ts(1008).body, WireBody::Voice { duration: Some(12), .. }));
    assert!(matches!(&by_ts(1009).body, WireBody::Gallery { items, text: Some(_) } if items.len() == 2));
    assert!(matches!(&by_ts(1010).body, WireBody::Sticker { id, .. } if id == "sticker-1"));
    assert!(matches!(&by_ts(1011).body, WireBody::Poll { title } if title == "Lunch?"));
    assert!(matches!(&by_ts(1012).body, WireBody::System { kind, .. } if kind == "ParticipantsChangedDiffV2"));
    assert_eq!(by_ts(1013).body, WireBody::Other("Hologram".into()));

    let custom = by_ts(1014).from.as_ref().unwrap();
    assert_eq!((custom.guid.as_deref(), custom.display_name.as_deref()), (None, Some("Team Bot")));

    // Paging: a smaller limit means the page may not reach the start.
    assert!(!parse_history(&fixture("research-history-page.json"), chat, 14).unwrap().reached_start);
}

#[test]
fn users_fixture() {
    let users = parse_users(&fixture("research-users.json")["data"]).unwrap();
    assert_eq!(users.len(), 2);
    assert_eq!(users[0].uid.as_deref(), Some("4001"));
    assert_eq!(users[0].avatar_id.as_deref(), Some("user_avatar/yapic/1/a"));
    assert_eq!(users[1].uid.as_deref(), Some("4002"));
    assert_eq!(users[1].avatar_id, None, "an empty avatar id means no avatar");
}

#[test]
fn push_fixtures() {
    let Ok(PushEvent::MessageUpserted(edit)) = decode_push(&fixture("research-push-edit.json")) else { panic!("edit") };
    assert!(edit.edited);
    assert_eq!(edit.timestamp, 1_784_000_000_001_001);
    assert!(matches!(&edit.body, WireBody::Text(t) if t == "edited via push"));

    assert_eq!(
        decode_push(&fixture("research-push-deleted.json")),
        Ok(PushEvent::MessageDeleted { chat: "0/0/33333333-3333-4333-8333-333333333333".into(), timestamp: 1_784_000_000_001_002 })
    );
}

/// Pushes captured on a live account (2026-11-20), identifiers and text replaced.
#[test]
fn live_push_fixtures() {
    let Ok(PushEvent::MessageUpserted(message)) = decode_push(&fixture("live-push-message.json")) else { panic!("message") };
    assert_eq!(message.timestamp, 1_795_172_236_485_108);
    assert!(matches!(&message.body, WireBody::Text(t) if t == "live sample text"));
    assert_eq!(message.from.unwrap().display_name.as_deref(), Some("Test User"));

    // Read on another device: `NotificationBehaviour` comes along and must not hide it.
    assert_eq!(
        decode_push(&fixture("live-push-seen-marker.json")),
        Ok(PushEvent::ChatChanged { chat: "100/0/00000003-0000-4000-8000-000000000003_1795070677887127".into() })
    );
    assert_eq!(
        decode_push(&fixture("live-push-typing.json")),
        Ok(PushEvent::Typing {
            chat: "100/0/00000005-0000-4000-8000-000000000005_1795170823506046".into(),
            user: "00000006-0000-4000-8000-000000000006".into(),
            name: None,
            timeout_secs: None,
        })
    );
    for (name, kind) in [("reaction", "Reaction"), ("heartbeat", "Heartbeat")] {
        assert_eq!(decode_push(&fixture(&format!("live-push-{name}.json"))), Ok(PushEvent::Ignored { kind: kind.into() }), "{name}");
    }
}

/// The echo of an own send carries the `PayloadId` the sender chose.
#[test]
fn live_echo_fixture_carries_the_payload_id() {
    let Ok(PushEvent::MessageUpserted(message)) = decode_push(&fixture("live-push-message.json")) else { panic!("message") };
    assert_eq!(message.payload_id.as_deref(), Some("fake-payload-id"));
}
