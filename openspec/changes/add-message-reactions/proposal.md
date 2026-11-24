# Proposal

## Why

The client can read, send, edit and delete, but reactions are invisible: a message someone reacted to looks the same as one nobody reacted to, and the user cannot react. Reactions are the first slice of roadmap item 3 (reactions, importance, pins, mentions, forwarding, threads). The research (`research.md`, next to this file) recommends splitting item 3 and starting with reactions: the wire shape is documented (`Reaction { ChatId, Timestamp, Type, Action }` outbound; `Reactions`, `ReactionsVersion`, `RecentUserReactions` inbound) and a live push fixture already exists (`crates/yandex-proto/tests/fixtures/live-push-reaction.json`).

`research.md` is the single context document for this change and for the later slices of item 3; read it before writing the design.

## What Changes

- Decode the reaction push into a domain event: per-message reaction counts, the version (`ReactionsVersion`) and who reacted recently (`RecentUserReactions`). Today it falls into `WireBody::Other("Reaction")` and `PushEvent::Ignored`.
- Show reactions on messages in the timeline: a chip per reaction type with its count, the user's own reaction highlighted; update live from push and keep them in the offline cache.
- Let the user add, change and remove their reaction on a message: a new closed `WriteRequest` variant (`Reaction`) and `"Reaction"` in `CLIENT_MESSAGE_VARIANTS`; applied optimistically, rolled back on failure.
- Entry points: a reaction control in the message context menu and a click on an existing chip toggles the user's own reaction.
- **BREAKING** (to the spec, not to users): the sending spec's "Writes only from user actions" requirement forbids reactions; it is relaxed to allow reactions as a user action. Presence heartbeats and status changes stay forbidden. No generic `push` is added: the write set stays closed and grows by exactly one variant.
- `bt-probe`: a reaction command limited to the self chat for live verification.

## Capabilities

### New Capabilities
- `message-reactions`: reading, live-updating, caching and displaying message reactions; adding, changing and removing the user's own reaction.

### Modified Capabilities
- `message-sending`: "Writes only from user actions" now includes reactions (still no presence heartbeats or status changes).
- `message-history`: a message carries its reactions when loaded from history (only if the open question on history payload below is confirmed; otherwise reactions arrive from push only and this line is dropped).
- `live-updates`: the reaction push event is handled instead of ignored.
- `offline-cache`: reactions and their version are cached with the message.

## Impact

- `crates/yandex-proto`: `write.rs` (`WriteRequest::Reaction`, `CLIENT_MESSAGE_VARIANTS`, encoding, tests); `wire.rs` (reaction wire model, `PushEvent` variant, `decode_push`; the existing test asserting `Other("Reaction")` changes); `request.rs` (mutating-method guard mentions "reaction"; module docs say reactions are not representable); fixture-based tests in `tests/wire_fixtures.rs`.
- `crates/chat-core`: `Message` gains reactions and a version; a command and backend method for toggling a reaction; cache schema and migration; handling of the live event and optimistic state.
- `crates/desktop`: reaction chips in `timeline.rs`, context-menu entry, click handling.
- `bt-probe`: self-chat-only reaction command.
- Risk: reactions change the real account and are visible to other participants; the outbound form is documented only in reverse-engineering notes (not an official API) and has no local live verification yet. All live checks use the self chat and are reversible (add then remove).
- Licensing: if reaction graphics from Telegram Desktop are reused, the project config rule applies (the first such change relicenses MIT to GPL-3.0 and updates `THIRD_PARTY.md`). Plain emoji text avoids this; see open question 2.

## Non-goals

- Message pins and message importance (`IsImportant`, `ImportanceFlag`), incl. display. Not mixed with the existing pinned-chat rank.
- Mentions (`MentionedUserIds`), forwarding as an action, threads.
- Custom or animated reactions beyond the types confirmed in the research.
- Arbitrary `push` / generic mutation API.
- Reaction notifications and a "who reacted" popup beyond what `RecentUserReactions` already gives for a tooltip (decided in the design).

## Follow-ups

Later slices of roadmap item 3, each its own change (order from the research):

1. `message-pins-importance`: pins and importance flags, display of incoming state first, then explicit actions once forms and permissions are verified.
2. `mentions-forwarding`: `MentionedUserIds` in the composer with participant suggestions, plus a real "Forward" action (chat picker, preview, optional comment).
3. `threads`: thread-scoped history (`Threads: true`, `ThreadState`), join/leave, cache keys, navigation, live thread state; a separate model, not a history mode.

## Open Questions

1. **Which reaction types does the UI offer, and what do the `Type` codes mean?** The fixture has `Type: 100101`; the code-to-emoji mapping is not in the research. Recommended: before the design, read-only inspect real history/push of the self chat (and the web client's reaction picker) to collect the set; ship only confirmed types and render unknown types as a neutral chip with the count. Trade-off: a small picker at first, but no wrong or invisible reactions.
2. **Emoji text or Telegram graphics for chips?** Recommended: render emoji as text/system emoji in the first slice. Trade-off: less Telegram-like than reusing their icons, but avoids the GPL relicense this change would otherwise trigger (project config); icons can come later in a change that accepts the relicense.
3. **One reaction per user, or several?** The notes give `Action: REPLACE=2` and `REMOVE=1`, which suggests one per user per message with replace semantics, but this is unverified. Recommended: assume replace semantics, confirm with a live check in the self chat, and fall back to the observed behavior in the design. Trade-off: if several are allowed the UI needs add-per-type logic, so confirm before specifying.
4. **Does history return `Reactions`, or only push?** Not confirmed locally (only the push fixture exists). Recommended: check the history fixture/live response first; if absent, reactions show only for messages seen live after startup, which must be stated in the spec. Trade-off: an extra check now versus a visibly incomplete UI later.
5. **May mutations be verified on the real account?** Recommended: yes, only in the self chat, with a reversible add-then-remove scenario, after the user approves the target message; everything else uses fixtures. Trade-off: slower than testing in a busy chat, but no effect on other people.
6. **Ordering and conflicts:** how to reconcile the optimistic reaction with a push carrying an older `ReactionsVersion`. Recommended: apply an event only when its version is newer than the cached one; the optimistic state is dropped when the server version for that message arrives. Trade-off: needs the version cached per message, but avoids flicker and stale overwrites.
7. **Is the sending spec edit acceptable as a delta here?** The "Writes only from user actions" requirement lives in the not yet archived `add-sending` (and `openspec/specs/message-sending`). Recommended: express it as a MODIFIED delta against the main spec and sequence this change after `add-sending` is archived. Trade-off: a dependency on archive order, versus editing another change's files, which this task forbids.
