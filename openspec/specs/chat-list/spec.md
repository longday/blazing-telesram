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
The client SHALL filter the loaded list by name as the user types and by kind (all, private, groups, channels, unread) without server requests; this filtering SHALL work offline and SHALL NOT depend on, or wait for, server search results (see `message-search`).

#### Scenario: Search by name
- **WHEN** the user types part of a chat name
- **THEN** only chats whose name contains it, case-insensitively, remain visible

#### Scenario: Unread filter
- **WHEN** the user selects the unread filter
- **THEN** only chats with unread messages remain visible

#### Scenario: Server search unavailable
- **GIVEN** the client is offline or the server search failed
- **WHEN** the user types part of a chat name
- **THEN** the local name matches are still shown immediately

### Requirement: Server results below the list
For a non-empty query the sidebar SHALL show the local matches first, unchanged in content and order, and the server search results of `message-search` below them; the kind filter SHALL narrow only the local matches.

#### Scenario: Local matches stay on top
- **WHEN** the user types a query that matches a chat name and the server returns message results
- **THEN** the matching chat rows appear above the server result sections

#### Scenario: Kind filter with server results
- **GIVEN** the groups filter is selected
- **WHEN** server results are shown for a query
- **THEN** only group chats remain among the local matches and the server sections are not narrowed

#### Scenario: Empty query
- **WHEN** the search field is empty
- **THEN** the sidebar shows the filtered chat list only, with no server result sections

### Requirement: New chat entry point
The sidebar SHALL offer a "Новый чат" action next to the search field that opens the new chat dialog of `chat-creation`. The action SHALL be available only while the client is online.

#### Scenario: Open the dialog
- **WHEN** the user presses "Новый чат" while online
- **THEN** the new chat dialog opens and the chat list is unchanged

#### Scenario: Offline
- **GIVEN** the client is offline
- **WHEN** the user looks at the sidebar
- **THEN** "Новый чат" is disabled

### Requirement: Created chat appears in the list
A chat created by the user SHALL appear in the chat list through the normal list refresh, ordered like any other chat, and SHALL be opened for the user.

#### Scenario: Created chat shows up
- **WHEN** a creation succeeds
- **THEN** the new chat is listed, ordered by its last activity, and the timeline of the new chat is shown
