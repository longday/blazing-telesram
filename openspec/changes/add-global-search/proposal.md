# Proposal

> Context document for later work. Full research (verbatim): [`research.md`](research.md). Design, specs and tasks are not written yet: the Open Questions below need user decisions first.

## Why

Search in the client is local only: the search field filters the loaded chat list by name (`chat-list` requirement "Local filtering"; `ChatFilter` and `visible_chats(query, filter)` in `crates/desktop/src/store.rs`). It cannot find a message, a person outside the loaded list, or a chat not yet loaded. To replace the web client for daily use the user must be able to search message content. This is roadmap item 4; the target look is `docs/mockups/4-search.svg` (results grouped into sections). The protocol has a registry method `search` (`POST /messenger/api/registry/api/`, `enableCSRF:false`) with `entities` = `messages`, `users`, `chats`, `contacts`, `query`, `limit`, optional `chat_id`; the response carries `items`, `total`, `limit`, `page`, `pages` (reference: protocol-research.md §3.1–3.3). The client has no `search` call: the read set in `crates/yandex-proto/src/request.rs` is closed (`request_user`, `get_users_data`, `history`, `subscribe`).

## What Changes

- `yandex-proto`: a new typed, closed read operation `search` on the registry client (not an open "call any method"); typed response with result sets, `total`, `limit`, `page`, `pages`.
- `chat-core`: a search command and result/error events through the backend; stale results for a superseded query are dropped.
- `desktop`: server results for a typed query shown below the existing local name matches in sections for chats, people and messages, with loading, empty and error states; opening a message result opens its chat. Local filtering of the chat list keeps working offline and unchanged.
- Only the first page of results is shown until the pagination parameter is confirmed (see Open Questions); the query text is sent to the server only after a debounce and a minimum length, never offline.
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
- New composite local filters beyond the existing all/private/groups/channels/unread (see Open Questions).

## Follow-ups

1. Folders: research of sources and the server-vs-local storage decision, then a change.
2. Mark as unread: verify `UnseenMarker` shape and semantics on a test account, then a change (new `WriteRequest` variant and `Command`).
3. Files and links search: after API support is confirmed.
4. Search pagination ("show more"): after the next-page parameter is confirmed with `bt-probe`.
5. In-chat search (`chat_id` scope), if wanted.

## Open Questions

1. **Server-side message search or chats only?** Recommended: server-side search of messages is in scope, since the mockup shows it. Trade-off: new network/API surface and async UI states; local search already works and stays as is.
2. **People and files/links?** Recommended: include people (and chats) as separate result types; leave files/links out until API support is confirmed. Trade-off: the mockup is implemented only partly, but we promise nothing unconfirmed.
3. **Pagination.** Recommended: do not fix `page` in the contract until `bt-probe search` confirms the parameter on a safe query; ship first page only (`limit`). Trade-off: no "show more" until confirmed; avoids skipped/duplicated results from a guessed parameter.
4. **When to send the query?** Recommended: debounce (about 300 ms), minimum 2 characters, no request while offline, and results of an older query are discarded. Trade-off: slight delay vs. fewer requests and less typed text leaving the machine.
5. **Extend local filters in this change?** Recommended: no — the current spec already covers name search and kind filters, and no concrete new filter is requested; add only if the user names one. Trade-off: the research's "extend local filters" step stays empty unless specified.
6. **Where do server results live in the UI?** Recommended: below the local name matches in the same sidebar list, grouped by section as in the mockup. Trade-off: reuses the existing field and list; a dedicated search view would be larger.
