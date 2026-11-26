# Design

## Context

`proposal.md` states the motivation, the decisions taken from evidence (numbered there 1-6, cited here as P1-P6) and the open questions (Q1-Q6). `research.md` holds the protocol facts and marks what a live read must still confirm (**[probe]**).

Code this change extends:

- `yandex-proto::request::ReadRequest` is the closed read set (`ALLOWED_METHODS`, seven names, pinned by tests); `route()` sends registry reads over HTTP and the rest over the socket; `socket_frame()` builds the body and adds `RequestId`. `Client` (`client.rs`) is the facade (`members`, `whoami`, `history`).
- `wire.rs` parses `UserInfo` into `WireSender {guid, display_name, avatar_id}` and the reaction siblings into `WireReactor {user: Option<guid>, code, timestamp}`; the latter is for `RecentUserReactions` and stays as is.
- `chat-core`: `Backend` (domain types only), `Command`/`ClientEvent`, the members flow (`Command::LoadMembers`, `ClientEvent::Members`, `MembersFailed {chat, reason, offline}`, `client.rs::load_members`) as the model for a read that is never cached and is refused offline without a request; `Message.reactions` with counts, `mine` and `version` (`add-message-reactions` D3).
- `desktop`: `YandexBackend` with the in-memory `users` cache and `Users::load`; `store.rs` `LoadState` for members; `info_panel.rs::body` as a pure state-to-view function; `composer.rs` message menu; `timeline.rs` chips (a left click toggles the own reaction, a right click opens the message menu).

## Goals / Non-Goals

**Goals:**
- The complete list of reactors of a message, read-only, honest about being partial.
- Contracts fixed here so protocol, core, adapter and UI tasks can proceed in parallel; the contracts do not depend on the open questions.

**Non-Goals:**
- The proposal's non-goals (readers, profile opening, caching, per-type server filter).
- Recovering from reaction changes missed offline beyond what a refetch on open returns.

## Decisions

### D1. Protocol (`yandex-proto`), P1, P5

```rust
ReadRequest::ListReactions { chat: String, timestamp: i64, before: Option<i64>, limit: u32 }
// method() "list_reactions"; route() Route::Socket
// body {"ChatId", "Timestamp", "Limit", "Mode": 0, "MaxTimestamp": before?}   (+ RequestId from the frame)
pub const REACTORS_PAGE: u32 = 30;     // provisional: the web client's 2 x 15; final value after P3

pub struct WireReactorEntry { pub guid: String, pub display_name: Option<String>, pub avatar_id: Option<String>,
                              pub code: i64, pub timestamp: Option<Micros> }
pub struct WireReactors { pub entries: Vec<WireReactorEntry>, pub skipped: usize,
                          pub raw: usize,                    // entries on the wire page, before any is dropped
                          pub oldest: Option<Micros> }       // smallest `Timestamp` over ALL raw entries
pub fn parse_reactors(body: &Json) -> Result<WireReactors, ProtoError>
Client::reactors(&self, chat: &str, timestamp: i64, before: Option<i64>, limit: u32) -> Result<WireReactors, ProtoError>
```

`ALLOWED_METHODS` becomes eight names; the exact-list tests are updated, not loosened. `InviteHash` and `Mode` other than 0 are not representable. `parse_reactors` reads `UserReactions` from the reply object (a missing key is an empty list; a reply that is not an object is a `Protocol` error). Server order is kept. An entry without `Type` or without `UserInfo.Guid` (no identity to show, key or mark as own) is counted in `skipped` and not returned, so the shown total stays honest (spec "Complete or labelled"). The name and avatar come from the same `UserInfo` (`DisplayName`, `AvatarId`).

Alternative rejected: add the sender to `WireReactor`. It would change every `RecentUserReactions` consumer (the push path needs only the guid) for no gain.

`raw` and `oldest` are taken from the page as the server sent it, before an entry is dropped for lacking `Type` or `Guid`: the cursor and the end-of-list test must not depend on which entries the client could show (a page whose oldest entries are malformed would otherwise move the cursor back to a newer entry and repeat them, or end early). `raw == entries.len() + skipped`.

### D2. Domain and backend seam (`chat-core`), P2, P5

