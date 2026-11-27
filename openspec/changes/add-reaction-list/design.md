# Design

## Context

`proposal.md` states the motivation, the decisions taken from evidence (numbered there 1-6, cited here as P1-P6) and the answered questions (Q1-Q6, all answered on 2026-11-26). `research.md` holds the protocol facts and marks what a live read must still confirm (**[probe]**).

Code this change extends:

- `yandex-proto::request::ReadRequest` is the closed read set (`ALLOWED_METHODS`, seven names, pinned by tests); `route()` sends registry reads over HTTP and the rest over the socket; `socket_frame()` builds the body and adds `RequestId`. `Client` (`client.rs`) is the facade (`members`, `whoami`, `history`).
- `wire.rs` parses `UserInfo` into `WireSender {guid, display_name, avatar_id}` and the reaction siblings into `WireReactor {user: Option<guid>, code, timestamp}`; the latter is for `RecentUserReactions` and stays as is.
- `chat-core`: `Backend` (domain types only), `Command`/`ClientEvent`, the members flow (`Command::LoadMembers`, `ClientEvent::Members`, `MembersFailed {chat, reason, offline}`, `client.rs::load_members`) as the model for a read that is never cached and is refused offline without a request; `Message.reactions` with counts, `mine` and `version` (`add-message-reactions` D3).
- `desktop`: `YandexBackend` with the in-memory `users` cache and `Users::load`; `store.rs` `LoadState` for members; `info_panel.rs::body` as a pure state-to-view function; `composer.rs` message menu; `timeline.rs` chips (a left click toggles the own reaction, a right click opens the message menu).

## Goals / Non-Goals

**Goals:**
- The complete list of reactors of a message, read-only, honest about being partial.
- Contracts fixed here so protocol, core, adapter and UI tasks can proceed in parallel; the contracts are written for the answers of Q1-Q6 (proposal, "Answered questions").

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
                          kind: RequestKind,      // First (replaces the rows) | More (appends)
                          state: ReactorsState }  // dropped on close
pub enum ReactorsState { Loading,
                         Ready { rows: Vec<Reactor>, skipped: usize, oldest: Option<MessageTs>,
                                 exhausted: bool, loading_more: bool, more_failed: Option<String> },
                         Failed { reason: String, offline: bool } }
