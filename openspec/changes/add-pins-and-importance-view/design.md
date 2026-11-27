# Design

## Context

See `proposal.md` (motivation, decisions 1-8, answered questions Q1, Q2, Q3, Q5, Q6; open question Q4) and `research.md` (protocol evidence, offsets into the web bundle). Contracts below are fixed so that protocol, core, adapter and UI work can proceed in parallel; the one contract that still depends on an open question is marked `[Q4]`; if its answer differs, only that contract and task 2.4 change.

Current code this change extends:

- `yandex-proto::wire`: `parse_server_message` builds `WireMessage` for history rows and pushes; `PLAIN_META_KEYS` lists `IsImportant` as "not content" and drops it. `parse_chat` builds `WireChat` from a chat entry and keeps the entry without `Messages` in `raw`. `decode_push` folds `Pin` into `ChatChanged { chat }` (with `SeenMarker`, `ClearUserHistory`, `StateSync`, `ChatApproval`).
- `desktop::yandex_backend`: the live loop answers `ChatChanged` with `client.chat_list()` and emits `LiveEvent::ChatChanged(summary)`; `summary()` maps `WireChat` to `ChatSummary` (and sets `pinned_rank: None`); `preview(&MessageBody)` makes the one-line text and placeholders; `message()` maps `WireMessage` to `Message`.
- `chat-core`: `ChatSummary` and `Message` are cached as JSON (`chats.json`, `messages.json`); `LiveEvent::ChatChanged` writes the summary and emits `ChatUpdated`; `MessagePreview { author, text, timestamp }` exists. The cache's `messages` table is flat and `chat_state.history_start` plus `replace_range` define what counts as history.
- `desktop`: the chat header is `chat-header` in `desktop.rs`; the bubble is `timeline.rs::bubble`; the store applies per-message changes with `TimelineChange::Delta`.

## Goals / Non-Goals

**Goals:** the pinned message and the importance flag are displayed, live, cached; the cache never shows a gap as history; no write is added.

**Non-Goals:** everything in the proposal's Non-goals.

## Decisions

### D1. Wire model and decoding (`yandex-proto`)

```rust
WireMessage { .., pub important: bool }          // Plain|Ephemeral .IsImportant == true; missing, false or non-bool is false
WireChat { .., pub pinned: Option<Micros>, pub pin_action_ts: Option<Micros> }  // one pin (Q1); the action timestamp is a separate field so that an unpinned entry still carries it (Q5)
PushEvent::PinChanged { chat: String, pinned: Option<Micros>, at: Option<Micros> }  // one pin (Q1); `at` for arbitration (Q5)
```

