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
| 5.1 | **blocked on the user**: needs a target with **no** existing private chat with the app account; the web client cannot delete a private chat, so the one target used so far is spent (a repeat only returns the existing chat); the shapes observed so far are in `design.md` "Observed shapes"; unticked; **[approval] [live]** | 2.2 | **[approval] [live]** |
| 5.3 | **blocked on the user**, behind 5.1 (same target problem); 5.2 is done | 5.1, 5.2 | **[approval] [live]** |
| 6.2 | **done** (live PASS, merged 2026-11-27 as part of `17e336b`): the group landed in «Внешние чаты» in the web client, and the spec has no organisation requirement; payload and reply shape in `design.md` "D4 outcome" and "Observed shapes"; live fixtures `live-create-chat.json`, `live-create-private.json` | 6.1 | `design.md`, `tasks.md`, fixtures, `wire_fixtures.rs` |
| 6.3 | **next**, inside unit CM-G (merges to `main` only together with 6.4); 6.2 PASS, 3.3 merged; PIN 2.4 released chat-core `client.rs`; PIN 3.3 starts after CM-G | 6.2 (done), 3.3 | unit CM-G (with 6.4): `backend.rs`, `model.rs`, chat-core `client.rs`, `store.rs`, `yandex_backend.rs` |
| 6.4 | blocked-by CM 6.3 (4.3 is merged) | 6.3, 4.3 | unit CM-G: `new_chat.rs`, `desktop.rs`, `tests.rs` |
| 6.F | **does not run** (6.2 PASS) | | |
| 7.1 | blocked-by CM 6.4 | 6.4 | any file named above (fixes only) |

Unit **CM-G** = 6.3 + 6.4 (rule 3: the backend and store methods of 6.3 are first called by the dialog of 6.4, so 6.3 alone would be a `dead_code` warning and does not merge into `main`). Files: `backend.rs`, `model.rs`, chat-core `client.rs`, `store.rs`, `yandex_backend.rs`, `new_chat.rs`, `desktop.rs`, `tests.rs`. The gate (live PASS of 6.2) is met. PIN 2.4 is merged and released chat-core `client.rs`, so CM-G is **next** on it; PIN 3.3 (`write.rs`, `backend.rs`, `client.rs`, `yandex_backend.rs`, `bt-probe.rs`) starts after **CM-G** (not after 6.3 alone) is on `main`. 7.1 is not started and locks nothing (rule 6).

## add-reaction-list (RL)

Done: 1.1, 1.2, 1.3, 2.1 (RL-proto), 3.1 (live verification, merged `7d97260`), 4.1, 4.2, 5.1, 6.1-6.5, 6.7 (RL-app) — all on `main` (RL-app merged 2026-11-27 as `b85d478`; build 0 warnings, `cargo test --workspace` 402 + 240 + 190 + 30 + 4 + 16 green, clippy at the 11-warning baseline of that time, `openspec validate --all --strict` 17/17), 6.6 (no work: flat list), 7.1, 8.1 (final full VM run passed on `main` `3bf6937`; checkbox merged to `main`), 8.0 (refusal reason; merged `68921eb`). All open questions (Q1-Q6) were answered on 2026-11-26. Units: **RL-proto** = 1.1, 1.2, 1.3, 2.1 and **RL-app** = 4.1, 4.2, 5.1, 6.1-6.5, 6.7, both on `main`. The VM network is back (3.1 ran on 2026-11-27); 8.2 (the manual app check) is in work (`RlAppCheck`, `reactors/8.2`). The clippy baseline of `main` is **10** (after PIN-app `270d503`; unchanged after `68921eb` and `23a226c`).