```

The store keeps a counter `next_request` that only grows (never reset by close or reopen). Every `LoadReactors` it sends (open, reopen, retry, load more, refresh) takes a fresh value, sets `generation` to it and thereby supersedes any request still out. A `Reactors` or `ReactorsFailed` is applied only if its `(chat, timestamp)` is the open list **and** its `request` equals `generation`; anything else is dropped (an answer after close, after reopening the same message, or to the page before a refresh). A `First` answer replaces the rows; a `More` answer appends in server order and de-duplicates by guid (the web client does the same).

**End of list** is decided from the page, with strict cursor progress; known counts never decide it. After a page requested with cursor `before` and limit `L`: the list is `exhausted` when `oldest` is `None`, or `raw < L`, or `before` is `Some(b)` and `oldest >= b` (the cursor did not move, so the page is a repeat; continuing would loop). Otherwise the next page is requested with `before = oldest`. An inclusive bound that repeats the boundary entry is harmless: de-duplication drops it and the page still progresses unless it contains nothing older. `raw < L` means "end" only if the probe confirms that a short page is never a server cap (P3 in `research.md`); until then `REACTORS_PAGE` is kept at or below the largest honoured `Limit`, so a short page is an end. The message's known counts only label progress: `total` is the sum of the counts, recomputed from the store's current message; a count that lags low or high changes the header "N из M", never whether more pages are requested or offered. `base_version` is the message's reaction version when the list was last read from the first page; it drives the refresh below.

**Load more (Q2).** The row «Показать ещё» (`!exhausted`) sends `LoadReactors { before: oldest }` with `kind = More` and sets `loading_more`; the answer is appended and de-duplicated. A failed `More` keeps `rows`, clears `loading_more` and sets `more_failed` (the row turns into a retry); a retry takes a fresh token. A failed `First` is `Failed`.

**Refresh (Q3).** A `ClientEvent::ReactionsChanged` for the open list's message whose version differs from `base_version` (re)starts a 1 s debounce timer in the store, whatever the list shows at that moment except the "message gone" failure (a failed list or a failed earlier read does not make the list current, so a change during the delay or while `Failed` is still read); when it fires, the store sends `LoadReactors { before: None }` with `kind = First` and a fresh token, which supersedes any request still out (a `More` in flight, or an earlier refresh). `base_version` is set to the message's current version whenever any `First` request is sent (open, retry, refresh), not only by the timer; a `First` request also takes over a refresh still waiting for its delay. A `Failed` list goes back to `Loading` when its refresh is sent (as for a retry). On a shown list the rows stay displayed until the answer, with `loading_more = true` for as long as the `First` request is out: the flag means "a request that gates the next page is out" (the next page itself, or a read of the first page), so «Показать ещё» cannot send a `More` from the old rows and supersede the refresh. The answer replaces the rows wholesale and recomputes `exhausted`, `oldest`, `skipped`; paging then continues from that page. A failed refresh is `Failed` (an earlier list must not look current). A deleted message (`ClientEvent::MessageDeleted`) turns the list into the terminal "message gone" failure (`MESSAGE_GONE`, "Сообщение удалено"), takes a fresh token without sending anything (so the answer the list was waiting for is dropped) and cancels the timer; a later change does not refresh it. Closing the list, opening another one and signing out cancel the timer; a timer that fires for a closed or different list does nothing.

### D6. View (`desktop`), P4

A pure `body(state, connection, total) -> Body {Loading, Offline, Failed(reason), Gone, Ready { rows, shown, total, skipped, more: More }}` like `info_panel::body`, tested without a window, with `More = Hidden | Offer | Loading | Retry(reason)` derived from `exhausted`, `loading_more` and `more_failed` (never from `total`). A `Ready` list while the connection is not online renders `Offline`, as does a `Failed { offline: true }`: the text «Нет соединения» and a retry (Q4); an offline retry sends a fresh `First` request, which the engine refuses at once while still offline (D4 step 2). The `MESSAGE_GONE` failure is `Body::Gone`: the text «Сообщение удалено», no entries and no retry (a retry could only be refused). The row is avatar, name (placeholder "Неизвестный", own entry suffixed "Вы"), the emoji of the code through `reactions::emoji` or `NEUTRAL_GLYPH`, and the time through the existing `format` helpers; one flat list, no tabs, sections or filters (Q5). Debug selectors `reactor-{guid}`, `reactors-more`, `reactors-retry`, `reactors-gone`. The header "N из M" appears only when `shown + skipped < total`, and a non-zero `skipped` adds "K без данных" (spec "Complete or labelled").

`more` is `Hidden` when the store state is `exhausted`, never derived from `total`; `total` only feeds the header.

### D7. Probe and verification

`bt-probe reactors <chat> <micros> [--before <micros>] [--limit <n>]` prints the parsed reply like `members` does (`{:#?}`, names and guids on stdout, header on stderr). It is a read: it is not covered by `BT_WRITE_ONLY_CHAT` and the write guard tests stay as they are. It sends the one form the app sends (`Mode: 0`); there is no `--mode`, because the request contract (D1) has no other mode and the app never sends one. The server accepted `Mode: 0` (task 3.1, research "Live verification"), so D1 stands. Unit and fixture tests carry everything except server behaviour; server behaviour was confirmed by task 3.1. The fixture `research-reactors-page.json` is a sanitized live shape (2026-11-27).

### D8. Ordering with other changes

`add-chat-members-and-creation` (unit CM-G: 6.3, 6.4, 7.1, not started, gated on approvals) holds no files (`DEPENDENCIES.md` rule 1), so the files of this change are free to take; whoever starts first holds a file until its unit merges. `add-pins-and-importance-view` shares `wire.rs`, `request.rs`, `bt-probe.rs` and the app files (`backend.rs`, `model.rs`, `fixtures.rs`, chat-core `client.rs`, `store.rs`, `yandex_backend.rs`, `desktop.rs`, `tests.rs`); the order is this change first (RL-proto, then RL-app), the pins tasks on those files after the matching unit has merged. The pins slice adds no menu entry, so `composer.rs` is touched only here.

## Decisions after the answers

The answers of 2026-11-26 (proposal "Answered questions"); each fixes the contract named here.

- **Q1 = A, entry point.** `composer.rs`: the entry «Кто отреагировал» (selector `menu-reactors`) for a sent, non-deleted message with at least one reaction; `open_menu` opens the menu even when this entry is its only content. `desktop.rs` hosts the popover, anchored where the menu was, and closes the menu when the entry is chosen. Chip gestures are untouched. Spec: "Opening the list" in `reaction-list` and the `message-editing` delta "Message context menu".
- **Q2 = A, paging.** First page `REACTORS_PAGE` = 30, then «Показать ещё» one request per click; load-more failure keeps the rows (D5). Spec: "Loading more".
- **Q3 = A, staleness.** Debounced (1 s) re-read from the first page on a version change; rows stay until the answer; failed refresh is `Failed` (D5). Spec: "List stays current".
- **Q4 = A, offline.** No new contract: D4 step 2 refuses at once, D6 renders «Нет соединения» with a retry, nothing is cached. Spec: "Failed and offline list" (text now names «Нет соединения» and the retry).
- **Q5 = A, types.** One flat list in server order with a glyph per row (D6). Spec: "Reactor entries" (flat, no grouping). No filter state in the store.
- **Q6 = A, verification target.** Task 3.1 picks a multi-reactor message by a read-only `bt-probe history` count scan inside `bt-vm`; names and guids stay in the VM.

## Risks / Trade-offs

- **Request shape, observed.** Task 3.1 (2026-11-27, research "Live verification") confirmed the shape, `Mode: 0`, the newest-first order and the exclusive `MaxTimestamp` live. Still open: the server `Limit` cap (no readable message sampled, one history page per chat, had more than 16 reactors), so the short-page-is-the-end rule (D5) is proved only for lists up to 16; `REACTORS_PAGE` and the dedupe stay adjustable. (`Mode` 2 disagreement between sources is not tested: the client never sends it.)
- **Count mismatch.** The header total comes from the message's known counts, which can lag the server. A list may show more entries than the total, or fewer; counts label progress only and never prove completeness or stop paging (D5): a low count cannot hide reactors, a high count cannot loop. If the list ends (D5) while shown + skipped is below the total, the header keeps "N из M" and offers no more.
- **Names via two sources.** A name can differ between the reply and the users cache; the reply wins (it is the fresher), the cache is only a fallback.
- **Privacy.** Reactor names and guids are on stdout of a probe run and in VM logs; they stay out of artifacts, fixtures are sanitized like the existing ones.
- **Channel/unreadable chats.** Observed live (2026-11-27, research "Live verification" P5): the server refuses the list for every channel message with `Status` 4 and `ErrorInfo.Message` "reactions are anonymous" (and with "message not found" / "chat not found" for a missing timestamp or chat); the refusal is a `Failed` state, not a special case in the store. Two follow-ups are open for the main session: hiding the menu entry in channel chats (the spec offers it for any sent message with a reaction), and the reason text, which `check_status` loses because the live refusal carries `ErrorInfo.Message` and not `Details`.
