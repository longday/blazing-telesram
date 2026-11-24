# Proposal

## Why

The client can read, send, edit and delete, but reactions are invisible: a message someone reacted to looks the same as one nobody reacted to, and the user cannot react. Reactions are the first slice of roadmap item 3 (reactions, importance, pins, mentions, forwarding, threads). The research (`research.md`, next to this file) recommends splitting item 3 and starting with reactions: the wire shape is documented (`Reaction { ChatId, Timestamp, Type, Action }` outbound; `Reactions`, `ReactionsVersion`, `RecentUserReactions` inbound) and a live push fixture already exists (`crates/yandex-proto/tests/fixtures/live-push-reaction.json`).

`research.md` is the single context document for this change and for the later slices of item 3; read it before writing the design.

## What Changes

- Decode the reaction push into a domain event: per-message reaction counts, the version (`ReactionsVersion`) and the user's own reaction when the push or `RecentUserReactions` shows it. Today it falls into `WireBody::Other("Reaction")` and `PushEvent::Ignored`.
- Show reactions on messages in the timeline: a chip per reaction type with its count, the user's own reaction highlighted; update live from push and keep them in the offline cache.
- Let the user add, change and remove their reaction on a message: a new closed `WriteRequest` variant (`Reaction`) and `"Reaction"` in `CLIENT_MESSAGE_VARIANTS`; applied optimistically, rolled back on failure.
- Entry points: a reaction control in the message context menu and a click on an existing chip toggles the user's own reaction.
- **BREAKING** (to the spec, not to users): the sending spec's "Writes only from user actions" requirement forbids reactions; it is relaxed to allow reactions as a user action. Presence heartbeats and status changes stay forbidden. No generic `push` is added: the write set stays closed and grows by exactly one variant.
- `bt-probe`: a reaction command limited to the self chat for live verification.

## Capabilities

### New Capabilities
- `message-reactions`: reading, live-updating, caching and displaying message reactions; adding, changing and removing the user's own reaction.

### Modified Capabilities
- `message-editing`: the "Message context menu" requirement gains a reaction row; its "Other user's message" scenario no longer says Reply and Copy text only.
- `message-sending`: "Writes only from user actions" now includes reactions (still no presence heartbeats or status changes).
- `live-updates`: the reaction push event is handled instead of ignored.
- `offline-cache`: reactions and their version are cached with the message.

## Impact

- `crates/yandex-proto`: `write.rs` (`WriteRequest::Reaction`, `CLIENT_MESSAGE_VARIANTS`, encoding, tests); `wire.rs` (reaction wire model, `WireMessage.reactions`, `PushEvent::Reaction`, `decode_push`; the fixture test that expects `Ignored { kind: "Reaction" }` changes; the unit test that a bare `Reaction` body parses as `Other("Reaction")` stays); `xiva.rs` (`push_kind` match); `request.rs` (module docs say reactions are not representable); fixture-based tests in `tests/wire_fixtures.rs`.
- `crates/chat-core`: `Message` gains `reactions` (counts, own reaction, version); a command, backend method and live event for reactions; the engine applies versions, the optimistic overlay and rollback. The cache stores messages as JSON, so the new field is `serde(default)` and needs no SQL migration (design D4).
- `crates/desktop`: `yandex_backend.rs` (wire-to-domain mapping, live event, `Backend::react`), `store.rs` (apply reaction changes, toggle API), `timeline.rs` (chips), `desktop/composer.rs` (picker row in the message menu, toggle handler).
- `bt-probe`: self-chat-only reaction command.
- Risk: reactions change the real account and are visible to other participants; the outbound form is documented only in reverse-engineering notes (not an official API) and has no local live verification yet. All live checks use the self chat and are reversible (add then remove).
- Licensing: if reaction graphics from Telegram Desktop are reused, the project config rule applies (the first such change relicenses MIT to GPL-3.0 and updates `THIRD_PARTY.md`). Plain emoji text avoids this; see decision 2.

## Non-goals

