# Proposal

## Why

The client shows reactions now, but two other pieces of message state set by other people are invisible: which message of a chat is **pinned** (the web client keeps it in a bar above the timeline) and which messages are marked **important** (a per-message flag). Both are first-slice items of roadmap 3a ("message pins and importance: first incoming data and display, then explicit actions", `add-message-reactions/research.md`, "Recommended scope split"). This change is the read side only: a user who opens a chat sees what others pinned and flagged, live and offline; the client does not pin, unpin or flag anything.

`research.md` (next to this file) is the single evidence document: where the pin and the flag come from in the protocol, how a change arrives, what the web client does, what is still unobserved.

## What Changes

- Decode the pin: the chat entry's `PinnedMessageInfo` / `ChatInfo.PinnedMessageTimestamps` and the `Pin` push (today a `Pin` push is folded into `PushEvent::ChatChanged` and nothing reads the pin). The push is a snapshot (a timestamp, or none), not a toggle.
- Show a **pinned-message bar** under the chat header of the open chat: author, one line of the pinned text or the usual placeholder; live from `Pin` pushes; edits and deletions of the pinned message follow; kept in the offline cache with the last shown text. The text comes from the loaded or cached copy, otherwise from one `history` read already in the closed read set (`MaxTimestamp = ts + 1`, `Limit = 1`); the pinned message is never written into the chat's cached history.
- Decode `Plain.IsImportant` (today dropped as meta) into a message flag and show an **important marker** in the bubble; the flag follows history rows, pushes and the offline cache. A change of importance made elsewhere is expected to arrive as an updated `Plain` (inferred from the web client, unobserved; see Q4).
- `bt-probe`: `history` prints the flag, `chats` prints the pinned timestamp, so the live reads in tasks 0.1 and 0.2 and later checks are possible without a raw trace.
- No new write is added: `Pin` and `UpdateFields` stay unrepresentable, no `WriteRequest` variant is added and `CLIENT_MESSAGE_VARIANTS` does not change. The one touch of the write path is the existing `Edit`: when the user edits an own message that is important, the edit re-sends `IsImportant: true`, because the server replaces the whole message on edit (Q6, decided B; non-important edits stay byte-identical). The client still never marks, unmarks, pins or unpins anything.

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
- Q6 (B): `yandex-proto/src/write.rs` (`WriteRequest::Edit.important`, key only when true), `bin/bt-probe.rs` (`edit-self --important`, for the live check), `Backend::edit` and the engine's edit path pass the cached flag (design D9).
- No new dependency, no new wire method, no new write variant.
- Coordination: `DEPENDENCIES.md` rule 1. The not-started tasks of `add-chat-members-and-creation` (CM 6.3, 6.4, 7.1, gated on approvals) hold no files; the open `add-reaction-list` holds `wire.rs`, `request.rs`, `bt-probe.rs` and the app files this change also edits (`backend.rs`, `model.rs`, chat-core `client.rs`, `store.rs`, `yandex_backend.rs`, `desktop.rs`, `tests.rs`). Order in `tasks.md`: this change's tasks on those files start after the reaction-list unit that holds them has merged.
- Risk: every wire shape except `Plain.IsImportant`'s key and `Pin`'s mapper is inferred from reverse-engineering notes and the web bundle; **no sample of a pinned chat, a pin push or an important row exists locally**. Tasks 0.1 and 0.2 collect them; code tasks that depend on the unobserved parts are marked blocked. Privacy: the pinned text is message content; it goes into the existing cache (same protection as messages) and is never logged (`diagnostic-log` rules).

## Non-goals

- Pinning, unpinning, marking or unmarking importance, the composer's "important" toggle, a menu entry, an unpin control on the bar: the later write slice (`Pin { ChatId, Timestamp? }`, `UpdateFields { ChatId, Timestamp, ImportanceFlag: 1|2 }`; `research.md` records both forms and the role rights `pin_message`, `mark_as_important` for it). Re-sending the existing flag in the edit of an own important message (Q6) is not marking.
- Several pins at once, a pin list or a "pinned messages" screen (Q1: one banner, the latest pin).
- Navigation to a pinned message that is not loaded ("load around a timestamp"), shared with search-result jumping (Q2: a click does nothing then; its own change).
- "Who pinned it", pin timestamps, system messages about pins.
- The web client's important-message features: a search filter, a chat-info tab, mute bypass for notifications.
- Fixing the chat-pin gap (`yandex_backend.rs` always sets `pinned_rank: None`), and anything about the chat-level pin bucket.
- Mentions, forwarding, threads (later slices of roadmap 3).

## Open questions

**Q4. Does an importance change look like an edit?** Expected: a `Plain` push with a higher `Version` and a changed `IsImportant` (web-client inference). Unknown: whether `LastEditTimestamp` is set then, in which case the existing rule would show the "edited" marker for a pure flag change, and whether the text is repeated. Options once observed:
- A. Take the server's `edited` as is. Right if the server does not set it.
- B. When the new copy has the same body and a different flag, keep the cached copy's `edited`. Right if the server sets it; a small, local rule in the engine upsert.
- C. Treat a flag change as a separate event if it arrives as one (it must not, per the web code).
- **Recommended: decide after 0.2**; if 0.2 shows `LastEditTimestamp` unchanged, A; if set, B. Blocks 2.4 and the "importance changes apply live" scenarios' implementation check (the requirement text holds in both cases). The only open question; it is answered by observation, not taste.

