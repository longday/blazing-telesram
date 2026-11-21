# Design

## Context

- `crates/chat-core` holds a synthetic `Core`, a synchronous `ChatService` keyed by `usize` indexes, `BT_STATE_PATH` event persistence and the unused `telemost_mock`; `crates/desktop` renders it with GPUI 0.2.2 (`list`/`ListState` timeline, custom `TextInput`). Nothing does I/O.
- There is no public API. `telemost.yandex.ru` serves the `telemessenger` 3.6.1 web client; its `globalParams` (anonymous fetch, 2026-11-19) point to the same endpoints reverse-engineered in `conarti/yandex-messenger-mcp`:
  - registry HTTP `https://yandex.ru/messenger/api/registry/api/` (multipart field `request` = JSON `{method, params}`), CSRF from `…/registry/csrf-token/`;
  - Xiva WebSocket `wss://push.yandex.ru/v2/subscribe/websocket?service=messenger-prod:…&session=…&client=web_main&user=<uid>`;
  - files `files.messenger.yandex.ru` (`file_shortterm/{fileId}?size=`), avatars via `get-mssngr`/`get-yapic` and `files…/{avatarId}?size=`;
  - `historyChunkSize` 50, `reconnectInterval` 11000 ms; `backendConfig.chat_list_excluded_namespaces`, `hidden_namespaces`.
- Auth is the Passport cookie session (`Session_id`, `sessionid2`, `yandexuid`, …) of `.yandex.ru`. Identity comes from the registry HTTP method `request_user` with params exactly `{bind_phone_number: false}`; it requires `X-CSRF-TOKEN` (403 `bad_csrf_token` without it) and returns `data.user.{uid, guid, display_name}`; Xiva's `user=` parameter is `uid`, falling back to `guid` (MCP `src/auth/requestUser.ts`). A WS identity call is not confirmed and is not used.
- WS frame: `0x01` + MessagePack `[serviceIndex, reqId, method]` + `0x05` + 11 zero bytes + UTF-8 JSON body; server pushes use frame type `0x03`; replies correlate by transport `reqId` and JSON `RequestId`.
- Project rules: exact dependency versions, desktop app runs on the host (no containers), Telegram Desktop design without its code; graphics reuse would force GPL-3.0, so this change uses none.

## Goals / Non-Goals

**Goals:**
- A thin, testable protocol layer whose public surface can only express read requests.
- Network work off the UI thread; the UI reacts to a single event stream.
- Every live behavior verifiable without opening windows on the developer's desktop.

**Non-Goals:**
- Mutating requests of any kind, even behind flags.
- Downloading or opening files, playing media, notifications, multiple accounts at once.
- Pixel-exact Telegram styling: layout, spacing and palette are matched by eye from Telegram Desktop screenshots; refinement continues in later changes.

## Decisions

### Crate layout
- `crates/yandex-proto` (new): wire types, frame codec, registry HTTP client, Xiva WS client, session file. No GPUI, no SQLite.
- `crates/chat-core`: domain model (`ChatId`, `MessageKey` = chat + server timestamp), sync engine, SQLite cache, `ChatClient` handle. Synthetic code and `telemost_mock` are deleted.
- `crates/desktop`: GPUI views and the login window.
- `crates/yandex-proto/src/bin/bt-probe.rs`: headless developer probe.
Alternative: one crate. Rejected: protocol code must stay free of UI and storage so the probe and unit tests build fast and stay focused.

### Read-only by construction
`yandex-proto` exposes a closed `ReadRequest` enum (`RequestUser`, `History`, `ChatList`, registry reads such as `get_users_data`, Xiva `subscribe`) and no generic "call any method" entry point. Fetching the CSRF token is a read; the token is sent only with allowlisted reads that require it, starting with `request_user`. A unit test checks the encoded method names against an allowlist. Read markers, typing and heartbeats with presence are simply not representable. Alternative: a runtime flag. Rejected: one mistaken flag would mutate a real account.

### Runtime model
A dedicated thread runs a Tokio 1.54.1 runtime that owns HTTP and WS. `ChatClient` sends commands over `async-channel` 2.5.0 and exposes one `ClientEvent` stream (`ChatsReplaced`, `HistoryChunk`, `MessageUpserted`, `MessageDeleted`, `Connection(state)`, `SessionExpired`). The desktop crate drains it in a GPUI `cx.spawn` task and updates a `ChatStore` entity. Alternative: run networking on GPUI's executor with `async-tungstenite`. Rejected: Tokio-based `reqwest`/`tokio-tungstenite` are the proven stack in both reference projects, and isolating them keeps GPUI executor details out of the protocol crate.

