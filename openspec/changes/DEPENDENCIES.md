# Task dependency graph of open changes

Owner: Integrator. Updated: 2026-11-24 (rev 2: one-owner-per-file, compile-complete merge units). Sources: the `tasks.md` of the four open changes. Prefixes: **GS** `add-global-search`, **MD** `add-media-download`, **CM** `add-chat-members-and-creation`, **MR** `add-message-reactions`.

`[live]` = needs the signed-in session in `.runtime` and the real account (reads only unless noted). `[approval]` = needs the user's explicit approval of the target first.

## Rules

1. **One owner per file.** A merge unit may start only when no other *in-flight* unit (started, not yet merged to `main`) edits any of its files. Two units that edit the same file are a hard edge: the later one starts after the earlier one is merged. "Different hunks" and "additive" do not exempt.
2. **Every merge to `main` compiles with no stubs and no no-op arms** (`cargo check --workspace --all-targets`; `cargo test --workspace` green). A task that cannot compile on its own is merged only inside a *merge unit* with the tasks that complete it. Inside a unit, tasks with disjoint files run in parallel on branches cut from the unit's integration branch (`integ/<change>`), and the Integrator merges them into it.
3. **Parked is not in flight.** A finished branch whose unit has not started holds no lock; its owner rebases it when the unit starts. (`integ/gs` with 2.1, `md/5.1`, `rx/2.1` are parked.)
4. **Order when several units are startable:** the first whose locks are free; ties go to GS > MD > MR > CM (GS is furthest along; MD has no live gate; MR is last because it is the largest compile-coupled bundle and would hold the most files for the longest; CM is gated by live reads and approvals so it fills the lanes while it waits).
5. Live-account tasks share one session and run one at a time, in this order: GS 1.3, CM 1.2, MD 1.3 (a re-check), MR 7.2, then the manual runs GS 4.1, MD 5.2, CM 5.2.

## Why these units (compile gaps found by `cargo check` and by reading the code)

- **GS 2.1** adds a required `Backend::search`, `Command::Search`/`CancelSearch`, `ClientEvent::SearchResults`/`SearchFailed`. The only `Backend` impl is `yandex_backend.rs` (GS 3.1, which needs 1.4) and `store.rs` matches `ClientEvent` exhaustively (GS 3.2); the engine got a no-op arm that 2.2 replaces. So 2.1, 2.2, 3.1, 3.2 are one unit **GS-C**, merged only after 1.4 is on `main`. 3.3 and 3.4 are one unit **GS-U** (a view model nothing calls would be dead code).
- **MD 1.1** adds `ProtoError::Storage`; `backend_error` in `yandex_backend.rs` matches `ProtoError` exhaustively. Re-cut: 1.1 also adds the one arm (`Storage` → `BackendError::Protocol("storage: …")`, a real mapping to an existing variant; 3.1 later maps it to `BackendError::Storage`), so 1.1 merges alone (**MD-P**) and releases `client.rs` of the proto crate quickly.
- **MD 2.1** adds `Backend::download`, `Command::Download`/`CancelDownload`, `ClientEvent::Download*`, `MessageBody::Gallery`, `BackendError::Storage`. Needs engine arms (2.3), a `YandexBackend::download` (3.1), store arms (4.1). The "download is not wired yet" stub in 2.1's text was removed. Unit **MD-C** = 2.1, 2.3, 3.1, 4.1 (2.2 merges first, alone, as **MD-K**). 4.3 and 4.2 follow as **MD-U1**, **MD-U2**; README 5.1 and 5.2 close the change as **MD-D** (5.1 describes the feature, so it merges with the last media unit).
- **MR 2.1** adds `PushEvent::Reaction`, matched exhaustively in the live loop of `yandex_backend.rs`, and `WireMessage.reactions` (breaks literals in `yandex_backend.rs` tests and `bt-probe.rs`); **MR 3.1** adds `Backend::react`, `Command::ToggleReaction`, `ClientEvent::ReactionsChanged`, `LiveEvent::ReactionsChanged`, `Message.reactions` (breaks the `Fake`, `YandexBackend`, `store.rs` and `tests.rs` literals). Smallest compile-complete set: unit **MR-B1** = 2.1, 2.2, 3.1, 3.2, 4.1, 5.1 plus the one-line literal fixes in `tests.rs` (from 5.4) and `bt-probe.rs` (from 6.1), all on `integ/reactions`. Then **MR-B2** = 3.3, 5.2, 5.3, 5.4 (chips and menu land together), **MR-P** = 6.1 (probe), then 7.x.
- **CM** was already compile-complete per task: 3.1 brings the `YandexBackend` impls and the `Fake`; 3.2/3.3 add `Command`/`ClientEvent` with engine and store in the same task; 6.3 likewise. No re-cut, only ordering.