| ID | Status | Needs | Files |
|----|--------|-------|-------|
| 3.1 | **done**, on `main` (merge `7d97260`, implementation `03dd898`, review approved): live `list_reactions` verified, `Mode: 0` accepted, newest-first order, exclusive `MaxTimestamp`; the server `Limit` cap is not observed (no list above 16 reactors was readable); sanitized live fixture swapped in; findings in `research.md` "Live verification" | 2.1 (done) | `research.md`, `design.md`, the fixture, `wire_fixtures.rs` |
| 8.1 | **done**, on `main` | 6.7 (done) | none |
| 8.2 | **in work** (agent `RlAppCheck`, branch `reactors/8.2`, worktree `../bt-reactors-8.2`): the manual app check in `bt-vm` (`BT_WRITE_ONLY_CHAT` set to the self chat; **[live]** read-only, no approval); one live task at a time (rule 5) | 8.1 (done), 8.0 (done) | `research.md` |
| 8.0 | **done**, merged into `main` as `68921eb` (`--no-ff` from `reactors/refusal` `d6f218b`; two conflicts, `bt-probe.rs` imports and `yandex_backend.rs` tests, both sides kept); the refusal reason reaches the list; checks in `bt-vm`: fmt green, build 0 warnings, tests 449 + 279 + 203 + 32 + 4 + 21 passed, clippy 10 (the baseline), `openspec validate --all --strict` 16/16. Released `wire.rs`, `yandex_backend.rs`, `bt-probe.rs`, `xiva.rs`, `tests/wire_fixtures.rs` | 3.1 (done) | per its `tasks.md` entry |
| 8.3 | blocked-by 8.2 (archive; 8.0 is done) | 8.2 | archive |

## add-pins-and-importance-view (PIN)

Done: 1.1 and 5.1 (on `main`; 5.1 is `7695968`) and **the whole PIN-app unit** (1.2, 1.4, 2.1-2.3, 3.1, 3.2, 4.1-4.4, 5.2; 1.3 dropped), merged into `main` as `270d503` on 2026-11-27 (`--no-ff` from `integ/pins-and-importance-view` `e4c2084`), plus 0.1 (`1dd26f2`), 0.2 (`6e34584`), 2.4 (`23a226c`) and 6.1. Checks on `main` `270d503` in `bt-vm`: `cargo fmt --all --check` green; `cargo build --workspace --all-targets --locked --offline` 0 warnings; `cargo test --workspace --locked --offline` 445 + 279 + 200 + 31 + 4 + 19 passed, 0 failed; `cargo clippy --workspace --all-targets --locked --offline` **10 warnings (the baseline of `main`, was 11: the `model.rs:361` warning is gone; unchanged on `68921eb` and `23a226c`, lines shifted: `xiva.rs:113`, `yandex_backend.rs:366`, `chat-core/src/client.rs:4541`)**: 2 deref (`chat-core/src/client.rs:4541`, `desktop/src/tests.rs:577`), 5 large size difference (`chat-core/src/client.rs:66` and `:164`, `yandex-proto/src/wire.rs:190` and `:295`, `xiva.rs:113`), 1 large `Err` (`yandex-proto/src/ws.rs:130`), 2 collapsible `if` (`desktop/src/desktop/composer.rs:612`, `desktop/src/yandex_backend.rs:366`); `openspec validate --all --strict` 16/16. On `23a226c` (PIN 2.4 merged): fmt green, build 0 warnings, tests 451 + 284 + 203 + 32 + 4 + 21 passed, clippy 10. All questions are answered (Q4 by 0.2 on 2026-11-27: B). Open: 1.5 (the wire `at` fix; in work, `pins/1.5`), 3.3 (edit flag, Q6; starts after unit CM-G (6.3 + 6.4) is on `main`, they share `backend.rs`, `client.rs`, `yandex_backend.rs`; closes with the live check 6.2), 6.2 (live; the bar/marker part needs no user, the Q6 part needs the user), 6.3 (archive).

