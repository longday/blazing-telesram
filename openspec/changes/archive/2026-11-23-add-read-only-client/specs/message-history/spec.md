# Spec Delta

## Purpose

Lets the user read a chat's real messages in a Telegram Desktop style timeline, scrolling back through older history, without changing anything on the server.

## ADDED Requirements

### Requirement: Open a chat
Selecting a chat SHALL show its newest messages, scrolled to the bottom, in the web client's history chunk size.

#### Scenario: Open chat
- **WHEN** the user selects a chat
- **THEN** the timeline shows its latest messages with the newest at the bottom

### Requirement: Load older history
Scrolling to the top of the loaded messages SHALL load the previous chunk and keep the message the user was looking at in place.

#### Scenario: Scroll to the top
- **WHEN** the user reaches the oldest loaded message and older messages exist
- **THEN** the previous chunk appears above it without moving the visible message

#### Scenario: Start of history
- **WHEN** the user reaches the first message of the chat
- **THEN** no further loading happens and no loading indicator remains

### Requirement: Message content
Each message SHALL show its author (in groups), text as received with line breaks kept, send time, edited marker, reply quote and forwarded-from line; deleted messages SHALL show a deleted placeholder.

#### Scenario: Group message
- **WHEN** a group message from another user is shown
- **THEN** it shows the author's name and avatar

#### Scenario: Own message
- **WHEN** a message was sent by the signed-in account
- **THEN** it is shown on the right in the outgoing bubble style

#### Scenario: Unsupported message type
- **WHEN** a message has a type the client does not render
- **THEN** it shows a placeholder naming the type instead of being hidden

### Requirement: Attachments preview
Image attachments SHALL show a thumbnail; other files SHALL show a card with name and size. Opening or downloading files is out of scope.

#### Scenario: Image message
- **WHEN** a message contains an image
- **THEN** a thumbnail is shown at the image's aspect ratio

#### Scenario: File message
- **WHEN** a message contains a non-image file
- **THEN** a card with the file name and size is shown

### Requirement: Read-only client
The client SHALL NOT send any request that changes server state: no messages, edits, deletions, reactions, read markers, typing or presence events. Message composers SHALL be disabled with a read-only notice.

#### Scenario: Opening a chat leaves it unread
- **GIVEN** a chat with unread messages
- **WHEN** the user opens it and scrolls through it
- **THEN** the web client still shows that chat as unread

#### Scenario: Composer is disabled
- **WHEN** a chat is open
- **THEN** the composer cannot accept input and states that sending is not available yet
