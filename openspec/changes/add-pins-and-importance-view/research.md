# Research: message pins and importance, read side

Read-only, 2026-11-26. Sources: the protocol notes (`conarti/yandex-messenger-mcp`, `docs/protocol-research.md`, called "notes" below), the public web client bundle, the repository. No account request was made and nothing was written (see "Live observation: not done").

Predecessor: `openspec/changes/archive/2026-11-26-add-message-reactions/research.md` section 1 (slice 3 split: "message pins and importance: first incoming data and display, then explicit actions").

## Sources

- Bundle: `https://yastatic.net/s3/chat-static/telemessenger/_/3.6.3/web/app.js` (4 083 377 bytes, sha256 `88292adf0e94312b84f24ccc91d0556519d85f7eea635b6016a12b41adb0bc73`, the build the reactions research used). Anonymous GET of a public static file. Offsets below are character offsets into that file, so a reader can jump to them. The code is minified; names are as in the bundle.
- Notes sections: 9 (push events), 9.2, 9.3 (outbound table), 11.1 (`Plain`), 11.4 (other kinds), 17.9 (shape of the chat-list `history` reply on a real profile).
- Repository: `crates/yandex-proto/src/wire.rs` (decoder), `request.rs` (read allowlist), `crates/desktop/src/yandex_backend.rs` (live loop, `summary`), `crates/chat-core` (model, cache, engine).

## Pins

### Where the pin state comes from

1. **The chat entry of a `history` reply** (chat list, single chat). Bundle offset 1226304, `eW` and `e$`, used by both chat normalisers (group `eQ` through `ChatInfo`, private through `PartnerInfo`, offsets 1227027 and 1228154):

   ```js
   function eW(e){ let t=e.PinnedMessageInfo; if(t&&t.PinnedMessageTs) return [t.PinnedMessageTs];
                   let n=e.ChatInfo; return n&&n.PinnedMessageTimestamps ? n.PinnedMessageTimestamps : [] }
   function e$(e){ return e.PinnedMessageInfo && e.PinnedMessageInfo.LastActionTsMcs }
   ```

   So an entry of `Chats[]` may carry `PinnedMessageInfo { PinnedMessageTs, LastActionTsMcs }` (one pinned message, plus when the pin last changed) or `ChatInfo.PinnedMessageTimestamps` (an array). The web client stores them as `pinned_messages` (array of micros) and `pinned_timestamp`. Both keys are optional; absence means no pin.
2. **The `Pin` push.** Dispatcher (offset 1627447): `else if(t.Pin) dispatch({type: PIN, data: deserializePin(e)})`; mapper (offset 1220731):

   ```js
   function ek(e){ let t=e.ClientMessage.Pin; return {chat_id:t.ChatId, pinned_messages: t.Timestamp ? [t.Timestamp] : []} }
   ```

   and the chat reducer (offset 1779084, action `chats/updatePinnedMessage`) does `chat.pinned_messages = diff.pinned_messages`. **The push is a snapshot of the pin, not a toggle**: `Pin.Timestamp` present means "this message is pinned now", absent means "nothing is pinned". A pin by someone else, an unpin and a replacement all arrive in this one shape. Notes section 11.4 lists the wire fields: `"Pin": { "ChatId": "", "Timestamp": 0 }`.
3. **Not in history rows.** No message-level pin flag exists in `Plain` or `ServerMessageInfo` (notes 11.1, 9.1, bundle `deserializePlainMessage`). A message does not know it is pinned.

Today `decode_push` (`wire.rs:928-940`) maps `Pin` together with `SeenMarker`, `ClearUserHistory`, `StateSync`, `ChatApproval` to `PushEvent::ChatChanged { chat }`, and the live loop answers it with a full `chat_list()` refetch (`yandex_backend.rs:327-343`). Nothing reads the pin from the reply: `WireChat.raw` keeps the entry without `Messages`, but no field decodes `PinnedMessageInfo`, so a pin push today only refreshes unrelated row fields.

### One pin or several

The web client shows **one**: the selector at offset 1030883 is `function ee(e){ let t=e.pinned_messages; if(t) return t[0] }`. `PinnedMessageTimestamps` is an array, so an older or multi-pin shape exists, but the `Pin` push and `PinnedMessageInfo` carry exactly one timestamp. Whether a chat can hold several pins at once is not settled by any source (open question Q1).

