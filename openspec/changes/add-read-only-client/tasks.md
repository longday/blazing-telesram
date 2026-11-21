# Tasks

## 1. Foundation

- [x] 1.1 Add `crates/yandex-proto` to the workspace with the pinned dependencies from design.md (rechecked on crates.io); verify `cargo build --locked --workspace` succeeds
- [x] 1.2 Require `BT_RUNTIME_DIR` in the app (exit with an error naming the variable when missing), export it to `<repo>/.runtime` in `run.sh`, add `/.runtime/` to `.gitignore`; verify the app fails fast without it and `git check-ignore .runtime/x` matches
- [x] 1.3 Implement the session file (cookies with domain/path/expiry, uid, guid) written atomically with mode 0600 and a redacting `Debug`; verify unit tests cover round-trip, permissions and absence of cookie values in `Debug` output

## 2. Sign-in

- [x] 2.1 Implement the HTTP bootstrap in `yandex-proto` on a plain cookie set (no stored session, no WebSocket): CSRF token from `…/registry/csrf-token/`, then `request_user` with `{bind_phone_number: false}` and `X-CSRF-TOKEN`, parsing `uid`/`guid`/display name and mapping cookie rejection (401, `invalid_cookies`, `not_authorized`, `no_credentials`, `cookie_auth_failed`) to session expiry and `bad_csrf_token` to a CSRF error; verify unit tests on synthetic responses cover success and each error
- [x] 2.2 Open a GPUI sign-in window with an incognito `wry` WebView child on `https://telemost.yandex.ru/` when no session exists, poll `yandex.ru` cookies, and once `Session_id` appears run the 2.1 bootstrap with them; save the session (cookies, uid, guid) and close only on success; closing the window quits without writing; verify unit tests for cookie-to-session conversion pass
- [x] 2.3 Add `bt-probe me` (bootstrap on the stored session); verify it prints uid, guid and display name and no cookie values
- [x] 2.4 Ask the user to run the one-time sign-in with their Yandex 360 account; verify `.runtime/session.json` exists with mode 0600, `bt-probe me` succeeds, and record in design.md which cookie domains the org sign-in produced

## 3. Protocol spike

- [x] 3.1 Implement the WS frame codec (request header, `0x05` separator, JSON body, push frames `0x03`); verify encode/decode unit tests on synthetic frames
- [x] 3.2 Connect to Xiva with the stored session and `user=<uid>` from the bootstrap (fallback `guid`) and wait for the `subscribed` operation frame in `bt-probe listen --raw`; verify the subscription id is printed and no cookie values
- [x] 3.3 Determine the chat-list and history calls, the Xiva service string and event types on the live account; record them in design.md; save secret-free fixtures under `crates/yandex-proto/tests/fixtures/`
- [ ] 3.4 Check read-call side effects: verify with `bt-probe` that unread counters are unchanged after reading history, and record whether connecting publishes presence (ask the user to look at their status from another device); stop and revisit specs with the user if any side effect exists. Deferred by the user: checkable only once this client is their primary one
- [x] 3.5 Resolve the per-chat `subscribe` question in design.md (Open Questions) with a live listen without subscriptions while a message arrives; then subscribe only the open chat or pace subscriptions under the throttle; verify a live run logs no `TOO_MANY_REQUESTS` and still receives new messages in a chat that is not open

## 4. Protocol client

- [x] 4.1 Implement the closed `ReadRequest` set and its encoding; verify a unit test rejects any encoded method outside the read allowlist
- [x] 4.2 Implement chat list, history chunks (50, `MaxTimestamp` paging), users data and media fetch (avatars, thumbnails) on fixtures; verify parsing tests for every fixture, including deleted, edited, forwarded, reply, image, file and unknown message types
- [x] 4.3 Implement Xiva subscription, push decoding into message upsert/delete and chat events, reconnect with backoff from 11 s, and `SessionExpired` on auth rejection; verify unit tests with a fake socket cover reconnect and expiry
- [x] 4.4 Add `bt-probe chats`, `bt-probe history <chat>` and `bt-probe listen`; verify each against the live account without windows and without cookie values in output

## 5. Core: model, cache, sync

- [x] 5.1 Replace the synthetic `Core`, `LocalChatService`, `BT_STATE_PATH` persistence, `telemost_mock`, its fixtures and `telemost-mock-check` with a domain model keyed by server IDs; verify the workspace builds and no references remain (`rg telemost_mock\|BT_STATE_PATH` is empty)
- [x] 5.2 Implement the SQLite cache per account under `BT_RUNTIME_DIR/cache/<uid>.sqlite` (mode 0600, `user_version` migrations, range replacement); verify tests for cold load, deletion on range replace and account isolation
- [x] 5.3 Implement `ChatClient` on its own Tokio thread with the command channel and `ClientEvent` stream (cache first, then HTTP bootstrap, chat list, subscribe, catch-up after reconnect); verify tests with a fake transport cover startup from cache, live upsert, delete and catch-up
- [x] 5.4 Implement sign out (delete session, account cache and media); verify a test that the files are gone afterwards
- [x] 5.5 Media the server answers 404 for (deleted users, bots) is not requested again for an hour and is logged at `debug`, not `warn`; verify a test that repeated requests within the TTL reach the backend once and that it is requested again after the TTL, and that a 404 maps to `NotFound`

## 6. Desktop UI

- [x] 6.1 Add a `ChatStore` GPUI entity fed by `ClientEvent`, with `gpui` `test-support` as a dev-dependency; verify a headless test applies events to the store
- [x] 6.2 Bind the chat list: avatar or initials, name, preview with time, unread (muted style), pinned and muted markers, ordering, name search and kind/unread filters, empty state; remove the demo meetings filter, mute toggle and info panel; verify headless tests for the chat-list scenarios
- [x] 6.3 Bind the timeline: open at newest, older chunk on reaching the top with anchor kept, start-of-history stop, author and avatar in groups, outgoing bubbles on the right, reply quote, forwarded line, edited marker, deleted and unsupported placeholders; verify headless tests for the message-history scenarios
- [x] 6.4 Render image thumbnails at aspect ratio and file cards with name and size; verify headless tests with fixture media
- [x] 6.5 Disable the composer and thread reply inputs with a read-only notice, add the connecting/offline indicator and a sign-out action; verify headless tests for these states
- [x] 6.6 Update README (run, sign-in, `BT_RUNTIME_DIR`, read-only status, unofficial-protocol risk) and remove the MCP experiment section; verify the documented commands run as written

## 7. Integration

- [ ] 7.1 Propose installing Tart and a macOS VM image per the system-configuration rules; after the user's approval, set up a VM started with `--no-graphics`; verify `tart list` shows it running and a screenshot can be taken inside the VM
- [ ] 7.2 Run the app in the VM with the copied session; verify by screenshots: chat list, open chat, pagination, thumbnails, read-only composer, cached start, offline indicator after cutting the VM network, and catch-up after restoring it
- [ ] 7.3 Verify live updates end to end: ask the user to send one message to their account from another device; confirm it appears in the VM app and in `bt-probe listen`, and that the chat stays unread in the web client
