# Design

## Context

- The read-only client (`openspec/changes/archive/2026-11-23-add-read-only-client/design.md`) makes writes unrepresentable: `ReadRequest` is closed and `ALLOWED_METHODS` holds four read methods, enforced by tests in `crates/yandex-proto/src/request.rs`.
- Every write in the web client is the socket method `push` with a JSON body; the reference `conarti/yandex-messenger-mcp` documents the forms (`docs/protocol-research.md` §9.3, §11.1, §14.4–14.6; `src/protocol/push.ts`, `src/protocol/mutations.ts`):
  - envelope `{RequestId, ClientTransportId:{XivaSubscriptionId}, UserAgent, ClientMessage:{<variant>, LogData:{YandexUid}}, Meta:{Origin}, ClientSupportedFeatures}`; `XivaSubscriptionId` is the id from Xiva's `subscribed` operation frame;
  - send: `Plain{ChatId, PayloadId, Text:{MessageText}}`; reply adds `ForwardedMessageRefs:[{ChatId, Timestamp}]` and `ForwardedMessageStyles:[{Quote}]`;
  - edit: `Plain{ChatId, Timestamp, Text:{MessageText}}`; delete: `Plain{ChatId, Timestamp}`;
  - read: `SeenMarker{ChatId, Timestamp, SeqNo?}`; typing: `Typing{ChatId}`;
  - the response `Status` is a commit status (1 FULLY_COMMITTED, 8 DUPLICATE are success; 3 FAILED, 4 NO_SUCH_CHAT, 7 SENDER_NOT_IN_CHAT, 15 NO_PERMISSION, 18 THROTTLED, 23 RATE_LIMIT_EXCEEDED, …), not the application status of reads; a send also returns `MessageInfo{TimestampMcs, SeqNo, Version}`.
- Evidence level: send, edit, delete and `SeenMarker` are confirmed live in the reference; reply and typing are only inferred from the web bundle.
- Observed here (`bt-probe listen`, 2026-11-23): the server echo of an own message carries `ClientMessage.Plain.PayloadId`, so the echo can be matched to the outbox entry that produced it.
- No text length limit, edit/delete time window, rate quota or server-side drafts are documented.

## Goals / Non-Goals

Goals: the four capabilities in `specs/`, with writes still limited to a closed, tested set.

Non-goals: mentions, forwarding, reactions, pins, files, presence status; marking a chat unread.

## Decisions

### D1. Closed write set in the protocol crate
A new `WriteRequest` enum (`SendText`, `Edit`, `Delete`, `Seen`, `Typing`) is the only way to build a `push`. `XivaHandle::push(WriteRequest) -> Result<Committed, ProtoError>` fills the envelope from actor state (subscription id, request id); `LogData.YandexUid` is the session's `yandexuid` cookie, as in the reference, and a session without it cannot write. `Meta.Origin = 27` and `ClientSupportedFeatures = 0` come from the reference and were accepted on every live write. Tests assert that `"push"` is the only new method, that the `ClientMessage` variants are exactly `Plain`, `SeenMarker` and `Typing`, and that the read allowlist is unchanged. Like reads, a `push` fails fast before the current connection's `subscribed` frame and while offline; the outbox retries after `Connected`.
Alternative: a generic `call(method, json)`. Rejected: it reopens every mutating method, including reactions and presence, which this change does not cover.

### D2. Commit status is its own type
`push` responses are parsed into `Committed { message: Option<MessageInfo> }` or `ProtoError::Commit { status, name, permanent }`; statuses split into permanent (NO_PERMISSION, SENDER_NOT_IN_CHAT, NO_SUCH_CHAT, BANNED, BLACKLISTED, BLOCKED_BY_PRIVACY, BAD_REQUEST, MESSAGE_NOT_FOUND) and transient (everything else, including THROTTLED and RATE_LIMIT_EXCEEDED). The read path's `check_status` is not reused, since status 1 means success here and an internal error there.

### D3. Outbox with stable PayloadId
Each send gets a UUID `PayloadId` once and is stored in a new cache table `outbox(payload_id PK, chat, created_us, json)` before any network call; the insert and the deletion of the chat's draft are one transaction, and a send that cannot be stored is refused with an error, keeping the draft. The core sends outbox entries one at a time in creation order, after `Connected`; a transient failure holds the queue (keeping order) and retries after reconnect or after a backoff of 5, 10 … 60 s (the unit of `RateLimit.WaitFor` is unknown); a permanent failure marks that entry failed with the status name and the queue moves on; the entry waits for Retry. Resending reuses the `PayloadId`, so the server answers DUPLICATE instead of storing a second copy; a DUPLICATE without `MessageInfo` reloads the chat's newest chunk.
A cache that cannot be opened is moved aside (`<account>.sqlite.broken-<secs>`), never deleted, because it now holds the only copy of pending messages and drafts.
Pending messages are ordinary `Message` values with a new `delivery` field (`Sent`, `Pending`, `Failed(reason)`) and a local timestamp (`created_us`); they sort after server messages. A confirmation (`MessageInfo.TimestampMcs`) or the echo push with the same `PayloadId`, whichever comes first, replaces the pending message with the server one; the other becomes a no-op.
Alternative: memory-only queue. Rejected: the spec keeps pending messages across restarts.

