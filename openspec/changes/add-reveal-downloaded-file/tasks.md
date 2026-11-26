# Tasks

Contracts (signatures, labels, selectors) are fixed in `design.md` D3, D5, D6, D7, so tasks can be coded against them in parallel. Each task names the files it owns; Integrator rules from `openspec/changes/DEPENDENCIES.md` apply (one owner per file, every merge to `main` compiles and passes tests with no stubs and no warnings, cargo and the app only in the Tart VM `bt-vm`, no `#[allow(dead_code)]`). Unit tests of a task belong to that task, except the shared `tests.rs` (task 4.1). No question is open; all decisions are recorded in `design.md`.

Order at a glance: 1.1, 2.1 and 3.1 start immediately (distinct files); 2.2 follows 2.1 (the `reveal` flag and `REVEAL_LABEL`); 4.1 follows 1.1, 2.1, 2.2; 5.1 follows the unit; 5.2 follows 5.1 and needs the VM network and a session; 5.3 follows everything.

Compile coupling: `StoreEvent::Reveal` and `reveal_download` (1.1) have no caller until 2.1 and 2.2 (a `dead_code` warning in the binary crate otherwise), and the event is matched by no one until 3.1; adding `ActionLine.reveal` (2.1) breaks the exhaustive `ActionLine { status, action, enabled }` pattern in `timeline.rs::action_row` until 2.2 updates it. None of these tasks can reach `main` alone, so they are one merge unit.

Overlap with the open changes (`DEPENDENCIES.md` rule 1): `store.rs` is held by RL 6.1/6.4/6.5 and PIN 4.1; `timeline.rs` by PIN 4.2; `main.rs` by RL 6.2; `tests.rs` by RL 6.7, PIN 4.4 and CM-G (not started, locks nothing, rule 6). `viewer.rs` is free. A task here starts only when its file is free; nothing here blocks those changes.

## Merge unit

- **REV** = 1.1, 2.1, 2.2, 3.1, 4.1 (branch `integ/reveal-downloaded-file`). Checks, in `bt-vm` in its own `~/bt/src-reveal` (never the shared `~/bt/src`): `cargo build --workspace --all-targets --locked` with zero warnings, `cargo test -p blazing-telesram --locked`, `cargo clippy -p blazing-telesram --all-targets --locked` (no new warnings against `main`), `cargo fmt --all --check`, `openspec validate add-reveal-downloaded-file --strict`; then `git pull --rebase`.

## 1. Store

- [x] 1.1 `StoreEvent::Reveal` and `ChatStore::reveal_download` (D3)
  - Owns: `crates/desktop/src/store.rs`.
  - Depends on: nothing. Waits until RL 6.1/6.4/6.5 and PIN 4.1 have merged or are not started (rule 1).
  - Do: add `StoreEvent::Reveal(PathBuf)` with a doc comment; add `pub fn reveal_download(&mut self, original: &MediaRef, cx: &mut Context<Self>)` per D3: nothing unless `Saved(path)`; `path.try_exists()` is `Ok(false)` -> `set_download(original.clone(), None, cx)` and no event, otherwise `cx.emit(StoreEvent::Reveal(path))`. No command is sent, no log line, the connection is not consulted (D4, D8). Update the doc comments of `DownloadState::Saved` only if they claim something this contradicts.
  - Tests: none here (they live in `tests.rs`, task 4.1).
  - Done when: the method and event exist with the exact signature of D3 and the unit builds with 2.1, 2.2, 3.1.

## 2. Views

- [x] 2.1 `ActionLine.reveal`, the label and the viewer button (D5, D6)
  - Owns: `crates/desktop/src/viewer.rs` (including its unit tests).
  - Depends on: 1.1 for `reveal_download` (code against D3).
  - Do: `ActionLine` gains `pub reveal: bool`, true exactly for `Some(DownloadState::Saved(_))`; `pub const REVEAL_LABEL: &str = "Показать в Finder"`; the viewer's bottom bar shows a `viewer-reveal` button before the download button when `line.reveal`, in `ACCENT`, whose click calls `this.store.update(cx, |store, cx| store.reveal_download(&original, cx))` for `item.media.original()`; it is enabled in every connection state (D4); the existing download button, status text and selectors are unchanged; update the `ActionLine` literals of the existing tests in the file.
  - Tests: `action_line` gives `reveal: true` for `Saved` and `false` for no state, `Running` and every `Failed(_)`, with `online` true and false; the existing assertions still hold with the new field.
  - Done when: these tests exist.

- [x] 2.2 The file card link (D5)
  - Owns: `crates/desktop/src/timeline.rs`.
  - Depends on: 1.1, 2.1. Waits until PIN 4.2 has merged or is not started (rule 1).
  - Do: `action_row` destructures the new `reveal` field and, when true, renders a «Показать в Finder» (`REVEAL_LABEL`) link between the status text and the download button: selector `msg-{ts}-reveal`, element id `file-reveal-{ts}`, `ACCENT`, `cursor_pointer`, click `store.update(cx, |s, cx| s.reveal_download(&original, cx))`; always enabled; the row keeps its 18 px height and `flex_1 min_w_0 truncate` status, the card its 300 px width; the doc comment of `file_card` lists the new selector.
  - Tests: none here (rendered in `tests.rs`, task 4.1).
  - Done when: the card shows the link in the `Saved` state and only then.

