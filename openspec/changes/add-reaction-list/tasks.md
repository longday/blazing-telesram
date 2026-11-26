# Tasks

Contracts (types and signatures) are fixed in `design.md` D1-D6, so tasks can be coded against them in parallel. Each task names the files it owns; Integrator rules from `openspec/changes/DEPENDENCIES.md` apply (one owner per file, every merge to `main` compiles and passes tests with no stubs and no warnings, cargo and `bt-probe` only in the Tart VM `bt-vm`, no `#[allow(dead_code)]`). Unit tests of a task belong to that task; the checks run per merge unit (see "Merge units") before the unit reaches `main`, and task 8.1 is the final full run.

All open questions of `proposal.md` (Q1-Q6) were answered on 2026-11-26 and recorded in the proposal, design and spec; no task is blocked by a question. Q4's answer (nothing cached, «Нет соединения» with a retry) is D4 step 2, D6 and the spec; it has no task of its own. Q5's answer (one flat list) needs no task either (6.6).

Order at a glance: 1.1, 1.2, 4.1 start immediately. 1.3 follows 1.1, 1.2; 2.1 follows 1.3; 3.1 follows 2.1 and the VM network; 4.2 follows 4.1; 5.1 follows 1.3, 4.1; 6.1 and 6.2 follow 4.1; 6.3, 6.4, 6.5 follow 6.1, 6.2; 6.6 needs no work; 6.7 follows 6.1-6.5; 7.1 is done; 8.1 follows everything; 8.2 follows 3.1 and 6.x; 8.3 follows 8.1, 8.2.

Compile coupling: adding `Backend::reactors` (4.1) breaks the fake backend in `client.rs` tests until 4.2 and `YandexBackend` until 5.1; adding `Command` and `ClientEvent` variants (4.1) breaks exhaustive matches in `store.rs` until 6.1; the store methods and view models of 6.1 and 6.2 have no caller until 6.3 (a `dead_code` warning in the binary crate). None of these tasks can reach `main` alone, so they are one merge unit.

Overlap: the not-started tasks of `add-chat-members-and-creation` (CM 6.3, 6.4, 7.1, gated on approvals) hold no files (`DEPENDENCIES.md` rule 1), so nothing delays this change; whoever starts a file first holds it until its unit merges. `add-pins-and-importance-view` shares `wire.rs`, `tests/wire_fixtures.rs` (1.2), `request.rs` (1.1), `bt-probe.rs` (2.1) and the app files; RL goes first, the pins tasks on those files start after the matching RL unit has merged.

## Merge units

Work happens on integration branches; a unit reaches `main` only after the checks below pass on the unit's tree, run in `bt-vm` in its own `~/bt/src-reactors` (never the shared `~/bt/src`), and after `git pull --rebase`.

- **RL-proto** = 1.1, 1.2, 1.3, 2.1 (branch `integ/reaction-list-proto`). Checks: `cargo build --workspace --all-targets --locked` with zero warnings, `cargo test -p yandex-proto --locked`, `cargo clippy -p yandex-proto --all-targets --locked`, `cargo fmt --all --check`. The probe is the first consumer of `Client::reactors`; the workspace still builds because nothing else changes. Independent of every question.
- **RL-app** = 4.1, 4.2, 5.1, 6.1, 6.2, 6.3, 6.4, 6.5, 6.7 (branch `integ/reaction-list-app`). It starts after RL-proto is on `main` and ends before the pins tasks on the shared app files start. Checks before the merge: `cargo build --workspace --all-targets --locked` with zero warnings, `cargo test --workspace --locked`, `cargo clippy --workspace --all-targets --locked` (no new warnings against `main`), `cargo fmt --all --check`, `openspec validate add-reaction-list --strict`. Tasks inside the unit may be written in parallel; the workspace is built once on the integrated tree, not mid-flight.

## 1. Protocol (`yandex-proto`)

