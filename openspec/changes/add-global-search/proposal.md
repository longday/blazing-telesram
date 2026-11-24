# Proposal

> Context document for later work. Full research (verbatim): [`research.md`](research.md). Design, delta specs and tasks are written; the former Open Questions are resolved below (Decisions).

## Why

Search in the client is local only: the search field filters the loaded chat list by name (`chat-list` requirement "Local filtering"; `ChatFilter` and `visible_chats(query, filter)` in `crates/desktop/src/store.rs`). It cannot find a message, a person outside the loaded list, or a chat not yet loaded. To replace the web client for daily use the user must be able to search message content. This is roadmap item 4; the target look is `docs/mockups/4-search.svg` (results grouped into sections). The protocol has a registry method `search` (`POST /messenger/api/registry/api/`, `enableCSRF:false`) with `entities` = `messages`, `users`, `chats`, `contacts`, `query`, `limit`, optional `chat_id`; the response carries `items`, `total`, `limit`, `page`, `pages` (reference: protocol-research.md §3.1–3.3). The client has no `search` call: the read set in `crates/yandex-proto/src/request.rs` is closed (`request_user`, `get_users_data`, `history`, `subscribe`).

## What Changes

- `yandex-proto`: a new typed, closed read operation `search` on the registry client (not an open "call any method"); typed response with result sets, `total`, `limit`, `page`, `pages`.
- `chat-core`: a search command and result/error events through the backend; stale results for a superseded query are dropped.
- `desktop`: server results for a typed query shown below the existing local name matches in sections for chats, people and messages, with loading, empty and error states; opening a message result opens its chat. Local filtering of the chat list keeps working offline and unchanged.
- Only the first page of results is shown until the pagination parameter is confirmed (see Decisions, 3); the query text is sent to the server only after a debounce and a minimum length, never offline.
- `bt-probe`: a `search <query>` command for live verification of the wire shape, including the next-page parameter, against a real account (read-only request).

## Capabilities

### New Capabilities
- `message-search`: server-side search of messages, people and chats from the search field, its states and result navigation.

### Modified Capabilities
- `chat-list`: "Local filtering" stays; a requirement is added that a non-empty query additionally shows server results, and that the list works offline without them.

## Impact

- `crates/yandex-proto`: `request.rs` (new read variant), `client.rs` (facade `search`), response parsing, tests with captured fixtures.
- `crates/chat-core`: `model.rs` (`Command`/event for search), backend wiring, request cancellation/debounce handling.
- `crates/desktop`: `store.rs`, `desktop.rs` (search field, result sections, async states).
- `bt-probe`: new read-only subcommand.
- Risk: the user's query text goes to the server on typing; mitigated by debounce, minimum length, explicit offline state. The request is read-only, so no account state changes; live checks can use any chat.

## Non-goals

- Files and links tabs from the mockup: the described `search` does not confirm these categories.
- Folders (separate research and decision on server vs. local storage).
- "Mark as unread" (`UnseenMarker`) and any other read-state write.
- Changing the automatic mark-read-on-visible behaviour of `read-markers`.
- Searching inside the message history cache offline.
- New composite local filters beyond the existing all/private/groups/channels/unread (see Decisions, 5).

## Follow-ups

1. Folders: research of sources and the server-vs-local storage decision, then a change.
2. Mark as unread: verify `UnseenMarker` shape and semantics on a test account, then a change (new `WriteRequest` variant and `Command`).
3. Files and links search: after API support is confirmed.
4. Search pagination ("show more"): after the next-page parameter is confirmed with `bt-probe`.
5. In-chat search (`chat_id` scope), if wanted.

## Decisions

Taken autonomously on the user's instruction to continue without questions: in every case the recommended option of the former Open Question was chosen. The user may revise any of them with `openspec-update-change`.

1. **Server-side message search or chats only?** Decision: server-side search of messages is in scope. Why: the mockup shows it and it is the point of roadmap item 4; local name search already works and stays unchanged.
2. **People and files/links?** Decision: people (`users`) and chats are separate result types; files/links are left out. Why: the described `search` does not confirm those categories, so nothing unconfirmed is promised. `contacts` is not requested either (it overlaps `users`; `bt-probe search` will show whether it adds anything).
3. **Pagination.** Decision: `page` is not part of the contract; only the first page (`limit`) is requested and shown, and `bt-probe search` records the next-page parameter for the follow-up. Why: a guessed parameter could skip or duplicate results.
4. **When to send the query?** Decision: debounce about 300 ms, minimum 2 characters (counted as Unicode scalar values after trimming), no request while offline, results of a superseded query discarded. Why: fewer requests and less typed text leaving the machine, at the price of a short delay.
5. **Extend local filters in this change?** Decision: no. Why: the current `chat-list` spec already covers name search and kind filters and no concrete new filter was requested. The research step "extend local filters" therefore produces no work.
6. **Where do server results live in the UI?** Decision: below the local name matches in the same sidebar list, grouped into sections (chats, people, messages) as in the mockup, independent of the kind chips (the chips describe the local list only). Why: reuses the existing field and list; a dedicated search view would be larger.
