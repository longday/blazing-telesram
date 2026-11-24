# Tasks

Contracts (types and signatures) are fixed in `design.md` D1-D3, so tasks can be coded against them in parallel. Each task names the files it owns; no file belongs to two tasks, so tasks without a dependency between them can run at the same time. "Depends on" lists real dependencies only. Unit tests of a task belong to that task; builds of the whole workspace are task 7.1.

Order at a glance: 1.1, 2.1, 2.2, 3.1 start immediately. 3.2 follows 3.1; 3.3 follows 1.1 and 3.1; 4.1 follows 2.1, 2.2 and 3.1; 5.1 follows 3.1; 5.3 follows 5.1; 5.2 follows 3.1; 5.4 follows 5.1-5.3; 6.1 follows 2.1 and 2.2; 7.1 follows 2-6; 7.2 follows 1.1, 6.1 and 7.1; 7.3 follows everything.

Compile coupling: adding `Backend::react` (3.1) breaks the fake backend in `client.rs` tests until 3.2 and `YandexBackend` until 4.1; adding `WireMessage.reactions` (2.1) breaks the `WireMessage` literals in `bt-probe.rs` (fixed by 6.1) and `yandex_backend.rs` tests (fixed by 4.1); adding `Message.reactions` (3.1) breaks `Message` literals in `store.rs` tests (5.1), `tests.rs` (5.4) and `yandex_backend.rs` (4.1). The workspace compiles again only when all of them have landed; that is why nobody builds mid-flight and 7.1 builds once.

Ordering on `bt-probe.rs` (Integrator, `DEPENDENCIES.md` rule 1): task 6.1 is the only task here that touches `bt-probe.rs`, and `add-chat-members-and-creation` 2.2 and 6.1 edit that file first. 6.1 therefore starts only after those two are merged to `main`; every other task of this change is independent of them. Until 6.1 lands, the `yandex-proto` bin test target fails to compile on the `WireMessage` literal in `bt-probe.rs` (missing `reactions`); this is expected, so check the other targets only: `cargo test -p yandex-proto --lib --test wire_fixtures`, and the `chat-core` and `desktop` packages where they compile.

## 1. Reaction type table

- [x] 1.1 Collect the confirmed (code, emoji) pairs, read-only (decision P1)
  - Owns: `openspec/changes/add-message-reactions/research.md` (append a section "Reaction types").
  - Depends on: nothing.
  - Do: find the code-to-emoji table in the public web client bundle (the static `app.js` named in the protocol notes, current build), cross-check code `100101` against `tests/fixtures/live-push-reaction.json`, and cross-check every pair with a second source (another bundle location or a self-chat observation). No account writes.
  - Done when: the section lists each confirmed pair with its evidence and the order for the picker, or states that no pair could be confirmed (then 3.3 leaves `KNOWN` empty, per P1).

## 2. Protocol (`yandex-proto`)

- [x] 2.1 Decode reactions from pushes and `ServerMessage` siblings (D1)
  - Owns: `crates/yandex-proto/src/wire.rs`, `crates/yandex-proto/src/xiva.rs` (`push_kind` only), `crates/yandex-proto/tests/wire_fixtures.rs`, new `crates/yandex-proto/tests/fixtures/research-push-reaction-removed.json` (synthetic: a `Reaction` push with `Action` 1 and no `Reactions` key, only a newer `ReactionsVersion`; add it to the credentials-check list in `wire_fixtures.rs`).
  - Depends on: nothing.
  - Do: `WireReactionCount`, `WireReactor`, `WireReactions`, `ReactionAction`, `WireMessage.reactions`, `PushEvent::Reaction`, `decode_push` branch, `push_kind` arm.
  - Tests: the live fixture decodes to the exact event (chat, timestamp `1795070677887127`, actor, `Replace`, code `100101`, counts, version 2, the recent entry); the removed fixture decodes with empty counts; `Reaction` without `ChatId`/`Timestamp` stays `Ignored`; a history row with the three siblings fills `WireMessage.reactions`, one without gives `None`; `Count` 0 and entries without `Type` are dropped; `live_push_fixtures` no longer expects `Ignored` for the reaction fixture (the heartbeat still does); the unit test that a bare `Reaction` body parses as `Other("Reaction")` is kept.
  - Done when: these tests exist and assert the above.

