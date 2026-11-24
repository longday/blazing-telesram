# chat-creation Specification

## Purpose

Lets the user start a private chat or a group chat from an explicit action, showing exactly who will be in it first, and handles failures without creating duplicate chats.

## ADDED Requirements

### Requirement: Explicit confirmation before creating
The client SHALL create a chat only after the user confirms a dialog that lists the chat title (groups) and every participant and has an explicit "Создать" button. Pressing Enter in a search or title field SHALL NOT create a chat.

#### Scenario: Confirm creation
- **WHEN** the user reviews the dialog and presses "Создать"
- **THEN** exactly one create request is sent

#### Scenario: Cancel
- **WHEN** the user presses "Отмена" or Escape in the dialog
- **THEN** nothing is sent and no chat is created

#### Scenario: Enter in a field
- **WHEN** the user presses Enter in the search or title field of the dialog
- **THEN** no create request is sent

### Requirement: Private chat
The user SHALL be able to start a private chat with a person the client knows. If the account already has a private chat with that person, the client SHALL open it instead of creating one.

#### Scenario: New private chat
- **WHEN** the user confirms a private chat with a person who has no chat with the account
- **THEN** a private chat is created and opened once it appears in the chat list

#### Scenario: Chat already exists
- **WHEN** the user chooses a person who already has a private chat with the account
- **THEN** that chat is opened and no create request is sent

### Requirement: Participants come from known people
The dialog SHALL offer only people the client already knows: the other participants of the account's private chats and the members of the group whose info panel is open (private chats and channels have no readable member list). It SHALL NOT offer the current user.

#### Scenario: Pick participants
- **WHEN** the user opens the new chat dialog
- **THEN** the choice lists names of known people, excluding the current user

### Requirement: Group chat
The user SHALL be able to create a group chat with a non-empty title and at least one other participant. "Создать" SHALL stay disabled until both are given.

#### Scenario: Valid group
- **WHEN** the user enters a title, selects participants and confirms
- **THEN** a group chat with that title and those participants is created and opened once it appears in the chat list

#### Scenario: Missing title or participants
- **WHEN** the title is blank or no participant is selected
- **THEN** "Создать" is disabled and nothing can be sent

### Requirement: No duplicate creation
While a creation is in flight the client SHALL ignore further creation requests. It SHALL NOT repeat a creation automatically and SHALL NOT queue creations while offline.

#### Scenario: Double click
- **WHEN** the user presses "Создать" twice quickly
- **THEN** only one create request is sent

#### Scenario: Offline
- **GIVEN** the client is offline
- **WHEN** the user confirms a creation
- **THEN** nothing is sent, the dialog says the connection is missing and keeps the entered data

### Requirement: Unknown result is reported honestly
When the connection fails after the create request was sent, the client SHALL say the result is unknown, refresh the chat list and let the user decide; it SHALL NOT retry by itself. Before another explicit attempt at a private chat, an existing chat with that person SHALL be opened instead.

#### Scenario: Connection lost after sending
- **WHEN** the create request was sent and no reply arrived
- **THEN** the dialog says the result is unknown, the chat list is refreshed and no second request is sent

#### Scenario: The chat appeared after all
- **WHEN** the refreshed list contains the private chat the user wanted
- **THEN** the client opens it instead of creating another

### Requirement: Rejection is shown
When the server rejects a creation, the client SHALL show the reason, keep the entered data, and SHALL NOT retry by itself.

#### Scenario: Server refuses
- **WHEN** the server refuses to create the chat
- **THEN** the dialog shows the reason and stays open with its data

#### Scenario: Session expired
- **WHEN** the server rejects the session during creation
- **THEN** the client handles it as the session expiry of `account-session`

### Requirement: Creation is the only chat management write
The client SHALL send no request that changes chat membership, roles, invites, chat information or the profile; creating a chat is the only chat-management write it can make.

#### Scenario: Idle client
- **WHEN** the client runs without user input
- **THEN** no create or other management request is sent

#### Scenario: Developer guard
- **GIVEN** the developer limited writes to one chat with `BT_WRITE_ONLY_CHAT`
- **WHEN** the user confirms a creation
- **THEN** the client refuses it without a request and says why
