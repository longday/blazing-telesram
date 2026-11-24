# Proposal

## Why

The client can read and (after `add-sending`) write messages, but the user cannot see who is in a chat, cannot see their own profile, and cannot start a new conversation without the web client. This is roadmap item 5 (chat management), slice (a) of three: read-only knowledge of members and the current profile, plus creating chats. Everything that removes people, changes roles or revokes links is deliberately left for later slices, because the protocol contract for those is only partly known and the effects on other people are not reversible.

Context document: `research.md` in this directory (verbatim research report with sources and verification marks). Read it before designing.

## What Changes

- Read the member list of the open chat: users, groups and departments, with the admin role in group chats (HTTP Registry methods `get_chat_members` / `get_group_members`); shown in a read-only members panel of the chat.
- Read the current user's profile from the existing WebSocket `whoami` answer (`UserInfo`, `UserStatusInfo`); `get_current_user_data` is NOT used (server answers `No such path`). Shown read-only (name, avatar, own status text as the server reports it).
- Create a private chat with a user (`create_private_chat`: `guid`, `wait_for_update`, `onetime_user_token`) and, subject to Open Question 1, a group chat (`create_chat`: parameters currently opaque), each from an explicit user action ("New chat") that shows the chosen participants and title before sending.
- Protocol layer: members/profile reads join the closed read allowlist; creation becomes the first Registry (HTTP, CSRF) mutations, as a closed, separately tested set that does not include any other management method.
- Chat-core/domain: new member and profile models, new backend commands for reading members and creating chats; created chat appears in the chat list through the existing update path.

## Capabilities

### New Capabilities
- `chat-members`: read-only view of a chat's participants (users, groups, departments, admin role) and of the current user's profile.
- `chat-creation`: creating private and group chats from an explicit user action, with confirmation of participants/title and safe failure handling.

### Modified Capabilities
- `chat-list`: a "New chat" entry point and the appearance of a freshly created chat in the list (to be confirmed at the specs stage; drop if the entry point lives outside the list spec).

## Impact

- `crates/yandex-proto`: `request.rs` (allowlist: member reads), `registry.rs` (first mutating HTTP calls, CSRF, response/error parsing), `wire.rs` (member/profile wire models, `whoami` profile fields); tests asserting that only the listed methods were added.
- `crates/chat-core`: `model.rs` (member, role and profile types), `backend.rs` (read-members, create-chat commands).
- `crates/desktop`: `yandex_backend.rs` implementation, members panel, profile view, "New chat" flow with confirmation.
- `bt-probe`: read-members/`whoami` probe; guarded create-chat probe for payload discovery.
- Dependency: shares the mutating-method allowlist convention introduced by `add-sending` (not touched here); this change must land after, or be reconciled with, its closed-set tests.
- Risk: `create_chat` / `create_private_chat` change the real account and may create duplicates on retry (no known idempotency key); the protocol is unofficial. Live verification limited to chats created with the user's explicit approval.

## Non-goals

- Changing membership, roles, admins, leaving chats (`change_chat_members`, `change_chat_role`, `change_chat_admins`, `leave`) — slice (b).
- Invite links (`invite`, `invite_renew`) and `invite_hash` handling — slice (b).
- Editing chat info (`set_chat_info`), editing the profile, setting statuses/availability — slice (c).
- Any mutation without an explicit user action; any live mutation on the real account without user approval.

## Follow-ups

- (b) Membership management: add/remove members, change roles/admins, leave, invites and link renewal; every destructive step confirmed; `version` conflicts re-read state instead of blind retry.
- (c) Profile and status changes: only after the semantics (status message vs availability vs online presence), rights and side effects on other devices are decided and probed.

## Open Questions

1. **Is group creation (`create_chat`) in this change, or only private chats?** Recommended: first implement reading members/`whoami` and private chat creation; include `create_chat` only after a guarded `bt-probe` run fixes its payload and result shape, keeping it in this change if the probe succeeds, else moving it to a follow-up. Trade-off: group creation delays usefulness slightly, but implementing an opaque payload blindly risks wrong or duplicated server-side chats.
2. **Retry policy for creation after a network failure.** Recommended: never auto-retry; show "unknown result", refresh the chat list and let the user retry explicitly. Trade-off: slightly more friction vs. risk of duplicate chats since no idempotency key is known.
3. **What exactly counts as "profile"/"status" on the read side?** Recommended: show only what `whoami` returns (`UserInfo`, `UserStatusInfo`) as read-only text, without interpreting availability; defer semantics to slice (c). Trade-off: profile view may look incomplete, but no wrong claims about presence.
4. **Confirmation UX for creation.** Recommended: a single dialog listing the title and participants with an explicit "Create" button; no creation on Enter in a search field. Trade-off: an extra step vs. accidental chats visible to other people.
5. **Live verification scope.** Recommended: reads against the real account are fine; create calls limited to a throwaway chat with the user (or a test colleague) and only after approval. Trade-off: weaker automated coverage of the creation path vs. no side effects on others; rely on recorded-fixture tests for parsing.
6. **Modified `chat-list` spec or not?** Recommended: yes, minimal delta (entry point plus newly created chat appears). Trade-off: touches an established spec vs. keeping "New chat" fully inside `chat-creation`.