- [x] 2.2 Encode the reaction write (D2)
  - Owns: `crates/yandex-proto/src/write.rs`, `crates/yandex-proto/src/request.rs` (module docs only).
  - Depends on: nothing.
  - Do: `ReactionChange`, `WriteRequest::Reaction`, `kind()` `"reaction"`, `client_message` arms, `CLIENT_MESSAGE_VARIANTS` with `"Reaction"`; rewrite the module docs of `write.rs` and `request.rs` that say reactions are unrepresentable (heartbeats, pins and every other mutating variant stay so).
  - Tests: frames for `Set(100101)` (`Type` is an integer, `Action` 2) and `Remove` (`Action` 1, no `Type`); `every_request` and its exhaustive `match` include the new variant; the variant-set assertions list four entries; `the_read_allowlist_does_not_gain_the_write_method` still passes unchanged.
  - Done when: those tests exist; no generic `push` or other new variant appears.

## 3. Core (`chat-core`)

- [x] 3.1 Domain types, rules and seam contracts (D3)
  - Owns: new `crates/chat-core/src/reactions.rs`, `crates/chat-core/src/model.rs`, `crates/chat-core/src/backend.rs`, `crates/chat-core/src/lib.rs`, `crates/chat-core/src/fixtures.rs`, and the `Message` literals in `crates/chat-core/src/client.rs` (non-test `outgoing_message` and test helpers: add `reactions: Default::default()` only).
  - Depends on: nothing.
  - Do: `ReactionCode`, `ReactionCount`, `Reactions` (with `apply`, `toggled`), `ReactionsUpdate`, `KnownReaction`, an empty `KNOWN`, `NEUTRAL_GLYPH`, `emoji`; `Message.reactions` with `serde(default)`; `Message::inherit_reactions`; `Command::ToggleReaction`; `ClientEvent::ReactionsChanged`; `LiveEvent::ReactionsChanged`; `Backend::react`; module and doc comments that list the writes now include reactions; export in `lib.rs`.
  - Tests (in `reactions.rs` and `model.rs`): `apply` ignores an older and an equal version, applies a newer and a versionless update, keeps `mine` when the update does not say, clears `mine` whose code left `counts`; `toggled` covers first reaction, replace (old count drops, removed at 0), remove, and a new type appended last; `inherit_reactions` copies only when the incoming copy has no counts and no version; a `Message` JSON without `reactions` deserializes.
  - Done when: the types match design D3 exactly (other tasks code against them) and the tests exist.

- [x] 3.2 Engine: toggle, overlay, live updates, replay
  - Owns: `crates/chat-core/src/client.rs` (engine and its tests, except the literals owned by 3.1), `crates/chat-core/src/cache.rs` (a legacy-row test and, if needed, a helper to read the cached rows of a range).
  - Depends on: 3.1.
  - Do: D5 in full: `Command::ToggleReaction` handling, `Action::React`, `reaction_inflight`, the guards (cached, `Sent`, not `Deleted`, online, nothing in flight), overlay emit, result handling (confirm writes cache; failure rolls back; superseded cases), `LiveChange::Reactions` in `live_log` and `replay_live`, `LiveEvent::ReactionsChanged` handling, `inherit_reactions` in the upsert path and in `store_chunk`, the new refusal reason constant; the fake backend gains `react`.
  - Tests (fake backend): toggle shows the overlay at once and a confirmed request writes the cache; a rejected request rolls back and emits `ActionFailed` with the reason; offline refuses at once with no overlay; a second toggle while one is in flight sends nothing; a pending, failed, deleted or unknown message is refused; a stale event changes nothing; a newer event replaces the overlay and a later failure of the request emits no rollback; a history chunk requested before an event keeps it after replay; an edit upsert and a history chunk without reactions keep the cached ones; an event for an uncached message is ignored without error; no preview or typing side effect; a row written without the field loads.
  - Done when: these tests exist.
  - Notes: `reaction_inflight` is `HashMap<(ChatId, MessageTs), bool>` (the flag is `superseded`); `Action::React { timestamp, overlay }` derives the requested target from `overlay.mine`. The refusal reason is "На это сообщение нельзя отреагировать". `store_chunk` reads the cached rows only for the span of the chunk (`Cache::messages_between`, new in `cache.rs`) and lets `inherit_reactions` fill rows that carry no reaction state before `replay_live` runs. `LiveEvent::MessageUpserted` inherits against the cached copy before it is logged, so the logged `Upsert` replays with the known reactions. Verified: `cargo test -p bt-chat-core --locked` (160 pass) with the `chat-core` crate compiling on its own (the desktop crate still needs 4.1 and 5.x).