## Answered questions

Answered on 2026-11-26 in the main session (autonomous mode: the user delegates decisions). Q1 and Q2 were chosen by the user; Q3, Q5 and Q6 were settled in the same session. Specs, design and tasks are now written for these answers. Rejected options are kept short for the record.

**Q1 = A. One pinned message: the latest pin.** The bar shows one message: `PinnedMessageInfo.PinnedMessageTs` (the server's current pin), else the first entry of `PinnedMessageTimestamps`; a `Pin` push replaces it, so the most recently pinned message wins. `Option`, matching the web bar. Cost accepted: a second simultaneous pin, if the server allows it, is invisible. Rejected: B (all pins, an "n of N" switcher), C ("+N" count). Task 0.1 still records how many timestamps real chats carry and in what order; a surprise there reopens this, nothing else does. Lifts the block from 1.2, 2.2, 2.3, 3.2, 4.1, 4.3, 5.2.

**Q2 = A. A click on the bar does nothing when the pinned message is not loaded.** A loaded message scrolls into view; there is no jump-to-message in the engine and none is added; a shared jump (also for search results) is its own change. Rejected: B (load history until reached), C (popup with the full text). Lifts the block from 4.3.

**Q3 = A. The important marker is a "⚡" glyph next to the time** in the bubble footer, plain text like the existing "🕓" pending mark. Rejected: B (tinted bubble or accent edge), C (both). Lifts the block from 4.2.

**Q5 = B. Pin arbitration is by the action timestamp, else by arrival.** An input (a chat-list entry's `LastActionTsMcs`, a push's `ServerMessageInfo.Timestamp`) whose action timestamp is older than the latest one applied for that chat is ignored; when either side has none, the last arrival wins. The clock travels as its own field, `WireChat.pin_action_ts` and `ChatSummary.pin_action_ts`, separate from the optional pinned message so an unpin snapshot keeps it and the engine can compare a chat-list reply against it. The two timestamps being comparable is an assumption that task 0.2 checks; if 0.2 shows they are not, the decoder stops filling `at` for pushes (one line in `wire.rs`) and the arrival rule applies to pushes, with no other change. Rejected: A alone (stale pin after a race), C (no overwrite while connected). Lifts the block from 1.2 and 2.3.

**Q6 = B. Edit of an own important message sends `IsImportant: true`.** The web client does the same on every edit (`convertMessageToPlain`, offset 1208286); the adapter's own comment says the server replaces the whole message on edit, so omission probably clears the flag for everyone, already today. Only an edit of a message the cache holds as important carries the key; non-important edits stay byte-identical. Still verified live in 6.2 on a disposable self-chat message the user marks as important (an edit without the flag and one with it, `history` after each): if omission turns out to keep the flag, the change is removed again and Q6 becomes A. Rejected: A (leave the silent loss), C (refuse the edit). Lifts the block of 3.3 for implementation; it closes only with 6.2.

## Decisions

Settled by evidence or by the existing architecture; no user input needed.

1. **Two capabilities, one change.** The research recommended separate capabilities for pins and importance even when they ship together. They share no code but the decoder and the cache habit. Why: each gets its own spec, so the later write slice adds to each without touching the other.
2. **The pin belongs to the chat, not to the message.** No message-level pin flag exists on the wire, so `Message` is unchanged for pins; `ChatSummary` gets `pinned_message` (a timestamp and the last shown preview), named apart from `pinned_rank` so the two pins cannot be mixed up. Why: chat state already has a live event, a cache row (JSON, no migration) and a per-chat UI hook.
3. **The pinned message's text is not written into the message cache.** The cache is a flat `(chat, ts)` table whose `history_start` and `replace_range` define what is history; a stored old message would read as history with a gap. The text lives in the chat's `PinnedMessage.preview`. Why: "the cache never shows what the server does not report" (`offline-cache`).
4. **No new read request for the text.** `ReadRequest::History { before: Some(ts + 1), limit: 1 }` is what the web client's `requestMessage` sends and is already encodable. `message_info` stays out of the allowlist. A dedicated chat-info read is added only if 0.1 shows that chat-list entries do not carry the pin (task 1.3).
5. **The preview is resolved lazily, for the open chat only.** One extra read per opened chat with an unresolved pin, none during chat-list refresh. Why: the chat list is one request today; resolving every pinned chat would add one per chat on every refresh and on every `ChatChanged`.
6. **The `Pin` push gets its own event.** It is a snapshot of state, like `Reaction` is, and the web client applies it directly. Leaving it in `ChatChanged` would need a full chat-list refetch per pin and would depend on the unobserved chat-list shape.
7. **The importance flag follows the newest copy of a message**, the way text does; it needs no version of its own. Local pending messages are not important.
8. **No pin or importance write, no notification, no filter.** See Non-goals; the only write-path change is the Q6 edit flag.
