//! The pinned message in the engine (design D5, rules 1-5): pushes, chat-list refreshes with
//! arbitration by `ChatSummary.pin_action_ts`, the lazily resolved preview of the open chat and
//! its follow-through of edits and deletions. The cache is a real (temporary) one; the backend
//! is the fake of the parent module.

use super::*;

fn pinned(timestamp: MessageTs, preview: Option<MessagePreview>) -> Option<PinnedMessage> {
    Some(PinnedMessage { timestamp, preview })
}

fn text_preview(timestamp: MessageTs, text: &str) -> MessagePreview {
    MessagePreview {
        author: Some("Аня".into()),
        text: text.into(),
        timestamp,
    }
}

/// A chat with unread messages and a last message, pinning `ts` (when given) with the pin clock
/// `clock`; the pin has no preview.
fn pinned_chat(chat_id: &str, ts: Option<MessageTs>, clock: Option<i64>) -> ChatSummary {
    let mut summary = chat(chat_id, Some(5), None);
    summary.unread = 3;
    summary.pinned_message = ts.map(|timestamp| PinnedMessage {
        timestamp,
        preview: None,
    });
    summary.pin_action_ts = clock;
    summary
}

fn pin_event(chat_id: &str, pinned: Option<PinnedMessage>) -> ClientEvent {
    ClientEvent::PinChanged {
        chat: id(chat_id),
        pinned,
    }
}

fn is_pin_event(event: &ClientEvent) -> bool {
    matches!(event, ClientEvent::PinChanged { .. })
}

/// Waits for a `PinChanged` whose pin carries a preview.
fn resolved(h: &Harness) -> Option<PinnedMessage> {
    match h.next(
        "a pin with a preview",
        |e| matches!(e, ClientEvent::PinChanged { pinned: Some(p), .. } if p.preview.is_some()),
    ) {
        ClientEvent::PinChanged { pinned, .. } => pinned,
        _ => unreachable!(),
    }
}

/// Everything the engine emitted up to now: a marker event is pushed behind whatever is queued
/// and read back, which also proves that earlier pushes were handled.
fn settle(h: &Harness) -> Vec<ClientEvent> {
    h.fake.push(LiveEvent::Typing {
        chat: id("barrier"),
        user: UserId("barrier".into()),
        name: None,
        timeout: Duration::from_secs(60),
    });
    let mut seen = h.until(
        "the barrier",
        |e| matches!(e, ClientEvent::Typing { user, .. } if user.0 == "barrier"),
    );
    seen.pop();
    seen
}

fn cached(h: &Harness, chat_id: &str) -> ChatSummary {
    h.cache().chat(&id(chat_id)).unwrap().unwrap()
}

fn push_pin(h: &Harness, chat_id: &str, pinned: Option<MessageTs>, at: Option<i64>) {
    h.fake.push(LiveEvent::PinChanged {
        chat: id(chat_id),
        pinned,
        at,
    });
}

/// A chat-list refresh as a reconnect brings it; returns when the list was applied.
fn refresh(h: &Harness, chats: Vec<ChatSummary>) {
    h.fake.state().chats = chats;
    h.fake.push(LiveEvent::Reconnected);
    h.next("the refreshed list", |e| {
        matches!(e, ClientEvent::ChatsReplaced(_))
    });
}

fn online_with(chats: Vec<ChatSummary>) -> Harness {
    let h = start(move |s, _| s.chats = chats, 50);
    h.online();
    settle(&h);
    h
}

// -- pushes (rule 1) ---------------------------------------------------------------------------

#[test]
fn a_pin_push_in_a_cached_chat_emits_only_pin_changed_and_leaves_the_row_alone() {
    let h = online_with(vec![pinned_chat("a", None, None)]);
    let before = cached(&h, "a");
    push_pin(&h, "a", Some(7), Some(100));
    assert_eq!(settle(&h), [pin_event("a", pinned(7, None))]);
    let after = cached(&h, "a");
    assert_eq!(after.pinned_message, pinned(7, None));
    assert_eq!(after.pin_action_ts, Some(100));
    // The row (preview, unread, order) is exactly what it was.
    let unpinned = ChatSummary {
        pinned_message: None,
        pin_action_ts: None,
        ..after
    };
    assert_eq!(unpinned, before);
    assert!(h.fake.state().preview_calls.is_empty());
}