### D4. Edits and deletions are not queued
Edit and delete run only while online and apply locally after a success status, then the echo push updates them again idempotently. Offline or failed attempts leave the message unchanged and show the reason in a toast. They target a message by timestamp, so a blind replay is unsafe to queue (edit replay may reapply an older text).
An edit rewrites the whole `Plain`: one without `ForwardedMessageRefs` drops a reply's quote on the server, and a later edit restores only the quote text, not the link (verified live). So `Backend::edit` takes the reply target and the core passes the cached message's `reply_to`; the core refuses (and the menu hides Edit for) forwarded messages, replies whose original is in another chat (a forward with a comment parses as a reply) or has no timestamp, and messages not in the cache. Deleting a chat's last message sets its row preview to "Сообщение удалено", as the server's summaries do.

### D5. Reply quote from the local copy
The reply target is a loaded message of the open chat; its text, cut to 200 characters (the reference's bound), becomes `Quote`. The reference rereads it from the server; the local copy is the same text the user saw and needs no extra request.

### D6. Read markers from the UI's visible range
Each timeline row reports its painted bounds; the desktop reports the newest sent message whose bottom edge is inside the viewport (a row taller than the viewport counts once its bottom is visible) and sends `Command::MarkRead { chat, timestamp }` when the window is active, no dialog or log view covers the chat, and that timestamp is newer than the last one sent for the chat; pending and failed messages are never reported. The core drops markers not newer than the newest one requested, drops the unread counter to 0 at once when the marker is at or after the chat's last message, and sends `Seen` without `SeqNo` (optional in the form); a marker requested offline is kept per chat and sent once online. A failed `Seen` is only logged; the next chat update brings the server's count.

### D7. Typing
Outgoing: on composer text change of a new message (not while editing a sent one), send `Typing` if none was sent for that chat in the last 4 s; nothing is sent offline or after the user stops, since typing expires on its own. Incoming typing pushes become `ClientEvent::Typing { chat, user, until }` with `until = now + TimeoutSeconds`, or 7 s when the push has none (live pushes repeat every 3–5 s and carried no timeout); the account's own typing from other clients is dropped. The store expires entries and clears a user's entry when their message arrives. Names come from the push or the users already known.
Outgoing typing is withheld while the chat's unread counter is above 0 (the draft is still saved): the server reads the chat for it. The counter, not the marker the UI sent, is the gate because the core raises it on every incoming message and zeroes it only when the newest message is marked (3.5), so it stays above 0 exactly while newer messages are unseen.

### D8. Multi-line input on the existing `TextInput`
`TextInput` gains a multi-line mode: text is shaped with `WindowTextSystem::shape_text` and a wrap width into `WrappedLine`s; caret, selection and hit-testing use `WrappedLineLayout::position_for_index` and `closest_index_for_position`. The composer grows to 8 lines, then scrolls inside. Enter is a composer action (send); Shift+Enter inserts `\n`.
Alternative: `gpui-component` 0.5.1, the last release on gpui 0.2.2. Rejected: 32 mandatory dependencies (tree-sitter, lsp-types, html5ever, notify, rust-i18n) for one text field, and the line is frozen there.

### D9. Drafts in the cache
New table `drafts(chat PK, json)`; the store saves the composer of the chat being left on switch and 1 s after the last keystroke, and the core deletes the row on send. The chat row shows "Черновик:" and the draft instead of the last message; typing by others takes precedence over both.

### D10. UI pieces
Context menu: a right-click on a bubble opens an anchored popup with the items of the `message-editing` spec. Delete confirmation reuses the modal pattern of the sign-out confirmation. Reply and edit bars sit above the composer with an accent left border, an icon, the quoted author/text and a close button (`docs/mockups/1-main.svg`). Delivery marks: a clock for pending, one check for sent, a red "!" for failed, shown at the bubble's time.

### D11. Live verification only in the self chat
`bt-probe` gains `send-self <text>`, `edit-self <ts> <text> [--reply <ts>]`, `delete-self <ts>`, `seen <chat> <ts>`, `typing-self` and `typing <chat>`; the self-chat commands refuse any other chat id. `seen` and `typing` name their chat on purpose and carry no text; `typing <chat>` exists because the server does not echo the account's own typing to the self chat. The app reads `BT_WRITE_ONLY_CHAT` at startup: when set, `YandexBackend` rejects every write to any other chat before it reaches the connection and logs each write (kind and chat, no content). VM runs always set it to the self chat, because opening a chat in the app already writes a read marker.

## Risks / Trade-offs

- [Writes act on the real account and cannot be undone] → the closed write set (D1), self-chat-only probe commands (D11), confirmation before deletion.
- [Reply and typing forms are inferred, not confirmed] → live checks in the self chat are tasks; a form that fails its check is removed from this change rather than shipped unverified.
- [Channels and restricted chats: write rights are not known in advance] → the server's permanent status marks the message failed with the reason; no retry loop.
- [Read markers change what the user's other clients show] → markers only for messages visible in the focused window (D6); task 3.4 of the archived change verifies that loading alone changes nothing.
- [The server treats an outgoing typing as reading the chat (live: a peer's message went from one check to two right after `bt-probe typing <chat>`)] → typing goes out only while the chat's unread counter is 0 (D7), so it cannot report unseen messages as read.
- [Unknown rate limits] → one send in flight, backoff on THROTTLED/RATE_LIMIT_EXCEEDED.

## Migration Plan

Cache migration 2 adds `outbox` and `drafts`; existing caches upgrade in place. No rollback beyond deleting the cache file, which is disposable apart from pending messages and drafts.

## Open Questions

None. Live typing pushes carry no `TimeoutSeconds`, so the 7 s fallback applies; the web client showed a single outgoing typing for about 5–9 s, which the 4 s repeat interval covers.
