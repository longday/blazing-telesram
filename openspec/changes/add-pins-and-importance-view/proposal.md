# Proposal

## Why

The client shows reactions now, but two other pieces of message state set by other people are invisible: which message of a chat is **pinned** (the web client keeps it in a bar above the timeline) and which messages are marked **important** (a per-message flag). Both are first-slice items of roadmap 3a ("message pins and importance: first incoming data and display, then explicit actions", `add-message-reactions/research.md`, "Recommended scope split"). This change is the read side only: a user who opens a chat sees what others pinned and flagged, live and offline; the client does not pin, unpin or flag anything.

`research.md` (next to this file) is the single evidence document: where the pin and the flag come from in the protocol, how a change arrives, what the web client does, what is still unobserved.

## What Changes

- Decode the pin: the chat entry's `PinnedMessageInfo` / `ChatInfo.PinnedMessageTimestamps` and the `Pin` push (today a `Pin` push is folded into `PushEvent::ChatChanged` and nothing reads the pin). The push is a snapshot (a timestamp, or none), not a toggle.
- Show a **pinned-message bar** under the chat header of the open chat: author, one line of the pinned text or the usual placeholder; live from `Pin` pushes; edits and deletions of the pinned message follow; kept in the offline cache with the last shown text. The text comes from the loaded or cached copy, otherwise from one `history` read already in the closed read set (`MaxTimestamp = ts + 1`, `Limit = 1`); the pinned message is never written into the chat's cached history.
- Decode `Plain.IsImportant` (today dropped as meta) into a message flag and show an **important marker** in the bubble; the flag follows history rows, pushes and the offline cache. A change of importance made elsewhere is expected to arrive as an updated `Plain` (inferred from the web client, unobserved; see Q4).
- `bt-probe`: `history` prints the flag, `chats` prints the pinned timestamp, so the live reads in tasks 0.1 and 0.2 and later checks are possible without a raw trace.
- Display only. **No write is added**: `Pin` and `UpdateFields` stay unrepresentable, `WriteRequest` and `CLIENT_MESSAGE_VARIANTS` do not change, the `message-sending` spec ("Writes only from user actions") needs no delta. One related hazard is recorded and left to a decision (Q6): an edit of an own important message may clear its flag, because the server replaces the whole message on edit.

## Capabilities

### New Capabilities
- `message-pins`: the pinned-message bar, its source, live updates, edit/delete follow-through, offline cache, separation from the pinned chat, display only.
- `message-importance`: the important marker, its source, live updates, offline cache, display only.

### Modified Capabilities
- `live-updates`: pin and importance changes apply live without side effects on scroll, rows and typing.
- `offline-cache`: the pin of a chat (with its shown text) and the flag of a message are cached; the pinned message is not stored as history.

`chat-list` is not modified: its pinned marker and ordering are the **chat** pin and stay as they are (`message-pins` has a requirement that pins a message does not touch them). `message-history` and `message-editing` are not modified (the marker is specified in `message-importance`; the menu gains nothing).

## Impact