#[test]
fn a_pin_push_for_an_unknown_chat_is_ignored() {
    let h = online_with(vec![pinned_chat("a", None, None)]);
    push_pin(&h, "nope", Some(7), Some(100));
    assert!(settle(&h).is_empty());
    assert_eq!(h.cache().chat(&id("nope")).unwrap(), None);
    assert_eq!(cached(&h, "a").pinned_message, None);
}

#[test]
fn an_unpin_push_removes_the_pin_and_keeps_the_clock() {
    let h = online_with(vec![pinned_chat("a", Some(7), Some(10))]);
    push_pin(&h, "a", None, Some(20));
    assert_eq!(settle(&h), [pin_event("a", None)]);
    let after = cached(&h, "a");
    assert_eq!(after.pinned_message, None);
    assert_eq!(after.pin_action_ts, Some(20));
}

#[test]
fn a_replaced_pin_keeps_no_stale_preview() {
    let h = start(
        |s, _| {
            s.chats = vec![pinned_chat("a", Some(7), Some(10))];
            s.preview_results
                .insert((id("a"), 7), Ok(Some(text_preview(7, "seven"))));
        },
        50,
    );
    h.online();
    h.client.send(Command::OpenChat(id("a")));
    assert_eq!(resolved(&h), pinned(7, Some(text_preview(7, "seven"))));
    push_pin(&h, "a", Some(8), Some(20));
    // The server no longer has message 8: nothing to show, and certainly not message 7's text.
    let event = h.next("the replacement", is_pin_event);
    assert_eq!(event, pin_event("a", pinned(8, None)));
    h.wait_for("the read of 8", |s| s.preview_calls.len() == 2);
    h.quiet();
    assert!(
        settle(&h).iter().all(|e| !is_pin_event(e)),
        "no preview was expected for message 8"
    );
    assert_eq!(cached(&h, "a").pinned_message, pinned(8, None));
    assert_eq!(h.fake.state().preview_calls, [(id("a"), 7), (id("a"), 8)]);
}

// -- chat-list refresh (rule 2) ----------------------------------------------------------------

#[test]
fn the_same_pinned_timestamp_keeps_the_preview_over_a_list_refresh() {
    let kept = text_preview(7, "kept");
    let seeded = {
        let mut summary = pinned_chat("a", Some(7), Some(10));
        summary.pinned_message = pinned(7, Some(kept.clone()));
        summary
    };
    let h = start(
        |s, dir| {
            seed_cache(dir, &[seeded], "a", &[], false);
            s.chats = vec![pinned_chat("a", Some(7), Some(10))];
        },
        50,
    );
    h.online();
    assert_eq!(
        cached(&h, "a").pinned_message,
        pinned(7, Some(kept.clone()))
    );
    // A later refresh (same pin, no clock) keeps it as well.
    refresh(&h, vec![pinned_chat("a", Some(7), None)]);
    let after = cached(&h, "a");
    assert_eq!(after.pinned_message, pinned(7, Some(kept)));
    assert_eq!(after.pin_action_ts, Some(10));
}

#[test]
fn a_list_refresh_with_a_different_pin_drops_the_old_preview() {
    let seeded = {
        let mut summary = pinned_chat("a", Some(7), None);
        summary.pinned_message = pinned(7, Some(text_preview(7, "seven")));
        summary
    };
    let h = start(
        |s, dir| {
            seed_cache(dir, &[seeded], "a", &[], false);
            s.chats = vec![pinned_chat("a", Some(8), None)];
        },
        50,
    );
    h.online();
    assert_eq!(cached(&h, "a").pinned_message, pinned(8, None));
}

#[test]
fn a_list_refresh_without_the_pin_removes_it() {
    let h = online_with(vec![pinned_chat("a", Some(7), None)]);
    assert_eq!(cached(&h, "a").pinned_message, pinned(7, None));
    refresh(&h, vec![pinned_chat("a", None, None)]);
    assert_eq!(cached(&h, "a").pinned_message, None);
}

// -- the preview (rule 3) ----------------------------------------------------------------------

#[test]
fn the_preview_of_the_open_chat_comes_from_the_cached_message_without_a_request() {
    let h = start(
        |s, dir| {
            let mut own = own_copy("a", 7, "mine");
            own.author_name = Some("ignored for own messages".into());
            seed_cache(
                dir,
                &[pinned_chat("a", Some(7), None)],
                "a",
                &[own.clone()],
                true,
            );
            s.chats = vec![pinned_chat("a", Some(7), None)];
            s.history.insert(id("a"), vec![own]);
        },
        50,
    );
    h.online();
    h.client.send(Command::OpenChat(id("a")));
    let own_preview = MessagePreview {
        author: Some("Вы".into()),
        text: "mine".into(),
        timestamp: 7,
    };
    assert_eq!(resolved(&h), pinned(7, Some(own_preview)));
    h.wait_for("the history request", |s| !s.history_calls.is_empty());
    settle(&h);
    assert!(h.fake.state().preview_calls.is_empty());
}

