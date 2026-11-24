# Task dependency graph of open changes

Owner: Integrator. Updated: 2026-11-24 (rev 2: one-owner-per-file, compile-complete merge units). Sources: the `tasks.md` of the four open changes. Prefixes: **GS** `add-global-search`, **MD** `add-media-download`, **CM** `add-chat-members-and-creation`, **MR** `add-message-reactions`.

`[live]` = needs the signed-in session in `.runtime` and the real account (reads only unless noted). `[approval]` = needs the user's explicit approval of the target first.

## Rules

1. **One owner per file, at task granularity.** A *task* may start only when no other *in-flight* task (started, not yet merged to `main`) edits any of its files; "different hunks" and "additive" do not exempt. A *unit* (rule 2) may run its tasks while the unit is unmerged, so a unit does not lock files it has not yet reached: each task waits only for in-flight tasks on its own files, and a unit merges when all its tasks are in. A task that has started holds its files until its unit is merged, so the order of tasks on one file is also the order of their merges: put the small, independent task first, the task that must wait for a long unit last.
2. **Every merge to `main` compiles with no stubs and no no-op arms** (`cargo check --workspace --all-targets`; `cargo test --workspace` green). A task that cannot compile on its own is merged only inside a *merge unit* with the tasks that complete it. Inside a unit, tasks with disjoint files run in parallel on branches cut from the unit's integration branch (`integ/<change>`), and the Integrator merges them into it.
3. **Withdrawn work is not in flight (decision, Integrator).** "In flight" means started and not yet merged. Three finished branches are *withdrawn* back to "not started" and kept only as saved patches, because their units are gated on something that has not happened and would otherwise keep their files locked for hours: `integ/gs` with GS 2.1 (one 93-line commit, 5798ec9, gated on the live chain GS 1.2 → 1.4), `md/5.1` (README text for a feature that does not exist yet, gated on every MD unit), `rx/2.1` (gated on the MR-B1 slot). Their owner re-applies the patch (cherry-pick or rebase) when the unit actually starts. This is a deliberate reading of the rule, chosen for the critical path: the alternative (GS 2.1 stays in flight) leaves `model.rs`, `backend.rs` and chat-core `client.rs` locked until GS 1.4 and GS-C merge, so no MD, CM or MR core work could start in the meantime. If Main prefers the strict reading, MD 2.1 and everything in MD-C become blocked-by GS-C and the core-lane order becomes GS-C → MD-C → MR-B1 → CM-C1…
4. **Order when several units are startable:** the first whose locks are free; ties go to GS > MD > MR > CM (GS is furthest along; MD has no live gate; MR is last because it is the largest compile-coupled bundle and would hold the most files for the longest; CM is gated by live reads and approvals so it fills the lanes while it waits).
5. Live-account tasks share one session and run one at a time, in this order: GS 1.3 (done), MD 1.3 (done), CM 1.2, MR 7.2, then the manual runs GS 4.1, MD 5.2, CM 5.2. The host firewall (Little Snitch) lets only the `bt-probe` binary of the main checkout (`<main checkout>/target/debug/bt-probe`) reach the network; a binary built in a worktree `target` directory gets no connection, so live runs use the main checkout's build (the live task's `bt-probe.rs` changes must be on `main` first, or the run is done after the merge).
6. **Zero build warnings** (project convention: the `tasks.md` of every archived change and of CM 7.1 require `cargo build --workspace --all-targets` with 0 warnings; there is no CI and no `-D warnings`, but `main` builds with 0 warnings today). `desktop` is a binary crate, so a store method, field or view model with no caller is a `dead_code` warning. Therefore a unit also contains the first consumer of everything it adds; `#[allow(dead_code)]` is not used. Consequence: unit boundaries are drawn at "first caller", see "Merge units".

## Why these units (compile gaps found by `cargo check` and by reading the code)