```rust
pub struct Reactor { pub user: UserId, pub name: Option<String>, pub avatar: Option<MediaRef>,
                     pub code: ReactionCode, pub at: Option<MessageTs>, pub me: bool }
pub struct ReactorsPage { pub chat: ChatId, pub timestamp: MessageTs, pub request: u64, pub reactors: Vec<Reactor>,
                          pub skipped: usize, pub raw: usize, pub oldest: Option<MessageTs> }
Backend::reactors(&self, chat: &ChatId, timestamp: MessageTs, before: Option<MessageTs>, limit: usize)
    -> impl Future<Output = Result<ReactorsPage, BackendError>> + Send
```

One page per call; no loop in the backend. `raw` and `oldest` pass through from the wire (D1); the backend does not decide whether another page exists. The store does, from the page itself (D5), never from the message's known counts. `skipped` is the wire's `skipped` (entries without identity); the adapter drops nothing else. `request` is the caller's token, copied into the page by the engine (D4), not by the backend.

### D3. Names (adapter), P4

`desktop::yandex_backend` maps each entry with the helpers the authors path already uses: the entry's own `display_name` (non-blank) else the `users` cache entry for the guid (loaded for the page's anonymous guids with `Users::load`, which fails soft; only `SessionExpired` is an error) else `name: None`, which the view renders as "Неизвестный". The avatar is `MediaRef {kind: Avatar}` from the entry's `avatar_id` or the cache. `me` is `guid == account guid`.

### D4. Engine (`chat-core::client`), P3

`Command::LoadReactors { chat, timestamp, before: Option<MessageTs>, request: u64 }` -> `ClientEvent::Reactors(ReactorsPage)` | `ClientEvent::ReactorsFailed { chat, timestamp, request, reason, offline }`, mirroring `LoadMembers`/`MembersFailed`. `request` is a load-generation token chosen by the store; the engine echoes it unchanged on every outcome, including the refusals of steps 1 and 2, and never interprets it:

1. The cached message must exist, be `Delivery::Sent` and not `Deleted`; otherwise `ReactorsFailed` with a constant reason ("Список недоступен"); nothing is sent.
2. Not online: `ReactorsFailed { offline: true }` with `OFFLINE_REASON`, no request.
3. The backend call runs as a read (not an action: nothing is queued or replayed, there is nothing to undo); a failure maps as for members (`Network` is offline, `Rejected` carries its reason, `SessionExpired` expires the session as elsewhere).
4. Nothing is written to the cache and nothing changes the message's `reactions`.
5. Matching an answer to a list is the store's job (D5): `(chat, timestamp)` alone cannot tell an answer to the list the user closed from one to the list they reopened for the same message, or an old page from the page of a refresh.

### D5. Store (`desktop::store`), P3

One open list at a time, in memory:

```rust
pub struct ReactorsView { chat: ChatId, timestamp: MessageTs, base_version: Option<i64>,
                          generation: u64,        // token of the one request outstanding for this list
                          state: ReactorsState }  // dropped on close
pub enum ReactorsState { Loading,
                         Ready { rows: Vec<Reactor>, skipped: usize, oldest: Option<MessageTs>,
                                 exhausted: bool, loading_more: bool },
                         Failed { reason: String, offline: bool } }
```

The store keeps a counter `next_request` that only grows (never reset by close or reopen). Every `LoadReactors` it sends (open, reopen, retry, load more, refresh) takes a fresh value, sets `generation` to it and thereby supersedes any request still out. A `Reactors` or `ReactorsFailed` is applied only if its `(chat, timestamp)` is the open list **and** its `request` equals `generation`; anything else is dropped (an answer after close, after reopening the same message, or to the page before a refresh). Rows are appended in server order and de-duplicated by guid (the web client does the same).

**End of list** is decided from the page, with strict cursor progress; known counts never decide it. After a page requested with cursor `before` and limit `L`: the list is `exhausted` when `oldest` is `None`, or `raw < L`, or `before` is `Some(b)` and `oldest >= b` (the cursor did not move, so the page is a repeat; continuing would loop). Otherwise the next page is requested with `before = oldest`. An inclusive bound that repeats the boundary entry is harmless: de-duplication drops it and the page still progresses unless it contains nothing older. `raw < L` means "end" only if the probe confirms that a short page is never a server cap (P3 in `research.md`); until then `REACTORS_PAGE` is kept at or below the largest honoured `Limit`, so a short page is an end. The message's known counts only label progress: `total` is the sum of the counts, recomputed from the store's current message; a count that lags low or high changes the header "N из M", never whether more pages are requested or offered. `base_version` is the message's reaction version when the list was opened, the hook for open question 3.

