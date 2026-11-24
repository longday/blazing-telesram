# Design

## Context

Motivation, scope and the resolved decisions: `proposal.md`. Observed state of the code:

- `crates/yandex-proto/src/request.rs`: `ReadRequest` is a closed enum (`RequestUser`, `GetUsersData`, `ChatList`, `History`, `SubscribeChat`); `ALLOWED_METHODS` has 4 entries and a test (`allowlist_has_no_mutating_or_presence_methods`) asserts `len() == 4` and forbids mutating names. Registry requests are encoded by `registry_body()` and run by `RegistryClient::execute` (`registry.rs`), which already maps HTTP 401 and cookie error codes to `SessionExpired`, 5xx to a network error, other failures to `Protocol`, and logs method/status/duration at `debug` and the body at `trace`. `GetUsersData` is the model: `Route::Registry { csrf: false }`, the `search` method also needs no CSRF (research: `enableCSRF:false`).
- `crates/yandex-proto/src/client.rs`: `Client` facade (`chat_list`, `history`, `push`, `users`, `fetch_media`); `bt-probe` (`src/bin/bt-probe.rs`) is the live verification tool, fixtures live in `crates/yandex-proto/tests/fixtures` and are exercised by `tests/wire_fixtures.rs`.
- `crates/chat-core`: `Backend` trait (`backend.rs`) is the seam; `Command`/`ClientEvent` (`model.rs`); the engine (`client.rs`) handles a command in `handle_command`, runs backend calls in `tokio::spawn` and returns the result as an `Input`, as `run_action` does for edit/delete (offline: refuse immediately; `SessionExpired`: `self.expire()`). A test `Fake` backend lives in `client.rs` tests.
- `crates/desktop`: `YandexBackend` (`yandex_backend.rs`) implements `Backend`, maps `ProtoError` with `backend_error`, resolves user names through a `Users` cache. `ChatStore` (`store.rs`) turns `ClientEvent`s into state and `StoreEvent`s and owns `visible_chats(query, filter)`. `desktop.rs::sidebar` reads the query from the `search` `TextInput` on every render and lists `visible_chats`. GPUI tests live in `tests.rs` (`harness`, `search_and_filters_narrow_the_list`).
- Specs: `chat-list` "Local filtering" says "without server requests"; `diagnostic-log` "Network trace" keeps message text at `trace` only.

What is not known: the shape of the items inside the `search` reply (research confirms only the envelope: per result set `items`, `total`, `limit`, `page`, `pages`) and the next-page parameter. Both are established with `bt-probe search` before any typed item is written; see Decision 2.

## Goals / Non-Goals

**Goals:**
- One closed, read-only `search` operation end to end: proto request, backend call, core command/events, store state, sidebar sections.
- Stale replies never change the UI; offline and failures leave local filtering untouched.
- Work splits into tasks that touch disjoint files, so they can be done in parallel once the shared contract (below) is fixed.

**Non-Goals:**
- Anything listed in the proposal's Non-goals; additionally: scrolling to or highlighting the found message after opening its chat (opening the chat is enough; a follow-up), caching search results, highlighting the matched substring.

## Decisions

### 1. A closed `ReadRequest::Search`, no generic call
`ReadRequest::Search { query: String, limit: u32, entities: Vec<SearchEntity> }` with `SearchEntity::{Messages, Users, Chats}` (closed enum; `contacts` is deliberately absent, proposal Decisions 2). Wire: method `search`, `Route::Registry { csrf: false }`, params `{ "query", "limit", "entities": [..] }`; no `page` and no `chat_id`. `limit` is the constant `SEARCH_LIMIT` = 10 (the server default per the reference), the same for every entity; the app always asks for all three entities. `ALLOWED_METHODS` grows to 5 and the `len()` assertion and `every_request()` list are updated in the same task; `search` contains none of the forbidden substrings. Alternative (an open `call(method, params)`) rejected: it would void the allowlist guarantee.

### 2. Envelope first, items after a live capture
The reply is parsed in two steps so that parallel work does not depend on a guess:
1. `SearchReply { sets: per entity { total, limit, page, pages, items: Vec<Json> } }` is typed from the reference (confirmed). `RegistryClient::search` and `Client::search` return it.
2. After `bt-probe search` has captured and (redacted) fixtured a real reply, `items` are decoded into `WireSearchChat`, `WireSearchUser`, `WireSearchMessage`. A tolerant parser (as `wire.rs`): an item without an address (chat id; user guid; chat id plus timestamp) is skipped and counted, never an error; an unknown extra field is ignored.

The new types and parser go into a new module `crates/yandex-proto/src/search.rs` (re-exported from `lib.rs`), not into `wire.rs`, to keep this change out of the file other changes are editing. The probe also records the next-page parameter in the task notes (follow-up 4); it is not put on the wire by the app.

