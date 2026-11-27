use serde_json::Value;
use yandex_proto::manage::{created_chat_id, parse_created_chat};
use yandex_proto::request::SearchEntity;
use yandex_proto::search::parse_search;
use yandex_proto::wire::{
    DEFAULT_EXCLUDED_NAMESPACES, PushEvent, ReactionAction, Relation, WireBody, WireChatKind,
    WireMemberRole, WireReactionCount, WireReactions, WireReactor, decode_push, parse_chat_list,
    parse_history, parse_members, parse_profile, parse_reactors, parse_users,
};

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn fixtures_hold_no_credentials() {
    let research = [
        "chat-list",
        "history-page",
        "users",
        "push-edit",
        "push-deleted",
        "push-reaction-removed",
        "history-important",
        "chat-pinned",
        "push-pin",
        "push-pin-replaced",
        "push-pin-replaced-newer",
        "push-pin-removed",
        "reactors-page",
    ]
    .map(|n| format!("research-{n}"));
    let live = ["message", "seen-marker", "typing", "reaction", "heartbeat"]
        .map(|n| format!("live-push-{n}"));
    let members = [
        "members-group",
        "members-private",
        "whoami",
        "create-private",
        "create-chat",
    ]
    .map(|n| format!("live-{n}"));
    for name in research.iter().chain(&live).chain(&members) {
        let text = fixture(&format!("{name}.json")).to_string().to_lowercase();
        for needle in ["session_id", "sessionid2", "csrf", "cookie", "@yandex"] {
            assert!(!text.contains(needle), "{name} contains {needle}");
        }
    }
}

#[test]
fn chat_list_fixture() {
    let chats = parse_chat_list(
        &fixture("research-chat-list.json"),
        &DEFAULT_EXCLUDED_NAMESPACES,
    )
    .unwrap();
    assert_eq!(chats.len(), 3, "namespaces 95 and 5 are excluded");

    let private = &chats[0];
    assert_eq!(private.kind, WireChatKind::Private);
    assert_eq!(private.title.as_deref(), Some("Boris Test"));
    assert_eq!(
        private.avatar_id.as_deref(),
        Some("user_avatar/yapic/1000/abcdef")
    );
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
    assert_eq!(
        last.from.as_ref().unwrap().display_name.as_deref(),
        Some("Announcements")
    );
    assert!(
        channel.raw.get("Messages").is_none(),
        "raw chat keeps no message rows"
    );
}

/// Synthetic list (`research-chat-pinned.json`, built from the documented `PinnedMessageInfo` /
/// `PinnedMessageTimestamps` shapes, not a live capture).
#[test]
fn synthetic_chat_list_carries_the_pin() {
    let chats = parse_chat_list(
        &fixture("research-chat-pinned.json"),
        &DEFAULT_EXCLUDED_NAMESPACES,
    )
    .unwrap();
    let pins: Vec<_> = chats
        .iter()
        .map(|c| (c.title.as_deref().unwrap(), c.pinned, c.pin_action_ts))
        .collect();
    assert_eq!(
        pins,
        [
            (
                "Pinned by info",
                Some(1_784_000_000_003_333),
                Some(1_784_000_000_004_444)
            ),
            ("Pinned by array", Some(1_784_000_000_005_555), None),
            ("Unpinned snapshot", None, Some(1_784_000_000_007_777)),
            ("No pin keys", None, None),
        ],
        "info wins over the array; the array's first positive entry; an unpin keeps its action time"
    );
    assert!(chats[0].raw.get("PinnedMessageInfo").is_some());

    // Entries that predate the pin keys carry no pin.
    let old = parse_chat_list(
        &fixture("research-chat-list.json"),
        &DEFAULT_EXCLUDED_NAMESPACES,
    )
    .unwrap();
    assert!(
        old.iter()
            .all(|c| c.pinned.is_none() && c.pin_action_ts.is_none())
    );
}