- [x] 1.1 `list_reactions` read request (D1)
  - Owns: `crates/yandex-proto/src/request.rs`.
  - Depends on: nothing.
  - Do: `ReadRequest::ListReactions {chat, timestamp, before, limit}`; `method()`, `route()` (socket), `socket_frame()` body `{ChatId, Timestamp, Limit, Mode: 0, MaxTimestamp?}`; `ALLOWED_METHODS` with eight names; `REACTORS_PAGE` (30, provisional until 3.1); module docs list reactions reads.
  - Tests: frame body of a first page has no `MaxTimestamp`, of a later page has it; `Mode` is the integer 0; no registry body (`registry_body()` is `None`); the exact allowlist assertions list eight names; `the_read_allowlist_does_not_gain_the_write_method` still passes unchanged.
  - Done when: these tests exist and no write variant or generic method appears.

- [x] 1.2 Parse the reply (D1)
  - Owns: `crates/yandex-proto/src/wire.rs`, `crates/yandex-proto/tests/wire_fixtures.rs`, new `crates/yandex-proto/tests/fixtures/research-reactors-page.json` (synthetic, shape from `research.md`; add it to the credentials-check list in `wire_fixtures.rs`; replaced by a sanitized live shape in 3.1).
  - Depends on: nothing.
  - Do: `WireReactorEntry`, `WireReactors` (with `raw` and `oldest`, D1), `parse_reactors`.
  - Tests: a page decodes in server order with guid, name, avatar, code and timestamp; a missing `UserReactions` key is an empty list (`raw` 0, `oldest` `None`); an entry without `Type` and one without `UserInfo.Guid` count in `skipped`, are not returned, but still count in `raw` and their `Timestamp` still counts in `oldest` (a page whose oldest entry is malformed: `oldest` is that entry's timestamp); an entry without `Timestamp` keeps `timestamp: None` and does not affect `oldest`; a reply that is not an object is a `Protocol` error; the existing `WireReactor` tests are untouched.
  - Done when: these tests exist.

- [x] 1.3 Client method
  - Owns: `crates/yandex-proto/src/client.rs`.
  - Depends on: 1.1, 1.2.
  - Do: `Client::reactors(chat, timestamp, before, limit)` through the socket request path of `history`/`whoami`, parsed with `parse_reactors`.
  - Tests: in the style of the existing facade tests (request built from the arguments; a refused reply is the usual `ProtoError`).
  - Done when: the method and its tests exist.

## 2. bt-probe

- [x] 2.1 `bt-probe reactors`
  - Owns: `crates/yandex-proto/src/bin/bt-probe.rs`.
  - Depends on: 1.3 (`add-chat-members-and-creation` 2.2/6.1, which also edit this file, are already merged).
  - Do: `reactors <chat> <micros> [--before <micros>] [--limit <n>]`, read-only, prints the parsed reply with `{:#?}` on stdout and a header on stderr, like `members`; `USAGE` and the module docs say it is a read and not covered by `BT_WRITE_ONLY_CHAT`. It sends only `Mode: 0` (the one request form of D1); there is no `--mode`.
  - Tests: argument parsing in the file's style (missing timestamp, bad number, unknown flag are usage errors with exit code 2); `probe_write_guard.rs` is not changed (nothing is written).
  - Done when: these tests exist.

## 3. Live verification (read-only, inside `bt-vm`)

- [ ] 3.1 Confirm the protocol points (research "Verification plan")
  - Owns: `openspec/changes/add-reaction-list/research.md` (append "Live verification"); `design.md`, `proposal.md`, `specs/**` of this change if a finding contradicts them; the fixture `research-reactors-page.json` (swap in a sanitized live shape) and its assertions.
  - Depends on: 2.1 built in the VM; VM outbound network (it had none on 2026-11-26, see `research.md`); a signed-in session in `~/bt/.runtime`. Without them: report "blocked by environment" and leave the box unchecked.
  - Do: P1 shape and that an explicit `Mode: 0` is accepted, P3 `Limit` cap, edge values and whether a short page is always the end, P5 errors (deleted message, missing timestamp, unreadable chat) on the disposable self-chat message of `add-message-reactions` 7.2 (one reactor) and read-only chats; P2 order, cursor bound (inclusive repeat) and strict progress of `oldest`, and P4 count consistency need a message with several reactors: Q6 = A, find it by a read-only scan of `bt-probe history` counts over the user's chats inside `bt-vm` (counts only), then run `reactors` on it. Use its own `~/bt/src-reactors`, not the shared `~/bt/src`. Keep names and guids in the VM; record only shapes, counts and booleans.
  - Done when: every P1-P5 point has an observed answer or an explicit "not observed" in `research.md`, `REACTORS_PAGE` is final, the fixture is a sanitized live shape, and any contradiction is reflected in the design and spec.

## 4. Core (`chat-core`)

- [x] 4.1 Domain types, seam, command and events (D2, D4)
  - Owns: `crates/chat-core/src/model.rs`, `crates/chat-core/src/backend.rs`, `crates/chat-core/src/lib.rs`, `crates/chat-core/src/fixtures.rs`.
  - Depends on: nothing.
  - Do: `Reactor`, `ReactorsPage` (with `request`, `raw`, `oldest`), `Backend::reactors`, `Command::LoadReactors` (with `request`), `ClientEvent::Reactors`, `ClientEvent::ReactorsFailed` (with `request`); module and doc comments that list the reads.
  - Tests: serde-free; a `Reactor` literal helper in `fixtures.rs` if the test harness needs one. Compiles only inside RL-app.
  - Done when: the types match D2 exactly (other tasks code against them).

- [x] 4.2 Engine: `LoadReactors` (D4)
  - Owns: `crates/chat-core/src/client.rs` (engine and its tests; the fake backend gains `reactors`).
  - Depends on: 4.1.
  - Do: D4 steps 1-5: the cached-message guard, the offline refusal without a request, the read, the failure mapping, no cache write and no change to `Message.reactions`; the `request` token is copied into every `Reactors` and `ReactorsFailed`, refusals included.
  - Tests (fake backend): a page is passed through as `Reactors` with the command's `request`; an unknown, pending, failed or deleted message gives `ReactorsFailed` (same `request`) and sends nothing; offline gives `ReactorsFailed {offline: true}` (same `request`) and sends nothing; `Rejected` carries its reason; `Network` is offline; `SessionExpired` expires the session; the cache and the message's reactions are unchanged after a successful load; two commands with different tokens for one message come back each with its own token.
  - Done when: these tests exist.

## 5. Desktop adapter

- [x] 5.1 `Backend::reactors` over the wire (D3)
  - Owns: `crates/desktop/src/yandex_backend.rs` (including its tests).
  - Depends on: 1.3, 4.1.
  - Do: build the page from `Client::reactors`; names from the entry, then the users cache (`Users::load` for the anonymous guids), then `None`; avatar and `me`; `raw` and `oldest` and `skipped` passed through from the wire.
  - Tests: name from the entry; name from the cache; no name gives `None`; the avatar from the entry and from the cache; `me` for the account's guid; `oldest` is the wire's (including a page whose oldest entry was dropped as malformed) and `None` for an empty page; `raw` and `skipped` pass through; a failing user lookup leaves the fallbacks, `SessionExpired` is an error; `BT_WRITE_ONLY_CHAT` does not affect the read.
  - Done when: these tests exist.

## 6. Desktop UI

- [x] 6.1 Store: the open list (D5)
  - Owns: `crates/desktop/src/store.rs` (including its tests).
  - Depends on: 4.1. Lands together with 6.3 (rule 3).
  - Do: `ReactorsView` (with `generation` and `kind`) and `ReactorsState` (with `loading_more`, `more_failed`), D5; the monotonic `next_request` counter; open and close; the first `Command::LoadReactors` (`kind = First`); `Reactors` and `ReactorsFailed` handling: `First` replaces the rows, `More` appends in server order with de-duplication by guid, applied only when `(chat, timestamp)` is the open list and `request == generation`; the end-of-list rule of D5 (`exhausted` from the page and strict cursor progress, never from counts); reads, never persisted. The `More` and refresh triggers are 6.4 and 6.5.
  - Tests: opening sends the first command with a fresh token and is `Loading`; a page becomes `Ready`; a second page appends and de-duplicates a repeated guid; a failure is `Failed` with its `offline` flag; a reply for a closed or different list is ignored; closing drops the state; **same-message reopen**: close and reopen the list of one message while the first request is out, then the first answer (page and failure variants) arrives: it is dropped and the second request's answer is applied; **refresh while an older page is in flight**: with a load-more request out, a refresh (new first-page request) supersedes it and the late page of the old generation changes nothing, including after the refresh's answer; a retry after `Failed` takes a fresh token and a late answer of the failed request is dropped; the token counter does not restart on close; `exhausted` is true for a short page (`raw < limit`), for an empty page, and for a page whose `oldest` is not below the cursor (a repeated boundary does not loop), false for a full page that progresses; a known count lower than the loaded rows does not stop paging, a known count higher than all rows does not keep it going after the end.
  - Done when: these tests exist.

- [x] 6.2 List view (D6)
  - Owns: new `crates/desktop/src/reactors.rs`, `crates/desktop/src/theme.rs` (colours only if needed), `crates/desktop/src/main.rs` (module line).
  - Depends on: 4.1, 6.1. Lands together with 6.3 (rule 3).
  - Do: the pure `body(state, connection, total)` and the list view (one flat list): rows, own marker, placeholder "Неизвестный", emoji or `NEUTRAL_GLYPH`, time, header "N из M" and "K без данных" rules, loading, failed with retry, «Нет соединения» with retry; the `More` model (`Hidden | Offer | Loading | Retry`) derived from `exhausted`, `loading_more`, `more_failed`; selectors `reactor-{guid}`, `reactors-more`, `reactors-retry`.
  - Tests: `body` for every state; a `Ready` state with a non-online connection and a `Failed { offline }` are `Offline`; the header rules; row content; `More` follows `exhausted`, `loading_more` and `more_failed` and not `total` (total below the rows and total above them).
  - Done when: these tests exist.

- [x] 6.3 Entry point: «Кто отреагировал» in the message menu (Q1 = A; spec "Opening the list", `message-editing` delta)
  - Owns: `crates/desktop/src/desktop/composer.rs`, `crates/desktop/src/desktop.rs`.
  - Depends on: 6.1, 6.2. Lands in RL-app.
  - Do: a menu entry «Кто отреагировал» (selector `menu-reactors`) for a sent, non-deleted message with at least one reaction; `open_menu` opens the menu when this entry is its only content; choosing it closes the menu and opens the list popover anchored where the menu was (`desktop.rs` hosts it); Escape or a click outside closes the popover; chip gestures are untouched.
  - Tests: menu content for a sent message with reactions, without reactions, pending, failed, deleted, own and other's (the existing menu tests' style); the popover opens at the menu's anchor and closes with Escape and an outside click; a left click on a chip still toggles the own reaction and opens no list.
  - Done when: these tests exist.

