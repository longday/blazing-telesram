# Tasks

Contracts (types and signatures) are fixed in `design.md` D1-D6, so tasks can be coded against them in parallel. Each task names the files it owns; Integrator rules from `openspec/changes/DEPENDENCIES.md` apply (one owner per file, every merge to `main` compiles and passes tests with no stubs and no warnings, cargo and `bt-probe` only in the Tart VM `bt-vm`, no `#[allow(dead_code)]`). Unit tests of a task belong to that task; the checks run per merge unit (see "Merge units") before the unit reaches `main`, and task 8.1 is the final full run.

**BLOCKED** marks a task that depends on an open question of `proposal.md` (Q1-Q6). A blocked task is not started, and its design and spec lines are written only after the answer (task 7.1). Q4 has no task of its own: its recommended answer (nothing cached, offline refused) is already D4 step 2 and the spec; the other option would add a cache field and a labelled partial list as new tasks.

Order at a glance: 1.1, 1.2, 4.1 start immediately. 1.3 follows 1.1, 1.2; 2.1 follows 1.3; 3.1 follows 2.1 (shape part) and Q6 (paging part); 4.2 follows 4.1; 5.1 follows 1.3, 4.1; 6.1 and 6.2 follow 4.1; 6.3 is blocked by Q1; 6.4, 6.5, 6.6 by Q2, Q3, Q5; 7.1 follows all answers; 8.1 follows everything; 8.2 follows 3.1 and 6.x; 8.3 follows 8.1, 8.2.

Compile coupling: adding `Backend::reactors` (4.1) breaks the fake backend in `client.rs` tests until 4.2 and `YandexBackend` until 5.1; adding `Command` and `ClientEvent` variants (4.1) breaks exhaustive matches in `store.rs` until 6.1; the store methods and view models of 6.1 and 6.2 have no caller until 6.3 (a `dead_code` warning in the binary crate). None of these tasks can reach `main` alone, so they are one merge unit.

Overlap: `bt-probe.rs`, `backend.rs`, `model.rs`, `client.rs`, `store.rs`, `yandex_backend.rs` are held by `add-chat-members-and-creation` (CM-G unit) until it merges; tasks here that edit them start after that (DEPENDENCIES rule 1). 1.x touch other files and are free.

## Merge units

Work happens on integration branches; a unit reaches `main` only after the checks below pass on the unit's tree, run in `bt-vm` in its own `~/bt/src-reactors` (never the shared `~/bt/src`), and after `git pull --rebase`.

- **RL-proto** = 1.1, 1.2, 1.3, 2.1 (branch `integ/reaction-list-proto`). Checks: `cargo build --workspace --all-targets --locked` with zero warnings, `cargo test -p yandex-proto --locked`, `cargo clippy -p yandex-proto --all-targets --locked`, `cargo fmt --all --check`. The probe is the first consumer of `Client::reactors`; the workspace still builds because nothing else changes. Independent of every question.
- **RL-app** = 4.1, 4.2, 5.1, 6.1, 6.2, 6.3, 6.7, plus whichever of 6.4, 6.5, 6.6 the answers require (branch `integ/reaction-list-app`). It cannot reach `main` before Q1 is answered (6.3 is its first consumer), and it starts after RL-proto and the CM-G unit are on `main`. Checks before the merge: `cargo build --workspace --all-targets --locked` with zero warnings, `cargo test --workspace --locked`, `cargo clippy --workspace --all-targets --locked` (no new warnings against `main`), `cargo fmt --all --check`, `openspec validate add-reaction-list --strict`. Tasks inside the unit may be written in parallel; the workspace is built once on the integrated tree, not mid-flight.

## 1. Protocol (`yandex-proto`)

- [x] 1.1 `list_reactions` read request (D1)
  - Owns: `crates/yandex-proto/src/request.rs`.
  - Depends on: nothing.
  - Do: `ReadRequest::ListReactions {chat, timestamp, before, limit}`; `method()`, `route()` (socket), `socket_frame()` body `{ChatId, Timestamp, Limit, Mode: 0, MaxTimestamp?}`; `ALLOWED_METHODS` with eight names; `REACTORS_PAGE` (30, provisional until 3.1); module docs list reactions reads.
  - Tests: frame body of a first page has no `MaxTimestamp`, of a later page has it; `Mode` is the integer 0; no registry body (`registry_body()` is `None`); the exact allowlist assertions list eight names; `the_read_allowlist_does_not_gain_the_write_method` still passes unchanged.
  - Done when: these tests exist and no write variant or generic method appears.