#[test]
fn the_preview_is_read_with_message_preview_and_never_stored_as_history() {
    let h = start(
        |s, _| {
            s.chats = vec![pinned_chat("a", Some(7), None)];
            s.preview_results
                .insert((id("a"), 7), Ok(Some(text_preview(7, "from the server"))));
        },
        50,
    );
    h.online();
    h.client.send(Command::OpenChat(id("a")));
    assert_eq!(
        resolved(&h),
        pinned(7, Some(text_preview(7, "from the server")))
    );
    settle(&h);
    assert_eq!(h.fake.state().preview_calls, [(id("a"), 7)]);
    // The preview lives in the chat's JSON only: no message row, no history position.
    let cache = h.cache();
    assert_eq!(cache.message(&id("a"), 7).unwrap(), None);
    assert_eq!(cache.newest_ts(&id("a")).unwrap(), None);
    assert_eq!(
        cached(&h, "a").pinned_message,
        pinned(7, Some(text_preview(7, "from the server")))
    );
}

#[test]
fn only_the_open_chat_resolves_its_preview() {
    let h = online_with(vec![
        pinned_chat("a", Some(7), None),
        pinned_chat("b", None, None),
    ]);
    push_pin(&h, "a", Some(8), Some(20));
    assert_eq!(settle(&h), [pin_event("a", pinned(8, None))]);
    assert!(h.fake.state().preview_calls.is_empty());
}

#[test]
fn offline_the_preview_stays_empty_without_a_request_or_an_error() {
    let h = start(
        |s, dir| {
            seed_cache(dir, &[pinned_chat("a", Some(7), None)], "a", &[], false);
            s.bootstrap_errors =
                std::iter::repeat_n(BackendError::Network("down".into()), 1000).collect();
        },
        50,
    );
    h.next("offline", |e| {
        *e == ClientEvent::Connection(ConnectionState::Offline)
    });
    h.client.send(Command::OpenChat(id("a")));
    h.history("the cached history");
    h.quiet();
    assert!(h.drain().iter().all(|e| !is_pin_event(e)));
    assert!(h.fake.state().preview_calls.is_empty());
    assert_eq!(cached(&h, "a").pinned_message, pinned(7, None));
}

#[test]
fn offline_the_cached_message_still_gives_the_preview() {
    let h = start(
        |s, dir| {
            seed_cache(
                dir,
                &[pinned_chat("a", Some(7), None)],
                "a",
                &[msg("a", 7, "cached")],
                true,
            );
            s.bootstrap_errors =
                std::iter::repeat_n(BackendError::Network("down".into()), 1000).collect();
        },
        50,
    );
    h.next("offline", |e| {
        *e == ClientEvent::Connection(ConnectionState::Offline)
    });
    h.client.send(Command::OpenChat(id("a")));
    let preview = MessagePreview {
        author: None,
        text: "cached".into(),
        timestamp: 7,
    };
    assert_eq!(resolved(&h), pinned(7, Some(preview)));
    assert!(h.fake.state().preview_calls.is_empty());
}

#[test]
fn a_failed_read_leaves_the_preview_empty_and_is_retried_on_the_next_opening() {
    let h = start(
        |s, _| {
            s.chats = vec![pinned_chat("a", Some(7), None)];
            s.preview_results
                .insert((id("a"), 7), Err(BackendError::Network("down".into())));
        },
        50,
    );
    h.online();
    h.client.send(Command::OpenChat(id("a")));
    h.wait_for("the first read", |s| s.preview_calls.len() == 1);
    let events = settle(&h);
    // The failed answer is handled before the next opening asks again.
    h.quiet();
    // No error event of any kind: the only thing the opening produced is its history.
    assert!(
        events.iter().all(|e| matches!(e, ClientEvent::History(_))),
        "{events:?}"
    );
    assert_eq!(cached(&h, "a").pinned_message, pinned(7, None));

    h.fake
        .state()
        .preview_results
        .insert((id("a"), 7), Ok(Some(text_preview(7, "now"))));
    h.client.send(Command::OpenChat(id("a")));
    assert_eq!(resolved(&h), pinned(7, Some(text_preview(7, "now"))));
    assert_eq!(h.fake.state().preview_calls.len(), 2);
}