- [x] 3.3 Fill the type table
  - Owns: `crates/chat-core/src/reactions.rs` (the `KNOWN` constant and its test only).
  - Depends on: 1.1, 3.1.
  - Do: transcribe the confirmed pairs from `research.md` in picker order; test that codes are unique and `emoji` finds each. If 1.1 confirmed nothing, leave `KNOWN` empty and say so in the commit message.
  - Done when: `KNOWN` matches `research.md`.

## 4. Desktop adapter

- [x] 4.1 Map reactions between wire and domain, implement `Backend::react` (D7)
  - Owns: `crates/desktop/src/yandex_backend.rs` (including its tests and the `WireMessage` literal there).
  - Depends on: 2.1, 2.2, 3.1.
  - Do: `message()` fills `Message.reactions`; the live loop maps `PushEvent::Reaction` to `LiveEvent::ReactionsChanged` through a small pure function; `react` builds `WriteRequest::Reaction` and goes through `push` (so `BT_WRITE_ONLY_CHAT` applies); add `reactions: Default::default()` to the `Message` literal.
  - Tests: counts and version map one to one; `mine` comes from the newest recent entry of the account; the actor-is-account cases for `Replace` and `Remove`; another actor with the account in `recent`; another actor without it gives "unknown" (`None`); `react(Some(code))` and `react(None)` produce the right requests; a write to another chat is blocked by `BT_WRITE_ONLY_CHAT`.
  - Done when: these tests exist.

## 5. Desktop UI

- [x] 5.1 Store: apply reaction changes, toggle API (D8)
  - Owns: `crates/desktop/src/store.rs` (including the `Message` literal in its tests).
  - Depends on: 3.1.
  - Do: handle `ClientEvent::ReactionsChanged` in place with `TimelineChange::Delta { touched }`, only when the reactions differ and the message is loaded; no typing or media side effect; `ChatStore::toggle_reaction`.
  - Tests: the update changes only the reactions of that message; an equal update emits nothing; an unloaded timeline or unknown message is a no-op; the author's typing indicator survives an update; `toggle_reaction` sends `Command::ToggleReaction`.
  - Done when: these tests exist.

- [x] 5.2 Reaction chips in the bubble (D8)
  - Owns: `crates/desktop/src/timeline.rs`, `crates/desktop/src/theme.rs`.
  - Depends on: 3.1. Lands together with 5.3 (it calls `Desktop::toggle_reaction`).
  - Do: the chip row below the body and failure line for `Sent`, non-deleted messages; `emoji(code)` or `NEUTRAL_GLYPH` plus count; highlighted own chip with new theme colors; selectors `msg-{ts}-reaction-{code}`; left click calls `Desktop::toggle_reaction(chat, ts, code, cx)`.
  - Done when: the row renders as specified; behavior is tested in 5.4.