#[test]
fn history_fixture_covers_every_message_type() {
    let chat = "0/0/33333333-3333-4333-8333-333333333333";
    let page = parse_history(&fixture("research-history-page.json"), chat, 50).unwrap();
    assert_eq!(page.messages.len(), 14);
    assert!(page.reached_start);
    assert_eq!(page.skipped, 0);
    assert!(
        page.messages
            .windows(2)
            .all(|w| w[0].timestamp < w[1].timestamp)
    );
    let by_ts = |n: i64| &page.messages[(n - 1001) as usize];

    assert!(matches!(&by_ts(1001).body, WireBody::Text(t) if t == "plain text"));
    assert_eq!(
        by_ts(1001).from.as_ref().unwrap().guid.as_deref(),
        Some("g-anna")
    );
    assert!(!by_ts(1001).edited);

    assert!(by_ts(1002).edited);
    assert_eq!(by_ts(1002).version, Some(2));

    assert_eq!(by_ts(1003).body, WireBody::Deleted);

    let Relation::Forward { originals } = &by_ts(1004).relation else {
        panic!("forward")
    };
    assert_eq!(by_ts(1004).body, WireBody::Empty);
    assert_eq!(
        originals[0].from.as_ref().unwrap().display_name.as_deref(),
        Some("Olga")
    );
    assert!(matches!(&originals[0].body, WireBody::Text(t) if t == "original of the forward"));

    let Relation::Reply {
        quote,
        original,
        reference,
    } = &by_ts(1005).relation
    else {
        panic!("reply")
    };
    assert_eq!(quote.as_deref(), Some("plain text"));
    assert_eq!(
        original
            .as_ref()
            .unwrap()
            .from
            .as_ref()
            .unwrap()
            .display_name
            .as_deref(),
        Some("Anna")
    );
    assert_eq!(
        reference.as_ref().unwrap().timestamp,
        Some(1_784_000_000_001_001)
    );
    assert!(matches!(&by_ts(1005).body, WireBody::Text(t) if t == "this is a reply"));

    let WireBody::Image(image) = &by_ts(1006).body else {
        panic!("image")
    };
    assert_eq!(
        (image.file.id.as_str(), image.width, image.height),
        ("bucket-a/99999999-9999-4999-8999-999999999999", 1280, 720)
    );

    let WireBody::File(file) = &by_ts(1007).body else {
        panic!("file")
    };
    assert_eq!(
        (file.name.as_deref(), file.size),
        (Some("report.pdf"), Some(99000))
    );

    assert!(matches!(
        &by_ts(1008).body,
        WireBody::Voice {
            duration: Some(12),
            ..
        }
    ));
    assert!(
        matches!(&by_ts(1009).body, WireBody::Gallery { items, text: Some(_) } if items.len() == 2)
    );
    assert!(matches!(&by_ts(1010).body, WireBody::Sticker { id, .. } if id == "sticker-1"));
    assert!(matches!(&by_ts(1011).body, WireBody::Poll { title } if title == "Lunch?"));
    assert!(
        matches!(&by_ts(1012).body, WireBody::System { kind, .. } if kind == "ParticipantsChangedDiffV2")
    );
    assert_eq!(by_ts(1013).body, WireBody::Other("Hologram".into()));

    let custom = by_ts(1014).from.as_ref().unwrap();
    assert_eq!(
        (custom.guid.as_deref(), custom.display_name.as_deref()),
        (None, Some("Team Bot"))
    );

    // Paging: a smaller limit means the page may not reach the start.
    assert!(
        !parse_history(&fixture("research-history-page.json"), chat, 14)
            .unwrap()
            .reached_start
    );
}

#[test]
fn users_fixture() {
    let users = parse_users(&fixture("research-users.json")["data"]).unwrap();
    assert_eq!(users.len(), 2);
    assert_eq!(users[0].uid.as_deref(), Some("4001"));
    assert_eq!(users[0].avatar_id.as_deref(), Some("user_avatar/yapic/1/a"));
    assert_eq!(users[1].uid.as_deref(), Some("4002"));
    assert_eq!(
        users[1].avatar_id, None,
        "an empty avatar id means no avatar"
    );
}

#[test]
fn push_fixtures() {
    let Ok(PushEvent::MessageUpserted(edit)) = decode_push(&fixture("research-push-edit.json"))
    else {
        panic!("edit")
    };
    assert!(edit.edited);
    assert_eq!(edit.timestamp, 1_784_000_000_001_001);
    assert!(matches!(&edit.body, WireBody::Text(t) if t == "edited via push"));

    assert_eq!(
        decode_push(&fixture("research-push-deleted.json")),
        Ok(PushEvent::MessageDeleted {
            chat: "0/0/33333333-3333-4333-8333-333333333333".into(),
            timestamp: 1_784_000_000_001_002
        })
    );
}

