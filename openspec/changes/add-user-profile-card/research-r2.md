# Research R2: how the web client fills its person card (Q4, task 0.3, static)

Method: read the public web client as the earlier changes did (CM 1.2 "Static path"). The page `https://yandex.ru/chat/` was fetched 2026-11-28 inside the Tart VM `bt-vm` (anonymous GET, no cookie, no account) and names **build 3.7.4** (`yastatic.net/s3/chat-static/telemessenger/_/3.7.4/web/`). Files read as text: `app.js`, `ui.chunk.js`, `info-panel.chunk.js`, `quick-access-panel.chunk.js`; `sidebar`, `search`, `settings`, `ui-additionals`, `global-bar` chunks were searched and hold no card code. Nothing was called on the account; the VM scratch directory was removed. No code is copied here: references are `file`, webpack module id (`mod N`), exported name and the role of the function. Module ids and export names are specific to build 3.7.4 and are only locators.

## Answer in one line

The card is filled by `get_users_data` with one guid (inside `ALLOWED_METHODS`). Around it the web client also calls three things the allowlist does not cover as `ReadRequest` expresses them: `staff_proxy` (absence "gaps"), `get_chats_info` with `common_with` (common groups) and the socket `subscribe` in two per-person forms (status, presence). **Outside the allowlist: yes** (`staff_proxy`, `get_chats_info`), both for card sections this change does not show. `whoami` is **not** called for another person.

## 1. Who opens the card

One shared action opens it: `app.js` mod 29068, export `qB` (open user profile; args: guid, `interceptable`, `sourceChatId`, `preventDefault`, optional `hit`). It logs the UI event `open_user_profile`, asks the embedding host page through a `uiInterceptor` message (no wire call), then opens a modal whose component is `UserInfoContainer` (`app.js` mod 7794 export `P`, a lazy wrapper over `info-panel.chunk.js` mod 47979, export `UserInfoContainer`), with the option for the new "Orbita" layout (flag `enableOrbitaContactCard`; both layouts render the same container and data). Sibling export `nc` (same module) opens by chat id: the partner's card for a private chat id, the chat info card (`info-panel.chunk.js` mod 52333, via `app.js` mod 46700 export `B`) for a group or channel.

| Trigger | Code | Call |
|---|---|---|
| Message avatar and author name | `ui.chunk.js` mod 17704 (component with the `message-user` style): the avatar and the name are each wrapped in a press handler that calls the message context's `onUserClick`. The message row, `ui.chunk.js` mod 4500, supplies it; its handler is `onMessageHeaderClick(guid, preventDefault, flag)` | `qB` with the author's guid and `sourceChatId` = the chat; in a **channel** (flag set) it calls `nc` with the chat id instead (chat card, not a person) |
| Member row (chat info panel) | `info-panel.chunk.js` mod 52333, the members list container: `onMemberClick(event, item)` acts only when `item.type` is `user` (group and department rows do nothing); the row menu entry `onShowProfileClick` does the same for a user | `qB` with the member's guid and `sourceChatId` |
| Reaction row (who reacted) | `ui.chunk.js` mod 89688 (export `j`, the reactions list; hook `W`, function `openUser`); the same module exists in `quick-access-panel.chunk.js` (the side panel list) | `qB` with the reactor's guid and `sourceChatId`, `preventDefault` false; the rows come from `list_reactions` (allowlisted) |

The same action is also reached from mentions in the composer (`onMentionClick`), poll voters, a meeting organizer, a URL-preview author, the threads list, forwarded-author labels; two further components of mod 4500 read the same `onUserClick` (not traced). None of these adds a wire call of its own.

## 2. Wire calls made for the card

| Wire method | Where | Called when | Inside `ALLOWED_METHODS`? |
|---|---|---|---|
| `get_users_data` body `{ guids: [guid] }`, sent without CSRF | `app.js` registry API class, method `getUsers` (beside `getUser`); caller: the fetch-user thunk, `app.js` mod 9945 export `me` | `UserInfoContainer` finds no loaded record for the guid (load state `idle`); an empty `users` makes it throw "user info was not found" and the card shows its error state. The same `getUsers` is used by the throttled batch fetcher (mod 9945 export `hU`, up to 100 guids per call) that fills names for message authors | yes |
| `subscribe` with `ToGuids`, `TtlMcs`, body type `USER_STATUS` | messenger API `subscribeToUsersStatuses` (`app.js`); controller `app.js` mod 29966 export `i`; started by the avatar hook `app.js` mod 47757 export `M2`, which the card's avatar (and every avatar) uses | an avatar for the guid is mounted and visible; refreshed about every minute | name yes; the **payload form (guid list, no chat) is not expressible**: `ReadRequest::SubscribeChat` carries a chat id only |
| `subscribe` with `ToGuid`, `TtlMcs`, body type `HEARTBEAT` | messenger API `subscribeHeartbeat`; controller `app.js` mod 58538 export `o`; same avatar hook | same | name yes; payload form not expressible (as above) |
| `staff_proxy` body `{ guids, staff_method: "export_gaps" }` | registry API method `getBulkGapUserInfo` (`app.js`); caller: the avatar hook's debounced loader (mod 47757) | an avatar is visible; once per guid per ten minutes | **no** |
| `get_chats_info` body `{ common_with: guid }`, sent without CSRF | registry API method `getCommonChats`; caller: the "common groups" query inside `info-panel.chunk.js` mod 47979 (local component `ei`); behind flag `enableCommonGroupChats` | the card opens | **no** (the name serves other lookups too: `getPrivateChatInfo`, `getGroupChatInfo` send chat id, alias or invite hash) |

