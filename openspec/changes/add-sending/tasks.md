# Tasks

## 1. Shared contracts

- [x] 1.1 Extend `chat-core` model: `Delivery` on `Message`, `Command::{Send, Retry, Edit, Delete, MarkRead, Typing, SaveDraft}`, `ClientEvent::{Typing, Draft, ActionFailed}`, `PayloadId`; extend `Backend` with `push`-backed methods; verify `cargo build --workspace --all-targets` passes with 0 warnings and existing tests pass

## 2. Protocol writes (`yandex-proto`)

- [x] 2.1 `WriteRequest` closed enum and `push` envelope encoding (D1); verify unit tests on exact JSON for send, reply, edit, delete, seen, typing and a test that `push` is the only new method and the `ClientMessage` variants are exactly `Plain`, `SeenMarker`, `Typing`
- [x] 2.2 Commit status parsing into `Committed`/`ProtoError::Commit` with permanent/transient split (D2); verify unit tests for statuses 1, 8, 15, 18 and a missing status
- [x] 2.3 `XivaHandle::push` waits for `subscribed`, fails fast offline, carries the uid; verify fake-socket tests for online push, offline failure and push after reconnect using the new subscription id
- [x] 2.4 Decode `PayloadId` of own-message echoes and incoming typing pushes (`timeout_seconds`); verify decoder tests on captured shapes

## 3. Core sending (`chat-core`)

- [x] 3.1 Cache migration 2 with `outbox` and `drafts` (D3, D9); verify migration test from a version-1 file keeps existing rows
- [x] 3.2 Outbox engine: one send in flight in order, stable `PayloadId`, retry and backoff for transient failures, failed state for permanent ones, Retry command (D3); verify engine tests with a fake backend for confirm, duplicate, permanent failure, offline-then-reconnect and restart with a pending entry
- [x] 3.3 Pending message replaced exactly once by confirmation or echo, whichever is first; verify tests for both orders
- [x] 3.4 Edit and delete apply after success and report failures (D4); verify tests for success, rejection and offline
- [x] 3.5 Read markers deduplicated per chat; unread counter drops to 0 when the newest message is marked (D6); verify tests
- [x] 3.6 Typing: outgoing throttled to 4 s per chat; incoming events expire and clear when the typist's message arrives (D7); verify tests with a paused clock
- [x] 3.7 Drafts saved, restored and deleted on send; verify tests
- [x] 3.8 Carried over from the archived change: verify by test and live probe that loading chats and history (including older pages) sends no write request and leaves unread counters in the web client unchanged

## 4. Composer (`desktop`)

- [x] 4.1 Multi-line `TextInput` mode with wrapping, caret/selection/hit-testing across wrapped lines, growth to 8 lines then inner scroll (D8); verify tests for wrapping, caret movement and Shift+Enter
- [x] 4.2 Composer replaces the read-only notice: Enter sends, blank text ignored, cleared after send; remove `READ_ONLY_NOTICE` and its tests; verify desktop tests
- [x] 4.3 Delivery marks (clock, check, red "!") and Retry on failed bubbles; verify render tests
- [x] 4.4 Drafts: restore on chat switch, "Черновик:" preview in chat rows; verify tests

## 5. Message actions (`desktop`)

- [x] 5.1 Context menu on right-click with items per ownership (`docs/mockups/5-menus.svg`), Copy text to clipboard, close on Escape/outside click; verify tests
- [x] 5.2 Reply bar and Escape cancel; sent reply carries the target; verify tests
- [x] 5.3 Edit bar, Up arrow in empty composer, Escape restores the previous draft; verify tests
- [x] 5.4 Delete confirmation modal; verify tests for confirm and cancel
- [x] 5.5 Error toast for failed edits and deletions; verify test

## 6. Read state and typing (`desktop`)

- [x] 6.1 Send `MarkRead` for the newest visible message while the window is active (D6); verify tests for active, inactive and scrolled-up cases
- [x] 6.2 Send `Typing` on composer edits; show "печатает…" in the header and chat row; verify tests

## 7. Live verification (self chat only)

- [x] 7.1 `bt-probe` commands `send-self`, `edit-self`, `delete-self`, `seen`, `typing-self`, refusing other chats for the self-chat ones (D11); verify a test for the refusal
- [x] 7.2 Live send, duplicate resend, edit and delete in the self chat; verify with `bt-probe history` and the web client (resend with the same payload returned status 8 with the original timestamp; the web client shows one copy, the edit mark and the deletion)
- [x] 7.3 Live reply in the self chat; verify the quote in the web client, or remove reply from the change if the form fails (quote shown in `bt-probe history`, the app and the web client)
- [ ] 7.4 Live typing; verify the push in `bt-probe listen` and record `timeout_seconds`, or remove outgoing typing if the form fails (done: push accepted with status 1; open: the server does not echo own typing to the self chat, so the push needs a peer's chat or a second account)
- [x] 7.5 VM run: send, reply, edit, delete, offline send and drafts in the self chat through the app UI; verify by screenshots and `bt-probe history` (offline: a send during real network timeouts and one with the VM's network off were each delivered once; restart while offline was not run)
- [x] 7.6 Workspace build with 0 warnings and all tests pass; code review by the `reviewer` agent with findings fixed