### D6. View (`desktop`), P4

A pure `body(state, connection, total) -> Body {Loading, Offline, Failed(reason), Ready { rows, shown, total, skipped, more }}` like `info_panel::body`, tested without a window. A `Ready` list while the connection is not online renders `Offline` (as the member panel does). The row is avatar, name (placeholder "Неизвестный", own entry suffixed "Вы"), the emoji of the code through `reactions::emoji` or `NEUTRAL_GLYPH`, and the time through the existing `format` helpers. Debug selectors `reactor-{guid}`. The header "N из M" appears only when `shown + skipped < total`, and a non-zero `skipped` adds "K без данных" (spec "Complete or labelled").

`more` in `Ready` is `!exhausted` of the store state (D5), never derived from `total`; `total` only feeds the header.

### D7. Probe and verification

`bt-probe reactors <chat> <micros> [--before <micros>] [--limit <n>]` prints the parsed reply like `members` does (`{:#?}`, names and guids on stdout, header on stderr). It is a read: it is not covered by `BT_WRITE_ONLY_CHAT` and the write guard tests stay as they are. It sends the one form the app sends (`Mode: 0`); there is no `--mode`, because the request contract (D1) has no other mode and the app never sends one. If the server refuses `Mode: 0`, 3.1 reopens D1 (omit `Mode`). Unit and fixture tests carry everything except server behaviour; server behaviour is task 3.1 (research "Verification plan"). The fixture `research-reactors-page.json` is synthetic until 3.1 yields a sanitized live shape.

### D8. Ordering with other changes

`add-chat-members-and-creation` edits `bt-probe.rs`, `backend.rs`, `model.rs`, `client.rs`, `store.rs`, `yandex_backend.rs` (see `DEPENDENCIES.md`: its CM-G unit holds them until merged). This change's tasks that touch those files start only when no other in-flight task holds them (rule 1). The pins slice may add a menu entry in `composer.rs`; whichever lands second adapts.

## Pending decisions (to finish after the answers)

Each line names what the answer fixes. None of these is in a task until answered.

- **Q1 entry point** fixes: which of `composer.rs` (menu entry, with a `message-editing` delta "Message context menu") and `timeline.rs` (a chip gesture, with a `message-reactions` delta for the chip's right-click behaviour) is edited; the anchor of the popover; spec requirement "Opening the list". Recommended: menu entry.
- **Q2 paging** fixes: whether the store requests the next page on a click (recommended), automatically until the bound, or incrementally; spec requirement "Loading more". D5's cursor and `more` rule hold in every case.
- **Q3 staleness** fixes: the store's reaction to `base_version` changing (refetch from page 1 with debounce, recommended; an "outdated" row; nothing); spec requirement "List stays current". The store already sees `ClientEvent::ReactionsChanged`.
- **Q4 offline** fixes: nothing new (recommended: D4 step 2 and the "Failed and offline list" requirement already express it); the other option adds a cached partial list and a cache-shape change.
- **Q5 types** fixes: flat list (recommended, D6 as written) or tabs/sections (D6 and a filter state); spec requirement on grouping.
- **Q6 verification target** fixes how task 3.1 picks a multi-reactor message.

## Risks / Trade-offs

- **Unobserved request.** The shape is documented, not yet seen: the cursor bound, order, the `Limit` cap and whether a short page is always the end are inferred from the web client's code (`Mode` 2 disagreement between sources is not tested: the client never sends it). Mitigation: task 3.1 before the tasks that depend on those facts; `REACTORS_PAGE` and the dedupe stay adjustable.
- **Count mismatch.** The header total comes from the message's known counts, which can lag the server. A list may show more entries than the total, or fewer; counts label progress only and never prove completeness or stop paging (D5): a low count cannot hide reactors, a high count cannot loop. If the list ends (D5) while shown + skipped is below the total, the header keeps "N из M" and offers no more.
- **Names via two sources.** A name can differ between the reply and the users cache; the reply wins (it is the fresher), the cache is only a fallback.
- **Privacy.** Reactor names and guids are on stdout of a probe run and in VM logs; they stay out of artifacts, fixtures are sanitized like the existing ones.
- **Channel/unreadable chats.** The server may refuse the list for chats the account follows only; the refusal is a `Failed` state with the reason, not a special case.