- [x] 6.4 Loading more (Q2 = A; spec "Loading more")
  - Owns: `crates/desktop/src/store.rs`, `crates/desktop/src/reactors.rs`.
  - Depends on: 6.1, 6.2. The cursor and short-page rules are adjustable by 3.1's findings (`REACTORS_PAGE`), not a gate for the code.
  - Do: the «Показать ещё» row (selector `reactors-more`) when not `exhausted`, `Loading` while a page is out, `Retry(reason)` (selector `reactors-retry`) after a failed page that keeps the rows; choosing it sends `LoadReactors { before: oldest }`, `kind = More`, fresh token (D5 "Load more").
  - Tests: the row is offered after a full page and absent after a short one; choosing it sends the command with `before = oldest` and a fresh token and shows `Loading`; the next page appends and de-duplicates; a failure keeps the rows and offers a retry; the retry takes a fresh token; a late answer of a superseded request is dropped; a known count below or above the rows does not change the row.
  - Done when: these tests exist.

- [x] 6.5 List stays current (Q3 = A; spec "List stays current")
  - Owns: `crates/desktop/src/store.rs`, `crates/desktop/src/reactors.rs`.
  - Depends on: 6.1, 6.2.
  - Do: on `ClientEvent::ReactionsChanged` for the open list's message with a version other than `base_version`, a 1 s debounce (a pure schedule/fire state machine in the store, driven by the app's existing timer mechanism) after which a `First` request with a fresh token is sent, `base_version` is updated and the rows stay shown until the answer replaces them (D5 "Refresh"); a deleted message gives the "message gone" failure and cancels the timer; closing cancels it.
  - Tests (no real time): a burst of version changes gives one request after the delay; the timer restarts on each change; no request for an unchanged version, for another message or after close; a refresh supersedes a `More` in flight and the late page is dropped; the answer replaces the rows (including after a deep list) and recomputes `exhausted`; a failed refresh is `Failed`; a deletion cancels the timer.
  - Done when: these tests exist.

