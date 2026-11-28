# Proposal

## Why

The client can read and (after `add-sending`) write messages, but the user cannot see who is in a chat, cannot see their own profile, and cannot start a new conversation without the web client. This is roadmap item 5 (chat management), slice (a) of three: read-only knowledge of members and the current profile, plus creating chats. Everything that removes people, changes roles or revokes links is deliberately left for later slices, because the protocol contract for those is only partly known and the effects on other people are not reversible.

Context document: `research.md` in this directory (verbatim research report with sources and verification marks). Read it before designing; `design.md` holds the technical decisions and the observed-shapes section that the probe tasks fill in.

## What Changes

- Read the member list of a group chat (HTTP Registry method `get_chat_members`, paged with `limit` / `guid_offset`; users with their `admin` or `member` role) and show it in a read-only chat info panel opened from the chat header. The server refuses the list for private chats, the self chat and (for this account) channels with HTTP 403 `no_permissions_for_this_chat_operation`, so the panel of a private chat shows the partner known from the chat list entry and a refused list is shown as not available to the account. `get_group_members` is not used: it answers `bad_request` for every shape tried (task 1.2). The reply also carries `groups` and `departments` arrays; they were empty in all 70 groups of the account and their element shape is unknown, so showing them is out of scope (see Non-goals).
- Read the current user's profile from the socket `whoami` call (`UserInfo`, `UserStatusInfo`). The client does not send `whoami` yet (only its frame encoding exists in `crates/yandex-proto/src/frame.rs`), so it becomes a new socket read; `get_current_user_data` is NOT used (server answers `No such path`). Shown read-only (name, avatar, own status text as the server reports it; `UserStatus` is `{}` when no status is set).
- Create a private chat with a user (`create_private_chat`: `guid`, `wait_for_update`, `onetime_user_token`) and, subject to the gate in decision 1 below, a group chat (`create_chat`: parameters currently opaque), each from an explicit user action ("New chat" / "Write" on a member) that shows the chosen participants and title in a confirmation dialog before anything is sent.
- Protocol layer: members/profile reads join the closed read allowlist (`whoami`, `get_chat_members`); creation becomes the first Registry (HTTP, CSRF) mutations, as a closed, separately tested set in a new module that contains no other management method.
- Chat-core/domain: new member and profile models, new backend commands for reading members and creating chats; the created chat appears in the chat list through the existing chat-list refresh path.

## Capabilities

### New Capabilities
- `chat-members`: read-only view of a chat's participants (users and the admin role) and of the current user's profile.
- `chat-creation`: creating private and group chats from an explicit user action, with confirmation of participants/title and safe failure handling.

### Modified Capabilities
- `chat-list`: a "New chat" entry point and the appearance of a freshly created chat in the list.
- `message-sending`: the requirement "Writes only from user actions" names the capabilities allowed to change server state; chat creation is added to that list so the rule stays true.

## Impact

- `crates/yandex-proto`: `request.rs` (allowlist: `whoami` and `get_chat_members` with `limit` / `guid_offset` / `roles` parameters; `get_group_members` is not added), new `manage.rs` (closed set of Registry mutations), `registry.rs` (CSRF-guarded mutation call with sent/not-sent outcome), `wire.rs` (member/profile parsers), `client.rs` (facade); tests asserting that only the listed methods were added. The existing tests in `request.rs` and `write.rs` pin `ALLOWED_METHODS` to an exact list and are updated to the new exact list.
- `crates/chat-core`: `model.rs` (member, role, profile, creation types; `ChatSummary.partner`), `backend.rs` (read-members, read-profile, create commands), `client.rs` (engine handling).
- `crates/desktop`: `yandex_backend.rs` implementation, `store.rs` state, chat info panel, profile view, "New chat" flow with confirmation.
- `bt-probe`: read-members/`whoami` probe; guarded create-chat probes for payload discovery.
- Dependency on `add-sending` is satisfied: it is archived (`openspec/changes/archive/2026-11-24-add-sending/`) and its closed-write-set conventions (`write.rs`, `BT_WRITE_ONLY_CHAT` guard in `yandex_backend.rs`) are the base this change follows. Sibling changes in flight (`add-message-reactions`, `add-media-download`, `add-global-search`) may also extend `ALLOWED_METHODS`; whoever lands second extends the exact-list tests instead of replacing them.
- Risk: `create_chat` / `create_private_chat` change the real account and may create duplicates on retry (no known idempotency key); the protocol is unofficial. Live verification limited to chats created with the user's explicit approval. The read shapes are observed live (task 1.2, `design.md`), except the populated `UserStatus`, which no account chat produced; the parser accepts only a non-blank `text` there.
- Residual risk accepted at close (user decision, 2026-11-28): private chat creation (`create_private_chat`) was run live only against a person who already had a private chat, so the call returned that chat and created nothing. Not verified live: the reply shape of a really created chat, the effect of `wait_for_update: true` on a new chat, and whether `onetime_user_token` is required then. The implementation, its unit and fixture tests and the "Private chat" requirement stay; every failure path is safe (no retry, list refresh, no duplicate). See the notes of tasks 5.1 and 5.3.