### Ordering of the two sources

The chat merge function `A` (offset 1776334) keeps the pin of the stored chat when the incoming chat entry has an older `pinned_timestamp` (`LastActionTsMcs`), and applies an incoming entry whose `pinned_timestamp` differs. That is the web client's answer to "a slow chat-list reply arrives after a newer `Pin` push": order by `LastActionTsMcs`. The push body does not carry that field in the notes; whether `ServerMessageInfo.Timestamp` of the `Pin` push plays that role is unknown (open question Q5).

### Fetching the pinned message's text

The web client loads the pinned message with `requestMessage` (offset 1621657; notes section 14.2): `history { ChatId, MaxTimestamp: ts + 1, Limit: 1 }`, `MaxTimestamp` being an exclusive bound (notes 17.9, confirmed on 13 chats). That is `ReadRequest::History { chat, before: Some(ts + 1), limit: 1 }`, already in the closed read set and already encoded by `request.rs`; **no new read request is needed for the text**. `message_info` (notes 17.12: `{Message, ErrorInfo, MyReactions?, ChatInfo}`) would also work but is not in `ALLOWED_METHODS` and is not needed.

### Outbound (not part of this change)

`setPinnedMessage(chat, ts?)` pushes `Pin { ChatId, Timestamp? }`; unpin omits `Timestamp` (offset 1619107; notes 9.3 "закреп"). Role right `pin_message` gates it (offset 942477, with `mark_as_important`). Recorded here for the later write slice only; the client sends neither.

### Not to be confused with the chat pin

- `ChatSummary.pinned_rank` and the "📌" row marker (`desktop.rs:1143`, `store.rs` ordering, cache column `chats.pinned_rank`) are the **chat** pin. The web client keeps chat pins in an account bucket, `buckets.pinned_chats` (offset 730702, `setBucket("pinned_chats", ...)`), per folder `pinned_chat_ids`: an HTTP registry concept unrelated to `Pin`.
- Observation, outside this change: `yandex_backend.rs:982` always sets `pinned_rank: None`, so the client never shows a pinned chat today. This change neither fixes nor depends on that.
- `Pin` in `ClientMessage` is always the message pin; the bucket has its own method.

## Importance

### Shape on read

`Plain.IsImportant` (bool). Notes 11.1 list it among the `Plain` keys; the bundle reads it in `deserializePlainMessage` (offset 1215902: `important: g.IsImportant`) and writes it in `convertMessageToPlain` (offset 1208286: `IsImportant: e.important`). `wire.rs:431` already lists `IsImportant` in `PLAIN_META_KEYS`, i.e. the decoder knows the key is not content and **drops it**. History rows and pushes share the `ServerMessage` shape (`parse_server_message`), so one decoder covers both. Whether the server omits the key when false (as it does `IsAnonymous` and the reaction siblings) or sends `false` is not recorded; the model treats missing as false either way.

### How a change arrives

- Outbound: `setImportance(chat, ts, on)` pushes `UpdateFields { ChatId, Timestamp, ImportanceFlag: on ? 1 : 2 }` (offset 1618658). Notes 9.3 agree; the meaning of 1 and 2 is "set" and "clear" by the call site, nothing else.
- **Inbound has no `UpdateFields` branch.** The dispatcher chain (offset 1626400-1628000) has branches for `StateSync, Plain, Ephemeral, SystemMessage, Typing, SeenMarker, BotRequest, Heartbeat, ChatApproval, Pin, CallingMessage, ClearUserHistory, Reaction, Notification, Vote, MeetingCallingMessage, ...`; the string `UpdateFields` occurs once in the whole bundle (the outbound call). `UPDATE_MESSAGES = 19` exists only in an enum with no handler (notes 9.3 say the same).
- The message reducer (offset 1810161) compares the old and the new message's `important` and records `importanceMessagesChanged` when they differ. This only makes sense if **a change of importance arrives as an updated message** (`Plain` with a changed `IsImportant`, in all likelihood a higher `ServerMessageInfo.Version`, like an edit). This is an inference from the web client's code, not an observation. If it is right, the current decoder already turns the change into `MessageUpserted`, once `IsImportant` is kept.
- Open: whether such an update also sets `LastEditTimestamp` (then the client would show "edited" for a pure flag change) or keeps the text byte for byte. Open question Q4, to be answered by observation (task 0.2).