- **GS 2.1** adds a required `Backend::search`, `Command::Search`/`CancelSearch`, `ClientEvent::SearchResults`/`SearchFailed`. The only `Backend` impl is `yandex_backend.rs` (GS 3.1, which needs 1.4) and `store.rs` matches `ClientEvent` exhaustively (GS 3.2); the engine got a no-op arm that 2.2 replaces; the store's `search`, `cancel_search`, `search_state` and the `search_results` view model have no caller until 3.4 (rule 6). So 2.1, 2.2, 3.1, 3.2, 3.3, 3.4 are one unit **GS-A** on `integ/gs`, merged only after 1.4 is on `main`.
- **MD 1.1** adds `ProtoError::Storage`; `backend_error` in `yandex_backend.rs` matches `ProtoError` exhaustively. Re-cut: 1.1 also adds the one arm (`Storage` → `BackendError::Protocol("storage: …")`, a real mapping to an existing variant; 3.1 later maps it to `BackendError::Storage`), so 1.1 merges alone (**MD-P**) and releases `client.rs` of the proto crate quickly.
- **MD 2.1** adds `Backend::download`, `Command::Download`/`CancelDownload`, `ClientEvent::Download*`, `MessageBody::Gallery`, `BackendError::Storage`. Needs engine arms (2.3), a `YandexBackend::download` (3.1), store arms (4.1). The "download is not wired yet" stub in 2.1's text was removed. **Decision (rule 6):** the store's `download`, `cancel_download`, `download_state` (4.1) are first called by the viewer (4.3) and the file card (4.2), and 4.3's `open_viewer` is first called by the gallery cells of 4.2, so unit **MD-C** = 2.1, 2.3, 3.1, 4.1, **4.3, 4.2**, all on `integ/md`, merged to `main` once, after 4.2. (2.2 merged earlier, alone, as **MD-K**.) Inside the unit, same-file tasks serialize: 2.3 → 4.3 (`main.rs`), 4.3 → 4.2 (`tests.rs`, `open_viewer`). README 5.1 and verification 5.2 close the change as **MD-D** (5.1 describes the feature, so it merges with or after MD-C).
- **MR 2.1** adds `PushEvent::Reaction`, matched exhaustively in the live loop of `yandex_backend.rs`, and `WireMessage.reactions` (breaks literals in `yandex_backend.rs` tests and `bt-probe.rs`); **MR 3.1** adds `Backend::react`, `Command::ToggleReaction`, `ClientEvent::ReactionsChanged`, `LiveEvent::ReactionsChanged`, `Message.reactions` (breaks the `Fake`, `YandexBackend`, `store.rs` and `tests.rs` literals); `ChatStore::toggle_reaction` (5.1) is first called by 5.3 (rule 6). Smallest compile-complete, warning-free set: unit **MR-B** = 2.1, 2.2, 3.1, 3.2, 4.1, 5.1, 3.3, 5.2, 5.3, 5.4 plus the one-line literal fixes in `tests.rs` (from 5.4) and `bt-probe.rs` (from 6.1), all on `integ/reactions`. Then **MR-P** = 6.1 (probe), then 7.x.
- **CM** was already compile-complete per task: 3.1 brings the `YandexBackend` impls and the `Fake`; 3.2/3.3 add `Command`/`ClientEvent` with engine and store in the same task; 6.3 likewise. No re-cut, only ordering.

## Merge units

