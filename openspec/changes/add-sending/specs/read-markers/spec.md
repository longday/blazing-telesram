# Spec Delta

## Purpose

Keeps the server's read position in step with what the user actually saw, so unread counters drop here and in the user's other clients.

## ADDED Requirements

### Requirement: Mark what the user saw
While the window is focused, the client SHALL report the newest message of the open chat that is visible in the timeline as read, at most once per newer message. It SHALL NOT report messages the user has not scrolled to, and SHALL report nothing while the window is unfocused.

#### Scenario: Open an unread chat at the bottom
- **GIVEN** a chat with unread messages
- **WHEN** the user opens it in the focused window and its newest message is visible
- **THEN** the chat becomes read here and in the web client

#### Scenario: Unfocused window
- **WHEN** a new message arrives in the open chat while the window is not focused
- **THEN** the chat stays unread until the window is focused with that message visible

#### Scenario: Scrolled up
- **GIVEN** the user is reading older messages and newer unread ones are below the visible area
- **WHEN** they stay there
- **THEN** the unread messages below are not reported as read

### Requirement: Unread counter follows the read position
When the client reports the newest message of a chat as read, the chat's unread counter SHALL drop to zero at once; read positions set in other clients SHALL update the counter live.

#### Scenario: Read elsewhere
- **WHEN** the user reads a chat in the web client
- **THEN** its unread counter here drops without a restart

### Requirement: Reading history alone changes nothing
Loading the chat list or history, opening a chat in an unfocused window, or paging through older messages SHALL NOT change the server's read position.

#### Scenario: Background loading
- **WHEN** the client loads chats and history at startup without the user opening a chat
- **THEN** every chat keeps its unread state in the web client