## Merge units

| Unit | Tasks | Files held | Gate |
|------|-------|------------|------|
| MD-P | MD 1.1 (+ `backend_error` arm) | proto `media.rs`, `net.rs`, `error.rs`, `client.rs`; `yandex_backend.rs` (one hunk) | in progress (md/1.1) |
| MD-K | MD 2.2 | chat-core `paths.rs`, `lib.rs` (one `mod` line) | none |
| MD-C | MD 2.1, 2.3, 3.1, 4.1 | `model.rs`, `backend.rs`, `fixtures.rs`, chat-core `client.rs`, `composer.rs`, `timeline.rs`, `yandex_backend.rs`, `store.rs`, `main.rs`, `tests.rs` | MD-P, MD-K merged |
| MD-U1 | MD 4.3 | `viewer.rs`, `desktop.rs`, `main.rs`, `tests.rs` | MD-C |
| MD-U2 | MD 4.2 | `timeline.rs`, `tests.rs` | MD-U1 |
| MD-D | MD 5.1, 5.2 | `README.md` | all MD units |
| MD-R1 / R3 | MD 1.2 (`bt-probe.rs`), MD 1.3 (`design.md`, maybe `media.rs`) | | MD-P; 1.2 after GS 1.3 on `bt-probe.rs` |
| GS-P1..P3 | GS 1.2, 1.3, 1.4 | 1.2: `search.rs`, `registry.rs`, proto `client.rs`, `lib.rs`; 1.3: `bt-probe.rs`, fixture [live]; 1.4: `search.rs`, `wire_fixtures.rs` | 1.2 after MD-P (proto `client.rs`) |
| GS-C | GS 2.1 (done, on `integ/gs`), 2.2, 3.1, 3.2 | `model.rs`, `backend.rs`, chat-core `client.rs`, `yandex_backend.rs`, `store.rs` | GS 1.4 merged; MD-C merged (files) unless GS-C is ready first |
| GS-U | GS 3.3, 3.4 | `search_results.rs`, `main.rs`, `desktop.rs`, `tests.rs` | GS-C, MD-U1 (files) |
| CM-P1.. | CM 1.1 (`request.rs`, `write.rs`, proto `client.rs`); 1.2 (`bt-probe.rs`, `README.md`, fixtures, `design.md`) [live]; 1.3 (`wire.rs`, `wire_fixtures.rs`, `client.rs`, `request.rs`); 2.1 (`manage.rs`, `registry.rs`, `lib.rs`); 2.2 (`client.rs`, `bt-probe.rs`, `README.md`); 6.1 (`manage.rs`, `registry.rs`, `client.rs`, `bt-probe.rs`, `README.md`) | | see per-task table |
| CM-C1..C3 | CM 3.1, 3.2, 3.3 | `model.rs`, `backend.rs`, `fixtures.rs`, chat-core `client.rs`, `yandex_backend.rs`, `store.rs`, `tests.rs` (literal) | CM 1.3, 2.2; core lane free |
| CM-U1..U3 | CM 4.1, 4.2, 4.3 | `info_panel.rs`, `profile.rs`, `new_chat.rs`, `desktop.rs`, `tests.rs` | CM-C2 / C3 |
| CM-G1, G2 | CM 6.3, 6.4 | 6.3: `backend.rs`, `model.rs`, `client.rs`, `store.rs`, `yandex_backend.rs`; 6.4: `new_chat.rs`, `desktop.rs`, `tests.rs` | live gate CM 6.2 |
| MR-B1 | MR 2.1, 2.2, 3.1, 3.2, 4.1, 5.1 (+ literal fixes) | `wire.rs`, `xiva.rs`, `wire_fixtures.rs`, `write.rs`, `request.rs` (docs), `reactions.rs`, `model.rs`, `backend.rs`, `lib.rs`, `fixtures.rs`, chat-core `client.rs`, `cache.rs`, `yandex_backend.rs`, `store.rs`, `tests.rs` + `bt-probe.rs` (literals) | files free of all other in-flight units |
| MR-B2 | MR 3.3, 5.2, 5.3, 5.4 | `reactions.rs`, `timeline.rs`, `theme.rs`, `composer.rs`, `desktop.rs` (if needed), `tests.rs` | MR-B1 |
| MR-P | MR 6.1 | `bt-probe.rs` | MR-B1, probe lane |

## Order on the shared files (hard edges)

