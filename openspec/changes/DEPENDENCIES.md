# Task dependency graph of open changes

Owner: Integrator. Updated: 2026-11-27. Three changes are open: **CM** `add-chat-members-and-creation`, **RL** `add-reaction-list`, **PIN** `add-pins-and-importance-view`. Source: their `tasks.md`.

`[live]` = needs the signed-in session in `.runtime` and the real account (reads only unless noted). `[approval]` = needs the user's explicit approval of the target first.

## Archived

| Change | Archive | Main specs |
|--------|---------|------------|
| `add-global-search` (GS) | `openspec/changes/archive/2026-11-25-add-global-search` | `message-search` created, `chat-list` updated |
| `add-media-download` (MD) | `openspec/changes/archive/2026-11-25-add-media-download` | `attachment-download` and `message-history` synced |
| `restore-runtime-io-test` | `openspec/changes/archive/2026-11-26-restore-runtime-io-test` | none (test only) |
| `add-message-reactions` (MR) | `openspec/changes/archive/2026-11-26-add-message-reactions` | `message-reactions` created; `live-updates`, `message-editing`, `message-sending`, `offline-cache` updated |
| `add-reveal-downloaded-file` (REV) | `openspec/changes/archive/2026-11-27-add-reveal-downloaded-file` | `attachment-download`: «Reveal in Finder» added, «Authenticated and private transfer» modified. Unit on `main` (merge `4b1eaa7`); final run 5.1 in `bt-vm` green (0 warnings, 417 + 240 + 190 + 30 + 4 + 16 passed, clippy at the 11-warning baseline); VM hand check 5.2 ticked (`8cf61c4`) |

## Rules