/// Live captures of `Pin` pushes (`research-push-pin*.json`, 2026-11-27, chat id and guids replaced by
/// placeholders, timestamps verbatim): a `Timestamp` pins, its absence unpins; the action time is
/// `LastEditTimestamp`, never the pinned message's own `Timestamp`.
#[test]
fn live_pin_push_fixtures() {
    let chat = "00000001-0000-4000-8000-000000000001_00000002-0000-4000-8000-000000000002";
    // Pin of A (the older message).
    assert_eq!(
        decode_push(&fixture("research-push-pin.json")),
        Ok(PushEvent::PinChanged {
            chat: chat.into(),
            pinned: Some(1_795_800_545_954_038),
            at: Some(1_795_800_682_870_038)
        })
    );
    // B (newer) replaces A: the pinned message is newer, the action time later.
    assert_eq!(
        decode_push(&fixture("research-push-pin-replaced-newer.json")),
        Ok(PushEvent::PinChanged {
            chat: chat.into(),
            pinned: Some(1_795_800_549_275_038),
            at: Some(1_795_800_826_776_038)
        })
    );
    // A (older) replaces B: the pinned message is older than the replaced one, the action time newer.
    assert_eq!(
        decode_push(&fixture("research-push-pin-replaced.json")),
        Ok(PushEvent::PinChanged {
            chat: chat.into(),
            pinned: Some(1_795_800_545_954_038),
            at: Some(1_795_800_969_725_038)
        })
    );
    assert_eq!(
        decode_push(&fixture("research-push-pin-removed.json")),
        Ok(PushEvent::PinChanged {
            chat: chat.into(),
            pinned: None,
            at: Some(1_795_801_115_044_038)
        })
    );
}

/// Synthetic page (`research-history-important.json`, built from the documented `IsImportant`
/// shape, not a live capture): the flag is true only for a literal JSON `true` on a live message.
#[test]
fn synthetic_history_marks_important_rows() {
    let chat = "0/0/44444444-4444-4444-8444-444444444444";
    let page = parse_history(&fixture("research-history-important.json"), chat, 50).unwrap();
    assert_eq!(page.messages.len(), 7);
    assert_eq!(page.skipped, 0);
    let flags: Vec<bool> = page.messages.iter().map(|m| m.important).collect();
    assert_eq!(
        flags,
        [true, false, false, false, true, false, true],
        "true, false, missing, non-bool, photo, deleted, edited"
    );
    assert!(matches!(&page.messages[0].body, WireBody::Text(t) if t == "important text"));
    assert!(matches!(page.messages[4].body, WireBody::Image(_)));
    assert_eq!(page.messages[5].body, WireBody::Deleted);
    assert!(page.messages[6].edited);
    assert_eq!(
        page.messages[0].raw["ClientMessage"]["Plain"]["IsImportant"], true,
        "raw keeps the flag"
    );
}

/// Every existing fixture predates the importance flag: all of its messages are not important.
#[test]
fn existing_fixtures_decode_as_not_important() {
    let chat = "0/0/33333333-3333-4333-8333-333333333333";
    let page = parse_history(&fixture("research-history-page.json"), chat, 50).unwrap();
    assert!(page.messages.iter().all(|m| !m.important));

    let chats = parse_chat_list(
        &fixture("research-chat-list.json"),
        &DEFAULT_EXCLUDED_NAMESPACES,
    )
    .unwrap();
    let last: Vec<_> = chats
        .iter()
        .filter_map(|c| c.last_message.as_ref())
        .collect();
    assert!(!last.is_empty());
    assert!(last.iter().all(|m| !m.important));

    for name in ["research-push-edit.json", "live-push-message.json"] {
        let Ok(PushEvent::MessageUpserted(m)) = decode_push(&fixture(name)) else {
            panic!("{name}")
        };
        assert!(!m.important, "{name}");
    }
}

