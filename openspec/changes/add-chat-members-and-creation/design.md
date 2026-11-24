# Design

## Context

See `proposal.md` for motivation and the resolved decisions; `research.md` for sources. What the code looks like today:

- Reads are a closed enum, `ReadRequest` in `crates/yandex-proto/src/request.rs`, with `ALLOWED_METHODS` listing exactly `request_user`, `get_users_data`, `history`, `subscribe`. Tests pin the list (`request.rs`: `allowlist_has_no_mutating_or_presence_methods` forbids names containing `create_`, `leave`, `invite`, `set_` and asserts length 4; `write.rs`: `the_read_allowlist_does_not_gain_the_write_method` asserts the exact list).
- Writes are the closed `WriteRequest` (`write.rs`): socket method `push` only, `ClientMessage` variants `Plain`, `SeenMarker`, `Typing`. There is no Registry (HTTP) mutation anywhere: `RegistryClient::execute` (`registry.rs`) takes only `&ReadRequest`, fetches a CSRF token when the route says so, maps `status != "ok"` to `ProtoError`, logs method and status at debug and the reply body only at trace.
- `whoami` is not sent by the client. `frame.rs` can encode it (`encode_whoami_is_byte_exact`), `request_user` (HTTP) is used by `bootstrap` for identity (`guid`, `uid`, `display_name`). The reference documents `whoami: { Guid } -> { UserInfo, UserStatusInfo, CurrentTime }` with `UserInfo { Guid, Uid, DisplayName, PublicName, Nickname, AvatarId, ... }` and `UserStatusInfo { UserStatus: {}, Timestamp }`.
- The reference catalogue (`docs/protocol-research.md` §7, §10): `get_chat_members` → `{users, groups, departments}`, `get_group_members` → same with `users[].role = "admin"`, params opaque (`chat_id`, paging); `create_private_chat { guid, wait_for_update, onetime_user_token }`; `create_chat { alias, ... }` opaque. Mutations are CSRF-guarded (`X-CSRF-TOKEN`), the member reads carry no CSRF flag.
- `chat-core::Backend` (`backend.rs`) has `bootstrap`, `chat_list`, `history`, `fetch_media`, `subscribe`, `watch`, `send_text`, `edit`, `delete`, `mark_seen`, `typing`; `BackendError` has `Rejected { reason, permanent }`. `YandexBackend` (`crates/desktop/src/yandex_backend.rs`) maps `ProtoError` to it, holds `Connection { client, events, me }` (`me` = own guid) and applies the `BT_WRITE_ONLY_CHAT` guard to every `push`. Private chat ids look like `<guid>_<guid>`; `WireChat.partner_guid` already exists but `ChatSummary` drops it.
- The engine (`crates/chat-core/src/client.rs`) is one `Engine<B>` with `Command`/`Input`/`Action` enums, a cache and an event channel; the desktop `ChatStore` (`store.rs`) turns `ClientEvent` into `StoreEvent`; `Desktop` (`desktop.rs`) owns overlays (`AccountMenu`, `delete_pending`, `LogView`) that make `blocked()` true.
- Mockups: `docs/mockups/3-info.svg` (chat info), `docs/mockups/6-settings.svg` (settings/profile). Only the read-only subset is built.

## Goals / Non-Goals

**Goals:** the three capabilities in `specs/`; mutations limited to a closed, tested pair of Registry methods; no creation without confirmation, no automatic retry of creation.

**Non-Goals:** any management beyond creation (see proposal Non-goals); caching members; directory search; avatars upload; showing presence.

## Decisions