| Unit | Tasks | Files held | Gate |
|------|-------|------------|------|
| MD-P | MD 1.1 (+ `backend_error` arm) | proto `media.rs`, `net.rs`, `error.rs`, `client.rs`; `yandex_backend.rs` (one hunk) | in progress (md/1.1) |
| MD-K | MD 2.2 | chat-core `paths.rs`, `lib.rs` (one `mod` line) | none |
| MD-C | MD 2.1, 2.3, 3.1, 4.1, 4.3, 4.2 | merged to `main` (86dd403) | done |
| MD-D | MD 5.1 (merged, ad90cc4), 5.2 (manual) | `README.md` | 5.2 after MD-C |
| MD-R1 / R3 | MD 1.2 (`bt-probe.rs`), MD 1.3 (`design.md`, maybe `media.rs`) | | MD-P; 1.2 after GS 1.3 on `bt-probe.rs` |
| GS-P1..P3 | GS 1.2, 1.3, 1.4 | 1.2: `search.rs`, `registry.rs`, proto `client.rs`, `lib.rs`; 1.3: `bt-probe.rs`, fixture [live]; 1.4: `search.rs`, `wire_fixtures.rs` | 1.2 after MD-P (proto `client.rs`) |
| GS-A | GS 2.1, 2.2, 3.1, 3.2, 3.3, 3.4, 3.5 | merged to `main` (bb34be8) | done |
| CM-P1.. | CM 1.1 (`request.rs`, `write.rs`, proto `client.rs`); 1.2 (`bt-probe.rs`, `README.md`, fixtures, `design.md`) [live]; 1.3 (`wire.rs`, `wire_fixtures.rs`, `client.rs`, `request.rs`); 2.1 (`manage.rs`, `registry.rs`, `lib.rs`); 2.2 (`client.rs`, `bt-probe.rs`, `README.md`); 6.1 (`manage.rs`, `registry.rs`, `client.rs`, `bt-probe.rs`, `README.md`) | | see per-task table |
| CM-C1 | CM 3.1 | `model.rs`, `backend.rs`, `fixtures.rs`, chat-core `client.rs`, `yandex_backend.rs`, `tests.rs` (literal) | CM 1.3, 2.2; core lane free (only trait methods and a pub field: no `dead_code`) |
| CM-M | CM 3.2, 4.1, 4.2 | `model.rs`, chat-core `client.rs`, `store.rs`, `info_panel.rs`, `profile.rs`, `desktop.rs`, `tests.rs` | CM-C1 (rule 6: members state is first used by the info panel, profile state by the profile view) |
| CM-N | CM 3.3, 4.3 | `model.rs`, chat-core `client.rs`, `store.rs`, `new_chat.rs`, `info_panel.rs`, `desktop.rs`, `tests.rs` | CM-M (creation state is first used by the dialog) |
| CM-G | CM 6.3, 6.4 | `backend.rs`, `model.rs`, chat-core `client.rs`, `store.rs`, `yandex_backend.rs`, `new_chat.rs`, `desktop.rs`, `tests.rs` | live gate CM 6.2; CM-N |
| MR-B | MR 2.1 (done), 2.2, 3.1, 3.2, 3.3, 4.1, 5.1, 5.2, 5.3, 5.4, 6.1 (carries the `WireMessage` literal in `bt-probe.rs`), 7.1 | `wire.rs`, `xiva.rs`, `wire_fixtures.rs`, `write.rs`, `request.rs` (docs), `reactions.rs`, `model.rs`, `backend.rs`, `lib.rs`, `fixtures.rs`, chat-core `client.rs`, `cache.rs`, `yandex_backend.rs`, `store.rs`, `tests.rs`, `timeline.rs`, `theme.rs`, `composer.rs`, `desktop.rs` (if needed), and `bt-probe.rs` only through MR 6.1 | branch `integ/reactions` (from `main` 19cf57a + MR 2.1). Task order below. MR 6.1 is the only MR task on `bt-probe.rs`: it starts after CM 2.2 and CM 6.1 are merged (they come first on that file; neither is blocked by MR-B). Until 6.1 lands the `yandex-proto` bin test target does not compile (the literal at `bt-probe.rs:590`; expected); the other targets and `desktop` are verified task by task with targeted `cargo test -p`. |

## Order on the shared files (hard edges)

