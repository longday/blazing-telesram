# Research: notifications and the Dock badge

Written 2026-11-28 before any code. Predecessor: `openspec/changes/archive/2026-11-23-add-read-only-client/` (`proposal.md` roadmap item 7 "macOS notifications, Dock badge, settings, themes"; `design.md` non-goals; the wire facts about `NotificationBehaviour` in its push-kinds list). This file is the single evidence document of this change: the **research tasks come first** (what is unknown, how it is measured, what each answer decides), then the facts already established, then a results log the tasks fill in.

Rules for every task here (`openspec/changes/DEPENDENCIES.md`): cargo, `bt-probe`, the app and every scratch program run **only in the Tart VM `bt-vm`**, each agent in its own `~/bt/src-<name>` (procedure: `docs/vm.md` section 3), removed afterwards; agents never start or stop the VM. Scratch crates live under `~/bt/src-<name>/scratch`, outside the repository tree, and are not committed. `[live]` tasks use the signed-in session in the VM's `~/bt/.runtime` and the real account and share the one live slot (rule 5; `BT_WRITE_ONLY_CHAT` stays set). **No task here writes to the account.** R1 needs no account at all.

## Research tasks

### R1 [VM, no account] Can this process show notifications and a Dock badge, and what identity does it need?

- **Question.** In a scratch crate in the VM, linked against the versions this change would use (`objc2 =0.6.5`, `objc2-foundation =0.3.2`, `block2 =0.6.2`, `objc2-app-kit =0.3.2`, `objc2-user-notifications =0.3.2`, `default-features = false`):
  1. **Bare binary.** What is `NSBundle.mainBundle.bundleIdentifier` for a plain `cargo run` binary? Does `UNUserNotificationCenter::currentNotificationCenter()` raise an exception (and what text) or return? If it returns, does `requestAuthorization` complete, what does `getNotificationSettings` report, does `addNotificationRequest` show a banner under what name?
  2. **Minimal bundle.** The same binary copied into `Name.app/Contents/MacOS/` with an `Info.plist` holding `CFBundleIdentifier`, `CFBundleName`, `CFBundleExecutable`, `CFBundlePackageType`; run unsigned, then ad-hoc signed (`codesign -s - --force`). The same questions; additionally: is the permission prompt shown (screenshot through `.runtime/vm/shot.sh`, then answered with the osascript helpers of `docs/vm.md` section 4), does a second launch remember the answer, and is `LSUIElement`/activation policy relevant when the app is a `gpui` app (`Application::new().run` makes it a regular app)?
  3. **Delegate.** With `UNUserNotificationCenterDelegate` (`objc2::define_class!`), installed before the first request: does `willPresentNotification` have to ask for a banner while the app is frontmost; is `didReceiveNotificationResponse` called when the banner is clicked (click it with `osascript`), on which thread, with the `userInfo` intact; does the delegate survive when only the centre holds it (expected: it does not, the notifier must keep it alive).
  4. **Withdraw.** `removeDeliveredNotificationsWithIdentifiers` and `getDeliveredNotifications`: does a delivered banner leave the notification centre, and does `threadIdentifier` group two notifications.
  5. **Badge.** `NSApplication::sharedApplication(mtm).dockTile().setBadgeLabel(Some("7"))` in the bare binary and in the bundle, with a window open (a plain AppKit window is enough; `gpui` is not required for this question): is the badge drawn on the Dock icon (screenshot), does it need the notification permission (repeat with permission refused and undetermined), does `None` remove it, does the badge disappear when the process exits, what does the icon look like for a bare binary (the Dock shows a generic icon; irrelevant, recorded)?
  6. **`gpui` interplay.** Does `Application::run` already install an `NSApplicationDelegate` or a notification centre delegate that conflicts with ours (read `gpui-0.2.2/src/platform/mac/platform.rs`), and are `cx.activate(true)` and `Window::activate_window` enough to bring a background app to the front on a click?
  7. **Reserve candidates**, only if 1-2 fail: `NSUserNotificationCenter` (N-B in `design.md` D3), the same questions.