### D1. Three new reads in the closed read set
`ReadRequest` gains `Whoami { guid }` (socket, body `{Guid, RequestId}`; route `Socket`), `ChatMembers { chat }` (`get_chat_members`) and `GroupMembers { chat }` (`get_group_members`), both `Route::Registry { csrf: false }` with params `{ "chat_id": chat }`. `ALLOWED_METHODS` becomes the exact seven names `request_user, get_users_data, history, subscribe, whoami, get_chat_members, get_group_members`; the existing tests are updated to that list and still reject any mutating-looking name. The backend uses `get_group_members` for `ChatKind::Group` and `Channel` (it carries the role) and `get_chat_members` for `Private`; if the live probe (task 1.2) shows `get_chat_members` unusable for private chats, the panel shows the partner from the chat row instead and the variant is dropped from the set.
`Client` keeps the guid of the session (`session.identity.guid`) to fill `Whoami`. Typed accessors `Client::members(chat, kind)` and `Client::whoami()` return parsed wire models (`WireMembers`, `WireProfile`); until the shapes are recorded (task 1.2) the facade returns raw JSON for the probe.
Alternative: read the profile from `request_user`, which bootstrap already calls. Rejected for the profile view: it returns identity fields only, while `whoami` carries `UserStatusInfo`; `request_user` stays the identity source for bootstrap.
If a live member read answers `bad_csrf_token`, the route flips to `csrf: true` (as `RequestUser`); task 1.3 owns that switch.

### D2. Closed Registry mutation set in a new module
New `crates/yandex-proto/src/manage.rs` with `ManageRequest` (initially only `CreatePrivateChat { guid }`; `CreateChat` is added by task 6.1 if the gate passes), `MANAGE_METHODS` (`["create_private_chat"]`, plus `create_chat` later) and `registry_body()`. `RegistryClient::manage(&ManageRequest)` always fetches a CSRF token first and shares the header/cookie code with `execute`, but is a separate entry point: `execute` keeps accepting reads only, so a mutation cannot be built through the read path and no generic "call any method" exists. Tests assert: the method list is exactly `MANAGE_METHODS`; none of its names is in `ALLOWED_METHODS`; no name of the deferred management set (`change_chat_members`, `change_chat_role`, `change_chat_admins`, `leave`, `invite`, `invite_renew`, `set_chat_info`, `create_token`) is representable; the CSRF header is sent; cookie and token values never reach the log (the existing `registry` log test is the model).
`create_private_chat` params: `{ "guid": <guid>, "wait_for_update": true }`; `onetime_user_token` is omitted (the token comes from `create_token`, a different mutating method, which this change does not add). If the live check (task 5.1) shows the server refuses without it, private creation moves to a follow-up exactly like group creation (D4 fallback).
Alternative: put creation on the socket `push` path. Rejected: the reference lists creation as Registry methods; `push` carries messages only.

### D3. Outcome classification: not sent, rejected, unknown
A repeated creation can duplicate a chat, so the transport layer reports what it knows: `manage` returns `Result<Json, ManageError>` where `ManageError` is `NotSent(ProtoError)` (CSRF exchange failed, or the connection could not be opened: nothing left the client), `Rejected(ProtoError)` (HTTP 401 = session expired, or a reply whose `status` is not `ok` with its `data.code`: the server answered and did not create) or `Unknown(ProtoError)` (the request was sent and the reply is a timeout, a dropped connection, HTTP 5xx/429 or a non-JSON body). A reply with `status: "ok"` is success even if the chat id cannot be found in it. `Backend::create_*` returns `Result<Option<ChatId>, CreateError>`, `CreateError = Failed(BackendError) | Unknown(String)`; `NotSent` and `Rejected` map to `Failed` (SessionExpired stays `SessionExpired`), `Unknown` carries a reason string without cookie or token values.
Alternative: reuse `BackendError::Network` for everything. Rejected: it cannot say whether the chat may exist, and the spec requires that distinction.