- [x] 5.3 Menu reaction row and the toggle entry point (D8)
  - Owns: `crates/desktop/src/desktop/composer.rs`; `crates/desktop/src/desktop.rs` only if a field or import is needed.
  - Depends on: 3.1, 5.1.
  - Do: `Desktop::toggle_reaction(chat, timestamp, code, cx)` (ignored while the session is blocked or the chat is not open); a row of `KNOWN` emoji at the top of `message_menu_view` for sent, non-deleted messages, own type marked, selectors `menu-reaction-{code}`, hidden when `KNOWN` is empty; `open_menu` opens the menu when only the reaction row applies; choosing a type closes the menu and toggles.
  - Done when: the menu behaves as specified; behavior is tested in 5.4.
  - Decisions: the row appears for `Sent` and non-`Deleted` bodies (so also `Unsupported`, like the chips and the "odd messages" risk); eight 24 px buttons fit the 220 px menu in one line (`flex_wrap` only if `KNOWN` grows); the own type is filled with `REACTION_OWN` (not assertable through debug bounds; 5.4 may check it through state); a separator follows the row when other entries exist; `Desktop::toggle_reaction` is `pub(crate)` and the only caller of `ChatStore::toggle_reaction`. Verified locally with temporary tests (not committed; 5.4 owns `tests.rs`): a selector for every `KNOWN` code, a click sends `Command::ToggleReaction` and closes the menu, no row on a pending message, a call for a chat that is not open is ignored.

- [x] 5.4 UI tests
  - Owns: `crates/desktop/src/tests.rs` (including its `Message` literal helper).
  - Depends on: 5.1, 5.2, 5.3.
  - Do, following the existing edit and delete tests: chips appear with counts and the own chip is highlighted; an unknown code shows the neutral glyph; a deleted message and a pending message show none and offer no row; clicking a chip sends `Command::ToggleReaction` with that chip's code; a refused reaction shows the error toast. Menu-row cases depend on the 3.3 outcome: with a non-empty `KNOWN`, choosing a type sends `Command::ToggleReaction` and the own type is marked; with an empty `KNOWN`, assert only that the menu has no row.
  - Done when: these tests exist.
  - Decisions: 13 tests in `tests.rs` (`Message` literal gets `reactions: Default::default()`). `KNOWN` is non-empty (3.3), so only the non-empty branch is tested (the selector table is asserted equal to `KNOWN`); `reactable` is not given a seam for an empty table. Debug bounds see neither fill nor glyph text, so the own type (chip and menu button fill) is asserted through `Reactions::mine` in the store, which both views read, and the neutral glyph through the chip for a code outside `KNOWN` being present and clickable (`emoji()` is `None` for it); the count is observed through chip width. A refused reaction is the core's real pair (overlay `ReactionsChanged`, then rollback `ReactionsChanged` + `ActionFailed`): the state returns and the toast shows and expires.
  - Finding (5.3 observation "pending menu opened as if Sent after closing another menu"): not a bug, reproduced as a test effect. Entries of a closed menu stay in the debug map (`shown("menu-reply")` stays true), and the open menu, 192 px tall, covers the message right below the click, so a right click there lands on the menu or its overlay and closes it. With the pending message out of the menu's reach, after Escape, after an outside click and after a right click that closed the other menu, the new menu is the copy-only one (same surface height as a fresh one, only Copy works, no command is sent). No code changed.

## 6. bt-probe

- [x] 6.1 `bt-probe react-self`
  - Owns: `crates/yandex-proto/src/bin/bt-probe.rs` (including the `WireMessage` literal in its tests).
  - Depends on: 2.1, 2.2.
  - Do: `react-self <micros> <code|remove> [--chat <self chat>]` built like the other `*-self` commands and refusing any chat but the account's self chat; update `USAGE` and the file's module docs; `listen` already prints pushes through `Debug`, so reaction pushes show up there.
  - Tests: argument parsing cases in the file's existing test style (a bad code, `remove`, a non-self chat refused).
  - Done when: these tests exist.
  - Done (decisions): `<code>` is a positive integer or `remove`, anything else is a usage error (exit 2); the self-chat refusal is `resolve_self_chat` through `write_self`, as for `delete-self`. Addition for 7.2 ("do rows carry `Reactions`"): `history` now appends the row's reaction state after its text (`describe`), because it prints no raw JSON; rows without the siblings print as before.

