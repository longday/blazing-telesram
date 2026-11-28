# Research R2: mute and silence on the wire (task 0.2)

Live run 2026-11-28, `bt-vm`, the app account's session in the VM's `~/bt/.runtime`, `bt-probe` release build of branch `nb/0.2` in `~/bt/src-NbR2`, reads only (`chats`, `chats --keys`, `history`; `BT_WRITE_ONLY_CHAT` set to a value that names no chat). Names, counts and booleans only: no chat names, ids, titles or message text are recorded here. Task 0.3 (R3) appends its results below if it runs.

## Chat list (117 entries)

| Question | Answer |
|---|---|
| Key that carries the mute flag | `Muted`, at the chat-entry top level (not inside `ChatInfo`, not inside `PrivateChatInfo`) |
| Its JSON type | `bool` in every entry that carries it (25 of 25), so `wire.rs` `parse_chat` (`entry.get("Muted").and_then(Json::as_bool).unwrap_or(false)`) reads it correctly |
| Entries that carry `Muted` | 25 of 117; the other 92 do not carry the key (absent = not muted, as decoded) |
| Entries decoded as muted (`bt-probe chats` prints ` muted`) | 25 (every `Muted` key present is `true`; `false` was not seen) |
| Muted chats by kind | 25 groups, 0 private chats, 0 channels (117 = 2 channels + 71 groups + 44 private) |
| Muted chats with unread messages | 3 of the 25 (14 unread in total); unmuted chats with unread: 3 (6 in total) |
| Any other mute-looking key | **None.** Top-level keys (count of entries): `ApprovedByMe` 117, `ApproximateOnlineUserCount` 117, `ChatId` 117, `ChatInfo` 73, `Counters` 117 (object, its inner keys were not listed), `LastEditTsMcs` 71, `LastModeratedRange` 86, `LastSeenByMeSeqNo` 114, `LastSeenByMeTsMcs` 116, `LastSeenByMeVersion2` 5, `LastSeenSeqNo` 111, `LastSeenTsMcs` 113, `LastSeqNo` 116, `LastTsMcs` 117, `Muted` 25, `MyRole` 117 (object), `PartnerInfo` 44, `PinnedMessageInfo` 11, `PrivateChatInfo` 44. `ChatInfo` (73 entries): `AvatarUrl` 28, `Description` 20, `HasBots` 71, `InviteHash` 17, `MemberCount` 73, `Name` 73, `OrganizationIds` 64, `Version` 73. `PrivateChatInfo` (44 entries): `OrganizationIds` 42, `Version` 44. No "muted until" time, no notification-level enum, no per-user setting at either level |
| Does a muted chat exist to test with (task 7.2) | **Yes**, 25 (all groups) |
| Does `wire.rs` need a decode change for mute | **No**: the real key, type and meaning match the decoder |

## Messages: `IsSilent` and `NotificationBehaviour`

Sample: the chat list's last message of each of the 117 chats (`Chats[].Messages[].ServerMessage`), plus the newest history page of 22 chats picked by kind (8 muted groups, 6 unmuted groups, 6 private chats, 2 channels; 888 messages; 16 chats returned a full page of 50). The newest message of each of those 22 chats is in both samples. "Incoming" = `From.Guid` differs from the account's own guid.

| Question | Answer |
|---|---|
| `IsSilent` | **Absent everywhere**: 0 occurrences in the raw traces of all 23 replies (chat list and 22 history pages), at any depth |
| `NotificationBehaviour`: where | One level above the body kinds: `ClientMessage.NotificationBehaviour`, next to `Plain` / `SystemMessage` (not inside `Plain`) |
| `NotificationBehaviour`: values | Only `2` (a number). No other value, no `0` or `1`, and no message that carries it with another value |
| History pages (888 messages: 717 incoming, 171 outgoing) | `NotificationBehaviour: 2` on 53 messages: 50 incoming, 3 outgoing; none on the other 835 |
| History: by body kind | incoming `SystemMessage` 47 of 123 carry it, incoming `Plain` 3 of 594, outgoing `SystemMessage` 3 of 5, outgoing `Plain` 0 of 166 |
| History: by chat kind (incoming) | muted groups 42 (41 `SystemMessage`, 1 `Plain`), unmuted groups 6 (4 `SystemMessage`, 2 `Plain`), private chats 2 (both `SystemMessage`), channels 0 |
| Chat-list last messages (117: 90 incoming, 27 outgoing) | `NotificationBehaviour: 2` on 5: incoming `SystemMessage` 1, outgoing `SystemMessage` 4; none on a `Plain` message |
| Any other mute- or silence-looking key inside a message (case-insensitive `silent`, `mute`, `notif`, `sound`, `quiet`, at any depth) | Only `NotificationBehaviour` (58 occurrences over both samples, the overlap counted twice) |
| Deleted messages among those carrying it | 0 |

Reading: `NotificationBehaviour: 2` is a service-message marker (it sits mostly on `SystemMessage` rows, also on 3 incoming `Plain` messages out of 594), and `IsSilent` never occurs in this account's data. Neither key distinguishes a message that should be silent from an ordinary one in a way the app could rely on, and neither is correlated with the chat's `Muted` flag (muted groups carry it on 42 of 360 incoming messages, unmuted groups on 6 of 188; the muted-group count comes from service messages). Follow-up of `design.md` D10 (honouring `IsSilent` / `NotificationBehaviour`): there is nothing to honour for `IsSilent`; for `NotificationBehaviour` the meaning of `2` stays unconfirmed (the sample is one account and the newest page of 22 chats).

## Method and cleanup

- Code: `bt-probe chats` prints ` muted` after the unread count (unchanged `pinned=` printing after it); `bt-probe chats --keys` prints the entry count and the sorted key names with the number of entries that carry each and the JSON value types (names and counts only), for the top level, `ChatInfo` and `PrivateChatInfo`, and nothing else on stdout (no connection banner: re-run live, all 32 stdout lines match the report line shapes, none carries a banner or a 32-hex id). `cargo test -p yandex-proto --locked` is green in the VM.
- The message counts come from `BT_LOG=yandex_proto::xiva=trace bt-probe chats` and `BT_LOG=yandex_proto::xiva=trace bt-probe history <chat>` saved to VM temp files and walked with a throw-away script that printed aggregates only; chats are identified only by their kind.
- The trace files, the scratch script, the sample lists and `~/bt/src-NbR2` in the VM were deleted afterwards; `ls` confirms none is left.