### D4. Gate for group creation (`create_chat`)
`create_chat` has opaque parameters in the reference, so it is not implemented blindly.
1. Task 0.1 (read-only, no account effects, can start immediately): read the web client's JavaScript bundle (the reference's `docs/protocol-research.md` cites it as the source of its §7 parameter shapes) and record the `createChat` parameters (title field name, participant list field and element form, optional fields, default values) and the reply shape under "D4 outcome" below.
2. If the payload is fully determined: task 6.1 adds `ManageRequest::CreateChat` and tests with the recorded shape; 6.2–6.3 add backend/core/UI; task 6.4 creates one throwaway group with the user's approved participants through `bt-probe create-chat` and checks it in the web client.
3. Fallback: if step 1 cannot determine the payload, or task 6.4 fails for a reason the client cannot correct (the server refuses the recorded shape), task 6.F deletes group creation from `specs/chat-creation/spec.md` (the requirement "Group chat" and the group wording in "Explicit confirmation"), removes any group code already written, and adds the follow-up `add-group-chat-creation` to `proposal.md`. Everything else in the change is unaffected and still archivable.

**D4 outcome:** recorded here by task 0.1 (payload and reply shape) and by task 6.4 (live result). Until then group tasks 6.x do not start.

### D5. Creation flow in the engine: no queue, no retry
New commands `CreatePrivateChat { user }` (and `CreateGroupChat { title, users }` from task 6.2); events `ChatCreated { chat: Option<ChatId>, existing: bool }` and `CreationFailed { reason: String, unknown: bool }`. The engine handles a creation like an `Action` (attempted once, not queued, nothing persisted):
- one creation at a time; a second command while one is in flight is ignored (the UI also disables the button);
- offline (`!is_online()`): `CreationFailed { unknown: false }` without a request;
- private chat that already exists in the cached list (`ChatSummary.partner == user`): `ChatCreated { chat: Some(id), existing: true }`, no request;
- success: refresh the chat list (the path `become_online` uses), then emit `ChatCreated` with the id from the reply, else, for a private chat, the list entry whose `partner` is the user, else `None`; the store opens it once it is in `chats`;
- `Failed`: `CreationFailed { unknown: false }`; `Unknown`: `CreationFailed { unknown: true }` and the same list refresh. Nothing re-sends by itself; the next attempt is the user pressing "Создать" again, and for a private chat the check above opens a chat that appeared in the meantime.
`ChatSummary` gains `partner: Option<UserId>` (`#[serde(default)]`, so cached rows from before still load) filled from `WireChat.partner_guid` in `summary()` of `yandex_backend.rs`; the struct-literal sites (`chat-core/src/fixtures.rs`, `desktop/src/tests.rs`, `yandex_backend.rs`) follow; the `unread_chat` helper in `client.rs` tests uses struct update and needs no change.

### D6. Members and profile in the engine and store
Domain types in `model.rs`: `MemberRole { Member, Admin }`, `Member { user: UserId, name: Option<String>, avatar: Option<MediaRef>, role: MemberRole, me: bool }` (`me` set by the backend, which knows the own guid), `MemberGroup { id, name }`, `ChatMembers { chat, users, groups, departments, partial: Option<(u32, u32)> }` (shown/total when the server says more exist), `Profile { user, name, avatar, status_text }`. Commands `LoadMembers(ChatId)` (kind looked up in the cached summary) and `LoadProfile`; events `Members(ChatMembers)`, `MembersFailed { chat, reason, offline }`, `Profile(Profile)`, `ProfileFailed { reason, offline }`. Both are plain reads: offline answers at once with the `offline` flag, nothing is cached or persisted, duplicates of an in-flight load for the same chat are dropped. Names missing from a members reply are filled with the existing `Users` helper (`get_users_data`, batched), avatars become `MediaRef { Avatar }` and are fetched through the existing media path.
`ChatStore` keeps `members: HashMap<ChatId, MembersState>` (`Loading | Ready | Failed`), the profile state and the creation state, and emits `StoreEvent::{Members(ChatId), Profile, Creation, ChatCreated}`.
`partial` rule: the parser keeps any "more available" marker the server sends (key recorded by task 1.2); if the server pages, the backend loads every page up to a bound of 20 requests and sets `partial` if the bound was reached; if the server does not page, `partial` is `None`.
`status_text` is the text of `UserStatusInfo.UserStatus` if it holds one (shape recorded by task 1.2), nothing otherwise; no availability is inferred.

