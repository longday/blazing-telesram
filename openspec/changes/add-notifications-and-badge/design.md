# Design

## Context

`proposal.md` states the motivation, `research.md` the unknowns and the facts. Code this change extends (read on 2026-11-28):

- `yandex-proto/src/wire.rs`: `WireChat { unread, muted, last_message, .. }`; `unread` = `LastSeqNo - LastSeenByMeSeqNo` clamped at 0, `muted` = the chat entry's `Muted` bool, absent = false (`parse_chat`). `PushEvent::{MessageUpserted, ChatChanged, ..}`.
- `desktop/src/yandex_backend.rs`: `subscribe` forwards `PushEvent::MessageUpserted` as `LiveEvent::MessageUpserted` and answers `PushEvent::ChatChanged` by re-reading the chat list and forwarding that chat's entry as `LiveEvent::ChatChanged(summary)`; `summary()` maps `muted`.
- `chat-core/src/client.rs`: `handle_live` emits `ClientEvent::MessageUpserted` and then `update_preview`, which for a message that is not outgoing, not older than the row's last message and not the same as it **adds one to `unread`**, replaces `last_message` and emits `ClientEvent::ChatUpdated`; it returns without an event when the chat is not in the cache. `LiveEvent::ChatChanged` emits `ChatUpdated` with the server's authoritative row. A reconnect reloads the whole list as `ChatsReplaced`; the start-up cache and the first list are `ChatsReplaced` too. Marking read emits `ChatUpdated` with `unread` 0.
- `desktop/src/store.rs`: `ChatStore.chats: HashMap<ChatId, ChatSummary>`, `apply` (`ChatsReplaced` and `ChatUpdated` both emit `StoreEvent::Chats`), `chat(&id)`, `selected()`, `select`, `forget_account` (sign-out and account switch). `StoreEvent` is the app's one change channel; `player.rs` subscribes to it directly.
- `desktop/src/desktop.rs` + `desktop/composer.rs`: `observe_window_activation`; `check_read` marks the open chat read only when `window.is_window_active()` and nothing covers the chat; the test harness has `cx.deactivate_window()` and `activate(cx)`.
- `desktop/src/main.rs`: `open_chat_window` creates the store and the window, subscribes to the store for sign-in return and reveal; `cx.on_window_closed` quits when the last window closes; globals such as `player::AudioFactory` are registered before the window (`audio.rs`, the pattern this change copies).
- `desktop/Cargo.toml`: `objc2 =0.6.5`, `objc2-foundation =0.3.2`, `block2 =0.6.2`, `objc2-app-kit =0.3.2` with `std, NSResponder, NSScreen, NSWindow, objc2-core-foundation`, `objc2-avf-audio =0.3.2`; `Cargo.lock` has no `objc2-user-notifications`. `objc2-app-kit 0.3.2` has `NSDockTile::{badgeLabel, setBadgeLabel}` and `NSApplication::dockTile` behind the feature `NSDockTile`.

## Goals / Non-Goals

**Goals:** the Dock badge and the notification of the proposal, each testable without the platform; the platform code in two small files that nothing else touches; the policy (what notifies, what does not) in one pure place with a table of tests; the contracts below fixed so store, policy, platform, wiring and tests can be written in parallel.

**Non-Goals:** the proposal's non-goals.

## Decisions

### D1. Shape of the change

Three seams, each testable alone: **facts** (the store: the badge total and the "incoming" signal, both pure functions over `ChatSummary`), **policy** (`alerts.rs`: given the facts and the window state, what to show; an entity that owns the state "what is delivered" and "what the badge shows"), **platform** (`dock.rs`, `notifier.rs`: traits implemented over `NSDockTile` and `UNUserNotificationCenter`; the only files with `unsafe` and objc2 code). The policy talks to the platform through two small traits and a global factory, exactly as `Player` talks to `AudioOutput` through `AudioFactory` (`add-media-playback` D5): the real factory is registered in `main.rs`, the test harness registers a scripted fake, no factory means no alerts.

### D2. The badge