| ID | Status | Needs | Files / notes |
|----|--------|-------|---------------|
| 0.1 | **done**, on `main` (integ merge `1dd26f2`); live observation recorded, 1.3 dropped | | `research.md` |
| 0.2 | **done**, merged into `main` as `6e34584` (docs only: `research.md`, `design.md`, `proposal.md`, `tasks.md`): the web client was the other party; answers Q4 = B (flag-only change sets `LastEditTimestamp`), Q5 = B (the comparable action time is `LastEditTimestamp`) | 0.1 (done) | `research.md` |
| 1.2 | **done**, on `main` (integ merge `b48b666`) | 1.1 | `wire.rs`, `tests/wire_fixtures.rs`, `xiva.rs` |
| 1.3 | **dropped**: chat-list entries carry `PinnedMessageTs`, so no `request.rs` change is needed (0.1) | 0.1 (done) | none |
| 1.4 | **done**, on `main` (`aefa006`, with 3.2): `Client::history_page` | 1.2 | `yandex-proto/src/client.rs` |
| 2.1 | **done**, on `main` (integ merge `06f58ba`) | 1.1 | `model.rs`, `fixtures.rs`, chat-core `client.rs`, `store.rs`, `tests.rs`, `yandex_backend.rs` |
| 2.2 | **done**, on `main` (integ merge `285cd05`) | 2.1 (done) | `model.rs`, `backend.rs`, `cache.rs` (test) |
| 2.3 | **done**, on `main` (integ merge `bbfc767`); holds no file any more | 2.2 (done) | chat-core `client.rs`, `fixtures.rs` |
| 2.4 | **done**, merged into `main` as `23a226c` (`--no-ff` from `pins/2.4` `3a6714e`, review approved; clean merge): a flag-only change (the same body, quote and forward) keeps the cached `edited` (Q4 = B); chat-core `client.rs` and `store.rs` tests; holds no file any more | 0.2 (done) | chat-core `client.rs`, `store.rs` |
| 1.5 | **in work** (agent `PinT15`, branch `pins/1.5`, worktree `../bt-pins-1.5`); RL 8.0 (merged `68921eb`) released `wire.rs`; the wire `at` fix. `PushEvent::PinChanged.at` must come from `ServerMessageInfo.LastEditTimestamp` (fallback `Version`), not `ServerMessageInfo.Timestamp` (the pinned message's timestamp, absent on unpin); rebuild `research-push-pin*.json` as sanitised live captures; PIN 2.3 and 3.2 tests use the same `at` meaning | 0.2 (done), 1.2, RL 8.0 (done) | `wire.rs`, `tests/wire_fixtures.rs`, the push fixtures |
| 3.1 | **done**, on `main` (integ merge `1444702`) | 1.1, 2.1 (done) | `yandex_backend.rs` (test hunks) |
| 3.2 | **done**, on `main` (integ merge `e4c2084`; implementation `aefa006`, with 1.4) | 1.2, 1.4, 2.2, 3.1 | `yandex_backend.rs` |
| 3.3 | open; implementation is not blocked on the live check (the task closes after live 6.2), but it overlaps unit CM-G (`backend.rs`, `client.rs`, `yandex_backend.rs`), so it starts after CM-G (6.3 + 6.4) is on `main` | 3.2 (done), unit CM-G | `write.rs`, `backend.rs`, `client.rs`, `yandex_backend.rs`, `bt-probe.rs` |
| 4.1 | **done**, on `main` (integ merge `fb51f01`) | 2.2 (done) | `store.rs`, `search_results.rs`, `tests.rs` (helper) |
| 4.2 | **done**, on `main` (integ merge `5f03cff`) | 2.1 (done) | `timeline.rs`, `theme.rs` |
| 4.3 | **done**, on `main` (integ merge `965cdc8`) | 4.1 | `desktop.rs` |
| 4.4 | **done**, on `main` (integ merge `9b42f4b`); its section follows REV's `// ---- reveal in Finder (REV 4.1) ----` in `tests.rs` | 4.1, 4.3 | `tests.rs` |
| 5.1 | **done**, on `main` (`7695968`) | 1.1 | `bt-probe.rs` |
| 5.2 | **done**, on `main` (integ merge `ce2d7ab`) | 1.2 | `bt-probe.rs` |
| 6.1 | **done** on `main` `270d503` (fmt green; build 0 warnings; tests 445 + 279 + 200 + 31 + 4 + 19 green; `openspec validate add-pins-and-importance-view --strict` valid); clippy: 10 pre-existing warnings, baseline per this file, accepted as "no new warnings over the baseline" by the main session on 2026-11-27 | PIN-app (merged) | build/test/lint matrix |
| 6.2 | blocked-by 3.3 and a free live slot; **[live]**; one at a time (rule 5). **Split:** the bar/marker part is runnable with the web client as the other party (no user); the **Q6 part needs the user** (a web session of the app account marks a self-chat message important, see "Needs the user") | 3.3 | live check; `BT_WRITE_ONLY_CHAT` set; the only importance frame is the edit's `IsImportant: true` (Q6) |
| 6.3 | blocked-by the wire `at` fix (1.5), 3.3 and 6.2: archive after they close; sync the specs (2.4 is done) | 1.5, 3.3, 6.2 | this file, archive |

Merge note (2026-11-27): PIN-app was merged into `main` as `270d503` (`--no-ff` from `integ/pins-and-importance-view` `e4c2084`). The only conflict was `crates/desktop/src/tests.rs` (REV 4.1's section and PIN 4.4's section appended at the same place); both sets are kept whole (REV first, PIN after). REV 4.1 has no `Message { .. }` literal, so no `important: false` was needed there; `store.rs` and `timeline.rs` merged without conflict. `pins/0.2` was merged into `main` as `6e34584` (docs only; the numbering clash was resolved there: the wire `at` fix is task 1.5, `1.4` stays `Client::history_page`); the branches `integ/pins-and-importance-view` and `pins/0.2` and their worktrees are removed (the integ branch on origin too).

## Ready now

- Startable without the user, no unmet dependency (rule 1 conflicts listed): PIN 2.4 is merged (`23a226c`) and chat-core `client.rs` is free. **Order now: unit CM-G (6.3 + 6.4) → PIN 3.3.** CM 6.3 alone does not release anything: it adds `Backend::create_group_chat` and store methods whose first caller is the dialog of 6.4, so by rules 2 and 3 it cannot merge into `main` without 6.4 and `main` takes CM-G as one unit; the files are released when CM-G merges. CM-G (`backend.rs`, `model.rs`, chat-core `client.rs`, `store.rs`, `yandex_backend.rs`, `new_chat.rs`, `desktop.rs`, `tests.rs`; 6.2 is done) is **next**; PIN 3.3 (`write.rs`, `backend.rs`, `client.rs`, `yandex_backend.rs`, `bt-probe.rs`) overlaps it on `backend.rs`, `client.rs`, `yandex_backend.rs` and starts after CM-G is on `main`. (Merging 6.3 alone into `integ/add-chat-members-and-creation` would release the files only inside the unit by rule 1, not for PIN 3.3, so no such exception is taken.) CM 6.4 has no overlap with PIN 3.3. **PIN 1.5 is in work** (agent `PinT15`, branch `pins/1.5`, worktree `../bt-pins-1.5`; `wire.rs`, `tests/wire_fixtures.rs`, the push fixtures); **RL 8.2 is in work** (see below).
- RL: 8.0 is done (merged `68921eb`); **RL 8.2 is in work** (agent `RlAppCheck`, branch `reactors/8.2`, worktree `../bt-reactors-8.2`; the manual app check in `bt-vm`, `BT_WRITE_ONLY_CHAT` set to the self chat; `research.md` only; one live task at a time, rule 5); RL 8.3 (archive) waits for 8.2.
- CM: 6.2 is done; 5.1 and 5.3 are blocked on the user (target); 6.F does not run (6.2 PASS); 7.1 needs 6.4.

## Needs the user

- Approval of the target: CM 6.2 is done (2026-11-27). **CM 5.1 and 5.3 need the user**: they need a target with **no** existing private chat with the app account (a target that already has one only returns or opens that chat, which is not a creation, and the web client cannot delete a private chat to free the existing target); the user names such a target (a person the app account has never chatted with) or decides to close CM without them. Do not repeat the run against the existing target.
- PIN 0.2 is done (the web client was the other party, no user action needed). **PIN 6.2, bar and marker part:** needs no user, the web client (`admin@sgnl.pro`) is the other party and the app account listens or runs the app. **PIN 6.2, Q6 part: needs the user**: an edit authored by the app account is required, and `bt-probe edit-self` refuses any chat but the self chat, so the user marks a message of the self chat as important in a web session **of the app account** (the web client of `admin@sgnl.pro` cannot, it is another account); then `edit-self <ts> <text>` with and without `--important` and `history` (task 6.2 text; `research.md` 0.2 note on Q6).
- The VM's outbound network works again (the 2026-11-26 failure was an environment fault, `add-reaction-list/research.md`); RL 8.2 and PIN 0.1 are no longer blocked by it. What remains is rule 5: one live task at a time. `WebLive` runs the live tasks now.