Not called for the card: `whoami` for another guid (the only live call sites are the transport's own connection-open `whoami` for the account itself and an unused messenger API wrapper that takes a guid; no card or avatar code reaches it), `request_user`, `get_chat_members`, `search`. The card's "write" button opens the chat through the existing private-chat flow (`create_private_chat` when none exists): a write, the subject of CM 5.1, not a card read.

## 3. Reply fields the card renders (names only)

Record normalizer: `app.js` mod 35088 export `Nl`, applied to every `users[]` entry of `get_users_data`. Facts that matter for D3:
- the singular `employee_info` is **not read by the card**; the normalizer folds it into `employees_info` (as organisation 0) **only when `employees_info` is absent**, so the web client's order is `employees_info` first, `employee_info` as the fallback (design D3 says the reverse);
- `employees_info` is re-keyed by `organization_id`; the entry is chosen by the viewer's current organisation (entry for it when not `deleted`, else the first non-deleted entry of an organisation the viewer is not in, else that entry, else the first) in `app.js` mod 76476 (`b`, used by `h9` and `M$`);
- other keys it passes or derives: `is_robot` (forced boolean), `public_name` (falls back to `display_name`), `discovery_info.restriction` (three closed values), `organizations` (re-keyed).

Rendered by `info-panel.chunk.js` mod 47979 (container, header, the row builder local `e0`) with the view model from `app.js` mod 76476 export `h9`/`M$` and selectors in `app.js` mod 48136 (exports `tE`, `fE`):
- **Header:** display name (`display_name`; for `account_deleted` or a `deleted` employment a localized "deleted account" / "removed from the organisation" text is shown instead and the rows are hidden), avatar (`avatar_id` through the avatar hook, a monogram when absent), a status line made of the person's custom status text (and icon/emoji), the availability title and the "notifications off" note, then " • " and the presence text (online or last seen), the bot marker (`is_robot`), guest and federation markers (`organizations`, `employees_info[].is_guest`), the blocked marker (`is_disabled`; filtered out of the Orbita header).
- **Rows (only when the account is not deleted):** position (`employees_info[].position`), department (`employees_info[].department.name`), organisation name (`organization_name`, only for a guest or a federated person), `guest_organization_name`, nickname (`employees_info[].nickname`, shown as a link to the staff profile), and for each `contacts[]` entry (kept when its `organization_id` matches the chosen organisation, or all when the organisation is public) by `type`: `email` (each email is its own row, not only the `default` one), `phone`, `work_phone`; email and phone rows get a copy affordance. Rows are suppressed when the load ended in `error`.
- **Absences:** `export_gaps` reply keyed by guid, each `{ login, gaps[] }`, rendered from `id`, `workflow`, `work_in_absence`, `comment` and the gap's date fields.
- **Common groups:** `chats[]` of `get_chats_info`, shown by chat id, name and avatar from the local store.
- **Status and presence sources:** status text and availability come from the `USER_STATUS` subscribe reply (`LastMessages` entries carrying `UserStatus`, plus `CurrentTime`), the store slice `userStatuses`; presence from the `HEARTBEAT` subscribe reply (`lastSeenMs`, `onlineDuration`).

For Q3: the key names in the web path that look like presence are `lastSeenMs` and `onlineDuration` (heartbeat subscribe reply) and `Availability`, `NotificationMode`, `Duration` (status). The `get_users_data` normalizer reads none of them.

## 4. Allowlist verdict (design D2c)

`ALLOWED_METHODS` (`crates/yandex-proto/src/request.rs`): `request_user`, `get_users_data`, `history`, `subscribe`, `search`, `whoami`, `get_chat_members`, `list_reactions`.

- **Outside the allowlist: yes.** `staff_proxy` and `get_chats_info`.
- `get_users_data`, the call that carries every field this change shows (name, avatar, position, department, email, nickname, deleted), is **inside**; the card needs no new method for them.
- `get_chats_info` with `common_with`: a read **by name** (`get_*`, no write verb, sent without CSRF like `get_users_data`); **by evidence not shown** (static reading only, nothing was called). It feeds the "common groups" section, which `proposal.md` lists as a non-goal.
- `staff_proxy` with `staff_method: "export_gaps"`: **not clearly a read**: the outer name is a generic proxy to another service and the inner `staff_method` is a free string outside any allowlist; `export_gaps` reads as a read by its name only. It feeds absences, which the spec does not show.
- `subscribe` in the per-person forms: same method name as an allowed one, but a different payload that `ReadRequest` cannot build today; a subscription with a time to live on the server, like `SubscribeChat`. It feeds the status and presence lines.
- `whoami` for another person: no call site in the web client; the web client's own source of another person's status is the `USER_STATUS` subscribe, not `whoami`. This is evidence for 0.4's D2b decision beside the live Q2 answer.

For 0.4 (decision c): no row of the UC spec depends on `staff_proxy`, `get_chats_info` or the per-person `subscribe`, so by this reading **no task 1.0 is needed**; for the two outside methods the answer is "outside the allowlist, not added", and the gap (absences, common groups, presence and status through `subscribe`) belongs in the follow-ups. The `ALLOWED_METHODS`, write-set and `manage.rs` exact-list tests stay unchanged.