- `crates/yandex-proto`: `wire.rs` (`WireMessage.important`; `WireChat.pinned`; `PushEvent::PinChanged`; `Pin` leaves the `ChatChanged` list; `IsImportant` is read, not dropped), `xiva.rs` (`push_kind` arm), tests and fixtures; `bin/bt-probe.rs` (output only). Conditionally (task 1.3, blocked by 0.1): `request.rs` gets one read variant (`ChatInfo`) if chat-list entries do not carry the pin; `ALLOWED_METHODS` does not change (the method is `history`).
- `crates/chat-core`: `model.rs` (`Message.important`, `ChatSummary.pinned_message`, `PinnedMessage`, text helper moved from the adapter), `backend.rs` (`LiveEvent::PinChanged`, `Backend::message_preview`), `client.rs` (pin rules, preview resolution, follow-through of edits and deletions), `cache.rs` (a legacy-row test only: both fields are `serde(default)` in JSON columns, no SQL migration), `fixtures.rs`.
- `crates/desktop`: `yandex_backend.rs` (mapping, live loop, `message_preview`), `store.rs` (open chat's pin), `timeline.rs` (marker), `desktop.rs` (bar), `theme.rs`, `tests.rs`.
- No new dependency, no new wire method, no write.
- Coordination: `DEPENDENCIES.md` rule 1. `add-chat-members-and-creation` (unit CM-G: `backend.rs`, `model.rs`, chat-core `client.rs`, `store.rs`, `yandex_backend.rs`, `desktop.rs`, `tests.rs`) and the open `add-reaction-list` hold the same files; tasks here that need them start after those merge (see `tasks.md`).
- Risk: every wire shape except `Plain.IsImportant`'s key and `Pin`'s mapper is inferred from reverse-engineering notes and the web bundle; **no sample of a pinned chat, a pin push or an important row exists locally**. Tasks 0.1 and 0.2 collect them; code tasks that depend on the unobserved parts are marked blocked. Privacy: the pinned text is message content; it goes into the existing cache (same protection as messages) and is never logged (`diagnostic-log` rules).

## Non-goals

- Pinning, unpinning, marking or unmarking importance, the composer's "important" toggle, a menu entry, an unpin control on the bar: the later write slice (`Pin { ChatId, Timestamp? }`, `UpdateFields { ChatId, Timestamp, ImportanceFlag: 1|2 }`; `research.md` records both forms and the role rights `pin_message`, `mark_as_important` for it).
- Several pins at once, a pin list or a "pinned messages" screen (Q1 recommends one).
- Navigation to a pinned message that is not loaded ("load around a timestamp"), shared with search-result jumping (Q2).
- "Who pinned it", pin timestamps, system messages about pins.
- The web client's important-message features: a search filter, a chat-info tab, mute bypass for notifications.
- Fixing the chat-pin gap (`yandex_backend.rs` always sets `pinned_rank: None`), and anything about the chat-level pin bucket.
- Mentions, forwarding, threads (later slices of roadmap 3).

## Open questions

Nothing below was decided without the user; the specs and design are written for the **recommended** option of each question, and the tasks that depend on a question are marked blocked until it is answered. Answers to Q4 and Q5 come from observation (tasks 0.1, 0.2), not from taste.

**Q1. One pinned message or several?** The web client shows `pinned_messages[0]`; `Pin` carries one timestamp; `PinnedMessageTimestamps` is an array, so several may exist (never observed). Options:
- A. One: the timestamp of `PinnedMessageInfo`, else the first of the array; a `Pin` push replaces it. Simple type (`Option`), matches the web bar. Cost: a second pin, if the server allows it, is invisible.
- B. All, as a list in the bar (an "n of N" switcher). Cost: a `Vec` through every layer, a UI that the web client does not have, a preview fetch per pin.
- C. One plus a "+N" count when the array has more. Cost: half a list's plumbing for a number nobody can act on in a display-only slice.
- **Recommended A**, revisit if 0.1 finds chats with several timestamps. Blocks 1.2, 2.2, 2.3, 3.2, 4.1, 4.3, 5.2.

**Q2. What does a click on the bar do when the pinned message is not loaded?** There is no jump-to-message in the engine (search results open the chat only). Options:
- A. Nothing for a message that is not loaded; a loaded one scrolls into view. No new engine work. The bar is mostly informative for old pins.
- B. Load history until the message is reached. Unbounded reads, a new "load around" engine path, a gap risk in the cache.
- C. Open the full pinned text in a small popup (no navigation). Works for any pin, but is a new surface.
- **Recommended A**; a shared jump-to-message (also for search results) is its own change. Blocks 4.3 (the click part only).

**Q3. How is the important marker drawn?** Options:
- A. A small glyph ("⚡") next to the time in the bubble footer, plain text like the existing "🕓" pending mark. No graphics, no licence question; the web client uses a lightning icon for important messages.
- B. A tinted bubble or an accent edge. More visible, changes the bubble look and collides with the own/incoming colours.
- C. Both. More noise.
- **Recommended A.** Blocks 4.2.

**Q4. Does an importance change look like an edit?** Expected: a `Plain` push with a higher `Version` and a changed `IsImportant` (web-client inference). Unknown: whether `LastEditTimestamp` is set then, in which case the existing rule would show the "edited" marker for a pure flag change, and whether the text is repeated. Options once observed:
- A. Take the server's `edited` as is. Right if the server does not set it.
- B. When the new copy has the same body and a different flag, keep the cached copy's `edited`. Right if the server sets it; a small, local rule in the engine upsert.
- C. Treat a flag change as a separate event if it arrives as one (it must not, per the web code).
- **Recommended: decide after 0.2**; if 0.2 shows `LastEditTimestamp` unchanged, A; if set, B. Blocks 2.4 and the "importance changes apply live" scenarios' implementation check (the requirement text holds in both cases).

**Q5. Which source wins when a pin push and a chat-list refresh disagree?** The web client orders by `PinnedMessageInfo.LastActionTsMcs`; the push body carries no such field in the notes. Options:
- A. Last arrival wins. Simple; a slow chat-list reply that started before an unpin can bring the old pin back until the next refresh.
- B. Order by an action timestamp: the entry's `LastActionTsMcs` against the push's `ServerMessageInfo.Timestamp`, if 0.2 shows they are comparable; otherwise A. Correct under races; depends on an unobserved equivalence.
- C. Never overwrite a live-set pin from the chat list while connected, and refresh only on reconnect. Misses a pin made by another client while this one lagged.
- **Recommended B with A as the fallback**, decided after 0.2. Blocks 2.3.

**Q6. Editing an own important message.** The adapter's own comment says "the server replaces the whole message" on edit; the web client sends `IsImportant` again with every edit (`convertMessageToPlain`, offset 1208286). `Edit` in `write.rs` sends no `IsImportant`, so editing an important own message from this client probably **clears the flag for everyone, today already** (invisible until this change shows the marker). Options:
- A. Leave it, document it, and verify when the write slice arrives. No write change; known silent data loss until then.
- B. Send `IsImportant: true` in the edit of a message the cache holds as important (and nothing otherwise, so non-important edits stay byte-identical). Matches the web client's own form; touches the write encoder (a field, not a new variant) and `Backend::edit`; verified live on a disposable self-chat message the user marks as important in the web client first.
- C. Refuse to edit an important own message ("Нельзя изменить важное сообщение"). Safe, costs a feature.
- **Recommended B**, after a live check that omission really clears the flag (task 6.2, self chat, user approval). If the check shows omission keeps it, no change is needed (A). Not a task of the display work: task 3.3 is blocked by Q6.

## Decisions

Settled by evidence or by the existing architecture; no user input needed.

1. **Two capabilities, one change.** The research recommended separate capabilities for pins and importance even when they ship together. They share no code but the decoder and the cache habit. Why: each gets its own spec, so the later write slice adds to each without touching the other.
2. **The pin belongs to the chat, not to the message.** No message-level pin flag exists on the wire, so `Message` is unchanged for pins; `ChatSummary` gets `pinned_message` (a timestamp and the last shown preview), named apart from `pinned_rank` so the two pins cannot be mixed up. Why: chat state already has a live event, a cache row (JSON, no migration) and a per-chat UI hook.
3. **The pinned message's text is not written into the message cache.** The cache is a flat `(chat, ts)` table whose `history_start` and `replace_range` define what is history; a stored old message would read as history with a gap. The text lives in the chat's `PinnedMessage.preview`. Why: "the cache never shows what the server does not report" (`offline-cache`).
4. **No new read request for the text.** `ReadRequest::History { before: Some(ts + 1), limit: 1 }` is what the web client's `requestMessage` sends and is already encodable. `message_info` stays out of the allowlist. A dedicated chat-info read is added only if 0.1 shows that chat-list entries do not carry the pin (task 1.3).
5. **The preview is resolved lazily, for the open chat only.** One extra read per opened chat with an unresolved pin, none during chat-list refresh. Why: the chat list is one request today; resolving every pinned chat would add one per chat on every refresh and on every `ChatChanged`.
6. **The `Pin` push gets its own event.** It is a snapshot of state, like `Reaction` is, and the web client applies it directly. Leaving it in `ChatChanged` would need a full chat-list refetch per pin and would depend on the unobserved chat-list shape.
7. **The importance flag follows the newest copy of a message**, the way text does; it needs no version of its own. Local pending messages are not important.
8. **No write, no notification, no filter.** See Non-goals.