- **Method.** As stated; scratch only; screenshots are deleted after the booleans are written down (they show the desktop). A banner is a GUI event in a VM without a user: if the VM does not show banners, record that and use `getDeliveredNotifications` and the click through the notification centre's own UI via osascript as the observation.
- **Decides.** The outcome of the rule in `design.md` D3 (1, 2 or 3), the exact feature list of `objc2-user-notifications` and `objc2-app-kit` for tasks 3.1 and 5.1, whether task 5.2 exists, whether the badge depends on the permission (D2), and the shape of the delegate (tasks 5.1).
- **Needs the user.** No (the prompt can be answered through the VM's osascript helpers). If the VM cannot show the prompt at all, record it; the first prompt on the user's Mac is then an unverified limit, not a blocker.
- **Files.** `research.md` only.

### R2 [code, then live] Mute and silence on the wire

- **Question.** In the real chat list of the app account: is the chat entry's `Muted` bool real (how many of the chats carry it, with which values), and is there any other mute-looking key at the chat-entry level or inside `ChatInfo`/`PrivateChatInfo` (a "muted until" time, a notification-level enum, a per-user setting)? In real incoming messages (history pages of chats, trace level): what values do `IsSilent` and `NotificationBehaviour` take on `Plain` messages from other people, how many messages carry each, and what is the distribution per value?
- **Method.** Code part (needs no account): in `bt-probe chats`, print the chat's mute flag after the unread count the way `pinned=<micros>` is printed (` muted` for a muted chat, nothing otherwise) and add a `chats --keys` mode that prints, over the raw chat entries (`WireChat.raw`, which already holds the entry without `Messages`), the sorted set of top-level key names with how many entries carry each, and the same for the keys of `ChatInfo`; **key names and counts only, never values, titles or ids**; tests for the formatter. Live part: run it in `bt-vm`; then `BT_LOG=yandex_proto::xiva=trace bt-probe history <chat>` over a few chats, saved to a VM temp file, and count with `grep -c` the messages carrying `IsSilent` (by value) and `NotificationBehaviour` (by value) inside `Plain` and next to the body kinds; chats are reported by ordinal, never by id or name; the trace file is deleted afterwards.
- **Decides.** Whether `wire.rs` needs a decode change for mute (then task 0.4 adds one with a live-capture fixture; the placeholder fixture `research-chat-list.json` is not evidence), whether a muted chat exists in the account to test with at all (task 7.2 needs one), what `IsSilent` and `NotificationBehaviour` mean for the follow-up in `design.md` D10.
- **Needs the user.** No (reads only).
- **Files.** `yandex-proto/src/bin/bt-probe.rs`, `research.md`.

### R3 [live, optional, non-gating] Does a mute change, or the first message of a new chat, reach the app as a push?

- **Question.** (a) When the user mutes or unmutes a chat in the web client of the app account, does a push arrive (which kind: `ChatChanged`, a message-like, `Ignored`)? (b) When a person who has never written to the app account sends the first message of a new private chat, which pushes arrive and in what order (the message upsert, `ChatChanged`, both), and does the chat-list entry exist by then?
- **Method.** `bt-probe listen` in `bt-vm` (read only) while a second party acts: (a) the user toggles the mute of one chat in a web session of the app account; (b) another account sends a first message. Record the push kinds and their order as `kind`s only (`listen` prints them), no content.
- **Decides.** Whether the "a chat that was not in the list" scenario of `incoming-notifications` holds (if no `ChatChanged` follows a first message, the scenario is dropped by 0.4 and the first message of a new chat notifies only after the next list refresh), and whether the stale-mute risk of `design.md` is real.
- **Needs the user.** **Yes**: someone other than the app account sends (b), and (a) is a mute toggle made by the user in the web client (a change of the user's own account state, made by them, not by this client). The task is **not gating**: nothing in the code tasks depends on its answer, only the wording of one scenario and one risk.
- **Files.** `research.md`.

### R4 [closing] Write the answers into the design

Task 0.4: put the R1-R3 answers into `design.md` (D2 badge permission, D3 outcome and feature lists), fix the tasks that depended on them (5.1, 5.2, 3.1 feature list, a `wire.rs` decode task if R2 asks for it), drop or keep the new-chat scenario, and validate.

## Facts already established

Read from the repository and the crates registry on 2026-11-28; nothing was run.

| Fact | Where | Consequence |
|---|---|---|
| A chat's mute flag is the chat entry's `Muted` bool, absent = false | `yandex-proto/src/wire.rs` `parse_chat` (`muted: entry.get("Muted").and_then(Json::as_bool).unwrap_or(false)`); `WireChat.muted`; `desktop/src/yandex_backend.rs` `summary()` (`muted: chat.muted`); `chat-core/src/model.rs` `ChatSummary.muted`, persisted in the cache | the cached row knows mute at notification time, no request needed |
| The only fixture with `Muted` is `research-chat-list.json`, whose names and ids are placeholders (`Boris Test`, `1111…`); no `live-*` fixture carries it | `crates/yandex-proto/tests/fixtures/` | the key's real name, type and any "muted until" form are **unconfirmed**: R2 |
| `IsSilent` and `NotificationBehaviour` are known keys the decoder ignores; `NotificationBehaviour` appears (value 2) on a seen marker in `live-push-seen-marker.json` and is treated as a modifier next to real kinds | `wire.rs` `PLAIN_META_KEYS`, `MODIFIERS`; the archived `add-read-only-client/design.md` | their meaning on a real message is unknown: R2 records it, no task uses it |
| Unread: `WireChat.unread = LastSeqNo - LastSeenByMeSeqNo` clamped at 0; the core adds 1 in `update_preview` for a message that is not outgoing, not older than the row's last message and not the row's last message, then emits `ChatUpdated`; marking read emits `ChatUpdated` with 0; a list refresh is authoritative | `wire.rs` `parse_chat`; `chat-core/src/client.rs` `update_preview`, the mark-read path | the badge total and `is_incoming` use `ChatSummary.unread` and need no core change |
| Pushes reach the UI as `ClientEvent::{MessageUpserted, ChatUpdated}`; a `ChatChanged` push makes the backend re-read the chat list and forward that chat's entry; reconnect, start-up cache and account switch are `ChatsReplaced` | `yandex_backend.rs` `subscribe`; `client.rs` `handle_live`, `become_online` (test "chat list reload") | `ChatsReplaced` can be silent by construction; `ChatUpdated` is the one signal for a live message |
| `update_preview` returns without an event for a chat the cache does not hold | `client.rs` `update_preview` | a first message in a new chat is only seen through the `ChatChanged` row: R3(b) |
| `ChatStore.apply` emits `StoreEvent::Chats` for both list events and keeps `chats`, `selected`; `forget_account` empties `chats` at sign-out and account switch | `desktop/src/store.rs` | the badge total is a pure function of `store.chats`; `Session` and `Account` events cover the clear |
| The key-window state is `Window::is_window_active()`, used by `check_read`; tests flip it with `cx.deactivate_window()` and `activate(cx)` | `desktop/src/desktop/composer.rs`, `tests.rs` | the policy can be tested headless |
| Platform globals and their test fakes follow one pattern: a `Global` factory registered in `main.rs`, a scripted fake in the test harness, "no global means off" | `player.rs` `AudioFactory`, `audio.rs`, `tests.rs` `fake::install` | `AlertsFactory` copies it |
| The last window closing quits the app; sign-out and expiry open the sign-in window and remove the chat window | `main.rs` `on_window_closed`, `open_chat_window` subscription | dropping `Desktop` is the moment the badge must go |
| `gpui` 0.2.2 has `App::activate(bool)`, `Window::activate_window`, `Window::is_window_active`, `App::on_window_closed`; it has no notification or badge API | `gpui-0.2.2/src/app.rs`, `window.rs` | the platform calls are ours |
| `Cargo.lock` has `objc2 0.6.5`, `objc2-app-kit 0.3.2`, `objc2-foundation 0.3.2`, `objc2-core-foundation 0.3.2`, `block2 0.6.2`, `bitflags 2.13.2`, `objc2-avf-audio 0.3.2`; it has **no** `objc2-user-notifications` | `Cargo.lock` | one new package |
| `objc2-user-notifications 0.3.2` (2025-10-04) exists on crates.io: Zlib OR Apache-2.0 OR MIT; depends on `objc2 >=0.6.2,<0.8`, `objc2-foundation ^0.3.2`, optional `bitflags`, `block2 >=0.6.1,<0.8`, `objc2-core-location` (a default feature we do not enable); features per class (`UNUserNotificationCenter`, `UNNotificationContent`, `UNNotificationRequest`, `UNNotificationResponse`, `UNNotificationSound`, `UNNotificationSettings`, `UNNotification`) | `https://crates.io/api/v1/crates/objc2-user-notifications` | version family matches the pinned set |
| `objc2-app-kit 0.3.2` has `NSDockTile::{badgeLabel, setBadgeLabel}` and `NSApplication::dockTile` behind the feature `NSDockTile`, `NSApplication::sharedApplication(mtm)`; the crate already shipped enables `NSResponder`, `NSScreen`, `NSWindow` | `objc2-app-kit-0.3.2/src/generated/NSDockTile.rs:55-64`, `NSApplication.rs:757-760`; `crates/desktop/Cargo.toml` | `dock.rs` needs two more features, no new crate |
| [INFERENCE, from general macOS knowledge, **not observed here**] `UNUserNotificationCenter` needs a bundle identifier (a bare process raises an NSException), delivers to a foreground app only through the delegate's `willPresentNotification`, calls the delegate on a non-main thread and holds it weakly; `NSDockTile.badgeLabel` is app-owned | | R1 measures each |

## Results log

Filled in by the tasks. One line per answered question: date, task, booleans and counts, no content.

| Task | Date | Result |
|---|---|---|
| R1 | | |
| R2 | | |
| R3 | | |