## 7. Verification and close-out

- [ ] 7.1 Build and test the workspace once
  - Owns: nothing; fixes only compile breaks that cross task boundaries.
  - Depends on: 2.1, 2.2, 3.1, 3.2, 3.3, 4.1, 5.1, 5.2, 5.3, 5.4, 6.1.
  - Do: `cargo test --workspace --locked`, `cargo clippy --workspace --all-targets --locked`, `cargo fmt --check`; run `openspec validate add-message-reactions --strict`.
  - Done when: all pass.
  - Result so far (Integrator, `integ/reactions` merged into `main` at 65204cb; 7.1 stays open because its gate "all pass" is not met): `cargo build --workspace --all-targets` 0 warnings; `cargo test --workspace --locked` green twice (232 + 163 + 178 + 20 + 13); `openspec validate add-message-reactions --strict` valid; `cargo clippy --workspace --all-targets --locked` has no errors and the same warnings as `main` except two `large_enum_variant` that this change causes (`Message` grew by the 48-byte `reactions` field of D3, which pushes `ClientEvent` in `model.rs` and `Input` in chat-core `client.rs` over the 200-byte threshold; boxing `Message` in the events touches about 30 sites and tests of other tasks); the two new `collapsible_if` in `reactions.rs` are fixed. `timeline.rs` (chips, 5.2) compiles and is exercised by the 5.4 harness tests. **Not passing: `cargo fmt --check`** reports 1561 hunks, and it does on `main` before this change too (no `rustfmt.toml`, the whole repository is wider than the default width). Needs Main's decision: (a) change the gate to "fmt: not applicable, baseline non-conforming", (b) format the repository in a separate commit first, or (c) something else; the two `large_enum_variant` need the same decision (accept or box).

- [ ] 7.2 Live check in the self chat (P5)
  - Owns: `openspec/changes/add-message-reactions/research.md` (append "Live verification"), and, if findings require it, `design.md`, `proposal.md` and `specs/**` of this change; optionally a sanitized fixture under `crates/yandex-proto/tests/fixtures/` with its assertion in `tests/wire_fixtures.rs`.
  - Depends on: 1.1, 6.1, 7.1. Needs a signed-in session in `.runtime`; without one, report "blocked by environment" and leave the box unchecked.
  - Approval gate (P5): before the first reaction is sent, show the user the chat id (it must be the self chat) and the exact target message, and wait for their approval. No approval, no mutation: leave the box unchecked and report that the task waits for approval. The proposed target is a disposable message this check sends to the self chat (extra safeguard); the user may approve it or name another message of the self chat.
  - Do, with `BT_WRITE_ONLY_CHAT` set to the self chat and only on the approved target: `listen --watch <self chat>` in one terminal; `send-self` the disposable message, if it is the target; `react-self <ts> <code>` and confirm status 1 and the push (REPLACE with no prior reaction creates one; whether the sender gets its own push); `react-self <ts> <other code>` (one own reaction, counts move); `react-self <ts> remove` (accepted without `Type`; push with no reactions); `history <chat>` (do rows carry `Reactions`/`ReactionsVersion`/`RecentUserReactions`); `delete-self <ts>` to clean up a disposable target. Run the app against the same chat once: chip, menu row and rollback on a refused write (for example by going offline).
  - Done when: the findings are in `research.md`; any contradiction of D2, D3, P3 or P4 is reflected in `design.md`, `proposal.md` and the specs (for example `Remove` carrying `Type`, several reactions per user).

- [ ] 7.3 Archive
  - Owns: the change directory and `openspec/specs` through the archive workflow.
  - Depends on: 7.1, 7.2 and every task above checked.
  - Do: follow the `openspec-archive-change` skill.
  - Done when: the change is archived and the main specs hold the new and modified requirements.