#[test]
fn a_result_for_a_pin_that_changed_meanwhile_is_dropped() {
    let gate = Arc::new(tokio::sync::Notify::new());
    let h = start(
        |s, _| {
            s.chats = vec![pinned_chat("a", Some(7), Some(10))];
            s.preview_gate = Some((7, gate.clone()));
            s.preview_results
                .insert((id("a"), 7), Ok(Some(text_preview(7, "old pin"))));
            s.preview_results
                .insert((id("a"), 8), Ok(Some(text_preview(8, "new pin"))));
        },
        50,
    );
    h.online();
    h.client.send(Command::OpenChat(id("a")));
    h.wait_for("the read of 7", |s| s.preview_calls.len() == 1);
    push_pin(&h, "a", Some(8), Some(20));
    assert_eq!(resolved(&h), pinned(8, Some(text_preview(8, "new pin"))));
    // Now the answer for the replaced pin arrives.
    gate.notify_one();
    h.quiet();
    let events = settle(&h);
    assert!(events.iter().all(|e| !is_pin_event(e)), "{events:?}");
    assert_eq!(
        cached(&h, "a").pinned_message,
        pinned(8, Some(text_preview(8, "new pin")))
    );
}

#[test]
fn at_most_one_read_is_in_flight_per_pinned_message() {
    let gate = Arc::new(tokio::sync::Notify::new());
    let h = start(
        |s, _| {
            s.chats = vec![pinned_chat("a", Some(7), Some(10))];
            s.preview_gate = Some((7, gate.clone()));
            s.preview_results
                .insert((id("a"), 7), Ok(Some(text_preview(7, "seven"))));
        },
        50,
    );
    h.online();
    h.client.send(Command::OpenChat(id("a")));
    h.wait_for("the read", |s| s.preview_calls.len() == 1);
    // Everything that asks for the preview again while the read is out.
    h.client.send(Command::OpenChat(id("a")));
    push_pin(&h, "a", Some(7), Some(10));
    h.fake
        .push(LiveEvent::ChatChanged(pinned_chat("a", Some(7), Some(10))));
    settle(&h);
    assert_eq!(h.fake.state().preview_calls.len(), 1);
    gate.notify_one();
    assert_eq!(resolved(&h), pinned(7, Some(text_preview(7, "seven"))));
    // Resolved: nothing asks again.
    h.client.send(Command::OpenChat(id("a")));
    settle(&h);
    assert_eq!(h.fake.state().preview_calls.len(), 1);
}

#[test]
fn a_reconnect_reads_the_open_chats_preview_again() {
    let seeded = {
        let mut summary = pinned_chat("a", Some(7), Some(10));
        summary.pinned_message = pinned(7, Some(text_preview(7, "before the edit")));
        summary
    };
    let h = start(
        |s, dir| {
            seed_cache(dir, &[seeded], "a", &[], false);
            s.chats = vec![pinned_chat("a", Some(7), Some(10))];
            s.preview_results
                .insert((id("a"), 7), Ok(Some(text_preview(7, "after the edit"))));
        },
        50,
    );
    h.online();
    h.client.send(Command::OpenChat(id("a")));
    settle(&h);
    // The cached preview is shown as it is: no request on opening.
    assert!(h.fake.state().preview_calls.is_empty());
    h.fake.push(LiveEvent::Reconnected);
    assert_eq!(
        resolved(&h),
        pinned(7, Some(text_preview(7, "after the edit")))
    );
    assert_eq!(h.fake.state().preview_calls, [(id("a"), 7)]);
}

// -- follow-through (rule 4) and storage (rule 5) ----------------------------------------------

#[test]
fn the_pinned_message_edited_then_deleted_updates_its_preview() {
    let h = start(
        |s, dir| {
            seed_cache(
                dir,
                &[pinned_chat("a", Some(7), None)],
                "a",
                &[msg("a", 7, "old")],
                true,
            );
            s.chats = vec![pinned_chat("a", Some(7), None)];
            s.history.insert(id("a"), vec![msg("a", 7, "old")]);
        },
        50,
    );
    h.online();
    h.client.send(Command::OpenChat(id("a")));
    let preview = |text: &str| MessagePreview {
        author: None,
        text: text.into(),
        timestamp: 7,
    };
    assert_eq!(resolved(&h), pinned(7, Some(preview("old"))));
    h.wait_for("the history request", |s| !s.history_calls.is_empty());
    settle(&h);

    let mut edited = msg("a", 7, "new");
    edited.edited = true;
    h.fake.push(LiveEvent::MessageUpserted(edited));
    assert_eq!(resolved(&h), pinned(7, Some(preview("new"))));
    assert_eq!(
        cached(&h, "a").pinned_message,
        pinned(7, Some(preview("new")))
    );

    h.fake.push(LiveEvent::MessageDeleted {
        chat: id("a"),
        timestamp: 7,
    });
    assert_eq!(resolved(&h), pinned(7, Some(preview(DELETED_PREVIEW))));
    assert!(h.fake.state().preview_calls.is_empty());
}