- **Proto `client.rs`:** MD-P (1.1) → GS 1.2 → CM 1.1 → CM 1.3 → CM 2.2 → CM 6.1. (CM 2.1 does not touch it.)
- **`registry.rs`, `lib.rs`:** CM 2.1 (starts now, small) or GS 1.2, whichever has its other files free first; then CM 6.1.
- **`request.rs`, `write.rs`:** GS 1.1 (done) → CM 1.1 → (CM 1.3 route flag) → MR-B1 (docs, variant-set test). CM 1.1 must raise the pinned allowlist to **8** names (4 + `search` + `whoami`, `get_chat_members`, `get_group_members`) and update `request.rs` (const, `len()` assertion) and `write.rs:384`.
- **`wire.rs`, `wire_fixtures.rs`:** GS 1.4 (fixtures) → CM 1.3 → MR-B1. MR 2.1 is parked until MR-B1 starts.
- **`bt-probe.rs`, `README.md`:** GS 1.3 → MD 1.2 → CM 1.2 → CM 2.2 → CM 6.1 → MR-B1 (literal) → MR-P; README also MD-D last.
- **Core lane** (`model.rs`, `backend.rs`, chat-core `client.rs`, `fixtures.rs`, `yandex_backend.rs`, `store.rs`, `tests.rs`): MD-K ‖ MD-C → GS-C → MR-B1 → CM-C1 → CM-C2 → CM-C3 → CM-G1. If GS 1.4 lags, MD-C goes first (no live gate); a unit only waits for units that are actually in flight.
- **Desktop UI lane** (`desktop.rs`, `tests.rs`, `main.rs`, `timeline.rs`): MD-U1 → MD-U2 → GS-U → MR-B2 → CM-U1 → CM-U2 → CM-U3 → CM-G2. GS-U may precede MD-U1 if GS-C merged first and MD-C is not done (no shared in-flight file).
- GS 3.1 and CM 3.1 both derive "private chat of a person": whoever lands second reuses the first one's data (`ChatSummary.partner` or the guid map), not a second map.

## add-global-search (GS)

| ID | Status | Needs | Unit |
|----|--------|-------|------|
| 1.1 | done (6fb4a72) | — | |
| 1.2 | blocked-by MD-P (proto `client.rs`) | 1.1 | GS-P1 |
| 1.3 | blocked-by GS 1.2 | 1.2 | GS-P2 [live] |
| 1.4 | blocked-by GS 1.3 | 1.3 | GS-P3 |
| 2.1 | done on `integ/gs` (5798ec9, parked) | — | GS-C |
| 2.2 | blocked-by GS 1.4 (unit gate) | 2.1 | GS-C |
| 3.1 | blocked-by GS 1.4 | 1.4, 2.1 | GS-C |
| 3.2 | blocked-by GS 1.4 (unit gate) | 2.1 | GS-C |
| 3.3 | blocked-by GS-C | 3.2 | GS-U |
| 3.4 | blocked-by GS-C, 3.3 | 3.2, 3.3 | GS-U |
| 4.1 | blocked-by all | 1.4, 2.2, 3.1, 3.4 | [live] manual run |

## add-media-download (MD)

| ID | Status | Needs | Unit |
|----|--------|-------|------|
| 1.1 | in progress (md/1.1) | — | MD-P (+ `backend_error` arm) |
| 1.2 | blocked-by MD 1.1, GS 1.3 (`bt-probe.rs`) | 1.1 | MD-R1 |
| 1.3 | blocked-by MD 1.2 | 1.2 | MD-R3 [live] re-check (URL already verified by plain GETs, see `design.md`) |
| 2.1 | blocked-by MD 1.1 (yandex_backend.rs) | — | MD-C |
| 2.2 | ready | — | MD-K |
| 2.3 | blocked-by MD 2.1, 2.2 | 2.1, 2.2 | MD-C |
| 3.1 | blocked-by MD 1.1, 2.1 | 1.1, 2.1 | MD-C |
| 4.1 | blocked-by MD 2.1 | 2.1 | MD-C |
| 4.3 | blocked-by MD-C | 2.1, 4.1 | MD-U1 |
| 4.2 | blocked-by MD 4.3 | 4.1, 4.3 | MD-U2 |
| 5.1 | done on `md/5.1` (7e9858a, parked), merges in MD-D | — | MD-D |
| 5.2 | blocked-by all MD | all | MD-D [live] manual if 1.3 found a file |

## add-chat-members-and-creation (CM)