- [x] 6.6 Type presentation (Q5 = A: one flat list): no work; the flat list is task 6.2 and the spec "Reactor entries".

- [x] 6.7 UI tests
  - Owns: `crates/desktop/src/tests.rs`.
  - Depends on: 6.1-6.5.
  - Do, in the style of the existing menu and members tests: the entry appears for a sent message with reactions and not for a pending, deleted or reaction-less one; opening sends `Command::LoadReactors`; the list shows rows, the loading, failed and «Нет соединения» states with their retry; «Показать ещё» appends; a reaction change re-reads from the first page; a late reply after closing changes nothing; no `Command::ToggleReaction` or other write is sent by opening, paging, refreshing or closing.
  - Done when: these tests exist.

## 7. Specs after the decisions

- [x] 7.1 Finish the spec and design (done 2026-11-26 with the answers)
  - Owns: `openspec/changes/add-reaction-list/specs/**`, `design.md`, `proposal.md`.
  - Done: `reaction-list` gained "Opening the list", "Loading more", "List stays current", the flat-list wording of "Reactor entries" and the «Нет соединения» wording of "Failed and offline list"; the `message-editing` delta for the menu entry (Q1 = A) was added; the "Pending decisions" section of `design.md` became "Decisions after the answers" and D5/D6 gained the load-more and refresh contracts; the answers are recorded in `proposal.md`; `openspec validate add-reaction-list --strict` passes.