#[test]
fn an_edit_of_the_pinned_message_in_a_chat_that_is_not_open_follows_too() {
    let h = online_with(vec![
        pinned_chat("b", Some(7), None),
        pinned_chat("a", None, None),
    ]);
    h.fake
        .push(LiveEvent::MessageUpserted(own_copy("b", 7, "mine")));
    let events = settle(&h);
    let own = MessagePreview {
        author: Some("Вы".into()),
        text: "mine".into(),
        timestamp: 7,
    };
    assert!(
        events.contains(&pin_event("b", pinned(7, Some(own)))),
        "{events:?}"
    );
    // Another message of the chat does not touch the pin.
    h.fake
        .push(LiveEvent::MessageUpserted(msg("b", 9, "other")));
    assert!(settle(&h).iter().all(|e| !is_pin_event(e)));
    assert!(h.fake.state().preview_calls.is_empty());
}

#[test]
fn a_history_chunk_with_a_different_copy_of_the_pinned_message_updates_the_preview() {
    let seeded = {
        let mut summary = pinned_chat("a", Some(7), Some(10));
        summary.pinned_message = pinned(7, Some(text_preview(7, "old")));
        summary
    };
    let h = start(
        |s, dir| {
            seed_cache(dir, &[seeded], "a", &[], false);
            s.chats = vec![pinned_chat("a", Some(7), Some(10))];
            s.history
                .insert(id("a"), vec![msg("a", 7, "edited while away")]);
        },
        50,
    );
    h.online();
    h.client.send(Command::OpenChat(id("a")));
    let preview = MessagePreview {
        author: None,
        text: "edited while away".into(),
        timestamp: 7,
    };
    assert_eq!(resolved(&h), pinned(7, Some(preview)));
    assert!(h.fake.state().preview_calls.is_empty());
}

// -- arbitration (Q5) --------------------------------------------------------------------------

#[test]
fn a_push_older_than_the_pin_clock_is_ignored_without_an_event() {
    let h = online_with(vec![pinned_chat("a", Some(7), Some(100))]);
    let before = cached(&h, "a");
    push_pin(&h, "a", None, Some(50));
    push_pin(&h, "a", Some(9), Some(99));
    assert!(settle(&h).is_empty());
    assert_eq!(cached(&h, "a"), before);
}

#[test]
fn a_push_without_a_timestamp_wins_by_arrival_and_keeps_the_clock() {
    let h = online_with(vec![pinned_chat("a", Some(7), Some(100))]);
    push_pin(&h, "a", Some(8), None);
    assert_eq!(settle(&h), [pin_event("a", pinned(8, None))]);
    let after = cached(&h, "a");
    assert_eq!(
        (after.pinned_message, after.pin_action_ts),
        (pinned(8, None), Some(100))
    );
    push_pin(&h, "a", None, None);
    assert_eq!(settle(&h), [pin_event("a", None)]);
    let after = cached(&h, "a");
    assert_eq!(
        (after.pinned_message, after.pin_action_ts),
        (None, Some(100))
    );
}

#[test]
fn the_pin_clock_never_decreases_and_survives_an_unpin() {
    let h = online_with(vec![pinned_chat("a", Some(7), Some(100))]);
    push_pin(&h, "a", None, Some(200));
    assert_eq!(settle(&h), [pin_event("a", None)]);
    // Older than the unpin: ignored, whatever it says.
    push_pin(&h, "a", Some(9), Some(150));
    assert!(settle(&h).is_empty());
    let after = cached(&h, "a");
    assert_eq!(
        (after.pinned_message, after.pin_action_ts),
        (None, Some(200))
    );
    // The same clock is not older: it applies.
    push_pin(&h, "a", Some(9), Some(200));
    assert_eq!(settle(&h), [pin_event("a", pinned(9, None))]);
    assert_eq!(cached(&h, "a").pin_action_ts, Some(200));
}