/// Pushes captured on a live account (2026-11-20), identifiers and text replaced.
#[test]
fn live_push_fixtures() {
    let Ok(PushEvent::MessageUpserted(message)) = decode_push(&fixture("live-push-message.json"))
    else {
        panic!("message")
    };
    assert_eq!(message.timestamp, 1_795_172_236_485_108);
    assert!(matches!(&message.body, WireBody::Text(t) if t == "live sample text"));
    assert_eq!(
        message.from.unwrap().display_name.as_deref(),
        Some("Test User")
    );

    // Read on another device: `NotificationBehaviour` comes along and must not hide it.
    assert_eq!(
        decode_push(&fixture("live-push-seen-marker.json")),
        Ok(PushEvent::ChatChanged {
            chat: "100/0/00000003-0000-4000-8000-000000000003_1795070677887127".into()
        })
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
    assert_eq!(
        decode_push(&fixture("live-push-heartbeat.json")),
        Ok(PushEvent::Ignored {
            kind: "Heartbeat".into()
        })
    );
}

/// The reaction push captured live: a first reaction (`Replace`) with its counts and the actor's own entry.
#[test]
fn live_reaction_push_fixture() {
    assert_eq!(
        decode_push(&fixture("live-push-reaction.json")),
        Ok(PushEvent::Reaction {
            chat: "0/0/00000003-0000-4000-8000-000000000003".into(),
            timestamp: 1_795_070_677_887_127,
            actor: Some("00000007-0000-4000-8000-000000000007".into()),
            action: Some(ReactionAction::Replace),
            code: Some(100101),
            reactions: WireReactions {
                counts: vec![WireReactionCount {
                    code: 100101,
                    count: 1
                }],
                version: Some(2),
                recent: vec![WireReactor {
                    user: Some("00000007-0000-4000-8000-000000000007".into()),
                    code: 100101,
                    timestamp: Some(1_795_172_185_683_559),
                }],
            },
        })
    );
}

/// Removing the last reaction: no `Reactions` key, only a newer `ReactionsVersion` (synthetic).
#[test]
fn removed_reaction_push_fixture_decodes_with_empty_counts() {
    assert_eq!(
        decode_push(&fixture("research-push-reaction-removed.json")),
        Ok(PushEvent::Reaction {
            chat: "0/0/00000003-0000-4000-8000-000000000003".into(),
            timestamp: 1_795_070_677_887_127,
            actor: Some("00000007-0000-4000-8000-000000000007".into()),
            action: Some(ReactionAction::Remove),
            code: None,
            reactions: WireReactions {
                counts: vec![],
                version: Some(3),
                recent: vec![]
            },
        })
    );
}

/// The echo of an own send carries the `PayloadId` the sender chose.
#[test]
fn live_echo_fixture_carries_the_payload_id() {
    let Ok(PushEvent::MessageUpserted(message)) = decode_push(&fixture("live-push-message.json"))
    else {
        panic!("message")
    };
    assert_eq!(message.payload_id.as_deref(), Some("fake-payload-id"));
}

/// The `search` reply of a real account (2026-11-24, redacted: names, texts, titles, e-mail and
/// organisation names are `<...>`, ids are `00000000-0000-4000-8000-0000000000NN`; keys, structure
/// and numbers are kept). The three sets come from three queries: a common word (messages), a
/// chat-title word (chats) and a name prefix (users); each set is complete as received.
///
/// Observed item shape. Every item is `{data, entity, matches, type}`; chats also carry a
/// top-level `member_count`.
/// - messages: `type: "messages"`, `entity: "message"`, `matches.text` = matched word forms
///   (stemming: `всем` matched `всему`); `data` = `{ClientMessage: {Plain: {ChatId, PayloadId,
///   Text: {MessageText}}}, ServerMessageInfo: {From: {AvatarId, DisplayName, Guid, PhoneId,
///   Version}, PrevTimestamp?, SeqNo, Timestamp, Version}}`, i.e. a `ServerMessage`.
///   `PrevTimestamp` is absent on a chat's first message. The hit carries no chat title. Only
///   `Plain` bodies were observed, so decoding tolerates any `ClientMessage` shape.
/// - users: `type: "users_pvp"`, `entity: "user"`, `matches: {}`, `data` = `{account_category,
///   avatar_id, contacts[], display_name, employee_info{..}, employees_info[..], gender, guid,
///   phone_id, public_name, registration_status, version}`. The set is not the chat partners
///   (about 15 name queries matched one unrelated record), so it is usually empty.
/// - chats: `type: "chats"`, `entity: "chat"`, `matches.name` (word prefix: `Dev` matched
///   `DevOps`), `data` = `{avatar_id?, chat_id, create_timestamp, description, members[guid],
///   members_count, name, public, rights[], role, role_version, roles{admin: [guid]}, version}`;
///   `chat_id` has the chat-list format (`0/22/<guid>`), and a hit can be absent from the loaded
///   chat list. Server matching is not the local substring filter.
///
/// Paging: `total` is the number of items in this reply (`total == min(limit, matches)`), `limit`
/// echoes the request, `page` and `pages` were always 1, so the server never reports that more
/// exists. An extra page parameter changes nothing: `page` (0, 1, 2, also next to `limit`),
/// `page_num`, `pageNumber`, `page_number`, `offset`, `skip`, `start`, `from`, `after`, `cursor`
/// and `next_page` all returned the first page unchanged, without an error code. Pagination is
/// therefore unsupported and stays a follow-up.
#[test]
fn live_search_fixture() {
    let raw = fixture("live-search.json");
    let text = raw.to_string().to_lowercase();
    for needle in ["session_id", "sessionid2", "csrf", "cookie", "@yandex"] {
        assert!(!text.contains(needle), "live-search contains {needle}");
    }

    let all = [
        SearchEntity::Messages,
        SearchEntity::Users,
        SearchEntity::Chats,
    ];
    let reply = parse_search(&raw, &all).unwrap();
    for set in [&reply.messages, &reply.users, &reply.chats] {
        assert_eq!((set.limit, set.page, set.pages), (10, 1, 1));
        assert_eq!(set.total as usize, set.items.len(), "every set is complete");
    }
    let hits = &reply.decoded;
    assert_eq!(hits.skipped, 0, "every captured item has an address");

    assert_eq!(hits.messages.len(), 7);
    let first = &hits.messages[0];
    assert_eq!(first.chat, "0/22/00000000-0000-4000-8000-000000000001");
    assert_eq!(first.timestamp, 1_790_838_566_171_053);
    assert!(matches!(&first.body, WireBody::Text(t) if t == "<message text>"));
    let from = first.from.as_ref().unwrap();
    assert_eq!(
        from.guid.as_deref(),
        Some("00000000-0000-4000-8000-000000000002")
    );
    assert_eq!(from.display_name.as_deref(), Some("<name>"));
    assert_eq!(
        from.avatar_id.as_deref(),
        Some("user_avatar/yapic/1/placeholder")
    );
    assert_eq!(
        hits.messages
            .iter()
            .filter(|m| m.chat.ends_with("0001"))
            .count(),
        5
    );
    // The last hit has no `PrevTimestamp`: still decoded.
    assert_eq!(hits.messages[6].timestamp, 1_769_608_744_339_002);

    let [user] = hits.users.as_slice() else {
        panic!("{:?}", hits.users)
    };
    assert_eq!(user.guid, "00000000-0000-4000-8000-000000000009");
    assert_eq!(user.display_name.as_deref(), Some("<name>"));
    assert_eq!(
        user.avatar_id.as_deref(),
        Some("user_avatar/yapic/1/placeholder")
    );

    let [with_avatar, without_avatar] = hits.chats.as_slice() else {
        panic!("{:?}", hits.chats)
    };
    assert_eq!(with_avatar.id, "0/22/00000000-0000-4000-8000-000000000011");
    assert_eq!(with_avatar.kind, WireChatKind::Group);
    assert_eq!(with_avatar.title.as_deref(), Some("<chat title>"));
    assert!(
        with_avatar
            .avatar_id
            .as_deref()
            .unwrap()
            .starts_with("chat_avatar/")
    );
    assert_eq!(with_avatar.members_count, Some(4));
    assert_eq!(
        without_avatar.id,
        "0/22/00000000-0000-4000-8000-000000000017"
    );
    assert_eq!(without_avatar.avatar_id, None);
    assert_eq!(without_avatar.members_count, Some(5));
}

/// A `get_chat_members` page of a real group (2026-11-24; four users from different groups to
/// cover every key seen, placeholders for names, e-mails, positions, organisations and ids).
/// `groups` and `departments` were empty in all 70 groups probed, so they are not modelled.
#[test]
fn live_members_group_fixture() {
    let members = parse_members(&fixture("live-members-group.json")).unwrap();
    assert_eq!(members.skipped, 0);
    let [admin, member, required_only, deleted] = members.users.as_slice() else {
        panic!("{:?}", members.users)
    };

    assert_eq!(
        (admin.guid.as_str(), &admin.role),
        (
            "00000000-0000-4000-8000-000000000001",
            &WireMemberRole::Admin
        )
    );
    assert_eq!(admin.display_name.as_deref(), Some("<name>"));
    assert_eq!(
        admin.avatar_id.as_deref(),
        Some("user_avatar/yapic/1/placeholder")
    );
    assert!(!admin.deleted);

    assert_eq!(member.role, WireMemberRole::Member);
    assert!(!member.deleted);

    // Only the keys every user carries: no avatar.
    assert_eq!(required_only.guid, "00000000-0000-4000-8000-000000000003");
    assert_eq!(required_only.role, WireMemberRole::Member);
    assert_eq!(
        (
            required_only.display_name.as_deref(),
            required_only.avatar_id.as_deref()
        ),
        (Some("<name>"), None)
    );

    assert!(deleted.deleted);
    assert_eq!(deleted.role, WireMemberRole::Member);

    // Empty `groups` and `departments` change nothing; neither does a page without them.
    let raw = fixture("live-members-group.json");
    assert_eq!(raw["groups"], serde_json::json!([]));
    assert_eq!(raw["departments"], serde_json::json!([]));
    let users_only = serde_json::json!({ "users": raw["users"] });
    assert_eq!(parse_members(&users_only).unwrap(), members);
}

/// What the server says for private chats, the self chat and channels: an error body, no users.
#[test]
fn live_members_refusal_fixture() {
    let refusal = fixture("live-members-private.json");
    assert_eq!(refusal["status"], "error");
    assert_eq!(
        refusal["data"]["code"],
        "no_permissions_for_this_chat_operation"
    );
    assert!(
        parse_members(&refusal["data"]).is_err(),
        "an error body is not a member page"
    );
}

/// Live `create_private_chat` reply (task 5.1, 2026-11-27, request `{guid, wait_for_update: true}`
/// with no `onetime_user_token`): `status: ok`, `data` = the chat (`data.chat.chat_id`) and the
/// partner's user object. The chat already existed, so this is the "open an existing chat" answer.
#[test]
fn live_create_private_fixture() {
    let reply = fixture("live-create-private.json");
    assert_eq!(reply["status"], "ok");
    let data = &reply["data"];
    assert_eq!(
        created_chat_id(data).as_deref(),
        Some("00000000-0000-4000-8000-0000000000b1_00000000-0000-4000-8000-0000000000a1")
    );
    assert_eq!(data["chat"]["private"], true);
    assert_eq!(data["chat"]["members_count"], 2);
    // The group parser reads the same envelope without inventing refusals.
    let created = parse_created_chat(data);
    assert!(created.refused.is_empty());
    assert_eq!(created.chat, created_chat_id(data));
}

/// Live `create_chat` reply (task 6.2, 2026-11-27, flag-on payload): `status: ok` and `data` is
/// the new chat itself, so the id is `data.chat_id` (not `data.chat.chat_id` as the web bundle
/// suggested); there is no `errors` key when every participant was added.
#[test]
fn live_create_chat_fixture() {
    let reply = fixture("live-create-chat.json");
    assert_eq!(reply["status"], "ok");
    let data = &reply["data"];
    let created = parse_created_chat(data);
    assert_eq!(
        created.chat.as_deref(),
        Some("0/0/00000000-0000-4000-8000-0000000000c1")
    );
    assert!(created.refused.is_empty());
    assert_eq!(data["members_count"], 2);
    assert_eq!(data["role"], "admin");
}

/// A live `list_reactions` page (2026-11-27, `research-reactors-page.json`: the first four entries
/// of a group message with 16 reactors, `Limit` 4, identities replaced by placeholders and
/// timestamps rebased): newest first, every entry carries `Type`, `Timestamp` and a full
/// `UserInfo`, and the reply has a top-level `ReadsVersion` the client ignores. The variants
/// below (a user without an avatar or a name, a missing `Timestamp`) were not observed live.
#[test]
fn live_reactors_page() {
    let mut raw = fixture("research-reactors-page.json");
    let rows = |page: &yandex_proto::wire::WireReactors| {
        page.entries
            .iter()
            .map(|e| {
                (
                    e.guid.rsplit('-').next().unwrap().to_owned(),
                    e.display_name.clone(),
                    e.avatar_id.is_some(),
                    e.code,
                    e.timestamp,
                )
            })
            .collect::<Vec<_>>()
    };
    let page = parse_reactors(&raw).unwrap();
    assert_eq!((page.skipped, page.raw), (0, 4));
    assert_eq!(page.oldest, Some(1795171900000000));
    assert_eq!(
        rows(&page),
        [
            (
                "000000000001".to_owned(),
                Some("Test User 1".to_owned()),
                true,
                100108,
                Some(1795489105771133)
            ),
            (
                "000000000002".to_owned(),
                Some("Test User 2".to_owned()),
                true,
                100108,
                Some(1795176644017321)
            ),
            (
                "000000000003".to_owned(),
                Some("Test User 3".to_owned()),
                true,
                100108,
                Some(1795172896602932)
            ),
            (
                "000000000004".to_owned(),
                Some("Test User 4".to_owned()),
                true,
                100104,
                Some(1795171900000000)
            ),
        ]
    );

    // Not observed live: the third user without an avatar, the fourth without a name and an
    // avatar, the second without a timestamp (it still counts as a raw entry).
    raw["UserReactions"][2]["UserInfo"]
        .as_object_mut()
        .unwrap()
        .remove("AvatarId");
    let fourth = raw["UserReactions"][3]["UserInfo"].as_object_mut().unwrap();
    fourth.remove("AvatarId");
    fourth.remove("DisplayName");
    raw["UserReactions"][1]
        .as_object_mut()
        .unwrap()
        .remove("Timestamp");
    let page = parse_reactors(&raw).unwrap();
    assert_eq!((page.skipped, page.raw), (0, 4));
    assert_eq!(page.oldest, Some(1795171900000000));
    let rows = rows(&page);
    assert_eq!(rows[1].4, None);
    assert_eq!((rows[2].1.is_some(), rows[2].2), (true, false));
    assert_eq!((rows[3].1.clone(), rows[3].2), (None, false));
}

/// What the server answered live (2026-11-27) for a message nobody reacted to, and for a read it
/// refuses: an empty list has no `UserReactions` key (only `ReadsVersion`); a refusal is
/// `Status` 4 with an `ErrorInfo {Message, WorkerId}` and no `Details` ("message not found" for a
/// timestamp that does not exist, "chat not found" for an unknown chat id, "reactions are
/// anonymous" for a channel message).
#[test]
fn live_reactors_empty_and_refused_replies() {
    let empty = parse_reactors(&serde_json::json!({ "ReadsVersion": 6539 })).unwrap();
    assert_eq!((empty.raw, empty.skipped, empty.oldest), (0, 0, None));
    assert!(empty.entries.is_empty());

    let refused = serde_json::json!({
        "ErrorInfo": { "Message": "reactions are anonymous", "WorkerId": 137 },
        "ReadsVersion": 2648,
        "Status": 4
    });
    let err = parse_reactors(&refused).unwrap_err();
    assert!(
        err.to_string().contains("ENTITY_NOT_FOUND"),
        "a refused read is an error, not an empty page: {err}"
    );
    // The reason is the server's `ErrorInfo.Message`, not lost with the missing `Details`.
    let yandex_proto::error::ProtoError::Protocol(text) = &err else {
        panic!("a refusal is a protocol error: {err}");
    };
    assert_eq!(
        yandex_proto::wire::refusal_reason(text),
        Some("reactions are anonymous")
    );
}

/// The `whoami` reply of a real account (2026-11-24, placeholders): no status set, so
/// `UserStatus` is `{}`. The populated shape was not observed live; the variants below are
/// synthetic and follow the web client's `CustomStatus.Text` path.
#[test]
fn live_whoami_fixture() {
    let mut raw = fixture("live-whoami.json");
    let profile = parse_profile(&raw).unwrap();
    assert_eq!(
        profile.guid.as_deref(),
        Some("00000000-0000-4000-8000-000000000001")
    );
    assert_eq!(profile.name.as_deref(), Some("<name>"));
    assert_eq!(
        profile.avatar_id.as_deref(),
        Some("user_avatar/yapic/1/placeholder")
    );
    assert_eq!(
        profile.status_text, None,
        "an empty UserStatus is no status"
    );

    raw["UserStatusInfo"]["UserStatus"] = serde_json::json!({ "Availability": "Away", "CustomStatus": { "Emoji": "x", "Text": "Out of office" } });
    assert_eq!(
        parse_profile(&raw).unwrap().status_text.as_deref(),
        Some("Out of office")
    );
    raw["UserStatusInfo"]["UserStatus"] = serde_json::json!({ "text": "Out of office" });
    assert_eq!(
        parse_profile(&raw).unwrap().status_text,
        None,
        "the flat `text` key is not the server's shape"
    );
}
