# Design

## Context

See `proposal.md` for motivation and the decisions that closed its open questions (numbered there as decisions 1-8; this file cites them as P1-P8). `research.md` holds the protocol facts.

Current code that this change extends:

- `yandex-proto::wire::decode_push` turns `Plain`/`Ephemeral`/`SystemMessage` into `MessageUpserted`/`MessageDeleted`, `Typing` into `Typing`, and sends everything else, including `Reaction`, to `PushEvent::Ignored`. The `Reaction` push in `tests/fixtures/live-push-reaction.json` carries `ClientMessage.Reaction {Action, ChatId, Timestamp, Type}` plus the siblings `Reactions [{Count, Type}]`, `ReactionsVersion`, `RecentUserReactions [{Timestamp, Type, UserInfo}]`, `MessageAuthorGuid` and `ServerMessageInfo.From` (the actor, as for `Typing`).
- `yandex-proto::write::WriteRequest` is a closed enum (`SendText`, `Edit`, `Delete`, `Seen`, `Typing`); `CLIENT_MESSAGE_VARIANTS` lists the three `ClientMessage` keys it can build and a test pins the set.
- `chat-core::Message` is stored in the cache as one JSON string per message (`messages.json`); `ClientEvent::MessageUpserted` is the UI's only per-message update and `ChatStore::upsert` clears the author's typing indicator for it. Edits and deletions are "actions": attempted once, never queued, offline refused with `OFFLINE_REASON` (`client.rs`: `run_action`, `handle_action`).
- `desktop::yandex_backend::YandexBackend` implements `Backend`; every write goes through its `push`, which applies `BT_WRITE_ONLY_CHAT`.
- The message context menu is `desktop/composer.rs` (`MenuAction`, `menu_actions`, `message_menu_view`); chips will live in `timeline.rs::bubble`.

## Goals / Non-Goals

**Goals:**
- Reactions are displayed, live-updated, cached, and writable by the user, with the write set still closed.
- Independent tasks: the contracts below are fixed here so protocol, core, adapter and UI work can proceed in parallel.

**Non-Goals:**
- Anything in the proposal's non-goals.
- Recovering reaction changes missed while offline beyond what history rows carry (see Risks).
- Re-applying an unconfirmed overlay onto a history chunk or edit that arrives while the request is in flight.

## Decisions

### D1. Wire model and push decoding (`yandex-proto`)

```rust
pub struct WireReactionCount { pub code: i64, pub count: u32 }          // Reactions[].{Type, Count}
pub struct WireReactor { pub user: Option<String>, pub code: i64,       // RecentUserReactions[].{UserInfo.Guid, Type, Timestamp}
                         pub timestamp: Option<Micros> }
pub struct WireReactions { pub counts: Vec<WireReactionCount>, pub version: Option<i64>, pub recent: Vec<WireReactor> }
pub enum ReactionAction { Remove, Replace, Other(i64) }                 // Action 1, 2, anything else
WireMessage { .., pub reactions: Option<WireReactions> }                // Some when any of the three siblings is present
PushEvent::Reaction { chat: String, timestamp: Micros, actor: Option<String>,
                      action: Option<ReactionAction>, code: Option<i64>, reactions: WireReactions }
```

