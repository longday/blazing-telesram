# Proposal

## Why

Roadmap item 7 of `add-read-only-client` ("macOS notifications, Dock badge, settings, themes") was left out on purpose. This change is its read-only part: when a message arrives and the user is not looking at that chat, macOS says so, and the Dock icon carries the number of unread messages, as every desktop messenger does. Today the only signal is the unread counter inside the window, so a message that arrives while the window is behind another app goes unnoticed until the user switches back. Nothing is sent, marked or written on the server; settings (a preview switch, sounds, per-chat rules) and themes stay out.

`research.md` (next to this file) is the single evidence document. It starts with the research tasks (what is unknown, how it is measured, what each answer decides), then the facts already established in the code and the crates.

## What Changes

- **Dock badge** (`dock-badge`): the Dock icon shows the total unread count of the chats that are not muted, as the chat list already knows it; nothing is shown at zero; above 999 it reads «999+». It follows the list live (a push, a read here, a read in another client, a reconnect refresh), and it is cleared when the session ends or the window closes. It makes no request of its own. When the app runs as a bundle the system draws it only while the notification permission includes the badge option (R1).
- **Notification for an incoming message** (`incoming-notifications`): a message from someone else that arrives live in a chat that is not muted shows a system notification titled with the chat, whose text is the same one-line preview the chat row shows (the author's name in front in a group). **None** is shown for: the user's own messages from another client, edits, deletions, reactions, typing, a muted chat, the backlog a reconnect or start-up brings in, and **the chat that is open while the window is the key window**. A window that is in the background, hidden or minimised does notify, also for the open chat.
- **Click** on a notification brings the window to the front and opens that chat. Reading it (here, or in another client) retires its delivered notifications; sign-out retires all.
- **Permission** is asked once, when the chat window opens, for alerts, sounds and the badge in one prompt. If it is refused, nothing is shown and nothing fails; the badge count is still computed, but in a bundle the system then draws no Dock badge either (R1).
- **Platform, one decision by measurement** (`design.md` D3, research R1, **outcome 2**): `UNUserNotificationCenter` through `objc2-user-notifications =0.3.2` (one new crate; the rest of the objc2 family is already in the tree) and `NSDockTile` through the `objc2-app-kit =0.3.2` already used (two more features). R1 measured in the VM that the notification centre aborts a process without a bundle identifier (a bare `cargo run` binary) and works from an **ad-hoc-signed application bundle**: the change therefore also ships the wrapper that makes and launches one (task 5.2), and a bare binary keeps the badge only and logs once that notifications are unavailable.
- `chat-core` (small): `model.rs` (one new `ClientEvent::Incoming { chat, preview }`) and `client.rs` (emitted from the live-message arm of `handle_live`, held back until the chat list reload after a reconnect has settled). `desktop`: `store.rs` (`badge_count`, forwards the event as `StoreEvent::Incoming`), new `alerts.rs` (the policy, the traits, the `Alerts` entity), new `dock.rs` and `notifier.rs` (the only files that touch platform APIs), `desktop.rs` and `main.rs` (wiring), `tests.rs`. `yandex-proto` is not changed by the code tasks. Research R2 adds a read-only print of the mute flag to `bt-probe`.
- Specs: two new capabilities, `dock-badge` and `incoming-notifications`. No existing requirement is modified.

## Capabilities

### New Capabilities
- `dock-badge`: what the Dock badge counts, how it follows the chat list, when it is cleared.
- `incoming-notifications`: which incoming messages notify and which never do, the content, the open-chat and key-window rule, click to open, retiring on read, permission, privacy.

### Modified Capabilities
None. `live-updates`, `read-markers` and `chat-list` keep their text: the badge only consumes the unread counter and the muted flag those specs already define, and the notification is triggered by a live arrival, an event the core already has the data for.

## Impact

- `crates/chat-core`: `src/model.rs` (`ClientEvent::Incoming`), `src/client.rs` (emission, recovery hold, tests in the module).
- `crates/desktop`: `src/store.rs` (new accessor, event mapping, tests in the module), new `src/alerts.rs`, `src/dock.rs`, `src/notifier.rs`, `src/desktop.rs` (owns the `Alerts` entity, forwards the key-window state, opens a chat from a click), `src/main.rs` (module lines, registers the platform factory), `src/tests.rs` (two sections), `Cargo.toml` and `Cargo.lock` (`objc2-app-kit` features `NSApplication`, `NSDockTile`; `objc2-foundation` features `NSBundle`, `NSArray`, `NSDictionary`, `NSError`; new `objc2-user-notifications`).
- `crates/yandex-proto`: `src/bin/bt-probe.rs` only (R2: print `muted`; research tool, no library change).
- `scripts/bundle.sh` (new), `run.sh` and `docs/vm.md` (task 5.2, the application bundle wrapper).
- New dependency: **one crate**, `objc2-user-notifications =0.3.2` (licence Zlib OR Apache-2.0 OR MIT, the family already shipped; `objc2`, `objc2-foundation`, `block2`, `bitflags` are in the tree; its default `objc2-core-location` is not enabled), no `THIRD_PARTY.md` line. No Telegram Desktop graphics or sounds are used (the system's default notification sound), so the relicense trigger of `openspec/config.yaml` is not hit.
- Coordination: `DEPENDENCIES.md` rule 1. This change touches chat-core `client.rs` and `model.rs`, `store.rs`, `desktop.rs`, `main.rs`, `tests.rs`, `Cargo.toml`, `Cargo.lock`; no unmerged task of CM or PIN holds them at the time of writing (their remaining tasks are live-only). Units and order are in `tasks.md` and `DEPENDENCIES.md`.
- Risks: the notification centre needs a signed application bundle (R1: measured, so 5.2); the Dock badge of a bundle needs the badge authorization (R1); a mute changed in another client may reach the app only with the next chat-list refresh (R3 was not run); the permission prompt and the notification banner are visible only in the VM's GUI; a notification's text appears on the lock screen under the user's system settings (privacy, D5).

## Non-goals

- Settings of any kind: a preview on/off switch, sounds, per-chat or per-account rules, "do not disturb", the badge counting chats instead of messages or including muted chats. Themes.
- Notification actions (reply from the banner, mark as read), notification for reactions, mentions, typing, calls, pinned-message changes, a per-message "silent" flag (`IsSilent`; R2 only records it).
- Dock bounce (`requestUserAttention`), a menu-bar item, a notification when the app is not running, push services, badge on the sign-in window.
- A notification for a message that arrived while the app was offline or closed (the badge covers it).
- Windows and Linux.

## Open questions

None blocks the plan; the three below are recorded with their defaults.

1. Should a muted chat count in the badge? Default **no** (D2); Telegram Desktop counts them by default and makes it a setting, which is out of scope here. One line in `alerts.rs` flips it.
2. Should the preview text be shown at all (privacy)? Default **yes**, the same one-line text the chat row shows, because a notification without the text is of little use and the switch is a setting (non-goal). macOS lets the user hide previews system-wide.
3. Needs the user at verification (task 7.2): someone other than the app account has to send the messages (and one chat has to be muted by the user); see `tasks.md`.

## Decisions taken without asking

Recorded in `design.md` (D1-D12); the ones a reviewer might want to veto: the badge counts messages in unmuted chats (D2); "incoming" is a live message arrival the core reports (a message push for a message the cache does not hold), not a change of the unread counter, and nothing notifies until the chat-list reload after a reconnect has settled (D4); the key-window rule is "open chat and key window, then nothing" with no further test of scroll position (D6); no sound of our own, no dock bounce (D10); the platform API was chosen by a written rule from measured facts and the bundle wrapper is a required task (D3, outcome 2).