### 3. Domain contract in `chat-core` (fixed up front)
```
SearchId(pub u64)                       // chosen by the store, monotonically increasing
SearchHits { chats: Vec<ChatHit>, people: Vec<PersonHit>, messages: Vec<MessageHit> }
ChatHit    { id: ChatId, kind: ChatKind, title: String, avatar: Option<MediaRef> }
PersonHit  { user: UserId, name: String, avatar: Option<MediaRef>, chat: Option<ChatId> }  // chat: the existing private chat with this person
MessageHit { chat: ChatId, chat_title: String, timestamp: MessageTs, author_name: Option<String>, text: String }
enum SearchFailure { Offline, Failed }
Command::Search { id: SearchId, query: String }
Command::CancelSearch
ClientEvent::SearchResults { id: SearchId, hits: SearchHits }
ClientEvent::SearchFailed { id: SearchId, reason: SearchFailure }
Backend::search(&self, query: &str, limit: usize) -> Result<SearchHits, BackendError>
```
The names above are the contract between tasks 2, 3 and 4; if the live capture shows a field cannot be filled (for example no `chat_title` on message items), `YandexBackend` fills it from the loaded chat list or leaves `None`/empty, and the contract does not change. `PersonHit::chat` is filled by `YandexBackend` from a guid-to-chat map it records on every `chat_list()` call (that call already reads `WireChat::partner_guid` of private chats); `ChatSummary` is not extended, so no existing struct literal or cached row changes. `Backend::search` is added without a default body so every implementor decides (`YandexBackend`, the `Fake` in `client.rs` tests).

### 4. Staleness and offline are owned by the core, the store checks again
The engine keeps `search_latest: Option<SearchId>`. `Command::Search` sets it; if the engine is offline it emits `SearchFailed { Offline }` without calling the backend. Otherwise it spawns `backend.search` and posts `Input::Search { id, result }`; on that input a result whose id is not `search_latest` is dropped silently. `Command::CancelSearch` sets `search_latest = None`. `SessionExpired` goes through `self.expire()`; other errors are logged at `warn` (error kind only) and become `SearchFailed { Failed }`. The store applies the same rule on `SearchResults`/`SearchFailed` (id must equal the id it last issued), because events may already be queued when it issues a new query. No in-flight request is aborted: a superseded request is cheap and read-only.

### 5. Debounce, minimum length and the query live in the UI
The policy constants (`SEARCH_DEBOUNCE` about 300 ms, `SEARCH_MIN_CHARS` = 2 counted as `char`s after `trim()`) live in `crates/desktop/src/store.rs`. `desktop.rs` observes the `search` input (it already does `cx.observe(&search, ...)`); on a change it bumps a local generation counter and spawns a timer on the GPUI executor; when the timer fires and the generation is unchanged, it calls `ChatStore::search(query)`. A query shorter than the minimum calls `ChatStore::cancel_search()` at once (no timer). While offline (`store.connection() == Offline`) the UI never calls `search` and shows the offline text. `ChatStore::search` allocates the next `SearchId`, stores `SearchState { id, query, status: Loading, hits }` and sends `Command::Search`; the previous `hits` stay in the state until replaced or cancelled (spec "Search states").

### 6. Sidebar layout
Local rows stay as they are (`visible_chats`, unchanged). A new module `crates/desktop/src/search_results.rs` renders, from `SearchState`, a status line (loading / offline / error / nothing found) and the sections "Чаты", "Люди", "Сообщения" (each hidden when empty); `desktop.rs::sidebar` appends it below the local rows when the query is non-empty. The chats section drops hits whose id is among the local matches. A person row is clickable only when its `PersonHit::chat` is `Some`, and then calls the existing `ChatStore::select` with it; otherwise it is shown without a click action (never a guess by name). A chat or message click calls `ChatStore::select` too. Search code never sends `Command::MarkRead`; an opened chat is marked read only by the existing visibility logic of `read-markers` (focused window, newest message visible), unchanged by this change.

### 7. Privacy and logging
The request logs through the existing registry path (`debug`: method, status, duration; `trace`: body) so query text appears only at `trace`, as `diagnostic-log` requires; no new log call may put the query or a hit's text in a field above `trace`. Results and the query are held in memory only: nothing is written to `chat-core`'s cache (`offline-cache` is not touched).

### 8. `chat-list` delta
The existing "Local filtering" requirement says "without server requests". It stays true for the filtering itself, so it is MODIFIED (full text) to say that the filtering works offline and never waits for server results, and a new ADDED requirement states the placement of server results below local matches and that the kind chips narrow only the local part.

## Risks / Trade-offs

- **Unknown item shape** → items typed only after `bt-probe search` capture (task 1.3 gates 1.4); until then only the envelope is relied on. If the live reply differs from the reference even in the envelope, task 1.2's parser fails with `Protocol` and the probe shows it before any UI work depends on it.
- **Person to private chat mapping** comes from the chat list the backend last loaded; a person whose private chat was created after that load is shown but not openable until the next chat-list load. A missing mapping hides the click action rather than opening a wrong chat.
- **Typed text leaves the machine** → debounce, minimum length, offline guard, no cache, `trace`-only logging (Decisions 4, 5, 7).
- **Concurrent changes touch the same files** (`model.rs`, `backend.rs`, `client.rs`, `yandex_backend.rs`, `store.rs`, `desktop.rs`, `tests.rs`, `wire.rs`, `request.rs`, `bt-probe.rs` are also candidates for `add-message-reactions`, `add-media-download`, `add-chat-members-and-creation`). Edits here are additive (new enum variants, new trait method, new module `search.rs` / `search_results.rs`); tasks rebase before editing and keep diffs minimal.
- **Search results are not complete** (first page only, Decision 3 of the proposal) → the spec forbids implying completeness; no "show more" until the follow-up.
- **Slow reply wasted work** → accepted; the request is read-only and small.