1. **One owner per file, at task granularity.** A task may start only when no other in-flight task edits any of its files; "different hunks" and "additive" do not exempt. "In flight" means started and not yet merged to `main`. Inside a merge unit (rule 2) a task stops holding its files once it is merged into the unit's integration branch (`integ/<change>`).
2. **Every merge to `main` compiles with no stubs and no no-op arms** (`cargo check --workspace --all-targets`; `cargo test --workspace` green). A task that cannot compile on its own is merged only inside a *merge unit* with the tasks that complete it.
3. **Zero build warnings** (`cargo build --workspace --all-targets`; no CI and no `-D warnings`, but `main` builds with 0 warnings). `desktop` is a binary crate, so a store method, field or view model with no caller is a `dead_code` warning. A unit therefore contains the first consumer of everything it adds; `#[allow(dead_code)]` is not used.
4. **Cargo, `bt-probe` and the app run only in the Tart VM `bt-vm`, never on the host** (rule `containers.md`). Procedure: sync the tree to test into `~/bt/src-<name>` (tracked and untracked, no ignored files; `~/bt/src` is shared, so concurrent agents use their own directory), put `$HOME/.cargo/bin` in `PATH` (`tart exec` lacks it), build and run there; `.runtime` for live runs is the VM's `~/bt/.runtime`. Remove the `~/bt/src-<name>` directory when the task is done. `git` and `openspec` on the host are fine. How to start the VM and the full workflow: [docs/vm.md](../../docs/vm.md).
5. **Live-account tasks share one session and run one at a time**: CM 5.1, 5.3, 6.2; RL 3.1, 8.2; PIN 0.1, 0.2, 6.2 (reads only, except PIN 6.2's one self-chat edit). Writes to the account need the user's approval of the target; every `bt-probe` write command obeys `BT_WRITE_ONLY_CHAT`, and `create-*` is refused whenever it is set, so the creation runs need it **unset**.
6. **A task that is not started locks nothing.** Rule 1 holds files only for started, unmerged tasks. Tasks gated on approvals, live runs or open questions (for example CM 6.3, 6.4, 7.1) hold no file until the moment they start; whoever starts a file first keeps it until its unit merges, the other waits.

## add-chat-members-and-creation (CM)

Done: 0.1, 1.1, 1.2, 1.3, 2.1, 2.2, 3.1, 3.2, 3.3, 4.1, 4.2, 4.3, 5.2, 6.1, 6.2 (live PASS, merged `17e336b`) and the review fixes (offline views, probe write guard). Open: 5.1 and 5.3 (blocked on the user: no target), 6.3, 6.4, 7.1.

| ID | Status | Needs | Files / notes |
|----|--------|-------|---------------|
| 5.1 | **blocked on the user**: needs a target with an existing private chat; the web client cannot delete or recreate a private chat, so there is no target for a creation test; the shapes observed so far are in `design.md` "Observed shapes"; unticked; **[approval] [live]** | 2.2 | **[approval] [live]** |
| 5.3 | **blocked on the user**, behind 5.1 (same target problem); 5.2 is done | 5.1, 5.2 | **[approval] [live]** |
| 6.2 | **done** (live PASS, merged 2026-11-27 as part of `17e336b`): the group landed in «Внешние чаты» in the web client, and the spec has no organisation requirement; payload and reply shape in `design.md` "D4 outcome" and "Observed shapes"; live fixtures `live-create-chat.json`, `live-create-private.json` | 6.1 | `design.md`, `tasks.md`, fixtures, `wire_fixtures.rs` |
| 6.3 | **ready** (6.2 PASS, 3.3 merged); starts when its files are free: PIN 2.4 holds chat-core `client.rs` | 6.2 (done), 3.3 | unit CM-G (with 6.4): `backend.rs`, `model.rs`, chat-core `client.rs`, `store.rs`, `yandex_backend.rs` |
| 6.4 | blocked-by CM 6.3 (4.3 is merged) | 6.3, 4.3 | unit CM-G: `new_chat.rs`, `desktop.rs`, `tests.rs` |
| 6.F | **does not run** (6.2 PASS) | | |
| 7.1 | blocked-by CM 6.4 | 6.4 | any file named above (fixes only) |

Unit **CM-G** = 6.3 + 6.4 (rule 3: the backend and store methods of 6.3 are first called by the dialog of 6.4). Files: `backend.rs`, `model.rs`, chat-core `client.rs`, `store.rs`, `yandex_backend.rs`, `new_chat.rs`, `desktop.rs`, `tests.rs`. The gate (live PASS of 6.2) is met. 6.3, 6.4 and 7.1 are not started, so by rule 6 they lock none of these files; when CM-G starts it waits for in-flight units that hold a file (now PIN 2.4 holds chat-core `client.rs`; PIN 3.3, if started first, would hold `backend.rs`, `client.rs`, `yandex_backend.rs`).

## add-reaction-list (RL)

Done: 1.1, 1.2, 1.3, 2.1 (RL-proto), 3.1 (live verification, merged `7d97260`), 4.1, 4.2, 5.1, 6.1-6.5, 6.7 (RL-app) — all on `main` (RL-app merged 2026-11-27 as `b85d478`; build 0 warnings, `cargo test --workspace` 402 + 240 + 190 + 30 + 4 + 16 green, clippy at the 11-warning baseline of that time, `openspec validate --all --strict` 17/17), 6.6 (no work: flat list), 7.1, 8.1 (final full VM run passed on `main` `3bf6937`; checkbox merged to `main`). All open questions (Q1-Q6) were answered on 2026-11-26. Units: **RL-proto** = 1.1, 1.2, 1.3, 2.1 and **RL-app** = 4.1, 4.2, 5.1, 6.1-6.5, 6.7, both on `main`. The VM network is back (3.1 ran on 2026-11-27); 8.2 is a live read (no write, no approval) and waits for a free live slot. After 3.1: `cargo test -p yandex-proto --locked --offline` 190 + 30 + 4 + 17 passed in `bt-vm`, fmt green. The clippy baseline of `main` is now **10** (after PIN-app, `270d503`).

| ID | Status | Needs | Files |
|----|--------|-------|-------|
| 3.1 | **done**, on `main` (merge `7d97260`, implementation `03dd898`, review approved): live `list_reactions` verified, `Mode: 0` accepted, newest-first order, exclusive `MaxTimestamp`; the server `Limit` cap is not observed (no list above 16 reactors was readable); sanitized live fixture swapped in; findings in `research.md` "Live verification" | 2.1 (done) | `research.md`, `design.md`, the fixture, `wire_fixtures.rs` |
| 8.1 | **done**, on `main` | 6.7 (done) | none |
| 8.2 | startable except rule 5 (the VM network is back; **[live]** read-only; after the current live task) | 8.1 (done) | `research.md` |
| 8.0 | **implemented** on branch `reactors/refusal`, in review; the review requires a fix first (a leak in `bt-probe`); the server refuses the list for every channel message ("reactions are anonymous", `Status` 4, `ErrorInfo.Message`, no `Details`) and `check_status` lost that reason; before 8.3 | 3.1 (done) | per its `tasks.md` entry |
| 8.3 | blocked-by 8.2 and 8.0 (archive) | 8.2, 8.0 | archive |

## add-pins-and-importance-view (PIN)

Done: 1.1 and 5.1 (on `main`; 5.1 is `7695968`) and **the whole PIN-app unit** (1.2, 1.4, 2.1-2.3, 3.1, 3.2, 4.1-4.4, 5.2; 1.3 dropped), merged into `main` as `270d503` on 2026-11-27 (`--no-ff` from `integ/pins-and-importance-view` `e4c2084`), plus 0.1 (`1dd26f2`) and 6.1. Checks on `main` `270d503` in `bt-vm`: `cargo fmt --all --check` green; `cargo build --workspace --all-targets --locked --offline` 0 warnings; `cargo test --workspace --locked --offline` 445 + 279 + 200 + 31 + 4 + 19 passed, 0 failed; `cargo clippy --workspace --all-targets --locked --offline` **10 warnings (the new baseline of `main`, was 11: the `model.rs:361` warning is gone)**: 2 deref (`chat-core/src/client.rs:4365`, `desktop/src/tests.rs:577`), 5 large size difference (`chat-core/src/client.rs:66` and `:164`, `yandex-proto/src/wire.rs:190` and `:295`, `xiva.rs:102`), 1 large `Err` (`yandex-proto/src/ws.rs:130`), 2 collapsible `if` (`desktop/src/desktop/composer.rs:612`, `desktop/src/yandex_backend.rs:354`); `openspec validate --all --strict` 16/16. All questions are answered (Q4 by 0.2 on 2026-11-27: B). Open: 0.2 in review (`pins/0.2`), 2.4 (in flight, `pins/2.4`), the wire `at` fix (pending), 3.3 (edit flag, Q6; its own unit after PIN-app, closes with the live check 6.2), 6.2 (live, no user needed), 6.3 (archive).

| ID | Status | Needs | Files / notes |
|----|--------|-------|---------------|
| 0.1 | **done**, on `main` (integ merge `1dd26f2`); live observation recorded, 1.3 dropped | | `research.md` |
| 0.2 | **done** on branch `pins/0.2` (`27b8dc3`), **in review**; docs only (`research.md`, `design.md`, `proposal.md`, `tasks.md`): the web client was the other party; answers Q4 = B (flag-only change sets `LastEditTimestamp`), Q5 = B (the comparable action time is `LastEditTimestamp`) | 0.1 (done) | `research.md` |
| 1.2 | **done**, on `main` (integ merge `b48b666`) | 1.1 | `wire.rs`, `tests/wire_fixtures.rs`, `xiva.rs` |
| 1.3 | **dropped**: chat-list entries carry `PinnedMessageTs`, so no `request.rs` change is needed (0.1) | 0.1 (done) | none |
| 1.4 | **done**, on `main` (`aefa006`, with 3.2): `Client::history_page` | 1.2 | `yandex-proto/src/client.rs` |
| 2.1 | **done**, on `main` (integ merge `06f58ba`) | 1.1 | `model.rs`, `fixtures.rs`, chat-core `client.rs`, `store.rs`, `tests.rs`, `yandex_backend.rs` |
| 2.2 | **done**, on `main` (integ merge `285cd05`) | 2.1 (done) | `model.rs`, `backend.rs`, `cache.rs` (test) |
| 2.3 | **done**, on `main` (integ merge `bbfc767`); holds no file any more | 2.2 (done) | chat-core `client.rs`, `fixtures.rs` |
| 2.4 | **needed** (0.2: Q4 = B, a flag-only change sets `LastEditTimestamp` and keeps `Version` 1, so the row would show «edited»); **in flight** on branch `pins/2.4` (worktree `../bt-pins-2.4`) | 0.2 (done) | chat-core `client.rs` |
| 1.5 (new, `pins/0.2` calls it 1.4) | **pending**, not started: the wire `at` fix. `PushEvent::PinChanged.at` must come from `ServerMessageInfo.LastEditTimestamp` (fallback `Version`), not `ServerMessageInfo.Timestamp` (the pinned message's timestamp, absent on unpin); rebuild `research-push-pin*.json` as sanitised live captures; PIN 2.3 and 3.2 tests use the same `at` meaning. Not in `tasks.md` on `main` yet (it arrives with `pins/0.2`) | 0.2 (done), 1.2 | `wire.rs`, `tests/wire_fixtures.rs`, the push fixtures |
| 3.1 | **done**, on `main` (integ merge `1444702`) | 1.1, 2.1 (done) | `yandex_backend.rs` (test hunks) |
| 3.2 | **done**, on `main` (integ merge `e4c2084`; implementation `aefa006`, with 1.4) | 1.2, 1.4, 2.2, 3.1 | `yandex_backend.rs` |
| 3.3 | open, ready (3.2 is on `main`); implementation is not blocked on the live check, the task closes after live 6.2 | 3.2 | `write.rs`, `backend.rs`, `client.rs`, `yandex_backend.rs`, `bt-probe.rs` |
| 4.1 | **done**, on `main` (integ merge `fb51f01`) | 2.2 (done) | `store.rs`, `search_results.rs`, `tests.rs` (helper) |
| 4.2 | **done**, on `main` (integ merge `5f03cff`) | 2.1 (done) | `timeline.rs`, `theme.rs` |
| 4.3 | **done**, on `main` (integ merge `965cdc8`) | 4.1 | `desktop.rs` |
| 4.4 | **done**, on `main` (integ merge `9b42f4b`); its section follows REV's `// ---- reveal in Finder (REV 4.1) ----` in `tests.rs` | 4.1, 4.3 | `tests.rs` |
| 5.1 | **done**, on `main` (`7695968`) | 1.1 | `bt-probe.rs` |
| 5.2 | **done**, on `main` (integ merge `ce2d7ab`) | 1.2 | `bt-probe.rs` |
| 6.1 | **done** on `main` `270d503` (fmt green; build 0 warnings; tests 445 + 279 + 200 + 31 + 4 + 19 green; `openspec validate add-pins-and-importance-view --strict` valid); clippy: 10 pre-existing warnings, baseline per this file, accepted as "no new warnings over the baseline" by the main session on 2026-11-27 | PIN-app (merged) | build/test/lint matrix |
| 6.2 | blocked-by 3.3 and a free live slot (the user is no longer needed: the web client is the other party); **[live]**; one at a time (rule 5) | 3.3 | live check; `BT_WRITE_ONLY_CHAT` set; the only importance frame is the edit's `IsImportant: true` (Q6) |
| 6.3 | blocked-by 2.4, the wire `at` fix, 3.3 and 6.2: archive after they close; sync the specs | 2.4, `at` fix, 3.3, 6.2 | this file, archive |

Merge note (2026-11-27): PIN-app was merged into `main` as `270d503` (`--no-ff` from `integ/pins-and-importance-view` `e4c2084`). The only conflict was `crates/desktop/src/tests.rs` (REV 4.1's section and PIN 4.4's section appended at the same place); both sets are kept whole (REV first, PIN after). REV 4.1 has no `Message { .. }` literal, so no `important: false` was needed there; `store.rs` and `timeline.rs` merged without conflict. `integ/pins-and-importance-view` is kept for the `pins/0.2` branch (based on integ `1dd26f2`); delete it after `pins/0.2` is merged (docs only). **Numbering clash to fix when `pins/0.2` merges:** it adds a task "1.4" (the wire `at` fix) to PIN `tasks.md`, but `1.4` on `main` is the done `Client::history_page` task; renumber the new one to 1.5 in `pins/0.2` before merging.

## Ready now

- Startable without the user, no unmet dependency (rule 1 conflicts listed): **CM 6.3** (`backend.rs`, `model.rs`, chat-core `client.rs`, `store.rs`, `yandex_backend.rs`; CM 6.2 is done) is startable, but it overlaps **PIN 3.3** (`write.rs`, `backend.rs`, `client.rs`, `yandex_backend.rs`, `bt-probe.rs`) on `backend.rs`, `client.rs`, `yandex_backend.rs`, and **PIN 2.4** (`client.rs`, branch `pins/2.4` in flight) on `client.rs`: only one of CM 6.3 and PIN 3.3 may start, the other waits until the first merges (rule 1; neither is a merge unit with the other). CM 6.4 follows 6.3 (`new_chat.rs`, `desktop.rs`, `tests.rs`; no overlap with PIN 3.3). PIN: 2.4 (in flight on `pins/2.4`), the wire `at` fix (new, see the PIN table; `wire.rs`, `tests/wire_fixtures.rs`), 3.3.
- RL: 8.0 (refusal reason) is implemented on `reactors/refusal` and in review; the review requires a fix first (a leak in `bt-probe`), so it is not mergeable yet; RL 8.2 is a live read, startable after the current live task finishes (rule 5); RL 8.3 (archive) waits for 8.2 and 8.0.
- CM: 6.2 is done; 5.1 and 5.3 are blocked on the user (target); 6.F does not run (6.2 PASS); 7.1 needs 6.4.

## Needs the user

- Approval of the target: CM 6.2 is done (2026-11-27). **CM 5.1 and 5.3 need the user**: they need a target with an existing private chat (the web client cannot delete or recreate a private chat, so a creation test has no target without one); the user names the target or decides to close CM without them.
- PIN 0.2 is done (the web client was the other party, no user action needed) and, per the main session, **PIN 6.2 no longer needs the user**: the web client is the other party. The only write stays the edit of the app account's own message under `BT_WRITE_ONLY_CHAT`.
- The VM's outbound network works again (the 2026-11-26 failure was an environment fault, `add-reaction-list/research.md`); RL 8.2 and PIN 0.1 are no longer blocked by it. What remains is rule 5: one live task at a time. `WebLive` runs the live tasks now.