- `important` is read in `parse_server_message` from the `Plain`/`Ephemeral` object; `IsImportant` stays in `PLAIN_META_KEYS` (it is still not content). A deleted message and a non-`Plain` kind give `false`.
- `WireChat.pinned` / `WireChat.pin_action_ts` (`parse_chat`): `pinned` = `PinnedMessageInfo.PinnedMessageTs` if present and positive, else the first positive entry of `ChatInfo.PinnedMessageTimestamps` (the web client's `eW`), else `None`. `pin_action_ts` = `PinnedMessageInfo.LastActionTsMcs` whenever `PinnedMessageInfo` is present and the value is positive, **also when `pinned` is `None`** (an unpin snapshot keeps its action time), else `None`. Values go through the same `micros` reader as every timestamp (ints, integral floats, digit strings). A zero or negative value is "no pin" / "no action timestamp".
- `PushEvent::PinChanged`: `decode_push` handles `ClientMessage.Pin` with a `ChatId` before the `chat_changing` list. `pinned = Pin.Timestamp` when positive, else `None` (the web client's `ek`: a missing `Timestamp` means nothing is pinned). `at = ServerMessageInfo.Timestamp` when present (Q5); if task 0.2 shows that it does not order against a later entry's `LastActionTsMcs`, `at` is set to `None` here and arrival order applies to pushes. A `Pin` without `ChatId` falls to `Ignored { kind: "Pin" }`. `Pin` is removed from the `chat_changing` array. `xiva.rs::push_kind` gets the arm (`"PinChanged"`); the debug line logs the kind only, as for the other kinds (no chat id, no timestamp).
- Several pins (Q1 = A) are not represented: `PinnedMessageInfo` is the server's current (latest) pin and wins over the array; the array's first entry is the fallback. A `Vec` would be a change of this file and of the contracts below, and is out of scope.

Alternative rejected: keep `Pin` in `ChatChanged` and read the pin from the refetched list. It costs a full chat-list request per pin and depends on whether list entries carry `PinnedMessageInfo` (unobserved; task 0.1). The decoder reads the list entry anyway (for the initial state), but the live path must not depend on it.

### D2. Optional read: chat info (`yandex-proto`, task 1.3, only if 0.1 says so)

If chat-list entries (`history { Limit: 1, ChatDataFilter: {} }`) do not carry the pin but a single-chat read does, one variant is added: `ReadRequest::ChatInfo { chat }`, socket `history { ChatId, Limit: 0, ChatDataFilter: {} }` (the web client's `getChat`), parsed by the existing `parse_chat`. `ALLOWED_METHODS` does not change (method `history`); the `every_request` test and its exhaustive `match` gain the variant. The adapter calls it for the open chat. If list entries do carry the pin, this task is dropped and no read request is added.

**Outcome (task 0.1, 2026-11-27):** list entries carry `PinnedMessageInfo` (and so do single-chat replies), so this task is dropped and no read request is added; research "Live observation".

### D3. Domain types (`chat-core`)

```rust
pub struct PinnedMessage { pub timestamp: MessageTs, pub preview: Option<MessagePreview> }  // Clone, PartialEq, serde
ChatSummary { .., #[serde(default)] pub pinned_message: Option<PinnedMessage>,                // not pinned_rank
               #[serde(default)] pub pin_action_ts: Option<i64> }                              // latest pin action applied (Q5); separate from pinned_message so an unpin keeps it
Message { .., #[serde(default)] pub important: bool }
LiveEvent::PinChanged { chat: ChatId, pinned: Option<MessageTs>, at: Option<i64> }
ClientEvent::PinChanged { chat: ChatId, pinned: Option<PinnedMessage> }
Backend::message_preview(&self, chat: &ChatId, timestamp: MessageTs)
    -> impl Future<Output = Result<Option<MessagePreview>, BackendError>> + Send
```

- `ChatSummary.pinned_message` is the only place a pin lives; `pinned_rank` and everything about the chat pin are untouched. `ChatSummary.pin_action_ts` is the arbitration clock of Q5: the latest action timestamp applied for the chat's pin, from a chat-list entry (`WireChat.pin_action_ts`) or a push (`at`), kept when the pin is removed and never decreased. It travels in the domain chat-list result (`Backend::chat_list` returns `ChatSummary`) so the engine can compare it, and is cached with the summary (JSON). Rows cached before the fields existed load with `None` (JSON `serde(default)`; no SQL migration, `cache.rs` gets a legacy-row test only).
- `Message.important` defaults to `false` for old rows and for pending local messages (`outgoing_message`).
- `Backend::message_preview` returns the preview of one message (`Ok(None)` when the server no longer has it); offline fails fast with `Network`. The adapter implements it with `ReadRequest::History { before: Some(ts + 1), limit: 1 }` (decision 4) and the shared preview function (D4).
- `ClientEvent::PinChanged` is separate from `ChatUpdated` on purpose: the store applies it without touching the row (spec: a pin change must not change a row's preview, unread or position), and tests can assert that nothing else is emitted. `ChatUpdated` still carries the summary, including `pinned_message`, from chat-list refreshes.

### D4. The one-line text is shared

`yandex_backend.rs::preview(&MessageBody) -> String` moves to `chat-core` as `MessageBody::preview_text(&self) -> String` (same strings), because the engine now builds a preview when a pinned message is edited, deleted or found in the cache (D5), and the adapter keeps calling it. The text of a preview is never logged.

### D5. Engine rules (`chat-core::client`)

1. **Pin push.** `LiveEvent::PinChanged { chat, pinned, at }`: read the cached summary of `chat`; none -> ignore (the next chat-list load brings the chat and its pin). Arbitration first (Q5 = B): when `at` and the cached `pin_action_ts` are both present and `at` is older, ignore the push entirely (no write, no event); otherwise it applies, and when `at` is present `pin_action_ts = max(cached, at)` (with no `at`, the last arrival wins and the cached value stays). New `pinned_message`: `None` if `pinned` is `None` (the summary keeps its `pin_action_ts`); the existing value (preview kept) if the timestamp is the same; else `PinnedMessage { timestamp, preview: None }`. Write the summary to the cache, emit `ClientEvent::PinChanged`, then resolve the preview (rule 3). No `ChatUpdated`, no preview/unread change, no typing effect.
2. **Chat-list refresh.** Summaries from `chat_list()` carry `pinned_message { timestamp, preview: None }` (or `None`) and `pin_action_ts` (also for an entry without a pin). The same arbitration applies per chat: when both the incoming and the cached `pin_action_ts` are present and the incoming one is older, the cached `pinned_message` and `pin_action_ts` are kept and the rest of the incoming summary applies (a stale list reply cannot bring back an unpinned or replaced message). Otherwise the incoming pin wins: the cached summary's resolved preview is kept when the pinned timestamp is the same, a different timestamp or no pin replaces it, and `pin_action_ts = max(cached, incoming)` (an incoming `None` keeps the cached value). A pin that disappears from a non-stale list disappears from the cache (spec "Unpinned while offline").
3. **Resolving the preview**, for the open chat only, when its pin has no preview: (a) the cached message `cache.message(chat, ts)` -> `MessagePreview` through `preview_text` and the author as the chat row does it (`"Вы"` for outgoing, else the author name); (b) else, online, one `Backend::message_preview` request, at most one in flight per `(chat, ts)`; (c) the result is applied only if the chat's pin still has that timestamp, then cached and emitted as `ClientEvent::PinChanged`; (d) offline, or a failure, leaves the preview empty without an error event; the next opening tries again. The resolved preview is part of the cached summary (`chats.json`), so it shows offline.
4. **Follow-through.** On `LiveEvent::MessageUpserted` and `MessageDeleted` for a message whose timestamp equals the chat's pinned timestamp, rebuild the preview from the message (a deletion gives the deleted placeholder), write it and emit `PinChanged`. A history chunk that contains the pinned message does the same when its copy differs.
5. **Not stored as history.** Rules 1-4 never call `upsert_message`, `replace_range` or `chat_state` for the pinned message; the pinned preview is only in the chat's JSON.
6. **Importance.** Nothing beyond the new field: `MessageUpserted` and history chunks replace the whole message, so `important` follows the newest copy; `inherit_reactions` does not touch it. `[Q4]` if the observation shows that a flag-only change sets `LastEditTimestamp`, the upsert path keeps the cached `edited` when the incoming body equals the cached one and only `important` differs.

### D6. Adapter (`desktop::yandex_backend`)

`message()` copies `wire.important`. `summary()` maps `WireChat.pinned` to `pinned_message` with `preview: None` and `WireChat.pin_action_ts` to `ChatSummary.pin_action_ts` (also when there is no pin); `pinned_rank` stays `None` and is not this change's business. The live loop maps `PushEvent::PinChanged` to `LiveEvent::PinChanged` (a small pure function, tested like the reaction mapping; `at` is passed through) and `ChatChanged` keeps its refetch for the remaining kinds. `message_preview` is the one-message `History` read plus `Users` loading for the author's name (the same helper as the other reads); it goes through the read path, never `push`.

### D7. UI (`desktop`)

- **Store:** `ChatStore` keeps the open chat's `pinned: Option<PinnedMessage>` from the summary and from `ClientEvent::PinChanged`; it emits a UI change only when the value differs and never touches the timeline, rows or typing state.
- **Bar:** one row between `chat-header` and the timeline, selector `pinned-message`; label "Закреплённое сообщение", then author (not for private chats) and the preview text, one line, truncated; no preview yet shows the label alone. No unpin control, no pin glyph shared with the chat-row marker (selector and wording keep the two apart). Click (Q2 = A): scrolls to the message when it is loaded, otherwise nothing: no request, no loading state, no error.
- **Marker (Q3 = A):** "⚡" as plain text next to the time in the bubble footer of a non-deleted message with `important`; selector `msg-{ts}-important`; a new theme colour for the glyph. It does not alter the bubble's width logic beyond its own footprint in the footer.

### D8. `bt-probe`

`history` appends a flag marker to rows with `important` (as it prints reaction state), `chats` prints the pinned timestamp of a chat that has one; `listen` already prints pushes through `Debug`, so `PinChanged` shows up. No new command, no write.

### D9. The edit of an own important message (Q6 = B)

`WriteRequest::Edit` gains `important: bool`; `client_message` adds `"IsImportant": true` to the `Plain` only when it is true (non-important edits stay byte-identical, and the existing frame tests keep passing); `Backend::edit` gains the flag, the engine passes the cached message's value. No new `WriteRequest` variant, no change to `CLIENT_MESSAGE_VARIANTS` or `ALLOWED_METHODS`. `bt-probe edit-self` gets `--important` (off by default, so the default frame is today's) so the live check can send both forms; the write guard (`BT_WRITE_ONLY_CHAT`) applies as to every probe write. If task 6.2 shows that omission already keeps the flag, this decision is reverted (Q6 becomes A).

## Risks / Trade-offs

- **Everything unobserved is inferred.** The pin keys come from the web client's decoder, not from a captured reply; the importance-change path is an inference from one reducer. Mitigation: the decoder is tolerant (missing keys mean "none"), fixtures are marked synthetic until 0.1/0.2 replace them, and the questions that depend on observation are blocked, not guessed.
- **A stale pin after a race** is ruled out by the arbitration of D5.1 when the timestamps are comparable (Q5 = B); without comparable timestamps a stale chat-list reply can bring an old pin back until the next refresh.
- **Preview freshness.** A pinned preview resolved once is refreshed by edits and deletions the client sees live; an edit made while the client was offline shows at the next resolve only if the preview is dropped on reconnect. Decision: on `Reconnected` the open chat's preview is re-resolved (one read), other chats' stay as cached until opened.
- **Privacy.** The pinned text sits in `chats.json` next to the last-message preview that is already there; no new location, no logging of text.
- **Sharing files** with `add-chat-members-and-creation` and `add-reaction-list` (Impact in the proposal): the contracts above add fields and one trait method; Rule 2 of `DEPENDENCIES.md` (merges compile with no stubs) is why 2.2 and 3.2 and 4.1/4.3 land as one unit.