| ID | Status | Needs | Files / notes |
|----|--------|-------|---------------|
| 0.1 | ready | — | `design.md` only; PASS/FAIL gates 6.1 |
| 1.1 | blocked-by MD 1.1 (proto `client.rs`) | — | `request.rs`, `client.rs`, `write.rs`; allowlist becomes 8 names |
| 1.2 | blocked-by CM 1.1, GS 1.3, MD 1.2 (`bt-probe.rs`) | 1.1 | `bt-probe.rs`, `README.md`, fixtures, `design.md` [live] |
| 1.3 | blocked-by CM 1.2, GS 1.4 (`wire_fixtures.rs`) | 1.2 | `wire.rs`, `wire_fixtures.rs`, `client.rs`, `request.rs` |
| 2.1 | ready | — | `manage.rs`, `registry.rs`, `lib.rs` |
| 2.2 | blocked-by CM 2.1, 1.3 | 2.1, 1.3 | `client.rs`, `bt-probe.rs`, `README.md` |
| 3.1 | blocked-by CM 1.3, 2.2 + core lane | 1.3, 2.2 | CM-C1 |
| 3.2 | blocked-by CM 3.1 | 3.1 | CM-C2 |
| 3.3 | blocked-by CM 3.2 | 3.2 | CM-C3 |
| 4.1 | blocked-by CM 3.2 + UI lane | 3.2 | CM-U1 |
| 4.2 | blocked-by CM 4.1 | 3.2, 4.1 | CM-U2 |
| 4.3 | blocked-by CM 3.3, 4.2 | 3.3, 4.2 | CM-U3 |
| 5.1 | blocked-by CM 2.2 | 2.2 | **[approval] [live]**: real private chat creation, the user approves the target |
| 5.2 | blocked-by CM 4.3 | 4.3 | VM run, `BT_WRITE_ONLY_CHAT` = self chat |
| 5.3 | blocked-by CM 5.1, 5.2 | 5.1, 5.2 | **[approval] [live]** |
| 6.1 | blocked-by CM 0.1 (static PASS), 2.2 | 0.1, 2.2 | probe support only; `manage.rs`, `registry.rs`, `client.rs`, `bt-probe.rs`, `README.md` |
| 6.2 | blocked-by CM 6.1 | 6.1 | **[approval] [live]**: real group creation (throwaway title, participants the user approves); live gate for 6.3, 6.4 |
| 6.3 | blocked-by CM 6.2 (live PASS), 3.3 | 6.2, 3.3 | CM-G1 |
| 6.4 | blocked-by CM 6.3, 4.3 | 6.3, 4.3 | CM-G2 |
| 6.F | conditional | 0.1 FAIL or 6.2 FAIL | removes group wording and code |
| 7.1 | blocked-by CM 4.3 and 6.4 (or 6.F) | | |

## add-message-reactions (MR)

| ID | Status | Needs | Unit |
|----|--------|-------|------|
| 1.1 | in progress (rx/1.1, `research.md` only; merges alone) | — | |
| 2.1 | parked (rx/2.1 work in progress, must not merge alone) | — | MR-B1 |
| 2.2 | cancelled, restarts in MR-B1 | — | MR-B1 |
| 3.1 | cancelled, restarts in MR-B1 | — | MR-B1 |
| 3.2 | blocked-by MR-B1 start | 3.1 | MR-B1 |
| 3.3 | blocked-by MR 1.1, MR-B1 | 1.1, 3.1 | MR-B2 |
| 4.1 | blocked-by MR-B1 start | 2.1, 2.2, 3.1 | MR-B1 |
| 5.1 | blocked-by MR-B1 start | 3.1 | MR-B1 |
| 5.2 | blocked-by MR-B1 | 3.1 | MR-B2 |
| 5.3 | blocked-by MR-B1 | 3.1, 5.1 | MR-B2 |
| 5.4 | blocked-by MR 5.1–5.3 | | MR-B2 |
| 6.1 | blocked-by MR-B1 | 2.1, 2.2 | MR-P |
| 7.1 | blocked-by all MR code | | |
| 7.2 | blocked-by MR 1.1, 6.1, 7.1 | | **[live]**, writes in the self chat only (`BT_WRITE_ONLY_CHAT`); confirm with the user |
| 7.3 | blocked-by MR 7.1, 7.2 | | archive |

## Ready now (nothing in flight touches their files)

- MD 2.2 (MD-K)
- CM 0.1
- CM 2.1

In flight: MD 1.1 (md/1.1), MR 1.1 (rx/1.1). Unblocks when MD 1.1 merges: GS 1.2, CM 1.1, MD 2.1 (then 2.3, 3.1, 4.1).

## Needs the user

- Approval of the target: CM 5.1, CM 5.3, CM 6.2 (real chat and group creation).
- Writes on the real account to confirm: MR 7.2 (self chat only).
- Live session, reads only, no approval: GS 1.3, GS 4.1, CM 1.2, CM 5.2 (VM), MD 1.3 / 5.2 (may end "unverified" when the self chat holds no file).
