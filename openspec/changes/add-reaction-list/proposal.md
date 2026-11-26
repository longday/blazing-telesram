# Proposal

## Why

Reactions are visible as counts (`add-message-reactions`), but the user cannot see who put them: a chip says "👍 3", not by whom. The first slice left it out on purpose: the push and history rows carry only `RecentUserReactions`, a list the server truncates, so a tooltip would silently omit people (its proposal decision 8). The complete list is a separate read method, `list_reactions` (the web client's `userReactions()`), paged by `Limit` and `MaxTimestamp`. This change adds that read and a read-only list in the UI. Roadmap item 3d.

`research.md` (next to this file) holds the evidence: the request and reply shape from the web client bundle and the protocol notes, the web client's paging and refetch rules, where the web client opens the list, and what is still to confirm with a live read.

## What Changes

- `yandex-proto`: `list_reactions` joins the closed read allowlist as one new `ReadRequest` variant (socket, `{ChatId, Timestamp, Limit, MaxTimestamp?, Mode: 0}`); a parser for `UserReactions[]` (`UserInfo`, `Type`, `Timestamp`); a client method. The write set is not touched: `list_reactions` is a read, nothing mutates.
- `bt-probe`: a read-only `reactors` command to confirm the open protocol points live before the app depends on them (research "Verification plan").
- `chat-core`: a domain `Reactor` (user, name, avatar, reaction type, time, own flag), a one-page `Backend::reactors` read, `Command::LoadReactors` and its result events, mirroring the members flow (read, never cached, offline refused at once).
- `desktop`: the adapter maps the reply (names from the inline `UserInfo`, then the adapter's users cache, then a neutral placeholder); the store holds the open list in memory; a list view shows, per entry, avatar, name, the reaction's emoji and when it was put; loading, failed and offline states.
- Entry point, paging, behaviour when the reactions change while the list is open, offline detail and presentation of types are **open questions** (below). The tasks that depend on them are marked blocked.

## Capabilities

### New Capabilities
- `reaction-list`: reading and showing the complete list of people who reacted to a message, read-only.

### Modified Capabilities
- `message-editing` (only if open question 1 is answered with the menu entry, the recommended answer): the "Message context menu" requirement gains an entry for the list. Written after the answer; see task 6.4.

## Impact

- `crates/yandex-proto`: `request.rs` (variant, allowlist of eight, tests pin the exact list), `wire.rs` (`parse_reactors`, entry type), `client.rs` (facade), `tests/wire_fixtures.rs` and a new sanitized fixture, `bin/bt-probe.rs` (`reactors`).
- `crates/chat-core`: `model.rs`, `backend.rs`, `client.rs` (engine and fake backend).
- `crates/desktop`: `yandex_backend.rs`, `store.rs`, a new list view module, and whatever open question 1 selects (`composer.rs` for the menu entry, `timeline.rs` for a chip gesture).
- Specs: new `reaction-list`; no change to `message-sending` (a read is not a write) and none to `offline-cache` (nothing is cached; see decision 3).
- Risk: the request shape is documented, not yet observed: the `Mode` semantics disagree between the bundle (`ALL = 2`) and the notes (2 answers reactions only), the cursor bound and the largest `Limit` are unknown, and no multi-reactor message has been read. All reads are read-only; the live verification (task 3.1) gates the tasks that depend on those facts.
- Overlap: `crates/desktop/src/desktop/composer.rs` and `store.rs` are also edited by the pins slice if it lands first; the one that lands second adapts (`DEPENDENCIES.md` rule 1).

## Non-goals

- Who **read** a message (`Mode 1`, `UserReads`, `ReadsCount`): a different feature with its own semantics ("absent means not tracked"); a later change.
- Adding, changing or removing reactions (exists), reaction notifications, extended picker.
- Persisting reactor lists in the offline cache (decision 3).
- Opening a reactor's profile or chat from the list.
- A per-type server filter: the server has none; type grouping is the client's (open question 5).

## Follow-ups

- Message readers list (`Mode 1`) once the read-receipt semantics are settled.
- Clicking a reactor to open their profile or chat (needs a profile view that does not exist for arbitrary users).
- Per-type tabs, if question 5 recommends the flat list first.

## Decisions

Taken from the evidence without asking, each reversible in `design.md`:

1. **Read, not write.** `list_reactions` is the web client's own read on the socket. The closed write set is unchanged; the closed read set grows by exactly one variant (D1).
2. **Page-level backend.** `Backend::reactors` returns one page (`limit`, optional `before`); the policy (how many pages, when) lives above it, so the answer to open question 2 changes no lower contract (D2, D4).
3. **Never cached.** The list is fetched on demand and held in memory while shown, like the member list (`add-chat-members-and-creation` decision 8): who reacted changes on the server, and a stale list shown as current is misleading. Offline therefore says so (open question 4 only asks whether to show a labelled partial list instead).
4. **Names.** Inline `UserInfo` first, then the adapter's existing users cache (`get_users_data`), then "Неизвестный", the placeholder the timeline already uses; never a blank row and never a raw guid (D3).
5. **Reactions only.** The request sends `Mode: 0`; reads are not asked for (non-goal).
6. **Honest counts.** The server sends no total, so a list that is not complete says how many of how many it shows, using the message's known counts (spec "Complete or labelled").

## Open questions

Each has a recommendation; none is decided. Tasks that depend on an answer are marked blocked in `tasks.md`.

### Q1. Where does the user open the list?

A left click on a chip already toggles the own reaction, and a desktop client has no long-press, so "click or long-press on a chip" is not available as is.

- **A (recommended): a "Кто отреагировал" entry in the message context menu**, shown for a sent, non-deleted message that has reactions; opens the list in a popover anchored where the menu was. Pros: matches the web client's menu item, no gesture conflict, discoverable next to the reaction row, trivial to test (a menu selector). Cons: two steps (right click, entry).
- **B: A plus a secondary gesture on a chip** (for example Shift+click, or a right click on a chip), opening the same popover already filtered to that chip's type. Pros: one step for "who put 👍". Cons: hidden gesture; a right click on a chip today opens the message menu, so this changes an existing behaviour and its spec; the filter needs question 5.
- **C: a hover tooltip over a chip**, filled by a delayed request. Pros: no click. Cons: a request per hover, flicker on every reaction update, not interactive (no paging, no scroll), so only good for a few reactors, and the problem that sank the first tooltip (a partial list) returns.

### Q2. How are many reactors loaded?

- **A (recommended): on demand.** First page of 30 (the web menu's `2 * 15`), then a "Показать ещё" row at the end of the list, one request per click; the header says "N из M". Pros: bounded traffic, instant first paint, the same cursor in every case. Cons: one more control and engine state per open list.
- **B: load every page, bounded** (like members: pages of 50, at most 20, so 1000), the list shown when done. Pros: no paging control, complete list at once, grouping and per-type tabs become exact. Cons: a viral message costs up to 20 sequential requests before anything shows; a hard cap still leaves the list "partial".
- **C: B but shown incrementally** (first page at once, the rest streamed in). Pros: both benefits. Cons: the most state (a growing list under the user's cursor).

### Q3. What if the message's reactions change while the list is open?

Facts: the message's `ReactionsVersion` is known to the store and changes with every push (`add-message-reactions` D5); the web client keys its query by it and refetches from the first page.

- **A (recommended): refetch from the first page when the version changes** (debounced, 1 s), keeping the current rows on screen until the reply replaces them; on-demand paging restarts. Pros: the list stays true to the chips without a click; same as the web client. Cons: automatic traffic on a busy message, and a user who scrolled deep returns to the top.
- **B: mark the list outdated** with a row "Список изменился: обновить" and reload on a click. Pros: no surprise movement, no automatic traffic. Cons: the list contradicts the chips until clicked.
- **C: a snapshot**: the list shows the state at the moment it was opened, with its time in the header. Pros: simplest. Cons: contradicts the chips visibly on any busy message.

### Q4. What does the list show without a connection?

- **A (recommended): the entry stays, the popover says "Нет соединения" with a retry**; nothing is cached (decision 3). Pros: never misleading, nothing new persisted, same as members. Cons: nothing to see offline.
- **B: show the partial `RecentUserReactions` already known**, labelled "Показаны последние N из M". Pros: something offline. Cons: needs the reactor names stored with the message in the offline cache (a new persisted field of personal data and a cache-shape change), and it is the partial list the first slice refused to show.

### Q5. How are the reaction types presented?

- **A (recommended): one flat list, newest first**, each row ending with its reaction's emoji (neutral glyph for an unknown type), like the web client. Pros: complete under any paging, no extra state. Cons: finding "who put 👍" means scanning.
- **B: tabs or a filter per type** ("Все 5", "❤️ 2", "👍 3"). Pros: answers "who put 👍" directly. Cons: exact only when every page is loaded (the server has no type filter); with Q2 A each tab would show "N из M" and may need more pages.
- **C: grouped sections by type.** Same completeness problem as B, plus a long list.

### Q6. Which message verifies paging live?

The only reacted message on record has one reactor (the account), so it can confirm the shape but not paging, order or the cursor bound.

- **A (recommended): read-only scan** inside `bt-vm` of `bt-probe history` over the user's chats printing counts only, pick a message with several reactors, run `reactors` on it; only shapes (counts, order, booleans) go into `research.md`, with a synthetic fixture; names and guids stay in the VM. Cons: reads the user's real chats (the same reads the app does).
- **B: the user names one message** (a chat and a timestamp). Pros: nothing scanned. Cons: needs the user.
- **C: no live paging check**: rely on the web client's code and a synthetic fixture. Cons: the cursor bound and order stay assumptions.