### D7. UI placement
Three new modules in `crates/desktop/src/`: `info_panel.rs` (right-hand panel from the chat header: member sections, admin marker, "Написать" on rows other than `me`, loading/error/offline/partial states), `profile.rs` (read-only profile opened from the account menu next to sign-out; shares the `blocked()` treatment of overlays), `new_chat.rs` (dialog: person picker or title field, selected participants, "Создать"/"Отмена"; candidates = distinct `ChatSummary.partner` guids with the chat title as name, plus the members loaded for the open chat, minus `me`). "Новый чат" is a button beside the search field in `Desktop::sidebar`, disabled unless `ConnectionState::Online`. Escape closes each overlay; Enter in the dialog's fields never triggers creation, only the "Создать" button does. The three modules all hook into `desktop.rs` (field, render call, `blocked()`), so their tasks run in sequence.
Alternative for candidates: a directory search. Rejected for this change (proposal decision 7).

### D8. Developer guard and live verification
When `BT_WRITE_ONLY_CHAT` is set, `YandexBackend::create_*` refuses with `Failed(Rejected { permanent: true })` and logs the attempt (kind only): the variable limits writes to one existing chat, and creation writes outside it. VM runs always set it, so they verify the refusal; a real creation through the app happens only in a task that unsets it with the user's approval.
`bt-probe` gains read commands `whoami` and `members <chat>` (task 1.2) and `create-private <guid> --confirm-create` (task 2.2; refuses without the flag and prints what it is about to create); `create-chat <title> <guid>... --confirm-create` arrives with task 6.1 if the gate passes. Output prints structure and names but never cookie or token values; fixtures recorded from live replies are sanitised (no cookies, no personal data beyond display names replaced by placeholders).

### D9. Sibling changes and merge order
`add-message-reactions`, `add-media-download` and `add-global-search` are being planned in parallel and may also touch `request.rs` (allowlist), `client.rs` (engine), `store.rs`/`desktop.rs` and the "Writes only from user actions" requirement of `message-sending`. Rule for every task here: edit by adding (new enum variants, new modules, new list entries) and extend the exact-list tests instead of replacing them; before editing a shared file read its current content. Archive order decides which delta is applied last; the later one re-reads the main spec and reapplies its wording.

## Observed shapes

Filled by task 1.2 (members and `whoami` replies: key names, `role`, paging marker, CSRF requirement), task 5.1 (`create_private_chat` reply, `wait_for_update` behaviour, whether `onetime_user_token` is required) and task 6.4 (group). Known before any run, from the reference only: see Context.

## Risks / Trade-offs

- [Creation acts on the real account and cannot be undone by this client] → closed mutation set (D2), confirmation dialog (spec), `--confirm-create` flag in the probe, `BT_WRITE_ONLY_CHAT` refusal (D8), live runs only with approval.
- [A duplicate chat after a lost reply] → no automatic retry, outcome classification (D3), existing private chat opened instead of created (D5); group duplicates remain possible and are left to the user, who is told the result is unknown.
- [Parameters of the member reads and the creation calls are partly inferred] → shapes recorded from live reads first (1.2), parsers tolerant like `parse_users`, tests on recorded fixtures; the group gate (D4) and the private-chat fallback (D2) take unverifiable parts out of the change instead of shipping them.
- [`get_chat_members` may not work for private chats] → fallback to the partner from the chat row (D1).
- [Large groups: unknown page size] → `partial` marker and bounded loading (D6); the panel never presents a cut list as full.
- [Status text semantics unknown] → shown verbatim only when present (D6); no presence claims.
- [Shared files collide with the three sibling changes] → additive edits, exact-list tests extended, read before editing (D9).

## Migration Plan

Additive. `ChatSummary.partner` is `#[serde(default)]`, so existing cache rows load without a migration; the next chat-list refresh fills it. No schema change. Rollback is reverting the change.

## Open Questions

None that change the specs or the task breakdown. The group-creation payload is handled by the gate in D4 with an explicit fallback; the page size, paging key and status-text shape are filled in by task 1.2 and only adjust parser details.
