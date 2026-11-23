# Proposal

## Why

The client only reads. To replace the web client for daily chatting, the user must write: send and answer messages, fix or remove their own, and have chats they read stop counting as unread. This is step 1 of the follow-up order in the archived `add-read-only-client` proposal; the target look is `docs/mockups/1-main.svg` (composer, reply bar) and `docs/mockups/5-menus.svg` (message context menu).

## What Changes

- **BREAKING** The read-only guarantee ends: the protocol layer gains a closed set of write requests sent over the socket method `push`; anything outside that set stays unrepresentable.
- Composer: a multi-line text field (Enter sends, Shift+Enter breaks the line, grows up to a limit), replacing the disabled composer and its notice.
- Sending text, with an optional reply to a message of the open chat; the message shows at once as pending, then as sent or failed with retry; sends made offline wait and go out after reconnecting without duplicates.
- Editing and deleting one's own messages from a message context menu (also Copy text and Reply); Up arrow in an empty composer edits the last own message.
- Per-chat local drafts, kept across chat switches and restarts.
- Read markers: reading the bottom of a chat in the focused window marks it read on the server, so the unread counter drops here and in other clients.
- Typing: the client reports the user's typing and shows other people's typing in the chat header and chat row.
- Task 3.4 of the archived change (side effects of reading) moves here, since read markers make it checkable.

Non-goals: mentions, forwarding, reactions, pins (change 3); files and images (change 2); presence status (online, away).

## Capabilities

### New Capabilities
- `message-sending`: composing and sending text messages, replies, delivery states, offline outbox and drafts.
- `message-editing`: editing and deleting the user's own messages and the message context menu.
- `read-markers`: when and how the client marks a chat read on the server.
- `typing-indicator`: reporting the user's typing and showing other people's typing.

### Modified Capabilities
- `message-history`: the "Read-only client" requirement is removed; its write restrictions move to `message-sending` as "Writes only from user actions".

## Impact

- `crates/yandex-proto`: write request enum, `push` envelope, commit-status parsing, mutating-method allowlist and its tests.
- `crates/chat-core`: new commands, outbox and drafts in the SQLite cache, optimistic messages, read-marker logic.
- `crates/desktop`: multi-line `TextInput`, composer, reply/edit bars, context menu, delete confirmation, delivery marks, typing line.
- `bt-probe`: a send command limited to the self chat for live verification.
- Risk: these requests change the user's real account; all live verification uses the self chat.