#[test]
fn a_stale_list_entry_with_the_old_pin_does_not_bring_back_an_unpinned_message() {
    let h = online_with(vec![pinned_chat("a", Some(7), Some(100))]);
    push_pin(&h, "a", None, Some(200));
    settle(&h);
    refresh(&h, vec![pinned_chat("a", Some(7), Some(100))]);
    let after = cached(&h, "a");
    assert_eq!(
        (after.pinned_message, after.pin_action_ts),
        (None, Some(200))
    );
}

#[test]
fn a_stale_list_entry_does_not_undo_a_replace() {
    let h = online_with(vec![pinned_chat("a", Some(7), Some(100))]);
    push_pin(&h, "a", Some(8), Some(200));
    settle(&h);
    refresh(&h, vec![pinned_chat("a", Some(7), Some(100))]);
    let after = cached(&h, "a");
    assert_eq!(
        (after.pinned_message, after.pin_action_ts),
        (pinned(8, None), Some(200))
    );
}

#[test]
fn a_stale_unpinned_entry_does_not_remove_a_pin_that_a_push_set() {
    let h = online_with(vec![pinned_chat("a", None, Some(100))]);
    push_pin(&h, "a", Some(8), Some(200));
    settle(&h);
    refresh(&h, vec![pinned_chat("a", None, Some(100))]);
    let after = cached(&h, "a");
    assert_eq!(
        (after.pinned_message, after.pin_action_ts),
        (pinned(8, None), Some(200))
    );
}

#[test]
fn a_list_entry_with_an_equal_or_newer_clock_applies() {
    let h = online_with(vec![pinned_chat("a", None, Some(100))]);
    push_pin(&h, "a", Some(8), Some(200));
    settle(&h);
    // Equal clock: the entry is the server's state at least as new as the push.
    refresh(&h, vec![pinned_chat("a", None, Some(200))]);
    let after = cached(&h, "a");
    assert_eq!(
        (after.pinned_message, after.pin_action_ts),
        (None, Some(200))
    );
    // Newer clock with another pin.
    refresh(&h, vec![pinned_chat("a", Some(9), Some(300))]);
    let after = cached(&h, "a");
    assert_eq!(
        (after.pinned_message, after.pin_action_ts),
        (pinned(9, None), Some(300))
    );
}

#[test]
fn a_list_entry_without_a_clock_applies_by_arrival_and_keeps_the_cached_clock() {
    let h = online_with(vec![pinned_chat("a", Some(7), Some(100))]);
    push_pin(&h, "a", Some(8), Some(200));
    settle(&h);
    refresh(&h, vec![pinned_chat("a", Some(9), None)]);
    let after = cached(&h, "a");
    assert_eq!(
        (after.pinned_message, after.pin_action_ts),
        (pinned(9, None), Some(200))
    );
}

#[test]
fn the_rest_of_a_stale_summary_still_applies() {
    let h = online_with(vec![pinned_chat("a", Some(7), Some(100))]);
    push_pin(&h, "a", None, Some(200));
    settle(&h);
    let mut stale = pinned_chat("a", Some(7), Some(100));
    stale.unread = 9;
    stale.title = "renamed".into();
    stale.muted = true;
    stale.last_message = Some(MessagePreview {
        author: None,
        text: "newer".into(),
        timestamp: 6,
    });
    refresh(&h, vec![stale.clone()]);
    let after = cached(&h, "a");
    assert_eq!(after.pinned_message, None);
    assert_eq!(after.pin_action_ts, Some(200));
    assert_eq!(
        ChatSummary {
            pinned_message: None,
            pin_action_ts: None,
            ..after
        },
        ChatSummary {
            pinned_message: None,
            pin_action_ts: None,
            ..stale
        }
    );
}

#[test]
fn a_pushed_chat_summary_goes_through_the_same_arbitration() {
    let h = online_with(vec![pinned_chat("a", Some(7), Some(100))]);
    push_pin(&h, "a", None, Some(200));
    settle(&h);
    let mut stale = pinned_chat("a", Some(7), Some(100));
    stale.unread = 9;
    h.fake.push(LiveEvent::ChatChanged(stale));
    let events = settle(&h);
    let [ClientEvent::ChatUpdated(row)] = events.as_slice() else {
        panic!("expected one ChatUpdated, got {events:?}");
    };
    assert_eq!((row.unread, &row.pinned_message), (9, &None));
    assert_eq!(cached(&h, "a").pin_action_ts, Some(200));
}
