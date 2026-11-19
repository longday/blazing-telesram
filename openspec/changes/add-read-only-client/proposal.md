## Why

The desktop app shows only synthetic chats. The goal is a client that looks like Telegram Desktop and works with Yandex Telemost chats; the fastest useful step is reading a real Yandex 360 account: signing in, browsing chats, reading history and receiving new messages live. Sending and every other feature come in later changes once this foundation is proven against the live service.

## What Changes

- Sign in through an embedded WebKit window on `telemost.yandex.ru`; keep the resulting Passport session under `.runtime/` and reuse it on later launches until it expires.
- Connect to the Telemost web client's protocol (Xiva WebSocket plus the messenger registry HTTP API, as used by `telemessenger` 3.6.1) and load the account identity.
- Replace synthetic chats with the account's real chat list: names, avatars, last message preview, unread counters, pinned and muted state; newest activity first.
- Show real message history with upward pagination, image thumbnails and file cards (no download yet).
- Apply incoming messages and chat list changes live while connected; reconnect after network loss.
- Cache chats and loaded history in SQLite under `.runtime/` so the app starts with the last known state before the network answers.
- Guarantee read-only behavior: the client sends no message, read marker, typing event or any other state-changing request.
- **BREAKING**: remove the offline demo: synthetic data, local send/reply, `BT_STATE_PATH` persistence, the mock "meetings" filter and the `telemost-mock-check` MCP experiment with its fixtures.
- Add a headless developer probe that exercises the protocol with the stored session, without opening windows.

Planned follow-up changes, in order (not part of this change):
1. Sending: text, reply, edit, delete, read markers, typing, drafts.
2. Media: image and file upload/download, gallery, voice and video playback.
3. Threads, reactions, mentions, forwarding, pins, important messages.
4. Search, folders, chat filters, mark unread.
5. Chat management: create chats, members, roles, invite links, profile and statuses.
6. Polls, stickers, bots.
7. macOS notifications, Dock badge, settings, themes.
8. Telemost calls and meetings.

## Capabilities

### New Capabilities

- `account-session`: signing in to a Yandex account, storing and reusing the session, detecting expiry and signing out.
- `chat-list`: the account's chats with metadata, ordering, unread state, avatars and local filtering.
- `message-history`: reading a chat's messages with pagination, text, image thumbnails and file cards, without changing server state.
- `live-updates`: realtime connection, incoming message and chat list events, connection status and reconnection.
- `offline-cache`: local persistence of chats and loaded history for instant start and offline viewing.

### Modified Capabilities

None: no specs exist yet.

## Impact

- `crates/chat-core`: synthetic `Core`, `LocalChatService` and `telemost_mock` replaced by a protocol client, domain model and cache; `ChatService` becomes asynchronous and keyed by server IDs.
- `crates/desktop`: UI bound to real data; composer and thread reply inputs disabled; new login window.
- New dependencies (exact versions pinned in design): WebView, WebSocket, HTTP, MessagePack, SQLite, async runtime.
- `.runtime/` holds the live session cookies and cached private messages; it must stay git-ignored.
- Uses an unofficial, reverse-engineered protocol: Yandex can change or block it at any time; this risk is accepted.
- Live verification runs without windows on the developer's desktop: headless probe and GPUI tests on the host, visual checks in a macOS VM (separately approved setup).