- **Proto `client.rs`:** MD-P (1.1) → GS 1.2 → CM 1.1 → CM 1.3 → CM 2.2 → CM 6.1. (CM 2.1 does not touch it.)
- **`registry.rs`, `lib.rs`:** CM 2.1 (starts now, small) or GS 1.2, whichever has its other files free first; then CM 6.1.
- **`request.rs`, `write.rs`:** GS 1.1 (done) → CM 1.1 (done, allowlist 8 names) → CM 1.3 (removes `GroupMembers`: allowlist **8 → 7** names: `request_user`, `get_users_data`, `history`, `subscribe`, `search`, `whoami`, `get_chat_members`; updates the const and `len()` assertion in `request.rs` and the pin `the_read_allowlist_does_not_gain_the_write_method` in `write.rs`; the member route stays `csrf: false`) → MR-B (docs, variant-set test; must count 7).
- **`wire.rs`, `wire_fixtures.rs`:** GS 1.4 (done) → CM 1.3 (done) → MR 2.1 (done on `integ/reactions`). No further CM task edits them.
- **`bt-probe.rs`, `README.md`:** GS 1.3 → MD 1.2 → CM 1.2 → CM 1.3 (done, all merged) → **CM 2.2 → CM 6.1 → MR 6.1** (with the `WireMessage` literal); README also MD-D last. CM 2.2 starts now (it conflicts with no MR task except 6.1), MR 6.1 is last in MR-B.
- **Core lane** (`model.rs`, `backend.rs`, chat-core `client.rs`, `fixtures.rs`, `yandex_backend.rs`, `store.rs`, `tests.rs`): MD-C → GS-A → MR-B → CM-C1 → CM-M → CM-N → CM-G (GS-A first if it is ready while MD-C is not yet started on the core lane; a unit only waits for units that are actually in flight). Desktop UI files (`desktop.rs`, `tests.rs`, `main.rs`, `timeline.rs`) now sit inside these units, so the UI lane has the same order.
- GS 3.1 and CM 3.1 both derive "private chat of a person": whoever lands second reuses the first one's data (`ChatSummary.partner` or the guid map), not a second map.

## add-global-search (GS)

| ID | Status | Needs | Unit |
|----|--------|-------|------|
| 1.1 | done (6fb4a72) | — | |
| 1.2 | done (f8bd206) | 1.1 | GS-P1 |
| 1.3 | done (gs/1.3 merge; `bt-probe search`, redacted `live-search.json`, no real strings: all text is placeholders) | 1.2 | GS-P2 |
| 1.4 | done (gs/1.4 merge; `WireSearchChat.kind` is only `Channel` or `Group`: no private-chat marker, so `PersonHit::chat` comes only from the guid map of 3.1) | 1.3 | GS-P3 |
| 2.1 | done (bb34be8, unit GS-A) | — | GS-A |
| 2.2 | done (bb34be8) | 2.1 | GS-A |
| 3.1 | done (bb34be8); `YandexBackend.known` map (`private`, `titles`) is what CM 3.1 reuses | 1.4, 2.1 | GS-A |
| 3.2 | done (bb34be8) | 2.1 | GS-A |
| 3.3 | done (bb34be8) | 3.2 | GS-A |
| 3.4 | done (bb34be8; `ChatStore::supersede_search`, documented in `tasks.md`) | 3.2, 3.3 | GS-A |
| 4.1 | ready [live] manual run with the `main` build | 1.4, 2.2, 3.1, 3.4 | [live] manual run |

## add-media-download (MD)

| ID | Status | Needs | Unit |
|----|--------|-------|------|
| 1.1 | done (b2242f0, with the `backend_error` arm) | — | MD-P |
| 1.2 | done (e5f2dde) | 1.1 | MD-R1 |
| 1.3 | done (merge of `md/1.3`, docs only; live re-check: a 57.5 MiB `MiscFile` and two images saved in full, sizes match, no change to `media.rs`) | 1.2 | MD-R3 |
| 2.1 | done (86dd403, unit MD-C) | — | MD-C |
| 2.2 | done (e8e533e) | — | MD-K |
| 2.3 | done (86dd403) | 2.1, 2.2 | MD-C |
| 3.1 | done (86dd403); the "download is not wired yet" placeholder is gone | 2.1 | MD-C |
| 4.1 | done (86dd403) | 2.1 | MD-C |
| 4.3 | done (86dd403; follow-up `md/4.3-fit`, the viewer picture fits the window, merged in aca841a) | 2.1, 4.1, 2.3 | MD-C |
| 4.2 | done (86dd403; valid PNG test fixture) | 4.1, 4.3 | MD-C |
| 5.1 | done (ad90cc4, README) | — | MD-D |
| 5.2 | ready [live] manual run with the `main` build | all | MD-D |