### Dependencies (exact versions)
`tokio` 1.54.1, `reqwest` 0.13.5 (`native-tls`, `multipart`, cookies off — cookies are set explicitly), `tokio-tungstenite` 0.30.0 (`native-tls`), `rmpv` 1.3.1 for the frame header, `serde` 1.0.229 / `serde_json` 1.0.154 / `thiserror` 2.0.21 (versions already in `Cargo.lock`), `uuid` 1.29.3, `rusqlite` 0.43.0 (`bundled`), `wry` 0.57.3, `async-channel` 2.5.0. TLS uses macOS Security.framework through `native-tls` on both HTTP and WS: one system trust store, no second rustls crypto provider to reconcile. Versions are rechecked against crates.io when the task adds them.

### Sign-in
A GPUI window hosts a `wry` WebView via `build_as_child` (GPUI's `Window` implements `HasWindowHandle`), incognito so WebKit keeps nothing outside `.runtime/`. It loads `https://telemost.yandex.ru/` and polls all cookies of the incognito store once a second; when a non-empty `Session_id` appears on `yandex.ru` or a subdomain, the client runs the HTTP bootstrap (CSRF token, then `request_user`) with those cookies, and only on success saves cookies plus `uid`/`guid` and closes the window. The bootstrap needs no stored session and no WebSocket. Telemost's sign-in button opens Passport via a new window, which WebKit drops without a handler; the WebView loads such requests in place. Alternative: system browser plus cookie import. Rejected: needs access to browser profiles and breaks with browser updates.

Observed with the user's Yandex 360 account (2026-11-20): cookies land on `yandex.ru` (`Session_id`, `sessionid2`, `L`, `sessar`, `yandexuid`, `yp`, `ys` and analytics), `passport.yandex.ru` (`sessguard`, `lah`, `mda2_beacon`, `uniqueuid`), `telemost.yandex.ru` and `mc.yandex.ru` (analytics only). No `360.yandex.ru` or other-domain session cookies were needed; the stored set is sent by domain-suffix match.

### Runtime directory and session file
All state lives under `BT_RUNTIME_DIR`, which is required: the app and probe exit with an error naming the variable when it is missing. `run.sh` sets it to `<repo>/.runtime`. Layout: `session.json` (mode 0600: cookies with domain/path/expiry, uid, guid), `cache/<uid>.sqlite`, `media/<uid>/` (avatar and thumbnail bytes). Alternative: macOS Keychain. Deferred by the user's choice while in development.

### Chat list and history loading
Startup: cached state → HTTP bootstrap (`request_user`) → chat list → subscribe. History: WS `history` by `ChatId` with `MaxTimestamp`/`Limit` = 50, newest first.

Confirmed on the live account (task 3.3):
- Chat list: WS `history` without `ChatId`, body `{Limit: 1, ChatDataFilter: {}}`, returns every chat's metadata with its newest message. History: WS `history` with `ChatId`, `Limit`, optional `MaxTimestamp` (exclusive). Both on `service_index` 0.
- Xiva: `service=messenger-prod:version5*common+version5*main` (form-encoded); pushes are binary frames whose Xiva event is `delivery`.
- Push kinds seen: message upsert (an edit re-sends the message with a higher version and `edited`), deletion as a plain message without text followed by a moderated-range push, heartbeats (type 2, every 30 s, from the account's own web session), typing, reactions, bot requests and seen markers. `ClientMessage` may hold a `NotificationBehaviour` key next to the real kind (seen markers, bot requests); the decoder treats it as a modifier, otherwise read-elsewhere updates were lost. Secret-free samples: `crates/yandex-proto/tests/fixtures/live-push-*.json`.
- `subscribe` (`TtlMcs` 60 s) replies with `CurrentTime` and `LastModeratedRange`. The server throttles it: refreshing ~100 chats every 30 s gets `TOO_MANY_REQUESTS` (transport error 7) for ~80 % of the requests.
- Message pushes need no per-chat subscription: a 12 h `bt-probe listen` with none (2026-11-19/20) received 84 `MessageUpserted` and 123 `ChatChanged` from 23 chats (private, `0/0/…`, `100/0/…`). Not observed for lack of traffic: channels `1/0/…` and `0/22`, `0/58`, `0/68` chats. Decision: `Backend::watch` subscribes only the open chat (one request per refresh); the engine sets it on open and after every connect. Alternative: pacing all chats under the throttle – ~100 requests per minute for no observed gain. If a chat kind turns out to need it, its pushes arrive while it is open and its row catches up on the next chat-list fetch.

### Cache
SQLite with `PRAGMA user_version` migrations; tables `chats` and `messages` keyed by server IDs/timestamps, storing normalized columns for ordering plus the raw JSON payload so later changes can render more fields without a resync. Server chunks replace the cached range they cover, which removes messages deleted on the server.

### Media
Avatars and image thumbnails are fetched with the session cookies, stored under `media/<uid>/`, and decoded with the `image` crate already in the dependency tree; GPUI renders them with `img()`. Request concurrency is bounded (4) so a long chat list does not flood the file host.
Deleted users and some bots have avatar ids the file host answers 404 for. That answer becomes `NotFound`, logged at `debug`; the engine does not request that media again for an hour (`ClientConfig::media_missing_ttl`, kept in memory, so a restart asks once more) and the UI keeps the initials. Other failures stay `warn` and are retried on the next request.

### UI binding
The current GPUI layout is kept and bound to `ChatStore`. The composer becomes a disabled control with a read-only notice. The demo "meetings" filter, mute toggle and synthetic info panel are removed; the thread panel goes too, because the model has no threads yet. The sidebar filter is one of All / Private / Groups / Channels / Unread.

### Composition
`desktop` is the composition root: `yandex_backend.rs` implements `chat-core`'s `Backend` over `yandex-proto` (wire → domain mapping, user-name lookup via `get_users_data`), so neither library depends on the other. Bootstrap waits for the Xiva `Connected` event (30 s deadline) before any WS request, because the socket actor rejects requests until subscribed. A `ChatChanged` push re-reads the chat list to emit the authoritative row. File downloads are out of scope; file cards show name and size only. Sign-in verification runs the HTTP bootstrap on a throwaway Tokio runtime; expiry and sign-out reopen the sign-in window.

### Live edits racing history fetches
The engine keeps the last 256 live changes per chat and replays those newer than a request's start over its history snapshot before replacing the cached range, so a fetch answered after a live delete or edit cannot resurrect or revert it.

### Verification without windows on the host
- Protocol and cache: unit tests on recorded frames (secrets stripped) and `bt-probe` commands (`me`, `chats`, `history <chat>`, `listen`) run on the host against the stored session; the probe never prints cookie values.
- UI: GPUI `test-support` headless tests (`TestAppContext`/`VisualTestContext`) drive `ChatStore` with fixture events.
- Visual end-to-end: a macOS VM under Tart started with `--no-graphics`, screenshots taken inside the VM. Installing Tart is system configuration and needs the user's approval before that task starts.
- The only window on the host is the one-time sign-in that the user starts.

## Risks / Trade-offs

- [Unofficial protocol changes or rejects the client] → protocol isolated in one crate; probe gives a one-command health check.
- [Yandex 360 sign-in sets the session on a domain other than `.yandex.ru` (e.g. via `cookier.360.yandex.ru`)] → the spike task signs in with the user's org account first; the cookie domain list becomes data, not code.
- [Connecting may itself publish "online" presence] → the spike compares the account's status in the web client while the probe is connected; if presence leaks, the cause is documented and the requirement is revisited with the user.
- [A read call has a hidden side effect such as marking read] → the spike checks unread counters in the web client after `history` calls.
- [Rate limiting or account flags] → bounded concurrency, reconnect backoff starting at the web client's 11 s interval, no bulk history prefetch.
- [Session cookies on disk] → `.runtime/` is git-ignored, files are 0600, secrets are never logged; Keychain is a later change.
- [WebView inside a GPUI window is an unusual combination] → if `build_as_child` misbehaves, fall back to a separate `wry` window driven by `tao` in a helper process that writes `session.json`; the rest of the design is unaffected.

## Migration Plan

The demo is removed in the same change; `BT_STATE_PATH` files are ignored and can be deleted. Rollback is reverting the change.
