# Proposal

## Why

A downloaded attachment lands in `~/Downloads` and the card says «Сохранено: report.pdf», but the user then has to leave the app and find the file in Finder by hand. Roadmap item 2c of `add-read-only-client` (media: file download, then the follow-ups) asks for «Показать в Finder»: one click that selects the saved file in a Finder window. It is a purely local action: it reads no server state, writes nothing to the account and does not open or run the file.

`attachment-download` (archived `add-media-download`) already holds everything needed to know *where* the file is: `DownloadState::Saved(PathBuf)` in the desktop store, keyed by the original's `MediaRef`, set from `ClientEvent::Downloaded`. Nothing was built on top of it besides the status text «Сохранено: <name>» and the «Скачать ещё раз» action, in the file card (`timeline.rs::file_card`/`action_row`) and in the image viewer (`viewer.rs::action_line`).

## What Changes

- `desktop`: a «Показать в Finder» action next to the saved status, on the file card and in the image viewer, offered only while the download state is `Saved`. Choosing it asks the platform to reveal (select) the saved file in Finder through `gpui`'s `App::reveal_path` (`NSWorkspace selectFile:inFileViewerRootedAtPath:`); the file is selected, never opened.
- `desktop::store`: `ChatStore::reveal_download` checks that the saved file still exists and emits a new `StoreEvent::Reveal(path)`; if the file is gone it forgets the saved state, so the card offers «Скачать» again. The platform call is made by the app-level subscription in `main.rs`, because the gpui test platform's `reveal_path` is `unimplemented!()` and the store and view tests must stay headless.
- `attachment-download` spec: one new requirement («Reveal in Finder») and one modified one («Authenticated and private transfer»: its sentence "the client SHALL NOT open, execute or index a saved file" gains the explicit exception of selecting the file in Finder).

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `attachment-download`: ADDED «Reveal in Finder»; MODIFIED «Authenticated and private transfer» (reveal is allowed, opening is not).

## Impact

- `crates/desktop/src`: `store.rs` (event and method), `viewer.rs` (`ActionLine` gains `reveal`, the label, the viewer button), `timeline.rs` (the card link), `main.rs` (the platform call), `tests.rs`.
- No change to `chat-core`, `yandex-proto`, `Command`/`ClientEvent`, the offline cache or any request: the saved path already travels in `ClientEvent::Downloaded`. No new dependency (`gpui` 0.2.2 has `reveal_path`).
- Specs: `attachment-download` only.
- Risk: the real Finder call cannot be exercised by unit tests (see design D1/D7); it is checked once by hand in the VM when a session is available (task 5.2), and is a single call to an API of the pinned `gpui`.
- Overlap with open changes: `store.rs` (RL 6.1, 6.4, 6.5; PIN 4.1), `timeline.rs` (PIN 4.2), `main.rs` (RL 6.2), `tests.rs` (RL 6.7, PIN 4.4, CM-G); `viewer.rs` is free. Details in `openspec/changes/DEPENDENCIES.md`.

## Non-goals

- Opening the file (`open_with_system`), previewing it, "Open with…", drag-out, copying the path: the spec keeps "the client SHALL NOT open … a saved file".
- Remembering saved files across launches or scanning `~/Downloads` for earlier downloads: after a restart a card offers «Скачать» again, as today.
- Any server call or write; a Finder action for avatars and previews (only originals are ever saved).
- Platforms other than macOS (the app is macOS-only).

## Follow-ups

- Persisting a "saved at" record per attachment so the button survives a restart (needs a new persisted field and a rule for stale entries; not asked for).

## Decisions

Taken without asking, each the conservative option and recorded in `design.md` (D1-D8): reveal, not open (D1); only files saved in this session (D2); a missing file clears the card instead of failing silently (D3); available offline (D4); label, selectors and placement (D5); the platform call outside the store and views (D7); nothing logged (D8).