## add-chat-members-and-creation (CM)

| ID | Status | Needs | Files / notes |
|----|--------|-------|---------------|
| 0.1 | done, static PASS (2a781dd; `create_chat` payload recorded in `design.md` D4) | — | gate for 6.1 passed |
| 1.1 | done (5fce6ae; allowlist 8 names, `write.rs` pin updated; CM 1.3 lowers it to 7) | — | |
| 1.2 | done (696177e; `bt-probe whoami`/`members`, redacted fixtures `live-members-group.json`, `live-members-private.json`, `live-whoami.json`; findings in `design.md`: `get_group_members` is `bad_request` for every shape, `get_chat_members` works for group chats only with `limit`/`guid_offset`/`roles`, private/self/channel chats answer 403) | 1.1 | |
| 1.3 | done (19cf57a; typed `Client::members`/`whoami`, `GroupMembers` removed, allowlist 7 names, `write.rs` pin updated) | 1.2 | CM-P |
| 2.1 | done (merge commit of `cm/2.1`; `manage.rs`, `registry.rs`, `lib.rs`; closed mutation set, no redirects followed) | — | `manage.rs`, `registry.rs`, `lib.rs` |
| 2.2 | ready (`client.rs`, `bt-probe.rs`, `README.md`; no MR task touches these except MR 6.1, which waits for it) | 2.1, 1.3 | `client.rs`, `bt-probe.rs`, `README.md` |
| 3.1 | blocked-by CM 2.2 + core lane (held by MR-B: `model.rs`, `backend.rs`, chat-core `client.rs`, `yandex_backend.rs` are in MR 3.1, 3.2, 4.1 until MR-B merges); notes: members only for group chats via paged `get_chat_members` (pages of 50, `guid_offset`), a private chat is answered from the chat row, a channel refusal is a failed load; reuses `YandexBackend.known.private` of GS 3.1 for the partner guid | 1.3, 2.2 | CM-C1 |
| 3.2 | blocked-by CM 3.1 | 3.1 | CM-M (with 4.1, 4.2) |
| 3.3 | blocked-by CM-M | 3.2 | CM-N (with 4.3) |
| 4.1 | blocked-by CM 3.1 (unit CM-M: store state from 3.2 first used here) | 3.2 | CM-M |
| 4.2 | blocked-by CM 4.1 | 3.2, 4.1 | CM-M |
| 4.3 | blocked-by CM-M, 3.3 | 3.3, 4.2 | CM-N |
| 5.1 | blocked-by CM 2.2 | 2.2 | **[approval] [live]**: real private chat creation, the user approves the target |
| 5.2 | blocked-by CM 4.3 | 4.3 | VM run, `BT_WRITE_ONLY_CHAT` = self chat |
| 5.3 | blocked-by CM 5.1, 5.2 | 5.1, 5.2 | **[approval] [live]** |
| 6.1 | blocked-by CM 2.2 (0.1 static PASS done); starts right after 2.2 so MR 6.1 can follow it on `bt-probe.rs` | 2.2 | probe support only; `manage.rs`, `registry.rs`, `client.rs`, `bt-probe.rs`, `README.md` |
| 6.2 | blocked-by CM 6.1 | 6.1 | **[approval] [live]**: real group creation (throwaway title, participants the user approves); live gate for 6.3, 6.4 |
| 6.3 | blocked-by CM 6.2 (live PASS), CM-N | 6.2, 3.3 | CM-G (with 6.4) |
| 6.4 | blocked-by CM 6.3, 4.3 | 6.3, 4.3 | CM-G |
| 6.F | conditional | 0.1 FAIL or 6.2 FAIL | removes group wording and code |
| 7.1 | blocked-by CM 4.3 and 6.4 (or 6.F) | | |

## add-message-reactions (MR)

