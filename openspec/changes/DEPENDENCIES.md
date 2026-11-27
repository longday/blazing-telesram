# Task dependency graph of open changes

Owner: Integrator. Updated: 2026-11-27. Four changes are open: **CM** `add-chat-members-and-creation`, **RL** `add-reaction-list`, **PIN** `add-pins-and-importance-view`, **REV** `add-reveal-downloaded-file`. Source: their `tasks.md`.

`[live]` = needs the signed-in session in `.runtime` and the real account (reads only unless noted). `[approval]` = needs the user's explicit approval of the target first.

## Archived

| Change | Archive | Main specs |
|--------|---------|------------|
| `add-global-search` (GS) | `openspec/changes/archive/2026-11-25-add-global-search` | `message-search` created, `chat-list` updated |
| `add-media-download` (MD) | `openspec/changes/archive/2026-11-25-add-media-download` | `attachment-download` and `message-history` synced |
| `restore-runtime-io-test` | `openspec/changes/archive/2026-11-26-restore-runtime-io-test` | none (test only) |
| `add-message-reactions` (MR) | `openspec/changes/archive/2026-11-26-add-message-reactions` | `message-reactions` created; `live-updates`, `message-editing`, `message-sending`, `offline-cache` updated |

## Rules

1. **One owner per file, at task granularity.** A task may start only when no other in-flight task edits any of its files; "different hunks" and "additive" do not exempt. "In flight" means started and not yet merged to `main`. Inside a merge unit (rule 2) a task stops holding its files once it is merged into the unit's integration branch (`integ/<change>`).
2. **Every merge to `main` compiles with no stubs and no no-op arms** (`cargo check --workspace --all-targets`; `cargo test --workspace` green). A task that cannot compile on its own is merged only inside a *merge unit* with the tasks that complete it.
3. **Zero build warnings** (`cargo build --workspace --all-targets`; no CI and no `-D warnings`, but `main` builds with 0 warnings). `desktop` is a binary crate, so a store method, field or view model with no caller is a `dead_code` warning. A unit therefore contains the first consumer of everything it adds; `#[allow(dead_code)]` is not used.
4. **Cargo, `bt-probe` and the app run only in the Tart VM `bt-vm`, never on the host** (rule `containers.md`). Procedure: sync the tree to test into `~/bt/src-<name>` (tracked and untracked, no ignored files; `~/bt/src` is shared, so concurrent agents use their own directory), put `$HOME/.cargo/bin` in `PATH` (`tart exec` lacks it), build and run there; `.runtime` for live runs is the VM's `~/bt/.runtime`. Remove the `~/bt/src-<name>` directory when the task is done. `git` and `openspec` on the host are fine. How to start the VM and the full workflow: [docs/vm.md](../../docs/vm.md).
5. **Live-account tasks share one session and run one at a time**: CM 5.1, 5.3, 6.2; RL 3.1, 8.2; PIN 0.1, 0.2, 6.2 (reads only, except PIN 6.2's one self-chat edit). Writes to the account need the user's approval of the target; every `bt-probe` write command obeys `BT_WRITE_ONLY_CHAT`, and `create-*` is refused whenever it is set, so the creation runs need it **unset**.
6. **A task that is not started locks nothing.** Rule 1 holds files only for started, unmerged tasks. Tasks gated on approvals, live runs or open questions (for example CM 6.3, 6.4, 7.1) hold no file until the moment they start; whoever starts a file first keeps it until its unit merges, the other waits.

## add-chat-members-and-creation (CM)

Done: 0.1, 1.1, 1.2, 1.3, 2.1, 2.2, 3.1, 3.2, 3.3, 4.1, 4.2, 4.3, 5.2, 6.1 and the review fixes (offline views, probe write guard). Open: the `[approval]` gates 5.1, 5.3, 6.2 and what depends on them.

| ID | Status | Needs | Files / notes |
|----|--------|-------|---------------|
| 5.1 | open, ready (2.2 merged); not a file task; **[approval] [live]**: real private chat creation with `bt-probe create-private`, the user approves the target guid; `BT_WRITE_ONLY_CHAT` unset; deferred by the user | 2.2 | **[approval] [live]** |
| 5.3 | blocked-by CM 5.1 (approval); 5.2 is done | 5.1, 5.2 | **[approval] [live]** |
| 6.2 | open, ready; **[approval] [live]**: real group creation (throwaway title, participants the user approves); live gate for 6.3, 6.4; `BT_WRITE_ONLY_CHAT` unset; deferred by the user | 6.1 | **[approval] [live]** |
| 6.3 | blocked-by CM 6.2 (live PASS); 3.3 is merged | 6.2, 3.3 | unit CM-G (with 6.4) |
| 6.4 | blocked-by CM 6.3 (4.3 is merged) | 6.3, 4.3 | unit CM-G |
| 6.F | conditional | 0.1 FAIL or 6.2 FAIL | removes group wording and code |
| 7.1 | blocked-by CM 6.4 (or 6.F) | | |

Unit **CM-G** = 6.3 + 6.4 (rule 3: the backend and store methods of 6.3 are first called by the dialog of 6.4). Files: `backend.rs`, `model.rs`, chat-core `client.rs`, `store.rs`, `yandex_backend.rs`, `new_chat.rs`, `desktop.rs`, `tests.rs`. Gate: live PASS of 6.2. 6.3, 6.4 and 7.1 are not started and gated on approvals, so by rule 6 they lock none of these files: RL and PIN may use them now; if CM-G starts while an RL or PIN unit holds a file, it waits for that unit to merge.

## add-reaction-list (RL)

Done: 1.1, 1.2, 1.3, 2.1 (RL-proto), 4.1, 4.2, 5.1, 6.1-6.5, 6.7 (RL-app) — all on `main` (RL-app merged 2026-11-27 as `b85d478`; build 0 warnings, `cargo test --workspace` 402 + 240 + 190 + 30 + 4 + 16 green, clippy at the 11-warning baseline, `openspec validate --all --strict` 17/17), 6.6 (no work: flat list), 7.1, 8.1 (final full VM run passed on `main` `3bf6937`; checkbox merged to `main`). All open questions (Q1-Q6) were answered on 2026-11-26. Units: **RL-proto** = 1.1, 1.2, 1.3, 2.1 and **RL-app** = 4.1, 4.2, 5.1, 6.1-6.5, 6.7, both on `main`. 3.1 and 8.2 are live reads in the VM (no write, no approval); they are still blocked: the VM has no outbound network (the host's full-tunnel VPN breaks the VM NAT).

| ID | Status | Needs | Files |
|----|--------|-------|-------|
| 3.1 | blocked: the VM network; **[live]** | 2.1 (done) | `research.md`, the fixture |
| 8.1 | **done**, on `main` | 6.7 (done) | none |
| 8.2, 8.3 | 8.2 blocked: the VM network **[live]**; 8.3 (archive) blocked-by 8.2 | 8.1 (done) | `research.md`, archive |

## add-pins-and-importance-view (PIN)

Done: 1.1 and 5.1 (on `main`; 5.1 is `7695968`), 1.2 (merged 2026-11-27 into `integ/pins-and-importance-view` as `b48b666`; reaches `main` with PIN-app, rule 3: 3.2 is its first consumer), 5.2 (same integ branch, `ce2d7ab`), 2.1 (merged `--no-ff` as `06f58ba`, 2026-11-27; implementation commit `97c9e2c`) and 4.2 (merged `--no-ff` as `5f03cff`, 2026-11-27; implementation commit `29c171b`, review approved; the important marker in the bubble footer in `timeline.rs`, `theme.rs`). Integ head `5f03cff` carries a bridge commit (`351de8f`): the adapter did not compile against 1.2's wire (`PushEvent::PinChanged`, `WireChat.pinned`/`pin_action_ts`), so `PinChanged` is matched together with `ChatChanged` (refetch of the row, the behaviour before 1.2) and the three test `WireChat` literals set both fields to `None`; PIN 3.2 replaces that arm with the real mapping. Integ checks in the VM (on `351de8f`): `cargo fmt --all --check` green, `cargo build --workspace --all-targets --locked --offline` 0 warnings, `cargo test --workspace --locked --offline` 402 + 242 + 199 + 31 + 4 + 18 green, clippy at the 11-warning baseline, `openspec validate add-pins-and-importance-view --strict` valid; on `5f03cff` (after 4.2): `cargo fmt --all --check` green, `cargo build --workspace --all-targets --locked --offline` 0 warnings, `cargo test -p blazing-telesram --locked --offline` 403 passed. Q1, Q2, Q3, Q5, Q6 were answered on 2026-11-26; only **Q4** is open (decided by observation in 0.2; it blocks 2.4 only). Units: **PIN-app** = 1.2, (1.3), 2.1-2.3, 3.1, 3.2, 4.1-4.4, 5.2 (1.2 adds a `PushEvent` variant matched exhaustively by the adapter; 3.2 is its first consumer); 5.1 merged alone; 3.3 (edit flag, Q6) is its own unit after PIN-app and closes with the live check 6.2. RL-app is on `main`, so PIN's tasks on files shared with RL are no longer queued behind it.

| ID | Status | Needs | Files / notes |
|----|--------|-------|---------------|
| 0.1 | blocked: VM network; **[live]** reads only | | `research.md` |
| 0.2 | blocked-by 0.1's network and the user at the web client; **[live]** | 0.1 | `research.md`; decides Q4 and checks the Q5 timestamps |
| 1.2 | **done**, merged into `integ/pins-and-importance-view` (`b48b666`); holds no file any more (rule 1, merged into the integration branch) | 1.1 | `wire.rs`, `tests/wire_fixtures.rs`, `xiva.rs` |
| 1.3 | conditional on 0.1; after RL 1.1 (same `request.rs`) | 0.1 | `request.rs` |
| 2.1 | **done**, merged into `integ/pins-and-importance-view` (`06f58ba`); holds no file any more (rule 1): chat-core `client.rs`, `model.rs`, `fixtures.rs`; desktop `store.rs`, `tests.rs`, `yandex_backend.rs` are released | 1.1 | `model.rs`, `fixtures.rs`, chat-core `client.rs`, `store.rs`, `tests.rs`, `yandex_backend.rs` |
| 2.2 | **done**, merged into `integ/pins-and-importance-view` (`285cd05`, implementation `8734360`); holds no file any more (rule 1): `model.rs`, `backend.rs`, `cache.rs` are released | 2.1 (done) | `model.rs`, `backend.rs`, `cache.rs` (test) |
| 2.3 | **in flight** (also owns `fixtures.rs`) | 2.2 (done) | chat-core `client.rs`, `fixtures.rs` |
| 2.4 | blocked-by Q4 (0.2) | 0.2 | chat-core `client.rs` |
| 3.1 | **done**, merged into `integ/pins-and-importance-view` (`1444702`); holds no file any more (rule 1): `yandex_backend.rs` is released | 1.1, 2.1 (done) | `yandex_backend.rs` (test hunks) |
| 3.2 | **in flight** (3.1 and 2.2 are in integ; 1.2 is done in integ) | 1.2, 2.2, 3.1 | `yandex_backend.rs` (replaces the interim `PinChanged` arm) |
| 3.3 | blocked-by 3.2 (5.2 is done in integ); closes with live 6.2 | | `write.rs`, `backend.rs`, `client.rs`, `yandex_backend.rs`, `bt-probe.rs` |
| 4.1 | **in flight** (also owns `search_results.rs` and the `chat()` helper in `tests.rs`) | 2.2 (done) | `store.rs`, `search_results.rs`, `tests.rs` (helper) |
| 4.2 | **done**, merged into `integ/pins-and-importance-view` (`5f03cff`); holds no file any more (rule 1): `timeline.rs` and `theme.rs` are released | 2.1 (done) | `timeline.rs`, `theme.rs` |
| 4.3 | blocked-by 4.1 only (4.2 is done in integ); starts after 4.1 | 4.1 | `desktop.rs` |
| 4.4 | blocked-by 4.1 and 4.3 (4.2 is done in integ); REV 4.1 is on `main`, so 4.4 appends after REV's section `// ---- reveal in Finder (REV 4.1) ----` | 4.1, 4.3 | `tests.rs` |
| 5.1 | **done**, on `main` (`7695968`) | 1.1 | `bt-probe.rs` |
| 5.2 | **done**, merged into `integ/pins-and-importance-view` (`537f7eb`, merge `ce2d7ab`); holds no file any more (rule 1) | 1.2 | `bt-probe.rs` |
| 6.1 | after the units | | build/test/lint matrix |
| 6.2 | blocked: network and the user's approval of the target; **[approval] [live]**; one at a time (rule 5) | 3.3 | live check; `BT_WRITE_ONLY_CHAT` set; the only importance frame is the edit's `IsImportant: true` (Q6) |
| 6.3 | after the units | | this file, archive |

Notes for the PIN integrator (`integ/pins-and-importance-view`): 2.2 is merged into integ (`285cd05`; releases `model.rs`, `backend.rs`, `cache.rs`) and 3.1 too (`1444702`; releases `yandex_backend.rs`). In flight: 2.3 (`client.rs`, also owns `fixtures.rs`), 3.2 (`yandex_backend.rs`), 4.1 (`store.rs`, `search_results.rs`, and the `chat()` helper in `tests.rs`); integ-pins intentionally does not build until all three land. When the integ branch absorbs `main` (REV is on it): REV 4.1 added no `Message { .. }` literal to `tests.rs` (checked on `main` `9d6406f`: the only literal is the pre-existing helper `message(..)` at line 78, which PIN 2.1 already extended with `important: false` in integ), so no new line needs `important: false`; re-check with a grep for `Message {` after the merge, and `store.rs`/`timeline.rs`/`viewer.rs`/`main.rs` merge cleanly only if PIN 4.1 and 4.2 have not touched REV's hunks (`reveal_download`, `action_row`, `ActionLine.reveal`).

## add-reveal-downloaded-file (REV)

Proposed 2026-11-27. Local action only: no protocol, core or server change (decisions in its `design.md`). One merge unit **REV** = 1.1, 2.1, 2.2, 3.1, 4.1 (rule 3: the store method and event are first called by the views and `main.rs`). **The REV unit is on `main`** (merge `4b1eaa7`, 2026-11-27; the integration branch `integ/reveal-downloaded-file` and the branch `rev/4.1` are deleted locally and on origin, their worktrees removed). 5.1 final run is done on `main` (details in its row); 5.2 hand check in the VM (**[live]**, not a gate) is blocked by the VM network; 5.3 archive follows 5.2 or the user's decision to archive without it. The D3 note that `reveal_download` calls `cx.notify()` after clearing a deleted file is in `design.md`. Nothing of REV holds a file any more (rule 1).

| ID | Status | Needs | Files / notes |
|----|--------|-------|---------------|
| 1.1 | **done**, merged into `integ/reveal-downloaded-file` (`96772e9`); holds no file any more (rule 1: `store.rs` is released) | | `store.rs` |
| 2.1 | **done**, merged into `integ/reveal-downloaded-file` (`764a891`); holds no file any more (rule 1: `viewer.rs` is released) | 1.1 (done) | `viewer.rs` |
| 2.2 | **done**, merged into `integ/reveal-downloaded-file` (`df3ce5c`); holds no file any more (rule 1: `timeline.rs` is released) | 1.1, 2.1 (done) | `timeline.rs` |
| 3.1 | **done**, merged into `integ/reveal-downloaded-file` (`72bf5fa`); holds no file any more (rule 1: `main.rs` is released) | 1.1 | `main.rs` |
| 4.1 | **done**, on `main` (implementation `8a7c29c`, review approved; merged into integ and then `main` `--no-ff`, 2026-11-27); 358 lines added to `tests.rs`, none of them a `Message { .. }` literal | PIN 2.1 (done) | `tests.rs` |
| 5.1 | **done**, on `main`: REV unit merge `4b1eaa7`, 5.1 checkbox `9d6406f`. In `bt-vm` on that tree: `cargo fmt --all --check` green; `cargo build --workspace --all-targets --locked --offline` 0 warnings; `cargo test --workspace --locked --offline` 417 + 240 + 190 + 30 + 4 + 16 passed, 0 failed; `cargo clippy --workspace --all-targets --locked --offline` at the 11-warning baseline (6 large-size-difference, 2 deref, 2 collapsible `if`, 1 large `Err`); `openspec validate add-reveal-downloaded-file --strict` valid | the REV unit (done) | build/test/lint matrix |
| 5.2 | blocked: the VM has no outbound network (needs the network and a signed-in session; **[live]**; the download reads the server, writes only to the VM's `~/Downloads`; one live task at a time, rule 5) | 5.1 (done) | none (result in the task report) |
| 5.3 | blocked-by 5.2, or by the user's decision to archive with 5.2 reported "blocked by environment" (5.2 is not a gate for archiving, `tasks.md`) | 5.1 (done) | this file, archive |

Unit **REV** shares files with: PIN-app (`tests.rs`: PIN 4.4, not started, locks nothing; `store.rs` 4.1 is free for PIN now, REV released it; `timeline.rs` 4.2 is done and merged into the PIN integ branch) and CM-G (`tests.rs`; not started, locks nothing, rule 6). RL-app is on `main` and holds nothing. It does not touch `desktop.rs`, `composer.rs`, `theme.rs`, `reactors.rs`, `yandex_backend.rs`, chat-core or `yandex-proto`.

## Ready now

- Startable without the user, no unmet dependency, pairwise disjoint files (rule 1): none new. In flight in the PIN unit: **PIN 2.3** (`client.rs`, `fixtures.rs`), **PIN 3.2** (`yandex_backend.rs`), **PIN 4.1** (`store.rs`, `search_results.rs`, the `chat()` helper in `tests.rs`); then PIN 4.3 (after 4.1) and 4.4 (after 4.1 and 4.3). The REV unit is on `main`; nothing of REV is startable except 5.2 (blocked by the VM network) and 5.3.
- RL 3.1 and 8.2 stay blocked on the VM network; RL 8.3 (archive) waits for 8.2.
- CM has nothing startable without the user: CM 7.1 needs 6.4 (or 6.F); the archive step needs 5.1, 5.3 and 6.2.

## Needs the user

- Approval of the target: CM 5.1, CM 5.3, CM 6.2 (real chat and group creation); deferred by the user, so these three and 6.3, 6.4, 7.1 stay open.
- PIN 0.2 and 6.2: the user performs pins, importance marks and an own-message mark in the web client on a disposable self-chat message and approves the target (the agent sends only the edit of 6.2, under `BT_WRITE_ONLY_CHAT`).
- RL 3.1 and 8.2, PIN 0.1 and REV 5.2 need the VM's outbound network (the host's full-tunnel VPN breaks the VM NAT), not a user decision.