- [ ] 1.2 Parse the reply (D1)
  - Owns: `crates/yandex-proto/src/wire.rs`, `crates/yandex-proto/tests/wire_fixtures.rs`, new `crates/yandex-proto/tests/fixtures/research-reactors-page.json` (synthetic, shape from `research.md`; add it to the credentials-check list in `wire_fixtures.rs`; replaced by a sanitized live shape in 3.1).
  - Depends on: nothing.
  - Do: `WireReactorEntry`, `WireReactors` (with `raw` and `oldest`, D1), `parse_reactors`.
  - Tests: a page decodes in server order with guid, name, avatar, code and timestamp; a missing `UserReactions` key is an empty list (`raw` 0, `oldest` `None`); an entry without `Type` and one without `UserInfo.Guid` count in `skipped`, are not returned, but still count in `raw` and their `Timestamp` still counts in `oldest` (a page whose oldest entry is malformed: `oldest` is that entry's timestamp); an entry without `Timestamp` keeps `timestamp: None` and does not affect `oldest`; a reply that is not an object is a `Protocol` error; the existing `WireReactor` tests are untouched.
  - Done when: these tests exist.

- [ ] 1.3 Client method
  - Owns: `crates/yandex-proto/src/client.rs`.
  - Depends on: 1.1, 1.2.
  - Do: `Client::reactors(chat, timestamp, before, limit)` through the socket request path of `history`/`whoami`, parsed with `parse_reactors`.
  - Tests: in the style of the existing facade tests (request built from the arguments; a refused reply is the usual `ProtoError`).
  - Done when: the method and its tests exist.

## 2. bt-probe

- [ ] 2.1 `bt-probe reactors`
  - Owns: `crates/yandex-proto/src/bin/bt-probe.rs`.
  - Depends on: 1.3; and `add-chat-members-and-creation` 2.2/6.1 merged (they edit this file first).
  - Do: `reactors <chat> <micros> [--before <micros>] [--limit <n>]`, read-only, prints the parsed reply with `{:#?}` on stdout and a header on stderr, like `members`; `USAGE` and the module docs say it is a read and not covered by `BT_WRITE_ONLY_CHAT`. It sends only `Mode: 0` (the one request form of D1); there is no `--mode`.
  - Tests: argument parsing in the file's style (missing timestamp, bad number, unknown flag are usage errors with exit code 2); `probe_write_guard.rs` is not changed (nothing is written).
  - Done when: these tests exist.

## 3. Live verification (read-only, inside `bt-vm`)

- [ ] 3.1 Confirm the protocol points (research "Verification plan")
  - Owns: `openspec/changes/add-reaction-list/research.md` (append "Live verification"); `design.md`, `proposal.md`, `specs/**` of this change if a finding contradicts them; the fixture `research-reactors-page.json` (swap in a sanitized live shape) and its assertions.
  - Depends on: 2.1 built in the VM; VM outbound network (it had none on 2026-11-26, see `research.md`); a signed-in session in `~/bt/.runtime`. Without them: report "blocked by environment" and leave the box unchecked.
  - Do: P1 shape and that an explicit `Mode: 0` is accepted, P3 `Limit` cap, edge values and whether a short page is always the end, P5 errors (deleted message, missing timestamp, unreadable chat) on the disposable self-chat message of `add-message-reactions` 7.2 (one reactor) and read-only chats; P2 order, cursor bound (inclusive repeat) and strict progress of `oldest`, and P4 count consistency need a message with several reactors: **BLOCKED by Q6** for choosing it (recommended: a read-only scan of `bt-probe history` counts). Use its own `~/bt/src-reactors`, not the shared `~/bt/src`. Keep names and guids in the VM; record only shapes, counts and booleans.
  - Done when: every P1-P5 point has an observed answer or an explicit "not observed" in `research.md`, `REACTORS_PAGE` is final, the fixture is a sanitized live shape, and any contradiction is reflected in the design and spec.

## 4. Core (`chat-core`)

- [ ] 4.1 Domain types, seam, command and events (D2, D4)
  - Owns: `crates/chat-core/src/model.rs`, `crates/chat-core/src/backend.rs`, `crates/chat-core/src/lib.rs`, `crates/chat-core/src/fixtures.rs`.
  - Depends on: nothing (after the CM-G unit merges).
  - Do: `Reactor`, `ReactorsPage` (with `request`, `raw`, `oldest`), `Backend::reactors`, `Command::LoadReactors` (with `request`), `ClientEvent::Reactors`, `ClientEvent::ReactorsFailed` (with `request`); module and doc comments that list the reads.
  - Tests: serde-free; a `Reactor` literal helper in `fixtures.rs` if the test harness needs one. Compiles only inside RL-app.
  - Done when: the types match D2 exactly (other tasks code against them).

- [ ] 4.2 Engine: `LoadReactors` (D4)
  - Owns: `crates/chat-core/src/client.rs` (engine and its tests; the fake backend gains `reactors`).
  - Depends on: 4.1.
  - Do: D4 steps 1-5: the cached-message guard, the offline refusal without a request, the read, the failure mapping, no cache write and no change to `Message.reactions`; the `request` token is copied into every `Reactors` and `ReactorsFailed`, refusals included.
  - Tests (fake backend): a page is passed through as `Reactors` with the command's `request`; an unknown, pending, failed or deleted message gives `ReactorsFailed` (same `request`) and sends nothing; offline gives `ReactorsFailed {offline: true}` (same `request`) and sends nothing; `Rejected` carries its reason; `Network` is offline; `SessionExpired` expires the session; the cache and the message's reactions are unchanged after a successful load; two commands with different tokens for one message come back each with its own token.
  - Done when: these tests exist.

## 5. Desktop adapter

- [ ] 5.1 `Backend::reactors` over the wire (D3)
  - Owns: `crates/desktop/src/yandex_backend.rs` (including its tests).
  - Depends on: 1.3, 4.1.
  - Do: build the page from `Client::reactors`; names from the entry, then the users cache (`Users::load` for the anonymous guids), then `None`; avatar and `me`; `raw` and `oldest` and `skipped` passed through from the wire.
  - Tests: name from the entry; name from the cache; no name gives `None`; the avatar from the entry and from the cache; `me` for the account's guid; `oldest` is the wire's (including a page whose oldest entry was dropped as malformed) and `None` for an empty page; `raw` and `skipped` pass through; a failing user lookup leaves the fallbacks, `SessionExpired` is an error; `BT_WRITE_ONLY_CHAT` does not affect the read.
  - Done when: these tests exist.

## 6. Desktop UI

- [ ] 6.1 Store: the open list (D5)
  - Owns: `crates/desktop/src/store.rs` (including its tests).
  - Depends on: 4.1. Lands together with 6.3 (rule 3).
  - Do: `ReactorsView` (with `generation`) and `ReactorsState`; the monotonic `next_request` counter; open and close; the first `Command::LoadReactors`; `Reactors` and `ReactorsFailed` handling with append in server order, de-duplication by guid, application only when `(chat, timestamp)` is the open list and `request == generation`; the end-of-list rule of D5 (`exhausted` from the page and strict cursor progress, never from counts); reads, never persisted.
  - Tests: opening sends the first command with a fresh token and is `Loading`; a page becomes `Ready`; a second page appends and de-duplicates a repeated guid; a failure is `Failed` with its `offline` flag; a reply for a closed or different list is ignored; closing drops the state; **same-message reopen**: close and reopen the list of one message while the first request is out, then the first answer (page and failure variants) arrives: it is dropped and the second request's answer is applied; **refresh while an older page is in flight**: with a load-more request out, a refresh (new first-page request) supersedes it and the late page of the old generation changes nothing, including after the refresh's answer; a retry after `Failed` takes a fresh token and a late answer of the failed request is dropped; the token counter does not restart on close; `exhausted` is true for a short page (`raw < limit`), for an empty page, and for a page whose `oldest` is not below the cursor (a repeated boundary does not loop), false for a full page that progresses; a known count lower than the loaded rows does not stop paging, a known count higher than all rows does not keep it going after the end.
  - Done when: these tests exist.

- [ ] 6.2 List view (D6)
  - Owns: new `crates/desktop/src/reactors.rs`, `crates/desktop/src/theme.rs` (colours only if needed), `crates/desktop/src/main.rs` (module line).
  - Depends on: 4.1, 6.1. Lands together with 6.3 (rule 3).
  - Do: the pure `body(state, connection, total)` and the list view: rows, own marker, placeholder "Неизвестный", emoji or `NEUTRAL_GLYPH`, time, header "N из M" and "K без данных" rules, loading, failed with retry, offline with retry; selectors `reactor-{guid}`; `more` is `!exhausted`.
  - Tests: `body` for every state; a `Ready` state with a non-online connection is `Offline`; the header rules; row content; `more` follows `exhausted` and not `total` (total below the rows and total above them).
  - Done when: these tests exist.

- [ ] 6.3 Entry point: **BLOCKED by Q1**
  - Owns: depends on the answer. Q1-A (recommended): `crates/desktop/src/desktop/composer.rs` (a menu entry for sent, non-deleted messages with reactions; selector `menu-reactors`; `open_menu` opens the menu when this entry is the only content) and `crates/desktop/src/desktop.rs` (popover hosting). Q1-B adds `crates/desktop/src/timeline.rs` (a chip gesture) and a `message-reactions` delta. Q1-C is a tooltip in `timeline.rs`.
  - Depends on: 6.1, 6.2, the answer to Q1.
  - Done when: written after Q1.

- [ ] 6.4 Loading more: **BLOCKED by Q2**
  - Owns: `crates/desktop/src/store.rs`, `crates/desktop/src/reactors.rs` (after 6.1, 6.2 merge).
  - Depends on: Q2; 3.1 for the cursor rule. Q2-A (recommended): a "Показать ещё" row issuing `LoadReactors {before: oldest}`; Q2-B: an automatic loop with the 20-page bound; Q2-C: incremental.
  - Done when: written after Q2.

- [ ] 6.5 Staleness while open: **BLOCKED by Q3**
  - Owns: `crates/desktop/src/store.rs`, `crates/desktop/src/reactors.rs`.
  - Depends on: Q3; 6.1. Q3-A (recommended): a refetch from the first page on a `base_version` change, debounced; Q3-B: an "outdated" row; Q3-C: nothing but the time in the header.
  - Done when: written after Q3.

- [ ] 6.6 Type presentation: **BLOCKED by Q5** (no work if the answer is the recommended flat list)
  - Owns: `crates/desktop/src/reactors.rs`. Q5-B: tabs or a filter by type; Q5-C: sections.
  - Depends on: Q5; 6.4 (exactness of a per-type view depends on how many pages are loaded).
  - Done when: written after Q5, or recorded as "no work" for Q5-A.

- [ ] 6.7 UI tests: **BLOCKED by Q1** (selectors and flow)
  - Owns: `crates/desktop/src/tests.rs`.
  - Depends on: 6.1, 6.2, 6.3; 6.4-6.6 as answered.
  - Do, in the style of the existing menu and members tests: the entry appears for a sent message with reactions and not for a pending, deleted or reaction-less one; opening sends `Command::LoadReactors`; the list shows rows, the loading, failed and offline states; a late reply after closing changes nothing; no `Command::ToggleReaction` or other write is sent by opening or closing.
  - Done when: these tests exist.

## 7. Specs after the decisions

- [ ] 7.1 Finish the spec and design: **BLOCKED by Q1, Q2, Q3, Q5**
  - Owns: `openspec/changes/add-reaction-list/specs/**`, `design.md`, `proposal.md`.
  - Depends on: the answers to Q1, Q2, Q3, Q5 (Q4 and Q6 only if the recommended answers are not taken).
  - Do: add the requirements the answers fix to `reaction-list` ("Opening the list", "Loading more", "List stays current", type presentation); the `message-editing` delta for the menu entry if Q1-A, or the `message-reactions` delta for a chip gesture if Q1-B; replace the "Pending decisions" section of `design.md` with the decisions; record the answers in `proposal.md`; `openspec validate add-reaction-list --strict`.
  - Done when: no pending line is left and validation passes.

## 8. Verification and close-out

- [ ] 8.1 Final full run
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
