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

## Live verification (task 3.1, 2026-11-27, read-only, inside `bt-vm`)

Setup: release `bt-probe` built in the VM from `main` (`df1a66f`, with `reactors`), signed-in session in `~/bt/.runtime`, outbound network restored (the 2026-11-26 failure, see "Live reads attempted", was an environment fault). Every call was `bt-probe reactors` (`list_reactions`, always `Mode: 0`) with `BT_LOG=yandex_proto::xiva=trace`; the raw reply frames were read inside the VM by a script that printed only keys, value types, counts, numeric timestamps and type codes, and booleans. No write of any kind was sent. Targets: the self-chat message with the ✅ reaction (1 reactor, the account); a read-only scan of `bt-probe history` over all 116 chats (2 channels, 70 groups, 44 private; counts only) found messages with 2 to 21 reactions. The scan read **one history page per chat** (the latest messages, up to 50; `reached_start=false` for most), so every "largest" or "none" below is about the messages sampled, not the whole account history. The paging target is a group message with 16 reactors (three types: 12 + 2 + 2).

### P1 Shape

- **Reply:** `{"ReadsVersion": int, "UserReactions": [...]}`. `ReadsVersion` is a top-level key not in the web client's deserializer (B) or in the notes; the client ignores it (it is not a total or a paging marker: 1607 for one message, 6540 for another).
- **Entry:** exactly `Timestamp` (int, microseconds, when the user reacted), `Type` (int, e.g. 100101 for ✅, 100108, 100104) and `UserInfo` (object); all three present in every entry inspected key by key (the 1-entry self-chat reply and a 4-entry page).
- **`UserInfo`:** `AvatarId`, `DisplayName`, `Guid`, `Nickname` (strings), `Uid` and `Version` (ints), all six present in every entry inspected. So the name and avatar come with the entry, as designed (D3). An entry without them was not observed (the variants in `wire_fixtures.rs` are synthetic).
- **Empty list:** a message nobody reacted to answers `{"ReadsVersion": n}` with no `UserReactions` key and no error, as assumed (`parse_reactors` reads it as an empty page).
- **`Mode: 0` is accepted:** every successful call sent it. Other modes were not sent (the client never sends them).
- **Own entry:** the account's own reaction is a normal entry (✅ on the self-chat message: 1 entry whose `UserInfo.Guid` is the account's guid).

### P2 Order and cursor

- **Newest reaction first.** The 4-entry page and the 1-entry page were strictly descending in `Timestamp`; for the 16-entry list the cursor splits it consistently with a strictly descending order (cursor = the 9th entry returned exactly the 7 after it).
- **`MaxTimestamp` is exclusive:** a page requested with cursor `c` holds only entries with `Timestamp < c`; the boundary entry is not repeated (cursor = the newest of the 16 returned the other 15; cursor = the oldest returned none). In four full walks (limits 5, 7, 16, 30) every page lay strictly below its cursor and its oldest entry was strictly older than the previous page's oldest. Design D5's "inclusive repeat" branch is harmless but never fires on this server.
- **Out-of-range cursors:** a cursor below every entry (1) or equal to the oldest: an empty reply (`{"ReadsVersion": n}`), not an error. A cursor above every entry (a future time, or the newest + 1): the first page again. **`MaxTimestamp: 0` also gives the first page** (0 behaves as unset); the client never sends 0 (`before` is the oldest timestamp of a page).

### P3 `Limit`

- Honoured exactly while the list has that many: `Limit` 1, 2, 15 and 16 returned 1, 2, 15 and 16 of the 16 entries; 17, 30, 50, 100, 1000 and 100000 were all accepted (no error) and returned the 16 that exist.
- **The server cap is not observed:** no readable message sampled (one page per chat) had more than 16 reactors (the larger ones are channel messages, which refuse, see P5), so `Limit` 1000 and 100000 cannot tell a cap from the end of the list. Older messages with longer lists may exist. `Limit` 0 and a missing `Limit` cannot be sent by `bt-probe` (`--limit` is at least 1) and were not tried.
- **A short page was the end in every walk:** after a page with `n < Limit` the next page was empty (limits 5, 7 and 30; with limit 16 the full page was followed by an empty one). So design D5's `raw < Limit` rule held for every list available; it is **not proved** for a list longer than 16. `REACTORS_PAGE` stays 30 (Q2): no `Limit` up to 100000 was refused. Residual risk: if the server capped `Limit` between 17 and 29, a capped page would read as the end and the list would stop early (labelled "N of M", never presented as complete, spec "Complete or labelled").

### P4 Consistency

- The 16 entries of the group message, over every page size, equal the sum of its row's counts (12 + 2 + 2 = 16), with 16 distinct guids and none repeated across pages (limit 5: pages of 5, 5, 5, 1; limit 7: 7, 7, 2; limits 16 and 30: one page). The one-reactor messages (self chat and a private chat) had 1 entry each. The own entry is present (P1).

### P5 Errors

All refusals are a reply with `Status` 4 and an `ErrorInfo` object `{Message: string, WorkerId: int}`; **there is no `Details` key** (the key `wire::check_status` reads), so today a refusal surfaces as `status 4 ENTITY_NOT_FOUND` without the server's reason.

| Request | Answer |
|---------|--------|
| a timestamp that does not exist (1, or a future time) in the self chat or a private chat | `Status` 4, `ErrorInfo.Message` "message not found" |
| an unknown or malformed chat id | `Status` 4, "chat not found" (a malformed id is not told apart from an unknown one) |
| a message of a **channel** the account follows (3 messages with 16 to 21 reactions, any `Limit`) | `Status` 4, **"reactions are anonymous"**: the list is never available for channel messages |
| `Timestamp` 0 or negative | no reply frame: the socket proxy answers with a transport error (`error_code=2`); the probe fails with exit code 1 (the app never sends such a timestamp) |
| a deleted message | **not observed**: none was at hand and none may be made (no writes); the nearest observation is "message not found" for a missing timestamp |
| a chat the account is not in | **not observed** (only an invented id: "chat not found") |
| a missing `Timestamp` key | **not observed** (the probe cannot omit it) |

### Consequences for the design (see `design.md` Risks)

1. **Channels:** the entry «Кто отреагировал» is offered for any sent message with a reaction (spec "Opening the list"), but in channels the server always refuses with "reactions are anonymous", so the list always ends in the `Failed` state. The design's "Failed with the reason, not a special case" holds, but the reason text is lost today (next point). Decision for the main session: hide the entry in channel chats, or keep the refusal.
2. **Reason text:** `check_status` reads `Details`, the live refusal carries `ErrorInfo.Message`; the failure shows no reason beyond the status name. The client test `a_refused_reactors_read_is_the_usual_proto_error` uses the synthetic `Details` shape. The fix is in `wire.rs` (not a file this task owns).
3. **No contradiction** with D1 (`Mode: 0` accepted, shape as designed), D5 (cursor, order, end rule), D3 (names in the entry) or `REACTORS_PAGE = 30`; the unobserved cap is recorded above.
4. **Fixture:** `research-reactors-page.json` is now the sanitized first 4 entries of the 16-reactor group message (placeholders for guid, name, nickname, avatar and uid; `Version` normalized; timestamps rebased); `live_reactors_page` and `live_reactors_empty_and_refused_replies` in `wire_fixtures.rs` assert it; the missing-avatar, missing-name and missing-timestamp variants are built from it in the test and marked unobserved.