## Non-goals

- Changing membership, roles, admins, leaving chats (`change_chat_members`, `change_chat_role`, `change_chat_admins`, `leave`) — slice (b).
- Invite links (`invite`, `invite_renew`) and `invite_hash` handling — slice (b).
- Editing chat info (`set_chat_info`), editing the profile, setting statuses/availability — slice (c).
- Searching the directory for people (`search`, `get_members_and_contacts_suggest`): participants are chosen only among people the client already knows (see decision 7).
- Showing the `groups` and `departments` entries of a member list: always empty in all 70 groups of the account, element shape unknown; the parser accepts and ignores them.
- Any mutation without an explicit user action; any live mutation on the real account without user approval.

## Follow-ups

- (b) Membership management: add/remove members, change roles/admins, leave, invites and link renewal; every destructive step confirmed; `version` conflicts re-read state instead of blind retry.
- (c) Profile and status changes: only after the semantics (status message vs availability vs online presence), rights and side effects on other devices are decided and probed.
- Directory search as a participant source for "New chat" (reuse the `search` read of `add-global-search` once it lands).
- Group creation, if the gate in decision 1 fails: a separate change `add-group-chat-creation`.
- `add-private-chat-creation-live-check`: run `bt-probe create-private <guid> --confirm-create` and the app flow once against a person with no private chat (needs the user's approval of the target); record the created chat's reply shape, the `wait_for_update` behaviour and whether `onetime_user_token` is needed; fix the parser or move private creation out if the server refuses without the token.
- Groups and departments in the member list: once a non-empty example is recorded (live reply or web client code), add their display with a spec delta.

## Decisions on the Open Questions

All six open questions were resolved by taking the recommended answer (autonomous run, no user input requested); two more questions that surfaced while grounding the plan in the code were resolved the same way. `design.md` details the mechanics.

1. **Group creation (`create_chat`) in this change?** Decision: members/`whoami` reads and private chat creation come first; `create_chat` is included only after a guarded `bt-probe` run fixes its payload and result shape. Gates: task 0.1 reads the web client's code (read-only, no account effects) and records the payload in `design.md` (D4, static PASS); task 6.1 then adds only the typed request and the guarded probe command; task 6.2 runs that probe once with the user's approval and records the confirmed payload and reply shape (live PASS); only then do 6.3 (backend, engine) and 6.4 (UI) add group creation to the app. If the payload cannot be determined, or the live run fails for a reason the client cannot fix, group creation moves to a follow-up: task 6.F deletes the group requirement from the `chat-creation` spec and any group code, and this proposal's Follow-ups name `add-group-chat-creation`. Why: an opaque payload implemented blindly can create wrong or duplicate chats visible to other people; delaying groups costs little.
2. **Retry policy after a network failure.** Decision: never retry automatically and never queue creation (unlike messages there is no outbox). A failure after the request left the client is reported as "unknown result", the chat list is refreshed, and only an explicit user action sends the request again; a private chat that already exists after the refresh is opened instead of created. Why: no idempotency key is known, so a blind retry can duplicate a chat.
3. **What counts as profile/status on the read side?** Decision: show exactly what `whoami` returns (`UserInfo` name and avatar, `UserStatusInfo` as text when it carries one) as read-only; no interpretation of availability or presence, no edit controls. `UserStatus` is an empty object when no status is set (live, task 1.2): that means no status line. Why: the semantics belong to slice (c); a wrong claim about presence is worse than a plain view.
4. **Confirmation UX for creation.** Decision: one dialog listing title (groups) and participants with an explicit "Создать" button and "Отмена"; Enter inside a search or title field never creates; the button is disabled while a creation is in flight. Why: accidental chats are visible to other people and cannot be undone by this client.
5. **Live verification scope.** Decision: reads against the real account are allowed; create calls run only through `bt-probe` commands that refuse to send without an explicit confirmation flag, and only after the user approves the target (a throwaway chat with the user or a test colleague). Parsing is covered by recorded-fixture tests. Why: no side effects on other people; weaker automated coverage of the creation path is accepted. The tasks that need approval are marked and keep the change from being archived until done.
6. **Modified `chat-list` spec?** Decision: yes, a minimal delta: the "New chat" entry point and the newly created chat appearing in the list. Why: the entry point is part of the sidebar; the creation behaviour itself stays in `chat-creation`.
7. **Where do participants come from?** (surfaced during design) Decision: only people the client already knows: partners of existing private chats (new `ChatSummary.partner`) and the members of the group whose info panel is open (private chats and channels have no readable member list); a "Write" action on a member row starts a private chat directly. Directory search is a follow-up. Why: it needs `search`-class reads that belong to `add-global-search`; the closed allowlist grows only by what this change needs.
8. **Are members cached offline?** (surfaced during design) Decision: no; members and the profile are fetched on demand, and offline the panel says so. Why: the offline-cache spec is untouched, member lists change on the server and a stale list presented as current is misleading.