## 8. Verification and close-out

- [x] 8.1 Final full run
  - Owns: nothing; fixes only compile breaks that cross task boundaries.
  - Depends on: 1.1-1.3, 2.1, 4.1, 4.2, 5.1, 6.1-6.7, 7.1, and both merge units on `main`. The per-unit checks of "Merge units" are not replaced by this task: they run before each unit merges.
  - Do: on `main` after both units, `cargo test --workspace --locked`, `cargo clippy --workspace --all-targets --locked`, `cargo fmt --all --check`, `cargo build --workspace --all-targets --locked` with zero warnings, all inside `bt-vm`; `openspec validate add-reaction-list --strict`.
  - Done when: all pass.

- [ ] 8.2 Live check of the app, read-only
  - Owns: `research.md` (append "App check").
  - Depends on: 3.1, 6.1-6.7, 8.1; the VM network.
  - Do: with `BT_WRITE_ONLY_CHAT` set to the self chat (so any accidental write is refused), run the app in `bt-vm`: open the list of the disposable self-chat message and of a multi-reactor message (Q6), check names, emoji, times, the own marker, "N из M" if applicable, the offline state with the network off (restored right after, in the same command by a trap), and that no reaction changed (the message's chips and version before and after). Screenshots stay in `.runtime/vm/` (not committed).
  - Done when: findings are in `research.md`; any contradiction is reflected in the design and spec.

- [ ] 8.3 Archive
  - Owns: the change directory and `openspec/specs` through the archive workflow.
  - Depends on: every task above checked.
  - Do: follow the `openspec-archive-change` skill.
  - Done when: the change is archived and the main specs hold the new and modified requirements.
