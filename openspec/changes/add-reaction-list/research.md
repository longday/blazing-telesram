# Research: who reacted (`list_reactions`)

Date 2026-11-26. Read-only. Static evidence only (public web bundle, protocol notes, repository). No live call to `list_reactions` was made: no read probe for it exists yet (task 3.1 adds one), and the Tart VM `bt-vm` had no outbound network during this discovery (see "Live reads attempted"). Everything marked **[probe]** is a claim the first live read must confirm or correct before the dependent tasks are finished.

## Sources

- Protocol notes (reverse engineering, not an official API): `conarti/yandex-messenger-mcp`, `docs/protocol-research.md`: section 2.3 (method table: `list_reactions` is the web client's `userReactions()`), 14.3 (request shape), 17.12 (live corrections), 9.2-9.3 (reaction events).
- Public web client, build `3.6.3` (the build `add-message-reactions/research.md` pinned): `https://yastatic.net/s3/chat-static/telemessenger/_/3.6.3/web/app.js` (sha256 `88292adf...0bc73`), the lazy chunk `.../web/ui.chunk.js` (sha256 `37303c76...a166b9`, 1 429 309 bytes) and `.../web/quick-access-panel.chunk.js` (sha256 `0a72ceaa...4e7`). Chunk names come from the `d.u` map in `app.js`. Anonymous GET, nothing signed in.
  - **A** `app.js`: `userReactions(e,t){return this.get(e,"list_reactions",t)}` on the socket transport (the same `get` as `history`, `whoami`, `message_info`); a second transport class repeats it.
  - **B** `app.js`: the backend facade `getUserReactions({chatId, timestamp, inviteHash, limit, cursor, mode})` builds `{ChatId, Timestamp, InviteHash, Limit, MaxTimestamp: cursor, Mode}` and reads `UserReactions` and `UserReads` of the reply; both default to `[]`.
  - **C** `app.js`: enum `ListReactionsMode`: `ONLY_REACTIONS = 0`, `ONLY_READ_TIMESTAMPS = 1`, `ALL = 2`.
  - **D** `app.js`, deserializers: `UserReactions[]` entry to `{user: deserializeUserInfo(UserInfo), type: Type, timestamp: Timestamp}`; `UserReads[]` entry to `{user, timestamp}`.
  - **E** `ui.chunk.js`, the list component (query key `user_reactions_list`, UI log scope `user_reacted_list`): paging, cursor, staleness, rows (details below).
- Repository: `crates/yandex-proto/src/request.rs` (closed read set, 7 methods, none for this), `wire.rs` (`WireReactor`, `WireSender`), `tests/fixtures/live-push-reaction.json`, `crates/desktop/src/yandex_backend.rs` (`Users` cache), `add-message-reactions` (archived; proposal decision 8, design D1-D8).

## Request and reply

| Item | Evidence | Value |
|------|----------|-------|
| Transport | A | socket method `list_reactions` (not the HTTP registry); like `history`, so `Route::Socket` and the frame's own `RequestId` |
| Request body | B, notes 14.3 | `{ChatId, Timestamp, Limit, MaxTimestamp?, Mode?, InviteHash?}`; `Timestamp` is the message's (it is its only id, notes 17.11) |
| `Mode` | C, notes 17.12 | 0 reactions only; 1 reads only (`UserReads` + `ReadsCount`); 2 `ALL` in the bundle, but notes 17.12 say live: default, 0 and 2 all answer `UserReactions` only, 3 is an error. The client sends **`0`** and nothing else (reads are out of scope; the 2-vs-notes disagreement is therefore not tested). If the server refuses an explicit `0`, the request omits `Mode` (notes: default answers reactions) **[probe: `0` accepted]** |
| `InviteHash` | B | for chats joined by invite link; our `history` sends none either, so none here |
| Paging | E, B | `Limit` + `MaxTimestamp`; no `Offset`, no `Type` filter: the list is always every type, grouped by the client |
| Reply | D, notes 17.12 | `UserReactions: [{Type: int, Timestamp: int(microseconds, when the user reacted), UserInfo{Guid, DisplayName, AvatarId, ...}}]`; the key is absent when empty (B defaults it) |
| Total | E | none in the reply. The web client computes "more" as `loaded items < sum of Reactions[].Count (+ ReadsCount)` using the counts it already holds |
| Own entry | notes 9.2, fixture | the account's own reaction is a normal entry (`RecentUserReactions` carries it too) **[probe]** |
| One entry per user | archive 7.2 (REPLACE keeps one own reaction) | the web client still de-duplicates rows by guid; the client must too |

`UserInfo` inside the reaction entries already carries `DisplayName`, `AvatarId`, `Guid`, `Nickname` (live fixture `live-push-reaction.json`, `RecentUserReactions[0].UserInfo`), so names usually need no second request. The web client's row is: avatar, display name, reaction image of `Type`, and the reaction's date and time as the description; a click opens the user's profile.

### Paging, from the web client (E)

- First page limit: `2 * maxItems`: 16 for the list under the reaction picker (`maxItems 8`), 30 for the item in the message menu (`maxItems 15`). Our probe and the server cap are open **[probe: largest accepted `Limit`]** (the notes show 50 in the shape).
- The list starts from the message's `RecentUserReactions` (no request) and the first fetch takes the cursor from its last entry: `MaxTimestamp = Timestamp of the last entry loaded`, `Mode = ONLY_REACTIONS`. So the order is **newest reaction first** and `MaxTimestamp` bounds the older page **[probe: order, and whether the bound is inclusive: the web client de-duplicates by guid, which suggests it can repeat the boundary entry]**. The web client decides "more" from counts; this client does not (design D5: page exhaustion plus strict cursor progress), because a lagging count would either hide reactors or, with a repeating boundary, loop.
- The query key includes `ReactionsVersion` and the reads version, so a version change in the store starts a new query (the old ones are removed): the web client's answer to staleness is "refetch under the new version", from the first page.
- Retry: unlimited retries with the connection, `cancelInOffline: false`, i.e. the web client waits for the network instead of failing.

### Where the web client opens it (E)

1. In the message context menu: an item that expands a list (`reacted-list`, 15 rows, reads included when the chat tracks reads).
2. Under the reaction picker popup (`reactions_picker_popup`, 8 rows, reactions only).

Chips are not the entry point: a click on a chip toggles the own reaction in this client too (`add-message-reactions` spec "Reacting is a user action").

## What this client has today

- No read method, no request variant, no probe command for `list_reactions`; `ALLOWED_METHODS` pins exactly seven names (`request.rs`).
- `RecentUserReactions` is decoded to `WireReactor {user: Option<guid>, code, timestamp}` and reduced to the own `mine` in the adapter; the other reactors are dropped (`add-message-reactions` D3, D7). It is truncated by the server (the notes record 3 recent entries for `ReadsCount 10`), which is why the first slice had no tooltip.
- User names: `WireSender {guid, display_name, avatar_id}` is parsed from inline `UserInfo`; the adapter's in-memory `users: HashMap<guid, WireUser>` (filled by `get_users_data`, `Users::load`) is the fallback for anonymous senders, and the UI shows "Неизвестный" when both are empty (`yandex_backend.rs`, `timeline.rs`). There is no persistent users table.
- A read-only list with loading, failed and offline states exists for members: `Command::LoadMembers`, `ClientEvent::Members` / `MembersFailed {reason, offline}`, `LoadState` in `store.rs`, never cached, "offline says so" (`add-chat-members-and-creation` decision 8). This is the pattern to mirror.
- The message context menu (`composer.rs`: `MenuAction`, `menu_actions`, `message_menu_view`) has the reaction row, Reply, Copy, Edit, Delete; a right click anywhere on the bubble, chips included, opens it (`timeline.rs::bubble`). A left click on a chip toggles the own reaction. The UI is GPUI: right click, left click, hover; there is no long-press gesture on desktop.

## Live reads attempted

2026-11-26, inside `bt-vm`, existing release `bt-probe` (`chats`, read-only): failed with `timed out waiting for the push socket`; `curl https://yandex.ru/chat` from the VM answered `000` (timeout) while the host reached it (200). The VM's Ethernet service was enabled; no network setting was changed (another agent shares the VM). So no history scan for messages with several reactors was possible. The scan is part of task 3.1.

## Verification plan (task 3.1, read-only, inside `bt-vm`)

New command `bt-probe reactors <chat> <micros> [--before <micros>] [--limit <n>]` (reads only; no write guard needed, nothing is sent but `list_reactions`, always with `Mode: 0`). Targets: the disposable self-chat message left by `add-message-reactions` task 7.2 (✅ x1, one reactor: the account) for the shape, and any message with two or more reactors found by scanning `bt-probe history` of the user's chats (counts only) for paging. Questions:

1. P1 Shape: exact keys, `UserInfo` fields present, key absence when empty; an explicit `Mode: 0` is accepted.
2. P2 Order and cursor: newest first; whether `MaxTimestamp` is inclusive (a repeated boundary entry); what an out-of-range cursor returns (empty reply, not an error); that the oldest `Timestamp` of a page strictly decreases from page to page unless the page is a repeat.
3. P3 `Limit`: the largest value honoured; what happens at `0`, `1`, a very large value; whether a page shorter than the `Limit` asked is always the end of the list or can be a server cap (decides design D5's `raw < limit` rule and the final `REACTORS_PAGE`).
4. P4 Consistency: sum of entries over all pages equals `sum(Reactions[].Count)` of the message row; the own entry appears; no repeated guid.
5. P5 Errors: a deleted message, a timestamp that does not exist, a message in a chat the account cannot read (a channel the account only follows): the status and `Details` strings the adapter must map.

Findings go into this file ("Live verification") when 3.1 runs; any contradiction of `design.md` reopens the affected decision.
