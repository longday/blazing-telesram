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

The web client shows **one**: the selector at offset 1030883 is `function ee(e){ let t=e.pinned_messages; if(t) return t[0] }`. `PinnedMessageTimestamps` is an array, so an older or multi-pin shape exists, but the `Pin` push and `PinnedMessageInfo` carry exactly one timestamp. Whether a chat can hold several pins at once is not settled by any source. Decided (proposal Q1 = A): one bar, the latest pin.

### Ordering of the two sources

The chat merge function `A` (offset 1776334) keeps the pin of the stored chat when the incoming chat entry has an older `pinned_timestamp` (`LastActionTsMcs`), and applies an incoming entry whose `pinned_timestamp` differs. That is the web client's answer to "a slow chat-list reply arrives after a newer `Pin` push": order by `LastActionTsMcs`. The push body does not carry that field in the notes; whether `ServerMessageInfo.Timestamp` of the `Pin` push plays that role is unknown. Decided (proposal Q5 = B): order by the action timestamp, else last arrival; task 0.2 checks that the two are comparable.

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

## Live observation

### 0.1 (2026-11-27, read-only, inside `bt-vm`): done

Setup: `bt-probe` built in the VM from `ce2d7ab` (the integ head with PIN 5.2's `chats` output; the later integ head did not build), signed-in session in `~/bt/.runtime`, `BT_LOG=yandex_proto::xiva=trace`. `bt-probe chats` once, and `bt-probe history <chat>` once for each of the 116 chats (44 private, 70 groups, 2 channels); the raw reply frames were analysed inside the VM by a script that printed only key paths, value types, booleans, counts and array lengths. No name, guid, text or timestamp left the VM; nothing was written to the account.

**Chat-list entries carry the pin.** The chat-list reply (`history` with `Limit: 1`, 116 entries) has the key `PinnedMessageInfo` (object) on 10 entries; the other 106 have no pin key at all.

- `PinnedMessageInfo` is `{PinnedMessageTs: int, LastActionTsMcs: int}`: 9 entries have both, **1 entry has only `LastActionTsMcs`** (a pin that was removed: the unpin snapshot D1 predicted, the action time stays). No entry has an empty `PinnedMessageInfo`.
- **`ChatInfo.PinnedMessageTimestamps` is on no chat-list entry** (`ChatInfo` exists on 72 entries, but never with that key), so the entry form is the scalar `PinnedMessageTs`: exactly one timestamp per chat (Q1). Whether a chat can hold several pins is still not shown by any source; the web client shows one.
- `bt-probe chats` (PIN 5.2) printed `pinned=` on exactly 9 lines, matching the 9 entries with `PinnedMessageTs`.
- The single-chat `history <chat>` replies (`Chats[0]`, 116 replies) carry `PinnedMessageInfo` on 10 chats as well (9 with `PinnedMessageTs`, 1 without): the same counts as the list (the two sets of chats were not compared one by one). So a chat-list refresh carries the pin of the chats that have one. An entry without `PinnedMessageInfo` establishes only that the server sent no pin metadata for that chat now, not that it never had a pin; a refresh does not drop a pin the server still has (D1's order rule by `LastActionTsMcs` stays needed against a refresh that lags a newer `Pin` push).
- Pin events also appear in history as system rows: 3 `SystemMessage.ChatInfoDiff.PinnedMessageTimestamps` rows (array of 1 int each). Not used by this change.

**Importance.** `Plain.IsImportant` is **`true` only**: of 3 104 `Plain` rows sampled (one history page per chat, up to 50 rows, 116 chats; older history was not read), 4 carry `IsImportant: true` (bool), the other 3 100 omit the key; `false` was never sent. So a missing key is the false form (the decoder of 1.1 already treats it so; synthetic `false` rows stay tolerated). The remaining 370 rows are system messages.

**Consequences.** Task 1.3 (`ReadRequest::ChatInfo`) is dropped (design D2: "list entries do carry the pin"). Q1: one timestamp per entry, as decided. The `PinnedMessageTimestamps` array branch of D1 remains as a tolerant fallback and is unobserved on entries. Q4 (does a flag-only change set `LastEditTimestamp`) and Q5 (comparability of the `Pin` push timestamp with `LastActionTsMcs`) need 0.2 and are untouched.

### 0.2 (2026-11-27, inside `bt-vm`, web client as the other party): done

**Deviation from the task text.** The task expects the user to act on a disposable *self-chat* message. Instead the agent drove the web client (Orca embedded browser, signed in as `admin@sgnl.pro`, "Сигнал Администратор") and acted in the existing **private chat between that account and the app account** (Вадим Пащенко, whoami guid `c9a71201-…`). The app account was only listening: `bt-probe listen --watch <that private chat>` with `BT_LOG=yandex_proto::xiva=trace`, `bt-probe` built in the VM from the integ head `1dd26f2` (`-p yandex-proto --bin bt-probe` builds even though the workspace does not). Nothing was written from the app account. Writes made by the web account (the allowed set, nothing else): pin message A, pin message B (replaces A), unpin, mark an own message important, clear it. Pins and importance are per-message menu entries of the web client ("Закрепить сообщение", "Отметить как важное" / "Убрать из важных", unpin through the bar's cross plus an "Открепить сообщение?" confirmation), so no throwaway message was sent. The pins and the flag were applied to existing own messages of the web account (A = the first message, B = the latest message, the flag on the second one; the intended "second message" for B was missed by a click and the latest message was pinned instead, which does not change the findings). Side effect that stays: three `SystemMessage` rows (`ChatInfoDiff`) in that chat, and the second message carries a `LastEditTimestamp` (see Q4). The listener also receives pushes of every other chat of the account; only the watched chat's frames were looked at and nothing from other chats is recorded here. Message text, names and guids are not copied below.

Shapes (timestamps in microseconds, abbreviated as `T_A`/`T_B` for the pinned messages' own timestamps, `X1..X3` for the action times):

| Action | `Pin` push (`ClientMessage.Pin`, `ServerMessageInfo`) | Then (about 10 ms later) | Chat entry (`history`, `Chats[0].PinnedMessageInfo`) |
|--------|--------------------------------------------------------|---------------------------|-------------------------------------------------------|
| pin A | `Pin { ChatId, Timestamp: T_A }`; info `{From.Guid, Timestamp: T_A, LastEditTimestamp: X1, Version: X1}` | `SystemMessage { ChatId, ChatInfoDiff { PinnedMessageTimestamps: [T_A] }, PayloadId: "pin-…" }`, info `Timestamp: X1, Version: 1, SeqNo` | `{PinnedMessageTs: T_A, LastActionTsMcs: X1}` |
| pin B (replace) | `Pin { Timestamp: T_B }`; info `Timestamp: T_B, LastEditTimestamp: X2, Version: X2` | `SystemMessage` with `[T_B]`, `Timestamp: X2` | `{PinnedMessageTs: T_B, LastActionTsMcs: X2}` |
| unpin | `Pin { ChatId }` with **no `Timestamp`**; info `{From.Guid, LastEditTimestamp: X3, Version: X3}`, **no `Timestamp`** | `SystemMessage` with an **empty `ChatInfoDiff {}`**, `Timestamp: X3` | `{LastActionTsMcs: X3}` only (no `PinnedMessageTs`) |

- **Q5 (answer).** `ServerMessageInfo.Timestamp` of the `Pin` push is **not** the action time: it is the *pinned message's own timestamp* (`T_A`, `T_B`) and is absent on unpin. The action time is `ServerMessageInfo.LastEditTimestamp` (equal to `ServerMessageInfo.Version`), and it equals, to the microsecond, the `LastActionTsMcs` of the next chat entry and the `Timestamp` of the accompanying system row (three of three actions). So the pinned message's timestamp is *not* comparable with `LastActionTsMcs` (it is older by days), but `LastEditTimestamp` is, and it is present on unpin as well. The current decoder (`wire.rs`, `PushEvent::PinChanged.at = ServerMessageInfo.Timestamp`, and its comment "`ServerMessageInfo.Timestamp` is the action time") is wrong on both counts: with it `at` would be `T_A` (older than a cached `LastActionTsMcs`, so a pin push would be dropped as stale) and `None` on unpin. Fix: `at = LastEditTimestamp` (fallback `Version`); see task 1.5.
- **Pin shape.** `Timestamp` absent on unpin: yes (the push is a snapshot, as the bundle's mapper says). The unpin chat entry keeps `LastActionTsMcs` without `PinnedMessageTs`, the same unpin snapshot as 0.1 saw on one real chat.
- **Pin and the pinned message.** The pinned `Plain` row is untouched: no `Version` change, no `LastEditTimestamp`, no flag; history shows the pin only through the system rows. The chat's last message becomes the `ChatInfoDiff` system row (chat list preview and ordering move with every pin action; an existing property of the server, not of this change).
- **Importance, Q4 (answer): a flag-only change sets `LastEditTimestamp`.** Mark: a **`Plain` push** with the whole message repeated (same text, same `PayloadId`, same `Timestamp`/`SeqNo`), `IsImportant: true` added, `ServerMessageInfo.LastEditTimestamp` newly set (the action time), and **`Version` unchanged (1)**. Clear: the same push with `IsImportant` **omitted** (not `false`) and `LastEditTimestamp` set again to a newer value, `Version` still 1. No `UpdateFields` push, no other kind. History rows afterwards carry the same `LastEditTimestamp` (and `IsImportant: true` only while set). `bt-probe history` consequently prints the row as `(edited)` after a pure flag change, also after the flag is cleared. So the inference about a `Plain` update holds, but the "higher `Version`" part does not: **`Version` is not a usable ordering for flag changes** (any later logic that arbitrates by `Version` must not be applied to the message body; `LastEditTimestamp` is the monotone value). Q4 = B is required (task 2.4).

**Consequences.**

- PIN 1.2 (`wire.rs`): `at` from `LastEditTimestamp` (new task 1.5); fixtures `research-push-pin*.json` must be rebuilt as sanitised live captures with that shape (an unpin without `Timestamp` and without `ServerMessageInfo.Timestamp`).
- PIN 2.3 / 3.2: no change of contract (`at` is still the arbitration clock; Q5 = B holds), but their tests must use `at` = action time, and the unpin push has `at` too (so a stale unpin is ignorable).
- PIN 2.4: unblocked and required: a flag flip arrives as a repeated body plus `LastEditTimestamp`, which `wire.rs` turns into `edited: true`; without the rule the bubble would show "изменено" after every mark and unmark, and clearing the flag still leaves "edited".
- PIN 6.2: the user-at-the-web-client part is replaceable by this setup (the web account acts, the app account listens); a self-chat is not needed, and importance can be set on existing messages, so no throwaway message is required.

Earlier attempt of 2026-11-26 (kept as history): the VM had no outbound network then (cause not determined; the host's full-tunnel VPN was suspected, not verified); the network was restored on 2026-11-27.

The repository held no sample before 0.1: no fixture, log or cache row contained `PinnedMessageInfo`, `PinnedMessageTimestamps`, `IsImportant` or a `Pin` push. The fixtures are still synthetic until task 1.5 replaces them with sanitised captures of the shapes above.

## What would settle the open questions

- 0.1 (reads, no user action): `bt-probe chats` and `bt-probe history <chat>` with the raw reply trace over the chats of the real account: does any entry carry `PinnedMessageInfo` or `PinnedMessageTimestamps`; does any row carry `IsImportant`; the exact keys and value types. Answers Q1 (array length), the need for a dedicated chat-info read (design D3), and the false-versus-missing form of the flag.
- 0.2 (done 2026-11-27, see "Live observation"; the web client acted as the other party of a private chat, not on a self chat): `bt-probe listen --watch <chat>` with the push trace while a pin, a replacement, an unpin, a mark and a clear were made in the web client. Answered Q4 and Q5.
