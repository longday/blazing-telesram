# Design

## Context

`proposal.md` states the motivation. Code this change extends (all in `crates/desktop/src`; read on 2026-11-27):

- `store.rs`: `DownloadState {Running, Saved(PathBuf), Failed}`, `ChatStore::downloads: HashMap<MediaRef, DownloadState>` in memory only, `download_state(&original)`, `download(...)`, private `set_download(media, Option<state>, cx)` which emits `StoreEvent::Download(media)`; `ClientEvent::Downloaded {media, path}` becomes `Saved(path)`. Nothing about downloads is persisted (a restart forgets every state). The path is the final path the core chose under `config.downloads_dir` (`$HOME/Downloads`, `main.rs::downloads_dir`).
- `viewer.rs`: `Action` (`Download`, `Cancel`, `Retry`, `DownloadAgain`), `ActionLine {status, action, enabled}` and the pure `action_line(state, online)` used by both surfaces; `render` builds the viewer's bottom bar (status text then one button).
- `timeline.rs`: `file_card` and `action_row` render the card's one action line from `ActionLine` (fixed 18 px row; selectors `msg-{ts}-saved|status|download|cancel|retry`). The card and the viewer share the state key `media.original()`.
- `main.rs`: `open_chat_window` holds the app-level `cx.subscribe(&store, ...)` that reacts to the session ending. It is not part of the test harness (`tests.rs::harness_with` builds `Desktop::new` directly).
- `gpui` 0.2.2 (pinned): `App::reveal_path(&Path)` on macOS spawns on the background executor an `NSWorkspace selectFile:inFileViewerRootedAtPath:` call (selects, never opens); `App::open_with_system` runs `open <path>`. In the test platform `reveal_path` is `unimplemented!()`.

## Goals / Non-Goals

**Goals:**
- One local action, «Показать в Finder», for a file saved in this session, on the card and in the viewer, with contracts fixed here so the store, the two views, the wiring and the tests can be written in parallel.

**Non-Goals:** the proposal's non-goals (opening, persistence across launches, scanning Downloads, copy path, other platforms).

## Decisions

### D1. Reveal, never open

The platform call is `App::reveal_path(&path)` (Finder window with the file selected). Rejected: `open_with_system` (opens the file: contradicts the spec's "SHALL NOT open … a saved file"), a hand-written `open -R` subprocess (a process spawn and shell-style argument for what gpui already does natively), `NSWorkspace` through `objc2` directly (new unsafe code, duplicates gpui). The call's result is not observable (gpui discards the `BOOL` and runs it on a background executor); a Finder that cannot select the file simply does nothing. Accepted.

### D2. Only files saved in this session

The action is offered exactly when `store.download_state(&original)` is `Some(Saved(_))`. The state is in memory, so after a restart the button is gone and the card offers «Скачать»: no persistence, no lookup or index of `~/Downloads` (the spec forbids indexing saved files), no new stored personal data. A later persistent "saved at" record is a follow-up, not part of this change.

### D3. The file is checked when the user chooses, and a missing one clears the card

```rust
// store.rs
pub enum StoreEvent { ..., Reveal(PathBuf) }      // "select this saved file in Finder"
impl ChatStore {
    pub fn reveal_download(&mut self, original: &MediaRef, cx: &mut Context<Self>)
}
```

`reveal_download` does nothing unless the state is `Saved(path)`. It then asks `path.try_exists()`:

- `Ok(false)` (the user deleted or moved the file): the state is cleared with `set_download(original.clone(), None, cx)`, which emits `StoreEvent::Download` and puts the card back to «Скачать» (as after a cancel); then `reveal_download` calls `cx.notify()` itself, because `set_download` does not notify and the card must re-render; no `Reveal`. Rejected: keeping `Saved` and doing nothing (a dead button that never explains itself), a new `DownloadFailure` variant or a toast (a new type in `chat-core`/`model.rs` and files outside this change's scope, for a rare case); the card that stops claiming «Сохранено» is the honest answer.
- `Ok(true)` or `Err(_)` (for example a permission error on a stat): emit `StoreEvent::Reveal(path.clone())`; Finder decides. Only a definite "does not exist" clears the state.

The stat runs on the UI thread: one metadata call on a local path, on a user click. Accepted.

### D4. Works offline

The action is local, so it is enabled whatever the connection (`ActionLine.enabled` keeps governing the download button only). The viewer and the card show it in the `Saved` state in every connection state.

### D5. Label, selectors, placement

```rust
// viewer.rs
pub struct ActionLine { pub status: Option<String>, pub action: Action, pub enabled: bool,
                        pub reveal: bool }          // true exactly for DownloadState::Saved
pub const REVEAL_LABEL: &str = "Показать в Finder";
```

- File card: the action row becomes `[«Сохранено: name» (flex, truncates)] [Показать в Finder] [Скачать ещё раз]`; the row keeps its 18 px height and the card its 300 px width, the status text is what shrinks. Selector `msg-{ts}-reveal`; element id `file-reveal-{ts}`.
- Viewer: the bottom bar shows `[status] [Показать в Finder] [Скачать ещё раз]`; selector `viewer-reveal`.
- Colour: `ACCENT` (as the other actions); no theme change.
- The viewer reveals the saved original of the image it shows (`item.media.original()`), the same key the download used.

### D6. Sharing `action_line`

The two surfaces already share the pure `action_line(state, online)`; the new `reveal` flag belongs there so that "when is the action offered" is decided once and tested without a window (`Saved` gives `reveal: true`; `Running`, `Failed` and no state give `false`, online or not).

### D7. Where the platform call lives

The store emits `StoreEvent::Reveal(path)`; the app-level subscription in `main.rs::open_chat_window` calls `cx.reveal_path(path)`. Reason: the test platform's `reveal_path` is `unimplemented!()`, and `Desktop` is built inside the test harness, so a handler in `desktop.rs` would panic as soon as a test chose the action. With the call in `main.rs`, which tests never run, store and view tests assert the emitted event and stay headless. Rejected: a trait or closure injected into `Desktop::new`/`ChatStore::new` (changes constructor signatures used by every test, for one call) and a `cfg(test)` no-op (a fake that hides the real path from review). The one line in `main.rs` is verified by hand (task 5.2).

### D8. Nothing logged

No `tracing` line mentions the path or file name at `debug` or above (the spec's privacy rule, now also naming saved paths); the reveal logs nothing at all.

### D9. Ordering with other changes

`store.rs`, `timeline.rs`, `main.rs` and `tests.rs` are also in the file lists of open RL, PIN and CM tasks; `viewer.rs` is free. By `DEPENDENCIES.md` rule 1 a task here starts only when no in-flight task edits its file; this change has no urgency over them, so it queues behind the RL and PIN units that hold the shared files (see "Overlap" in `tasks.md` and the table in `DEPENDENCIES.md`). Rule 3: the store method and event have no caller until the views and `main.rs` use them, so tasks 1.1, 2.1, 2.2, 3.1 and 4.1 are one merge unit (REV).

## Risks / Trade-offs

- **The Finder call is untested by code.** The test platform cannot run it; the unit tests prove "the right path is emitted for the right state". Mitigation: one hand check in the VM (task 5.2, needs the VM network and a session, so it may stay blocked and is not a merge gate), and the call is a single line of a pinned `gpui` API.
- **Stat on the UI thread.** One call per click. Accepted (D3).
- **Silent no-op if Finder cannot select the file** (D1). Accepted; no more can be known from `gpui`.
- **A path that is no longer the one downloaded** (the user replaced the file with another of the same name): Finder selects whatever is there; not detectable and not our business.