| ID | Status | Needs | Unit |
|----|--------|-------|------|
| 1.1 | done (a56f811, `research.md`) | — | |
| 2.1 | done on `integ/reactions` (be7c461, cherry-pick of 3e4f081 onto `main` 19cf57a; does not compile alone) | — | MR-B |
| 2.2 | ready (`write.rs`, `request.rs` docs) | — | MR-B |
| 3.1 | ready (`reactions.rs`, `model.rs`, `backend.rs`, `lib.rs`, `fixtures.rs`, chat-core `client.rs` literals) | — | MR-B |
| 3.2 | blocked-by MR 3.1 (`client.rs`, `cache.rs`) | 3.1 | MR-B |
| 3.3 | blocked-by MR 3.1 (`reactions.rs`); 1.1 is done | 1.1, 3.1 | MR-B |
| 4.1 | blocked-by MR 2.2, 3.1 (`yandex_backend.rs`) | 2.1, 2.2, 3.1 | MR-B |
| 5.1 | blocked-by MR 3.1 (`store.rs`) | 3.1 | MR-B |
| 5.2 | blocked-by MR 3.1 (`timeline.rs`, `theme.rs`; lands with 5.3) | 3.1 | MR-B |
| 5.3 | blocked-by MR 3.1, 5.1 (`composer.rs`, `desktop.rs`) | 3.1, 5.1 | MR-B |
| 5.4 | blocked-by MR 5.1–5.3 (`tests.rs`) | | MR-B |
| 6.1 | blocked-by MR 2.2 and by CM 2.2, CM 6.1 being merged (`bt-probe.rs`; carries the `WireMessage` literal fix) | 2.1, 2.2 | MR-B |
| 7.1 | blocked-by all MR code | | |
| 7.2 | blocked-by MR 1.1, 6.1, 7.1 | | **[live]**, writes in the self chat only (`BT_WRITE_ONLY_CHAT`); confirm with the user |
| 7.3 | blocked-by MR 7.1, 7.2 | | archive |

## Ready now (not started, nothing in flight touches their files)

- **MR-B tasks on `integ/reactions` (head be7c461), ready now:** MR 2.2 and MR 3.1 (disjoint files, parallel). Then, once 3.1 is in: MR 3.2, 3.3, 4.1 (also needs 2.2), 5.1, 5.2 in parallel (files `client.rs`+`cache.rs`, `reactions.rs`, `yandex_backend.rs`, `store.rs`, `timeline.rs`+`theme.rs`: pairwise disjoint); then 5.3 (after 5.1), then 5.4; MR 6.1 last (waits for CM 2.2 and CM 6.1); then 7.1 and the unit merges. No MR task touches `proto client.rs`, `README.md`, `manage.rs` or `registry.rs`, so MR-B and CM 2.2 / CM 6.1 do not collide except on `bt-probe.rs` (MR 6.1 only).
- GS 4.1 [live, manual] (`main` build; owns `tasks.md` only; all GS code is on `main`).
- MD 5.2 [live, manual] (`main` build; owns no files; worker `md/5.2`).

In flight: `cm/1.2-status` (CM `design.md`/`tasks.md`), `md/5.2`, `integ/reactions` (MR 2.1 done, nothing else started). Merged so far: GS 1.1–1.4, GS-A (2.1–3.5); MD 1.1, 1.2, 1.3, 2.2, MD-C (2.1, 2.3, 3.1, 4.1, 4.2, 4.3 and its follow-up `md/4.3-fit`), 5.1; CM 0.1, 1.1, 1.2, 1.3, 2.1, spec reconcile; MR 1.1. CM 2.2 is ready now (`proto client.rs`, `bt-probe.rs`, `README.md`); CM 3.1 waits for CM 2.2 and for MR-B on the core lane.

## Needs the user

- Approval of the target: CM 5.1, CM 5.3, CM 6.2 (real chat and group creation).
- Writes on the real account to confirm: MR 7.2 (self chat only).
- Live session, reads only, no approval: GS 4.1, CM 5.2 (VM), MD 5.2 (may end "unverified" when the self chat holds no file). Done: GS 1.3, MD 1.3, CM 1.2.