- **Count** = the sum over `ChatStore.chats` of `unread` for every chat with `muted == false`. It is what the chat list shows, so it needs no request and cannot drift from the list. Messages, not chats: a chat with 120 unread messages weighs 120, as the row's counter says.
- **Muted chats are not counted** (veto-able, proposal question 1): a user who muted a chat asked not to be disturbed by it, and the muted counter in the row is already drawn in a quieter style (`chat-list`, "Muted chat"). The rule is one `filter(|c| !c.muted)` in one pure function, `badge_total(chats)`.
- **Label** (`badge_label(total) -> Option<String>`): `None` at 0 (the badge is removed), the decimal number up to 999, `"999+"` above.
- **When it is recomputed**: on `StoreEvent::Chats` (covers `ChatsReplaced` and `ChatUpdated`: a push, a read here, a read elsewhere, a reconnect), `StoreEvent::Session` and `StoreEvent::Account` (the store forgets the chats at sign-out and account switch). The result is compared with the last label handed to the platform and handed over only when it differs.
- **Cleared** on the events above (an empty store gives 0) and when the `Alerts` entity is dropped (`Drop` hands `None` to the platform): the window closing on sign-out or expiry drops its `Desktop` and with it the entity, so the badge never outlives the window. A quit clears the Dock badge by itself; task 7.2 checks that on the real system.
- **No authorization needed** is [INFERENCE]: `NSDockTile.badgeLabel` is an app-owned label that does not go through the notification centre. R1 measures whether the label is shown when notifications are refused or undetermined; if it is not, the badge is documented as following the system's "Badge app icon" switch and nothing else changes.

### D3. Platform API and identity, chosen by a rule from R1

```rust
// desktop/src/alerts.rs (contracts; the real ones are in dock.rs and notifier.rs)
pub trait DockBadge { fn set(&mut self, label: Option<&str>); }

pub struct Notification { pub chat: ChatId, pub id: String /* chat:timestamp */, pub title: String, pub body: String }
pub enum Permission { Undetermined, Granted, Denied }
pub trait Notifier {
    fn request_permission(&mut self);                 // once; the answer arrives through `permission()`
    fn permission(&self) -> Permission;
    fn show(&mut self, notification: Notification);
    fn withdraw(&mut self, chat: &ChatId);            // every delivered notification of this chat
    fn withdraw_all(&mut self);
    fn clicks(&self) -> async_channel::Receiver<ChatId>;
}
pub struct AlertsFactory(pub fn() -> Platform);       // a gpui Global
pub struct Platform { pub badge: Box<dyn DockBadge>, pub notifier: Option<Box<dyn Notifier>> }
```

`notifier: None` is the honest "this process cannot show notifications" (below); the badge never depends on it. Unit NB-B ships `Platform` with the `badge` field only; unit NB-N adds `notifier` (task 2.2).

**Candidates.**

| | API | New code and deps | Click comes back to the process | Needs |
|---|---|---|---|---|
| **N-A** | `UNUserNotificationCenter` through `objc2-user-notifications =0.3.2` (`default-features = false`, features `std`, `block2`, `bitflags` and the `UN*` modules actually used; R1 lists them) | **1 new crate** (Zlib OR Apache-2.0 OR MIT; the objc2 family is in the tree; `bitflags 2.13.2` is in the lock) | yes: the delegate's `didReceiveNotificationResponse`, called on a non-main thread, so the chat id goes through an `async_channel` the foreground executor drains | a bundle identifier; **[INFERENCE, R1 measures it]** `+currentNotificationCenter` raises an exception in a process without one, i.e. a bare `cargo run` binary |
| N-B | `NSUserNotificationCenter` (deprecated since macOS 11) through `objc2-foundation`'s `NSUserNotification` | no new crate | yes (activation delegate) | possibly a bundle identifier too; deprecated, may vanish; kept only as the reserve if N-A fails in a bundle |
| N-C | `osascript -e 'display notification'` | none | **no** | nothing, but it shows under "Script Editor", has no click, needs the text escaped into AppleScript (injection) and spawns a process per message: rejected, it cannot meet the click requirement |

**Rule.** Pick the smallest candidate that, in the VM, (1) delivers a notification with a title and a body under the app's own name, (2) reports the click with the chat identifier back to the process, and (3) does not change how the app is launched. Outcomes of R1 for N-A:

1. **Works from the bare binary** (the identity is not required, or `bundleIdentifier` is non-nil by itself): N-A, no packaging, `notifier` is always `Some`.
2. **Needs a bundle, and works from a minimal ad-hoc-signed `.app`** (`Info.plist` with `CFBundleIdentifier` and `CFBundleExecutable`, the binary copied in): N-A, and task 5.2 ships the wrapper (a script that builds the bundle under `target/` and a `run.sh` that launches it), so that the daily way of running the app has notifications. `notifier::create()` first reads `NSBundle.mainBundle.bundleIdentifier()` and returns `None` when it is nil, **before** any `UNUserNotificationCenter` call, so a bare `cargo run` binary keeps the badge, does not crash, and logs one `info` line ("notifications unavailable: not running from an application bundle"; no content).
3. **Works in neither** (signing, entitlement, or the VM refuses): N-B is measured the same way; if it fails too, the research ends with "none", `notifier` stays `None` everywhere, the change ships the badge only (unit NB-B), and the main session decides about the rest. No code path for N-A is merged in that case.

R1 also records, for the chosen candidate: the permission prompt and what `getNotificationSettings` reports before and after; whether a notification is shown while the app is frontmost (a foreground app needs the delegate's `willPresentNotification` to ask for a banner, so the real notifier asks for banner and sound there and the policy, not the system, decides what is shown); that the delegate must be installed before the first request and is held only weakly by the centre (the notifier keeps the delegate object alive); what `removeDeliveredNotifications` does.

**Threading.** `NSDockTile` and `NSApplication` are main-thread: `dock.rs` takes a `MainThreadMarker` (`MainThreadMarker::new()` succeeds in gpui callbacks) and `DockBadge::set` is only called from entity updates on the main thread. The notification centre's completion handlers and delegate callbacks come from system threads: they hold nothing but an `async_channel::Sender` (which is `Send`) and a small atomic for the permission.

### D4. What an incoming message is: derived from the chat list's own update

The store, which keeps the previous row, decides in the `ChatUpdated` arm, before it replaces the row:

```rust
// desktop/src/store.rs
fn is_incoming(old: Option<&ChatSummary>, new: &ChatSummary) -> bool {
    let newest = new.last_message.as_ref().map(|m| m.timestamp);
    let was = old.and_then(|c| c.last_message.as_ref()).map(|m| m.timestamp);
    new.unread > old.map_or(0, |c| c.unread) && newest.is_some() && newest > was
}
// emits StoreEvent::Incoming(chat) (after StoreEvent::Chats) when it holds
```

Why this and not a new `ClientEvent::Incoming` out of `chat-core`:
- `update_preview` already raises `unread` exactly when a message from someone else is newer than the row (the rule is `chat-core`'s, so it is not copied here), and `LiveEvent::ChatChanged` raises it from the server's side; both end in `ChatUpdated`. One signal, both paths.
- It covers **a chat that was not in the list**: `update_preview` returns early for an unknown chat, but the `ChatChanged` push that follows arrives as a `ChatUpdated` with no old row (`old = None`, `unread > 0`), if the server sends one (R3 b).
- It leaves `chat-core` and `yandex-proto` untouched, so the change owns only `desktop` files.
- `ChatsReplaced` (start-up, reconnect reload, account switch) never emits it: the backlog is the badge's job (spec "What never notifies").
- Own messages do not raise `unread` (`update_preview` skips `outgoing`; the server counts only what is unseen by the account). Edits, deletions and reactions keep the same last timestamp. A read lowers `unread`.

Two updates can describe the same message (the message push bumps the counter, then the refreshed row says the same count and the same timestamp, or the reverse order): neither the same timestamp nor an equal counter satisfies `is_incoming` the second time, and `Alerts` also remembers the newest timestamp it notified per chat and drops anything not newer. The text is the row's `last_message` (`MessagePreview { author, text, timestamp }`): the preview of the newest message, which is what D5 wants.

Rejected: a `ClientEvent::Incoming(Message)` from `handle_live` (precise, carries the full `Message`, but touches the 10 000-line `client.rs` and `model.rs`, misses a chat the cache does not know, and has to be kept in step with the unread rule); deriving from `ClientEvent::MessageUpserted` in the store (it also fires for edits, own sends and the echo, and the store does not hold timelines of chats that were never opened).

### D5. The notification's content and privacy

- `Notification.title` = `ChatSummary.title`. `body` = `MessagePreview.text` (`MessageBody::preview_text`: the text, «Фото», «Голосовое сообщение», «Видео», a file name, `[kind]` for an unsupported body) with every run of whitespace, line breaks included, collapsed to one space, trimmed, cut to **140 grapheme clusters** (`unicode-segmentation` is already a dependency) with «…» appended when cut; in a group or channel (`ChatKind` other than private) prefixed with `"{author}: "` when `MessagePreview.author` is `Some`. An empty body after trimming reads «Новое сообщение».
- `id` = `"{chat id}:{timestamp}"`; the platform's thread identifier is the chat id, so macOS groups a chat's notifications; the user info carries the chat id and nothing else.
- **Sound**: the system's default notification sound (`UNNotificationSound.defaultSound`); no sound file of ours (Telegram Desktop sounds would trigger the licence rule of `openspec/config.yaml`). The user can turn it off in System Settings > Notifications. A setting is a non-goal.
- **Privacy**: the text is the same text the chat row shows on screen, and a notification is how a messenger works; the system hides previews on a locked screen if the user says so. The chat title, author and text go only to the notification centre. `alerts.rs` and `notifier.rs` log at most `notification shown` / `notification skipped (muted | open chat | denied)` with no content at `debug`; the permission outcome at `info`. The user-info payload is the chat id only, and nothing is persisted by the app. Showing and removing notifications send no request.

### D6. Focus rules

`Alerts::incoming(chat, window_key: bool, open: Option<&ChatId>, ..)`, called by `Desktop` from its `subscribe_in` closure, which has the `Window` (`window.is_window_active()`; `Alerts` holds no window handle, so it never updates a window while one is being updated). The decision, in one pure function `decide(...) -> Skip | Show`, in this order:

1. the chat is muted: skip (`muted`);
2. `window_key && open == Some(chat)`: skip (`open chat`): the user is looking at it and `check_read` reads it;
3. the permission is `Denied` or the notifier is absent: skip (`denied`); `Undetermined` queues nothing and skips, since the prompt is asked at window open and its answer normally precedes any message;
4. the preview's timestamp is not newer than the newest already notified for the chat: skip (`duplicate`);
5. otherwise show.

No test of the scroll position: an open chat in the key window that the user has scrolled up in does not notify (the unread counter and the badge still tell). A dialog or the log view covering the chat does not change the rule. Both are accepted simplifications; a finer rule is a follow-up.

### D7. Click: open the chat

The notifier's delegate pushes the chat id from the user info into the `clicks()` channel. `Alerts` runs one foreground task that drains it and emits `AlertsEvent::Open(ChatId)`; `Desktop` subscribes: it calls `store.select(chat)` when `store.chat(&chat)` is `Some` (the existing selection path runs: timeline, composer, `check_read`), then `window.activate_window()` and `cx.activate(true)`; for a chat that is not in the list it only activates. This mirrors `PlayerEvent::OpenExternal`: platform call at the edge, an event the tests can assert (`Desktop` is asserted by the store's selection and the harness window state, not by a platform call). The click does not mark anything read: `check_read` does that when the window is key and the newest message is visible.

### D8. Retiring notifications

`Alerts` keeps `delivered: HashSet<ChatId>` (chats with a notification it showed) and the newest notified timestamp per chat. On every `StoreEvent::Chats`, for each chat in `delivered` whose row is gone or has `unread == 0`: `notifier.withdraw(chat)` and forget it. On `StoreEvent::Session` or `Account` (sign-out, expiry, switch): `withdraw_all()` and clear both maps. `Drop` also calls `withdraw_all()`.

### D9. Permission

`Alerts::new` calls `request_permission()` once when it is created (the chat window just opened). The notifier asks for the alert and sound authorizations, and for the badge authorization only if R1 shows that the Dock label needs it (D2); the answer is read back through `permission()`. A refusal is silent: the policy skips, the badge still works (D2, R1). There is no in-app "enable notifications" prompt (setting, out of scope).

### D10. What is deliberately not built, and why

- **Dock bounce, our own sounds, notification actions, a setting for any of it**: each is additive; none was asked for ("notifications for incoming messages + Dock badge").
- **A notification for the backlog**: after a night offline a burst of banners helps nobody; the badge carries the number.
- **`IsSilent` / `NotificationBehaviour` per message**: ignored today by the decoder (`PLAIN_META_KEYS`, `MODIFIERS`); R2 records what real values mean. Using them would add a field to `Message` and every `Message { .. }` literal in the tests; it is a follow-up, not a task here.
- **Withdrawing on window activation alone**: only a falling unread counter retires a notification, which is what "read" means everywhere else.

### D11. Tests strategy

- **Pure, in the module** (no window): `badge_total` (muted excluded, empty, overflow of `u32` sums), `badge_label` (0, 1, 999, 1000, `u32::MAX`), `is_incoming` (no old row, same count, count up with older/same/newer timestamp, count down, no last message), `notification_for` (private and group, author absent, whitespace collapse, 139/140/141 graphemes, an emoji cluster at the cut, empty text, a file name, a voice), `decide` (the table of D6: muted x key window x open chat x permission x duplicate), the user-info codec of the chat id (`notifier.rs`, the part without UN).
- **Store**: `ChatsReplaced` never emits `Incoming`; the push path (a `ChatUpdated` with unread +1 and a newer preview) emits it once, a second identical update does not; a `ChatUpdated` for an unknown chat emits it; a read does not; an edit (same timestamp) does not; sign-out empties the badge total.
- **Headless gpui with a scripted fake** (`tests.rs`, the harness registers the fake `AlertsFactory` like `AudioFactory`): the badge sequence over a scripted run (arrive, read, muted arrive, reconnect reload, sign-out); the notification matrix with `cx.deactivate_window()` / `activate`; a click opens the chat and activates the window; a click for a vanished chat changes nothing; withdraw on read and on sign-out; permission asked exactly once at window open; denied or absent notifier shows nothing and the badge still moves; no `Command` is sent by any of it (`h.commands()` empty except the `MarkRead` the read-markers rule already sends).
- **Platform code** (`dock.rs`, `notifier.rs`): no mock of AppKit or UserNotifications. What has no platform dependency is unit-tested (label strings, the user-info codec, `create()` returning `None` without a bundle identifier through a small seam). The rest is exercised by R1's scratch run and by task 7.2 in the VM.
- **Wire**: no fixture changes unless R2 finds a mute form the decoder does not read (then one more `wire.rs` task is added by 0.4, with a live-capture fixture as in `add-pins-and-importance-view`).
- Everything that cargo runs runs in `bt-vm` (rule 4).

### D12. Order and merge units (`DEPENDENCIES.md` rules 1-3)

- Research 0.x first (0.1 and 0.2 are independent; 0.3 is optional and non-gating; 0.4 closes). Only 5.1 and 5.2 (and the exact feature lists of 3.1) wait for R1; everything else is codable now against the contracts of D2-D9.
- **NB-B** (badge) = 1.1, 2.1, 3.1, 4.1, 6.1 on `integ/add-notifications-and-badge`: `badge_count` is first called by `Alerts`, `Alerts` is created by `Desktop` and fed a real `DockBadge` by `main.rs`; none of it can reach `main` alone without a `dead_code` warning or a stub (rule 3). It merges first and needs no R1 outcome.
- **NB-N** (notifications) = 1.2, 2.2, 4.2, 5.1, 5.2 (only for R1 outcome 2), 6.2, started after NB-B is on `main` (it extends `store.rs`, `alerts.rs`, `desktop.rs`, `main.rs`, `tests.rs`, `Cargo.toml`, `Cargo.lock`, which NB-B holds), and after 0.4.
- `Cargo.lock` gains one package, `objc2-user-notifications`, generated once without `--locked` (VM network works); the dependency and `notifier.rs` land in one commit (cargo's `unused_dependencies`, rule 3).

## Risks / Trade-offs

- **Bundle identity** is the dominant risk and is measured first (R1, D3). Residual: a bundle that works in the VM may behave differently on the user's Mac with notification settings the VM never had; task 7.2 is run in the VM and the first run on the user's Mac is reported as an unverified limit.
- **`Muted` is known from a placeholder fixture only** (R2). If the live key differs, the badge and the policy would count a muted chat; the fix is in `wire.rs` (a decode task added by 0.4), not here.
- **A mute changed in another client** reaches the cache only with the next list entry for that chat (a `ChatChanged` push or a reconnect reload), if the server pushes it at all (R3). Until then a chat muted elsewhere can still notify, and a chat unmuted elsewhere stays silent. Accepted; the stale window is the one `chat-list` already has for the muted marker.
- **`Incoming` is derived**, so a list row that raises `unread` for another reason (a server-side recount) would notify once with the newest preview. Accepted: the user has more unread messages than before.
- **Banner and prompt are visible only in the VM's GUI** (screenshots through `.runtime/vm/shot.sh`, which show chat text and are deleted after; booleans are recorded). Audible sound is not verifiable (as in `add-media-playback`).
- **Unofficial protocol**: `Muted`, `LastSeqNo` and the push order can change; research results are the record of what was observed.