`WireReactions` is read from the siblings of `ClientMessage` on any `ServerMessage` (push or history row, P4). Entries without `Type` or with `Count` 0 are dropped; server order is kept. `decode_push` returns `PushEvent::Reaction` when `ClientMessage.Reaction` has a `ChatId` and a `Timestamp` (the target message; `ServerMessageInfo.Timestamp` is the fallback), `actor` is `ServerMessageInfo.From.Guid`, `action` and `code` come from the `Reaction` body. A missing `Reactions` key means no reactions (an empty list is probably omitted by the server's JSON encoder), so removing the last reaction still decodes. A `Reaction` without address stays `Ignored { kind: "Reaction" }`. `xiva.rs::push_kind` gets the new variant.

Alternative rejected: reuse `MessageUpserted` with a `Plain`-less message. The push has no body; it would fabricate a message.

### D2. Closed write: one new variant (`yandex-proto`)

```rust
pub enum ReactionChange { Set(i64), Remove }
WriteRequest::Reaction { chat: String, timestamp: i64, change: ReactionChange }   // kind() == "reaction"
CLIENT_MESSAGE_VARIANTS = ["Plain", "SeenMarker", "Typing", "Reaction"]
```

Encoding, from the notes (section 9.3 and 17.12): `Set(code)` is `{"Reaction": {"ChatId", "Timestamp", "Type": <int>, "Action": 2}}` (`REPLACE`), `Remove` is `{"Reaction": {"ChatId", "Timestamp", "Action": 1}}` (`REMOVE`) without `Type`, which the server demands only for add and replace. `Type` is a JSON integer; strings fail on the server. `Remove` without `Type` is unverified; if task 7.2 sees a refusal, `Remove` carries the current code instead (the engine knows it) and this paragraph changes. `ADD` is not used: `REPLACE` with no existing reaction is the add (checked in 7.2). The `every_request` exhaustive `match` and the variant-set test in `write.rs` change with the enum; the read allowlist (`ALLOWED_METHODS`) is untouched, so `list_reactions` and `message_info` stay unreachable.

### D3. Domain types (`chat-core::reactions`, new module)

```rust
pub struct ReactionCode(pub i64);                                 // Copy, Eq, Hash, serde
pub struct ReactionCount { pub code: ReactionCode, pub count: u32 }
pub struct Reactions { pub counts: Vec<ReactionCount>, pub mine: Option<ReactionCode>, pub version: Option<i64> }  // Default, serde
pub struct ReactionsUpdate { pub counts: Vec<ReactionCount>, pub version: Option<i64>,
                             pub mine: Option<Option<ReactionCode>> }   // outer None: the event does not say
pub struct KnownReaction { pub code: ReactionCode, pub emoji: &'static str }
pub const KNOWN: &[KnownReaction];                                // filled by task 3.3 from task 1.1
pub const NEUTRAL_GLYPH: &str = "❔";
pub fn emoji(code: ReactionCode) -> Option<&'static str>;
```

Pure rules, unit-tested in the module:

- `Reactions::apply(&mut self, &ReactionsUpdate) -> bool`: ignored when both versions exist and `update.version <= self.version`; otherwise `counts` are replaced, `version` becomes the update's (kept when the update has none), `mine` becomes the update's when it says one and stays otherwise; then `mine` is cleared if its code is absent from `counts`. Returns whether anything changed. This is P6 and the "Stale reaction updates" requirement.
- `Reactions::toggled(&self, code) -> (Reactions, Option<ReactionCode>)`: the target is `None` when `mine == Some(code)`, else `Some(code)`; the returned overlay lowers the count of the old `mine` (dropping it at 0), raises or appends the count of the target, sets `mine`, and keeps `version`. This is "One own reaction per message".
- `Message::inherit_reactions(&mut self, cached: &Message)`: copies `cached.reactions` when `self.reactions` has no counts and no version (the copy carries no reaction state: an edit push, a history row without reactions).

`Message` gains `#[serde(default)] pub reactions: Reactions`. `Command::ToggleReaction { chat, timestamp, code }` is the UI's intent; the engine, which knows the cached `mine`, decides add, replace or remove. `ClientEvent::ReactionsChanged { chat, timestamp, reactions: Reactions }` is a new event on purpose: `MessageUpserted` would clear the author's typing indicator in the store and re-run media and preview side effects, and a reaction update has no message body. `LiveEvent::ReactionsChanged { chat, timestamp, update: ReactionsUpdate }` and `Backend::react(&self, chat, timestamp, target: Option<ReactionCode>) -> Result<(), BackendError>` (`None` removes) complete the seam.

### D4. Cache: no SQL migration

Messages are stored as JSON, and `Reactions` is `serde(default)`, so old rows load without reactions and `PRAGMA user_version` stays at 2. The proposal's "schema and migration" reduces to a test that a row written without the field loads (offline-cache requirement "Cache from an older version"). Only server-confirmed state is written (D5), so an unconfirmed reaction cannot survive a restart.

### D5. Engine behaviour (`chat-core::client`)

Reactions are an action in the sense of `run_action`: attempted once, never queued (a replay could undo a later change by the same user), refused offline. The cache holds server-confirmed state; the UI shows that state plus an overlay.

`ToggleReaction { chat, timestamp, code }`:
1. The cached message must exist, be `Delivery::Sent` and not `MessageBody::Deleted`; otherwise `ActionFailed` with a new reason constant.
2. Offline: `ActionFailed` with `OFFLINE_REASON`, no overlay.
3. An entry for `(chat, timestamp)` in `reaction_inflight` means a request is unconfirmed: ignore the command ("Second choice while one is unconfirmed").
4. `(overlay, target) = cached.reactions.toggled(code)`; insert `reaction_inflight[(chat, timestamp)] = { superseded: false }`; emit `ClientEvent::ReactionsChanged` with the overlay; spawn `backend.react(chat, timestamp, target)`; the result returns as `Input::Action` with a new `Action::React { timestamp, overlay }`.

On the result: `Ok`, entry not superseded: write the overlay to the cache (the version stays, so any later push with a newer version overwrites it) and emit nothing. `Ok`, superseded: nothing. `Err`, entry not superseded: re-emit `ReactionsChanged` with the cached state (rollback) and `ActionFailed` with the reason mapped as for edits. `Err`, superseded: only `ActionFailed`, because the pushed state already is the truth. `SessionExpired` expires the session as elsewhere. The entry is removed in every case.

`LiveEvent::ReactionsChanged`: always recorded in `live_log` as `LiveChange::Reactions(timestamp, update)` so that a history chunk that was requested before the event does not undo it (`replay_live` applies it with the same `apply`). If the message is cached, `apply` runs on its reactions; when that changed anything the message is written back, `ClientEvent::ReactionsChanged` is emitted, and a `reaction_inflight` entry for it is marked `superseded` (P6: the server's state replaces the overlay). An ignored stale update touches nothing. Neither `update_preview` nor `record_live(Upsert)` runs.

`inherit_reactions` runs in `LiveEvent::MessageUpserted` (against the cached copy) and in `store_chunk` (against the cached rows of the replaced range, read once before `replace_range`), so edits and reloaded history keep known reactions (P4).

Rejected: writing the overlay into the cache at once (a restart during the request would show a reaction the server never saw); queuing reactions in the outbox (ordering and replay hazards); a Command per action (add, replace, remove), which would put the cached-state decision in the UI.

### D6. Reaction types (P1, P2)

`reactions::KNOWN` is the only source of emoji and of the picker's contents, in picker order. Task 1.1 produces the table with evidence; task 3.3 transcribes it. Unknown codes render `NEUTRAL_GLYPH` and the count and remain clickable (their `code` is genuine). An empty `KNOWN` hides the picker only. Emoji are drawn as text (P2).

### D7. Adapter (`desktop::yandex_backend`)

- `message()` maps `WireMessage.reactions` into `Message.reactions`: counts and version one to one; `mine` is the code of the newest `recent` entry whose `user` is the account.
- `PushEvent::Reaction` becomes `LiveEvent::ReactionsChanged`. `update.mine` is `Some(None)` when `actor` is the account and the action is `Remove`; `Some(Some(code))` when the actor is the account and the action is `Replace` with a `code`; otherwise the newest `recent` entry of the account if there is one; otherwise `None` (unknown: `RecentUserReactions` is truncated, P8, so absence proves nothing).
- `Backend::react` builds `WriteRequest::Reaction` and calls `push`, so `BT_WRITE_ONLY_CHAT` and the one-line write log apply unchanged.

### D8. UI (`desktop`)

- `store.rs`: `ClientEvent::ReactionsChanged` sets the message's `reactions` in place and emits `TimelineChange::Delta { touched: [i] }` only when they differ; it does not touch typing state. `ChatStore::toggle_reaction(chat, timestamp, code)` sends `Command::ToggleReaction`.
- `timeline.rs`: below the body (and the failure line), inside the bubble: a wrapping row of chips for `m.reactions.counts` when the message is `Sent` and not `Deleted`. A chip is a rounded pill with `emoji(code)` or `NEUTRAL_GLYPH` and the count; the own chip uses a highlighted background from `theme.rs`. Debug selectors: `msg-{ts}-reaction-{code}`. A left click calls `Desktop::toggle_reaction`.
- `desktop/composer.rs`: `Desktop::toggle_reaction(chat, timestamp, code, cx)` ignores the call while the session is blocked or the chat is not open, then calls the store. `message_menu_view` draws a row of `KNOWN` emoji above the other entries for a sent, non-deleted message (selectors `menu-reaction-{code}`, own type marked); choosing one closes the menu and toggles. `open_menu` must open the menu when the reaction row is the only content, so its emptiness check includes the row. A refused reaction arrives as `ActionFailed` and reuses the existing error toast.
- Layout follows Telegram Desktop's reaction chips and menu strip; no Telegram graphics are used (P2).

### D9. Verification and safety (P5)

Unit and fixture tests carry everything except server behaviour. Server behaviour (REPLACE with no prior reaction, replace semantics, `Remove` without `Type`, whether the sender receives its own `Reaction` push, whether history rows carry `Reactions`) is checked once by task 7.2 in the self chat through `bt-probe`. No reaction is sent until the user has approved the target message (P5); the proposed target is a disposable message created by the check itself, which is an extra safeguard on top of the approval. The findings are recorded in `research.md`. A finding that contradicts D2, D3 or P3 reopens the affected artifact before archive.

### D10. Ordering with other changes

`add-sending` is finished and synced, so the `message-sending` delta applies to the main spec directly. Other active changes may edit `timeline.rs`, `store.rs`, `yandex_backend.rs`, `client.rs` and `composer.rs`; each task here edits only the places it names and rebases onto whatever landed first.

## Risks / Trade-offs

- **Unverified write form.** `Set` and `Remove` come from reverse-engineering notes; one live-checked integer `Type` (`1`) is the only confirmation. Mitigation: self-chat-only verification (P5), closed write set, `BT_WRITE_ONLY_CHAT`.
- **Missing type table.** Without confirmed (code, emoji) pairs the picker is hidden (P1); chips still work.
- **Missed reactions while offline.** After a reconnect the engine reloads the open chat's newest chunk; reaction changes missed meanwhile appear only if history rows carry `Reactions`. Task 7.2 records whether they do; if not, this is a known limit of the slice.
- **Own highlight is best effort.** `mine` comes from the actor field or `RecentUserReactions`, which the server truncates. A history row whose recent list omits the user shows no highlight.
- **Overlay flicker.** A history chunk or edit emitted while a reaction request is in flight shows the cached state until the request finishes (Non-Goals).
- **Reactions on odd messages.** System messages map to text in the domain and cannot be told from user text, so the menu offers reactions on them; the server may refuse, which rolls back and shows the reason.
- **Version semantics assumed.** `ReactionsVersion` is treated as monotonic per message; an update without a version is always applied, so a stale versionless update could win. The fixture and notes show versioned pushes only.