## 3. Wiring

- [x] 3.1 The platform call (D7)
  - Owns: `crates/desktop/src/main.rs`.
  - Depends on: 1.1. Waits until RL 6.2 has merged or is not started (rule 1).
  - Do: in the app-level `cx.subscribe(&store, ...)` of `open_chat_window`, handle `StoreEvent::Reveal(path)` with `cx.reveal_path(path)` (the existing `StoreEvent::Session` branch keeps its behaviour); no logging of the path (D8). If the closure's shape forces a `match`, keep the session arm byte-for-byte.
  - Tests: none (the test harness never runs `main.rs`; `TestPlatform::reveal_path` is `unimplemented!()`); checked by hand in 5.2.
  - Done when: the `StoreEvent::Reveal` arm calls `cx.reveal_path(path)` and nothing else, the session arm is unchanged, and `cargo build --workspace --all-targets` has no new warning. Observing Finder is not part of this task: it is only task 5.2.

## 4. Tests

- [x] 4.1 Store and UI tests
  - Owns: `crates/desktop/src/tests.rs` (a new section `// ---- reveal in Finder (REV 4.1) ----` at the end).
  - Depends on: 1.1, 2.1, 2.2. Waits until RL 6.7, PIN 4.4 have merged or are not started (rule 1).
  - Do, in the style of the existing `download_*` tests (store harness; real files in `tempfile::tempdir()` where the file must exist): (a) `reveal_download` on a `Saved` state whose file exists emits `StoreEvent::Reveal(path)` with exactly that path and sends no command (`h.commands()` is empty); (b) on a state whose file was deleted it emits no `Reveal`, clears the state (`download_state` is `None`) and sends no command; (c) with no state, `Running` and `Failed(_)` it emits nothing and changes nothing; (d) while the connection is `Offline` and `Connecting` it still emits `Reveal`; (e) after a second `ClientEvent::Downloaded` for the same original with a new path, the event carries the new path; (f) another original's state is untouched; (g) render: a file card shows `msg-{ts}-reveal` only in the `Saved` state (not before a download, not while running, not after a failure) and clicking it yields the event; the viewer shows `viewer-reveal` for a saved image original and not otherwise, and clicking it yields the event; (h) nothing in the file calls the platform's `reveal_path` (the tests only observe the event).
  - Done when: these tests exist and pass in the unit run.

## 5. Verification and close-out

- [x] 5.1 Final full run
  - Owns: nothing; fixes only compile breaks that cross task boundaries.
  - Depends on: 1.1, 2.1, 2.2, 3.1, 4.1 and the REV unit on `main`. The per-unit checks of "Merge unit" are not replaced by this task: they run before the unit merges.
  - Do: on `main`, in `bt-vm`: `cargo test --workspace --locked`, `cargo clippy --workspace --all-targets --locked`, `cargo fmt --all --check`, `cargo build --workspace --all-targets --locked` with zero warnings; `openspec validate add-reveal-downloaded-file --strict`.
  - Done when: all pass.

- [ ] 5.2 Hand check in the VM **[live]**
  - Owns: nothing (findings are reported in the task result; no artifact is committed).
  - Depends on: 5.1; the VM's outbound network and a signed-in session in the VM's `~/bt/.runtime` (without them: report "blocked by environment", leave the box unchecked; it is not a gate for archiving, see 5.3). Its own `~/bt/src-reveal`, removed when done; the shared `~/bt/src` is not used. Live rule 5 of `DEPENDENCIES.md`: one live task at a time. A download is a read of the server and writes only to the VM's `~/Downloads`; no account write, `BT_WRITE_ONLY_CHAT` may stay set.
  - Do: run the app, download one file and one image original from a chat the account already has, choose «Показать в Finder» on the card and in the viewer (a Finder window shows the file selected), delete the file in Finder and choose it again (the card returns to «Скачать»), restart the app (no button until a new download). Remove the downloaded test files from the VM afterwards. Report only booleans, never file names.
  - Done when: each step has an observed answer or an explicit "not observed".

- [ ] 5.3 Archive
  - Owns: the change directory and `openspec/specs` through the archive workflow.
  - Depends on: 1.1, 2.1, 2.2, 3.1, 4.1, 5.1 checked; 5.2 checked or explicitly reported as blocked by environment (the user decides whether to wait for it).
  - Do: follow the `openspec-archive-change` skill; the `attachment-download` main spec gains «Reveal in Finder» and the modified «Authenticated and private transfer»; update `DEPENDENCIES.md` (move the change to "Archived").
  - Done when: the change is archived and the main spec holds the new and modified requirements.
