# Proposal

## Why

Reactions are visible as counts (`add-message-reactions`), but the user cannot see who put them: a chip says "👍 3", not by whom. The first slice left it out on purpose: the push and history rows carry only `RecentUserReactions`, a list the server truncates, so a tooltip would silently omit people (its proposal decision 8). The complete list is a separate read method, `list_reactions` (the web client's `userReactions()`), paged by `Limit` and `MaxTimestamp`. This change adds that read and a read-only list in the UI. Roadmap item 3d.

`research.md` (next to this file) holds the evidence: the request and reply shape from the web client bundle and the protocol notes, the web client's paging and refetch rules, where the web client opens the list, and what is still to confirm with a live read.

## What Changes

- `yandex-proto`: `list_reactions` joins the closed read allowlist as one new `ReadRequest` variant (socket, `{ChatId, Timestamp, Limit, MaxTimestamp?, Mode: 0}`); a parser for `UserReactions[]` (`UserInfo`, `Type`, `Timestamp`); a client method. The write set is not touched: `list_reactions` is a read, nothing mutates.
- `bt-probe`: a read-only `reactors` command to confirm the open protocol points live before the app depends on them (research "Verification plan").
- `chat-core`: a domain `Reactor` (user, name, avatar, reaction type, time, own flag), a one-page `Backend::reactors` read, `Command::LoadReactors` and its result events, mirroring the members flow (read, never cached, offline refused at once).
- `desktop`: the adapter maps the reply (names from the inline `UserInfo`, then the adapter's users cache, then a neutral placeholder); the store holds the open list in memory; a list view shows, per entry, avatar, name, the reaction's emoji and when it was put; loading, failed and offline states.
- Entry point, paging, refresh while open, offline detail and presentation of types are decided (see "Answered questions"): a «Кто отреагировал» entry in the message context menu; on-demand «Показать ещё» paging; a debounced re-read from page 1 when the reactions change; «Нет соединения» with a retry offline; one flat list.

## Capabilities

### New Capabilities
- `reaction-list`: reading and showing the complete list of people who reacted to a message, read-only.

### Modified Capabilities
- `message-editing`: the "Message context menu" requirement gains the «Кто отреагировал» entry for a sent, non-deleted message that has reactions (delta in this change).

## Impact

- `crates/yandex-proto`: `request.rs` (variant, allowlist of eight, tests pin the exact list), `wire.rs` (`parse_reactors`, entry type), `client.rs` (facade), `tests/wire_fixtures.rs` and a new sanitized fixture, `bin/bt-probe.rs` (`reactors`).
- `crates/chat-core`: `model.rs`, `backend.rs`, `client.rs` (engine and fake backend).
- `crates/desktop`: `yandex_backend.rs`, `store.rs`, a new list view module, `composer.rs` (the menu entry) and `desktop.rs` (the popover host).
- Specs: new `reaction-list`; no change to `message-sending` (a read is not a write) and none to `offline-cache` (nothing is cached; see decision 3).
- Risk: the request shape is documented, not yet observed: the cursor bound, the order, the largest `Limit`, and whether a short page is always the end are unknown, and no multi-reactor message has been read (the client sends only `Mode: 0`, so the sources' disagreement on `Mode` 2 does not matter unless the server refuses 0). All reads are read-only; the live verification (task 3.1) gates the tasks that depend on those facts.
- Overlap: the not-started tasks of `add-chat-members-and-creation` (CM 6.3, 6.4, 7.1, gated on approvals) hold no files (`DEPENDENCIES.md` rule 1), so the files listed above are free. `add-pins-and-importance-view` shares `wire.rs`, `request.rs`, `bt-probe.rs` and the app files; the order is RL first (all questions answered), the pins tasks on shared files start after the matching RL unit has merged.

## Non-goals

- Who **read** a message (`Mode 1`, `UserReads`, `ReadsCount`): a different feature with its own semantics ("absent means not tracked"); a later change.
- Adding, changing or removing reactions (exists), reaction notifications, extended picker.
- Persisting reactor lists in the offline cache (decision 3).
- Opening a reactor's profile or chat from the list.
- A per-type server filter: the server has none; the client shows one flat list (Q5), no type grouping.

## Follow-ups

- Message readers list (`Mode 1`) once the read-receipt semantics are settled.
- Clicking a reactor to open their profile or chat (needs a profile view that does not exist for arbitrary users).
- Per-type tabs, if the flat list proves hard to scan (Q5 chose the flat list first).

## Decisions

Taken from the evidence without asking, each reversible in `design.md`:

1. **Read, not write.** `list_reactions` is the web client's own read on the socket. The closed write set is unchanged; the closed read set grows by exactly one variant (D1).
2. **Page-level backend.** `Backend::reactors` returns one page (`limit`, optional `before`); the policy (how many pages, when) lives above it, in the store (Q2: on demand).
3. **Never cached.** The list is fetched on demand and held in memory while shown, like the member list (`add-chat-members-and-creation` decision 8): who reacted changes on the server, and a stale list shown as current is misleading. Offline therefore says so (Q4).
4. **Names.** Inline `UserInfo` first, then the adapter's existing users cache (`get_users_data`), then "Неизвестный", the placeholder the timeline already uses; never a blank row and never a raw guid (D3).
5. **Reactions only.** The request sends `Mode: 0`; reads are not asked for (non-goal).
6. **Honest counts, counts do not page.** The server sends no total, so a list that is not complete says how many of how many it shows, using the message's known counts (spec "Complete or labelled"). Whether more pages exist is decided from the page itself (a short page, or a cursor that did not move), never from the counts, which can lag in either direction (design D5).

## Answered questions

Answered on 2026-11-26 in the main session (autonomous mode: the user delegates decisions); every answer is the recommended option. The spec (`reaction-list`, `message-editing` delta), `design.md` and `tasks.md` are written for them.

### Q1 = A. Entry point: «Кто отреагировал» in the message context menu

A «Кто отреагировал» entry in the message context menu, shown for a sent, non-deleted message that has reactions; it opens the list in a popover anchored where the menu was. No gesture conflict (a left click on a chip still toggles the own reaction, a right click opens the menu), discoverable, testable by a menu selector. Cost accepted: two steps. Rejected: B (a secondary gesture on a chip; changes an existing behaviour), C (a hover tooltip: a request per hover, not interactive, the partial-list problem again). Needs the `message-editing` delta of this change.

### Q2 = A. Paging: on demand

First page of 30 (the web menu's `2 * 15`), then a «Показать ещё» row at the end, one request per click; the header says "N из M" while the list is incomplete. The cursor and the end-of-list rule are `design.md` D5. Rejected: B (all pages up to 20, shown when done), C (B shown incrementally).

### Q3 = A. Reactions change while the list is open: re-read from page 1

When the message's reaction version changes, the store re-reads from the first page after a 1 s debounce, keeps the current rows on screen until the reply replaces them, and restarts on-demand paging; same as the web client. Cost accepted: automatic traffic on a busy message and a deep list returning to the top. Rejected: B (an "outdated" row), C (a snapshot with its time).

### Q4 = A. Offline: «Нет соединения» with a retry, nothing cached

The entry stays; the popover says «Нет соединения» and offers a retry; nothing is persisted (decision 3), no cache change. Rejected: B (partial `RecentUserReactions` with names stored in the offline cache: a new persisted field of personal data).

### Q5 = A. One flat list, a type glyph per row

Server order (newest first), each row ending with its reaction's emoji (neutral glyph for an unknown type). No tabs, no sections, no filter state. Rejected: B (tabs per type), C (sections): exact only when every page is loaded. Task 6.6 therefore has no work.

### Q6 = A. Paging verification target: a read-only scan in the VM

Inside `bt-vm`, `bt-probe history` over the user's chats prints counts only; a message with several reactors is picked and `reactors` is run on it; only shapes (counts, order, booleans) go into `research.md`, with a synthetic fixture; names and guids stay in the VM. The same reads the app does, no write. Task 3.1's paging part is no longer blocked by a decision; it still needs the VM's outbound network.
