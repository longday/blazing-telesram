# Spec Delta

## Purpose

Shows the number of unread messages on the app's Dock icon, so the user sees from outside the window that something is waiting, without changing anything on the server.

## ADDED Requirements

### Requirement: Unread count on the Dock icon
The Dock icon SHALL carry a badge with the total of the unread counters of all chats that are not muted, as the chat list shows them. No badge SHALL be shown when that total is zero. A total above 999 SHALL read «999+». The counters of muted chats SHALL NOT be added.

#### Scenario: Unread messages
- **GIVEN** two chats that are not muted with 3 and 4 unread messages
- **WHEN** the chat list is shown
- **THEN** the Dock badge reads 7

#### Scenario: Muted chat is not counted
- **GIVEN** a chat with 120 unread messages that is muted, and a chat with 2 that is not
- **WHEN** the chat list is shown
- **THEN** the Dock badge reads 2

#### Scenario: Nothing unread
- **WHEN** every chat that is not muted has zero unread messages
- **THEN** the Dock icon shows no badge

#### Scenario: Very large count
- **GIVEN** unmuted chats whose unread counters add up to 1500
- **WHEN** the chat list is shown
- **THEN** the Dock badge reads «999+»

### Requirement: Badge follows the unread state
The badge SHALL change whenever the total changes, without user action and without a request of its own: when a message arrives, when the user reads a chat here, when a chat is read in another client, when a chat is muted or unmuted in a list refresh, and when a reconnect reloads the chat list. A change that leaves the total as it was SHALL NOT touch the badge.

#### Scenario: Message in another chat
- **GIVEN** the badge reads 2
- **WHEN** a message arrives in a chat that is not muted and is not the open chat
- **THEN** the badge reads 3

#### Scenario: Read here
- **GIVEN** the badge reads 3 and one chat holds 3 unread messages
- **WHEN** the user opens that chat in the focused window and reads it
- **THEN** the badge disappears

#### Scenario: Read elsewhere
- **GIVEN** the badge reads 5
- **WHEN** the user reads a chat with 2 unread messages in the web client
- **THEN** the badge reads 3 without a restart

#### Scenario: Messages that arrived while offline
- **GIVEN** the badge reads 0 and messages arrived while the client was offline
- **WHEN** the connection returns and the chat list is reloaded
- **THEN** the badge reads the number of those unread messages in chats that are not muted

#### Scenario: Message in a muted chat
- **GIVEN** the badge reads 2
- **WHEN** a message arrives in a muted chat
- **THEN** the badge still reads 2

### Requirement: Badge ends with the session
The badge SHALL be cleared when the session expires, when the user signs out, when the account behind the session changes, and when the chat window closes. The badge of one account SHALL NOT be shown for another.

#### Scenario: Sign out
- **GIVEN** the badge reads 7
- **WHEN** the user signs out
- **THEN** the Dock icon shows no badge while the sign-in window is open

#### Scenario: Session expired
- **GIVEN** the badge reads 7
- **WHEN** the server rejects the session
- **THEN** the Dock icon shows no badge

#### Scenario: Quit
- **GIVEN** the badge reads 7
- **WHEN** the user quits the app
- **THEN** no badge remains on the Dock icon

### Requirement: Badge changes nothing on the server
Computing or showing the badge SHALL NOT send anything to the server and SHALL NOT mark anything as read. It SHALL use the chat list the client already holds.

#### Scenario: No request
- **WHEN** the badge changes
- **THEN** no request is made because of it
