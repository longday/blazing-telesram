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
4. **Cargo, `bt-probe` and the app run only in the Tart VM `bt-vm`, never on the host** (rule `containers.md`). Procedure: sync the tree to test into `~/bt/src-<name>` (tracked and untracked, no ignored files; `~/bt/src` is shared, so concurrent agents use their own directory), put `$HOME/.cargo/bin` in `PATH` (`tart exec` lacks it), build and run there; `.runtime` for live runs is the VM's `~/bt/.runtime`. Remove the `~/bt/src-<name>` directory when the task is done. `git` and `openspec` on the host are fine.
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

Done: 1.1 and 5.1 (on `main`; 5.1 is `7695968`), 1.2 (merged 2026-11-27 into `integ/pins-and-importance-view` as `b48b666`; reaches `main` with PIN-app, rule 3: 3.2 is its first consumer) and 5.2 (in the same integ branch, `ce2d7ab`, head). Q1, Q2, Q3, Q5, Q6 were answered on 2026-11-26; only **Q4** is open (decided by observation in 0.2; it blocks 2.4 only). Units: **PIN-app** = 1.2, (1.3), 2.1-2.3, 3.1, 3.2, 4.1-4.4, 5.2 (1.2 adds a `PushEvent` variant matched exhaustively by the adapter; 3.2 is its first consumer); 5.1 merged alone; 3.3 (edit flag, Q6) is its own unit after PIN-app and closes with the live check 6.2. RL-app is on `main`, so PIN's tasks on files shared with RL are no longer queued behind it.

| ID | Status | Needs | Files / notes |
|----|--------|-------|---------------|
| 0.1 | blocked: VM network; **[live]** reads only | | `research.md` |
| 0.2 | blocked-by 0.1's network and the user at the web client; **[live]** | 0.1 | `research.md`; decides Q4 and checks the Q5 timestamps |
| 1.2 | **done**, merged into `integ/pins-and-importance-view` (`b48b666`); holds no file any more (rule 1, merged into the integration branch) | 1.1 | `wire.rs`, `tests/wire_fixtures.rs`, `xiva.rs` |
| 1.3 | conditional on 0.1; after RL 1.1 (same `request.rs`) | 0.1 | `request.rs` |
| 2.1 | ready (RL-app is on `main`); **owns every file with a `Message` literal** (a new field breaks all of them): chat-core `client.rs`, `model.rs`, `fixtures.rs`; desktop `store.rs` (test literal), `search_results.rs`, `yandex_backend.rs`, `tests.rs`. The `store.rs` literal is a test hunk; REV 1.1 already released `store.rs` (merged into `integ/reveal-downloaded-file`), `tests.rs` is held by PIN 2.1 until it merges into `integ/pins-and-importance-view` | 1.1 | the files listed in the note |
| 2.2 | blocked-by 2.1 | 2.1 | `model.rs`, `backend.rs`, `cache.rs` (test) |
| 2.3 | blocked-by 2.2 (1.2 is done in integ) | 2.2 | chat-core `client.rs` |
| 2.4 | blocked-by Q4 (0.2) | 0.2 | chat-core `client.rs` |
| 3.1 | blocked-by 2.1 | 1.1, 2.1 | `yandex_backend.rs` |
| 3.2 | blocked-by 2.2, 3.1 (1.2 is done in integ) | 1.2, 2.2, 3.1 | `yandex_backend.rs` |
| 3.3 | blocked-by 3.2 (5.2 is done in integ); closes with live 6.2 | | `write.rs`, `backend.rs`, `client.rs`, `yandex_backend.rs`, `bt-probe.rs` |
| 4.1 | blocked-by 2.2 | 2.2 | `store.rs` |
| 4.2 | blocked-by 2.1 only (REV 2.2 is merged into `integ/reveal-downloaded-file`, `timeline.rs` is released) | 2.1 | `timeline.rs`, `theme.rs` |
| 4.3 | blocked-by 4.1 | 4.1 | `desktop.rs` |
| 4.4 | blocked-by 4.1-4.3 | | `tests.rs` |
| 5.1 | **done**, on `main` (`7695968`) | 1.1 | `bt-probe.rs` |
| 5.2 | **done**, merged into `integ/pins-and-importance-view` (`537f7eb`, merge `ce2d7ab`); holds no file any more (rule 1) | 1.2 | `bt-probe.rs` |
| 6.1 | after the units | | build/test/lint matrix |
| 6.2 | blocked: network and the user's approval of the target; **[approval] [live]**; one at a time (rule 5) | 3.3 | live check; `BT_WRITE_ONLY_CHAT` set; the only importance frame is the edit's `IsImportant: true` (Q6) |
| 6.3 | after the units | | this file, archive |

