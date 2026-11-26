# Task dependency graph of open changes

Owner: Integrator. Updated: 2026-11-26. Three changes are open: **CM** `add-chat-members-and-creation`, **RL** `add-reaction-list`, **PIN** `add-pins-and-importance-view`. Source: their `tasks.md`.

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

Done: 1.1, 1.2, 1.3, 2.1 (RL-proto, on `main`), 4.1, 4.2, 5.1, 6.1-6.5 (merged into `integ/reaction-list-app`, head `6a9205a`; 6.5 merged 2026-11-27, build 0 warnings, `cargo test --workspace` 389 + 240 + 190 + 30 + 4 + 16 green, clippy at the 11-warning baseline), 6.6 (no work: flat list), 7.1. All open questions (Q1-Q6) were answered on 2026-11-26. Units: **RL-proto** = 1.1, 1.2, 1.3, 2.1 (on `main`); **RL-app** = 4.1, 4.2, 5.1, 6.1-6.5, 6.7 (in progress on `integ/reaction-list-app`; merges to `main` after 6.7). 3.1 and 8.2 are live reads in the VM (no write, no approval); they are still blocked: the VM has no outbound network (the host's full-tunnel VPN breaks the VM NAT).

| ID | Status | Needs | Files |
|----|--------|-------|-------|
| 3.1 | blocked: the VM network; **[live]** | 2.1 (done) | `research.md`, the fixture |
| 6.7 | ready (6.1-6.5 are merged into integ) | | `tests.rs` |
| 8.1, 8.2, 8.3 | blocked-by 6.7 (and the VM network for 8.2 **[live]**) | | final run, `research.md`, archive |

## add-pins-and-importance-view (PIN)

Done: 1.1. Q1, Q2, Q3, Q5, Q6 were answered on 2026-11-26; only **Q4** is open (decided by observation in 0.2; it blocks 2.4 only). Units: **PIN-app** = 1.2, (1.3), 2.1-2.3, 3.1, 3.2, 4.1-4.4, 5.2 (1.2 adds a `PushEvent` variant matched exhaustively by the adapter; 3.2 is its first consumer); 5.1 may merge alone; 3.3 (edit flag, Q6) is its own unit after PIN-app and closes with the live check 6.2. PIN's tasks on files shared with RL start after the matching RL unit has merged (RL first: all its answers are in).

| ID | Status | Needs | Files / notes |
|----|--------|-------|---------------|
| 0.1 | blocked: VM network; **[live]** reads only | | `research.md` |
| 0.2 | blocked-by 0.1's network and the user at the web client; **[live]** | 0.1 | `research.md`; decides Q4 and checks the Q5 timestamps |
| 1.2 | ready; start after RL 1.2 merges (same `wire.rs`) | 1.1 | `wire.rs`, `tests/wire_fixtures.rs`, `xiva.rs` |
| 1.3 | conditional on 0.1; after RL 1.1 (same `request.rs`) | 0.1 | `request.rs` |
| 2.1 | ready; start after RL-app merges | 1.1 | `model.rs`, `fixtures.rs` |
| 2.2 | blocked-by 2.1 | 2.1 | `model.rs`, `backend.rs`, `cache.rs` (test) |
| 2.3 | blocked-by 2.2 (and 1.2 for the unit) | 2.2 | chat-core `client.rs` |
| 2.4 | blocked-by Q4 (0.2) | 0.2 | chat-core `client.rs` |
| 3.1 | blocked-by 2.1 | 1.1, 2.1 | `yandex_backend.rs` |
| 3.2 | blocked-by 1.2, 2.2, 3.1 | | `yandex_backend.rs` |
| 3.3 | blocked-by 3.2, 5.2; closes with live 6.2 | | `write.rs`, `backend.rs`, `client.rs`, `yandex_backend.rs`, `bt-probe.rs` |
| 4.1 | blocked-by 2.2 | 2.2 | `store.rs` |
| 4.2 | blocked-by 2.1 | 2.1 | `timeline.rs`, `theme.rs` |
| 4.3 | blocked-by 4.1 | 4.1 | `desktop.rs` |
| 4.4 | blocked-by 4.1-4.3 | | `tests.rs` |
| 5.1 | ready; start after RL 2.1 merges (same `bt-probe.rs`) | 1.1 | `bt-probe.rs` |
| 5.2 | blocked-by 1.2 (and RL 2.1) | 1.2 | `bt-probe.rs` |
| 6.1 | after the units | | build/test/lint matrix |
| 6.2 | blocked: network and the user's approval of the target; **[approval] [live]**; one at a time (rule 5) | 3.3 | live check; `BT_WRITE_ONLY_CHAT` set; the only importance frame is the edit's `IsImportant: true` (Q6) |
| 6.3 | after the units | | this file, archive |

## Ready now

- Startable without the user, no unmet dependency: **RL 6.7** (`tests.rs`, on `integ/reaction-list-app`); **PIN 1.2** (RL 1.2 is on `main`), **PIN 5.2** needs PIN 1.2.
- Queued behind RL-app on shared files (rule 1): **PIN 2.1** (after RL-app merges to `main`). Each chain then unlocks the rest of its column above.
- CM has nothing startable without the user: CM 7.1 needs 6.4 (or 6.F); the archive step needs 5.1, 5.3 and 6.2.

## Needs the user

- Approval of the target: CM 5.1, CM 5.3, CM 6.2 (real chat and group creation); deferred by the user, so these three and 6.3, 6.4, 7.1 stay open.
- PIN 0.2 and 6.2: the user performs pins, importance marks and an own-message mark in the web client on a disposable self-chat message and approves the target (the agent sends only the edit of 6.2, under `BT_WRITE_ONLY_CHAT`).
- RL 3.1 and 8.2 and PIN 0.1 need the VM's outbound network (the host's full-tunnel VPN breaks the VM NAT), not a user decision.