### What the web client does with it (not in this change)

Composer toggle `SWITCH_IMPORTANT_MSG` and a message-menu entry (role right `mark_as_important`), a search filter `important` and an "important messages" tab (flag `important_messages`), and important messages bypass the chat's mute for notifications (offset 1358522: `s = ... || !!t.important`). The desktop client has no notifications and no filters of that kind; none of it is in scope.

## Existing local code that matters

| Fact | Where |
|------|-------|
| `IsImportant` is classed as meta and dropped | `wire.rs:423-438` |
| `Pin` push becomes `ChatChanged { chat }`, answered with a full `chat_list()` | `wire.rs:928-940`, `yandex_backend.rs:327-343` |
| Chat list read is `history { Limit: 1, ChatDataFilter: {} }`; the entry's `PinnedMessageInfo` would be in `WireChat.raw` | `request.rs:195`, `wire.rs:696-713` |
| `ChatSummary` is cached as JSON per chat (`chats.json`); a new `serde(default)` field needs no SQL migration | `cache.rs:30-35`, `model.rs:58-73` |
| `Message` is cached as JSON per message; `serde(default)` fields are the established way to add data | `model.rs:212-229` |
| `LiveEvent::ChatChanged(ChatSummary)` already replaces a cached summary and emits `ClientEvent::ChatUpdated` | `client.rs:1065-1068` |
| `MessagePreview { author, text, timestamp }` and the adapter's `preview(&MessageBody)` already produce the one-line text and the placeholders ("Фото", the file name, "Сообщение удалено", `[kind]`) | `model.rs:52-56`, `yandex_backend.rs:987-997` |
| There is no "scroll to message" or "load around a timestamp" in the engine or the store; a search message result opens the chat only | `message-search` spec "Opening a result" |
| The cache is a flat `(chat, ts)` table; `replace_range` plus `history_start` define what is history. A lone old message upserted there would read as history with a gap | `cache.rs:236-330` |
| Chat header element (`chat-header`) is the place a bar goes under | `desktop.rs:1291` |
| Reads allowed: `request_user, get_users_data, history, subscribe, search, whoami, get_chat_members`; writes: `Plain, SeenMarker, Typing, Reaction` | `request.rs:18`, `write.rs` |

## Live observation: not done

Goal: confirm on the real account (reads only) (a) that chat-list entries carry `PinnedMessageInfo` / `PinnedMessageTimestamps` for chats with a pin, (b) the key and value of `IsImportant` in history rows, (c) the shape and timing of `Pin` pushes and of an importance change.

Attempt (2026-11-26): VM `bt-vm` started, sources synced to `~/bt/src-pins`, release `bt-probe` built, `chats` with `BT_LOG=yandex_proto::xiva=trace` (raw reply bodies) started. The connection timed out. Checks: Ethernet service in the VM is enabled; DNS answers; every TCP connection from the VM times out (`https://yandex.ru/chat`, `http://example.com`, a bare IP). The host has a full-tunnel VPN default route (`utun4`, `0/1` and `128.0/1`), which the VM's NAT does not traverse. The host network is not to be changed here, and host runs are forbidden, so no live read was made. The observations above are therefore tasks 0.1 and 0.2, and every decision that depends on them is an open question.

The repository holds no sample: no fixture, log or cache row contains `PinnedMessageInfo`, `PinnedMessageTimestamps`, `IsImportant` or a `Pin` push (grep over `crates/`, `.runtime/logs`).

## What would settle the open questions

- 0.1 (reads, no user action): `bt-probe chats` and `bt-probe history <chat>` with the raw reply trace over the chats of the real account: does any entry carry `PinnedMessageInfo` or `PinnedMessageTimestamps`; does any row carry `IsImportant`; the exact keys and value types. Answers Q1 (array length), the need for a dedicated chat-info read (design D3), and the false-versus-missing form of the flag.
- 0.2 (needs the user, no agent write): `bt-probe listen --watch <self chat>` with the push trace while the user, in the web client, pins a disposable self-chat message, replaces and removes the pin, and marks and unmarks a message as important. The agent sends nothing. Answers Q4 and Q5 and gives the live fixtures.
