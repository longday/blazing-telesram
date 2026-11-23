# chat-list Specification

## Purpose
Shows the signed-in account's chats in a Telegram Desktop style sidebar so the user can find and open any chat.

## Requirements

### Requirement: Real chat list
After sign-in the client SHALL list every chat the account sees in the Telemost web client, excluding namespaces the web client hides from its chat list.

#### Scenario: Chats load after sign-in
- **WHEN** the chat window opens with a valid session
- **THEN** the sidebar lists the account's private chats, groups and channels

#### Scenario: Empty account
- **WHEN** the account has no chats
- **THEN** the sidebar shows an empty state instead of a list

### Requirement: Chat row content
Each chat row SHALL show the avatar (or initials when no avatar exists), the chat name, the last message preview with its time, the unread counter, and pinned and muted markers.

#### Scenario: Chat with unread messages
- **WHEN** a chat has unread messages
- **THEN** its row shows the unread count

#### Scenario: Chat without avatar
- **WHEN** a chat or user has no avatar
- **THEN** its row shows initials on a colored circle

#### Scenario: Muted chat
- **WHEN** a chat is muted
- **THEN** its row shows the muted marker and a muted-style unread counter

### Requirement: Chat ordering
Pinned chats SHALL appear first in pin order; all other chats SHALL follow ordered by last activity, newest first.

#### Scenario: New activity reorders chats
- **WHEN** an unpinned chat receives a newer message than the chat above it
- **THEN** it moves above that chat

### Requirement: Local filtering
The client SHALL filter the loaded list by name as the user types and by kind (all, private, groups, channels, unread) without server requests.

#### Scenario: Search by name
- **WHEN** the user types part of a chat name
- **THEN** only chats whose name contains it, case-insensitively, remain visible

#### Scenario: Unread filter
- **WHEN** the user selects the unread filter
- **THEN** only chats with unread messages remain visible