## add-reveal-downloaded-file (REV)

Proposed 2026-11-27. Local action only: no protocol, core or server change (decisions in its `design.md`). One merge unit **REV** = 1.1, 2.1, 2.2, 3.1, 4.1 (rule 3: the store method and event are first called by the views and `main.rs`; RL-app is on `main`); 5.1 final run, 5.2 hand check in the VM (**[live]**, not a gate), 5.3 archive. Integration branch `integ/reveal-downloaded-file`: **1.1, 2.1, 2.2 and 3.1 are merged into it** (1.1 `96772e9`, 3.1 `72bf5fa`, 2.1 `764a891`, 2.2 `df3ce5c`, head `5df8abc`, 2026-11-27). Checks on the integ tree in the VM: `cargo fmt --all --check` green, `cargo build --workspace --all-targets --locked --offline` 0 warnings, `cargo test -p blazing-telesram --locked --offline` 403 passed, `openspec validate add-reveal-downloaded-file --strict` valid. The unit now compiles warning-free; it reaches `main` with 4.1 (tests) and 5.1. The D3 note that `reveal_download` calls `cx.notify()` after clearing a deleted file is in `design.md`. REV has no urgency over RL, PIN and CM: each of its tasks starts when its file is free (rule 1).

| ID | Status | Needs | Files / notes |
|----|--------|-------|---------------|
| 1.1 | **done**, merged into `integ/reveal-downloaded-file` (`96772e9`); holds no file any more (rule 1: `store.rs` is released) | | `store.rs` |
| 2.1 | **done**, merged into `integ/reveal-downloaded-file` (`764a891`); holds no file any more (rule 1: `viewer.rs` is released) | 1.1 (done) | `viewer.rs` |
| 2.2 | **done**, merged into `integ/reveal-downloaded-file` (`df3ce5c`); holds no file any more (rule 1: `timeline.rs` is released) | 1.1, 2.1 (done) | `timeline.rs` |
| 3.1 | **done**, merged into `integ/reveal-downloaded-file` (`72bf5fa`); holds no file any more (rule 1: `main.rs` is released) | 1.1 | `main.rs` |
| 4.1 | blocked-by PIN 2.1: wait for it to merge into `integ/pins-and-importance-view` (PIN 2.1 owns every file with a `Message` literal, `tests.rs` included) and then PIN 4.4 (same `tests.rs`); 1.1, 2.1, 2.2 are done; RL-app's 6.7 is on `main` | PIN 2.1, PIN 4.4 | `tests.rs` |
| 5.1 | blocked-by the REV unit on `main` | | build/test/lint matrix |
| 5.2 | blocked-by 5.1, the VM network and a session; **[live]** (the download reads the server, writes only to the VM's `~/Downloads`; one live task at a time, rule 5) | 5.1 | none (result in the task report) |
| 5.3 | blocked-by 5.1 (and 5.2 or its "blocked by environment" report) | | this file, archive |

Unit **REV** shares files with: PIN-app (`tests.rs` 4.4; `store.rs` 4.1 and `timeline.rs` 4.2 are free for PIN now, REV released both) and CM-G (`tests.rs`; not started, locks nothing, rule 6). RL-app is on `main` and holds nothing. It does not touch `desktop.rs`, `composer.rs`, `theme.rs`, `reactors.rs`, `yandex_backend.rs`, chat-core or `yandex-proto`.

## Ready now

- Startable without the user, no unmet dependency and no file overlap (rule 1): **PIN 2.1** (every file with a `Message` literal, see its row); it unlocks PIN 2.2, 4.2 and REV 4.1's `tests.rs`. REV 4.1 is the only REV task left before 5.1.
- RL 3.1 and 8.2 stay blocked on the VM network; RL 8.3 (archive) waits for 8.2.
- CM has nothing startable without the user: CM 7.1 needs 6.4 (or 6.F); the archive step needs 5.1, 5.3 and 6.2.

## Needs the user

- Approval of the target: CM 5.1, CM 5.3, CM 6.2 (real chat and group creation); deferred by the user, so these three and 6.3, 6.4, 7.1 stay open.
- PIN 0.2 and 6.2: the user performs pins, importance marks and an own-message mark in the web client on a disposable self-chat message and approves the target (the agent sends only the edit of 6.2, under `BT_WRITE_ONLY_CHAT`).
- RL 3.1 and 8.2, PIN 0.1 and REV 5.2 need the VM's outbound network (the host's full-tunnel VPN breaks the VM NAT), not a user decision.