- Message pins and message importance (`IsImportant`, `ImportanceFlag`), incl. display. Not mixed with the existing pinned-chat rank.
- Mentions (`MentionedUserIds`), forwarding as an action, threads.
- Custom or animated reactions beyond the types confirmed in the research.
- Arbitrary `push` / generic mutation API.
- Reaction notifications and any "who reacted" UI, including a tooltip over a chip (decision 8): the push carries only `RecentUserReactions`, a truncated list.

## Follow-ups

Later slices of roadmap item 3, each its own change (order from the research):

1. `message-pins-importance`: pins and importance flags, display of incoming state first, then explicit actions once forms and permissions are verified.
2. `mentions-forwarding`: `MentionedUserIds` in the composer with participant suggestions, plus a real "Forward" action (chat picker, preview, optional comment).
3. `threads`: thread-scoped history (`Threads: true`, `ThreadState`), join/leave, cache keys, navigation, live thread state; a separate model, not a history mode.

## Decisions

Every question that was open when this change was proposed took its recommended answer, chosen without asking the user (they asked for autonomous work). Each entry: decision, then why.

1. **Reaction types and codes.** `Type` is an integer, not an emoji (`research.md` and the protocol notes, section 17.12: `"👍"` and `"1"` fail, int `1` is accepted); the only code seen in a real push is `100101`, and no code-to-emoji table exists in the research. Decision: the client ships a closed table of confirmed (code, emoji) pairs and offers only those in the picker; any other code the server sends renders as a neutral chip with the count and stays clickable. Task 1.1 collects the table read-only from the public web client bundle and the self chat. If it confirms no pair, the table stays empty, the picker is hidden, and display, live updates and chip clicks still ship; the picker then arrives with a follow-up that fills the table. Why: a wrong mapping would put the wrong emoji on other people's messages.
2. **Emoji text, not Telegram graphics.** Chips and the picker render emoji as text (the timeline already draws "🕓" the same way). Why: no GPL relicense in this change; icons can come later in a change that accepts it.
3. **One reaction per user and message, replace semantics.** The notes give `Action` `REMOVE=1`, `REPLACE=2`, and `message_info` returns `MyReactions` as a list, so the server may allow several; the notes do not settle it. Decision: model and spec one own reaction per message; choosing another type replaces it, choosing the current one removes it. Task 7.2 checks this live in the self chat. If several are observed, the model and the spec are revised before archive. Why: it matches the only documented write form and keeps the UI small.
4. **History payload.** The history fixtures hold no reaction fields, and nothing local shows history returning them. Decision: the spec states that a message shows only the reactions the client was told about (reaction pushes, the cache, and `Reactions` on a history row if the server sends it); a message whose reactions never arrived shows none. The decoder reads `Reactions`, `ReactionsVersion` and `RecentUserReactions` next to `ClientMessage` on any `ServerMessage`, so history rows work if the server provides them, and an incoming copy without a reaction version keeps the cached reactions. The `message-history` delta is dropped. Task 7.2 records whether history rows carry them.
5. **Mutations on the real account.** Allowed, self chat only. Decision: the live check (task 7.2) runs through `bt-probe react-self` or the app with `BT_WRITE_ONLY_CHAT` set to the self chat, reacts only to a message the check itself sends there, and removes the reaction at the end. This replaces "after the user approves the target message": no existing message is touched, which is stricter. Without a signed-in session the task is reported as blocked by environment, not skipped. Everything else uses fixtures.
6. **Versions and the optimistic state.** An event is applied only if its `ReactionsVersion` is newer than the cached one (an event without a version is applied). The optimistic state is a UI overlay over the cached server state; the server's state for that message replaces it. Details in design D5.
7. **The sending spec.** `add-sending` is finished and its specs are already synced into `openspec/specs/message-sending`, so the change is a MODIFIED delta against that main spec with no archive-order dependency left.
8. **No tooltip in this slice.** `RecentUserReactions` is truncated server side (the protocol notes record `ReadsCount` 10 against 3 recent entries for reads), so a tooltip would list some reactors and silently omit others. The domain keeps only the counts and the user's own reaction. Who reacted comes with `list_reactions`, a separate read method, in a later change.
