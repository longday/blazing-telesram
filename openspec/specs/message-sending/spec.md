# message-sending Specification

## Purpose

Lets the user write text messages and replies in any chat that accepts them, with visible delivery state, offline queueing and drafts.

## Requirements

### Requirement: Composer
An open chat SHALL show a multi-line composer that wraps text and grows with it up to a maximum height, then scrolls. Enter SHALL send; Shift+Enter SHALL insert a line break. Text that is empty or only whitespace SHALL NOT be sent.

#### Scenario: Send with Enter
- **WHEN** the user types text in the composer and presses Enter
- **THEN** the message is sent and the composer becomes empty

#### Scenario: Line break
- **WHEN** the user presses Shift+Enter
- **THEN** a line break is inserted and nothing is sent

#### Scenario: Blank text
- **WHEN** the composer holds only spaces or line breaks and the user presses Enter
- **THEN** nothing is sent

### Requirement: Delivery states
A sent message SHALL appear at the bottom of the timeline at once as pending, become sent when the server confirms it, and become failed with a retry action when the server rejects it. The server's copy of the message SHALL replace the pending one without a duplicate.

#### Scenario: Confirmed send
- **WHEN** the user sends a message and the server accepts it
- **THEN** the bubble shows the sent mark and exactly one copy of the message stays in the timeline

#### Scenario: Rejected send
- **WHEN** the server rejects the message
- **THEN** the bubble shows a failed mark with the reason, and Retry sends it again

### Requirement: Offline outbox
Messages sent while offline SHALL stay pending, survive an app restart, and go out in order once the connection returns. Resending SHALL NOT create a second copy on the server.

#### Scenario: Send while offline
- **GIVEN** the client is offline
- **WHEN** the user sends a message and the network later returns
- **THEN** the message is delivered once and becomes sent

#### Scenario: Restart with a pending message
- **GIVEN** a message is pending
- **WHEN** the app is restarted
- **THEN** the message is still shown as pending and is delivered after connecting

### Requirement: Reply
The user SHALL be able to reply to a message of the open chat. A reply bar above the composer SHALL show the quoted author and text until the message is sent or the user cancels with Escape or the bar's close button.

#### Scenario: Reply to a message
- **WHEN** the user chooses Reply on a message and sends text
- **THEN** the sent message shows a quote of that message

#### Scenario: Cancel reply
- **WHEN** the reply bar is shown and the user presses Escape
- **THEN** the bar disappears and the next message is sent without a quote

### Requirement: Drafts
Unsent composer text and a pending reply target SHALL be kept per chat across chat switches and app restarts, and the chat row SHALL show a draft preview instead of the last message while a draft exists.

#### Scenario: Switch chats
- **WHEN** the user types in one chat, opens another and returns
- **THEN** the typed text and reply target are restored

#### Scenario: Draft in the chat list
- **WHEN** a chat has a draft
- **THEN** its row shows "Черновик:" and the draft text

### Requirement: Writes only from user actions
The client SHALL change server state only as a direct result of the user's actions defined by the sending, editing, read-marker, typing and message-reactions capabilities. It SHALL NOT send presence heartbeats or status changes.

#### Scenario: Idle client
- **WHEN** the client runs without user input
- **THEN** no request that changes server state is sent

#### Scenario: Server rejects writes to a chat
- **WHEN** the server refuses a message because the user may not write to that chat
- **THEN** the message is shown as failed with the reason, and no further automatic retries happen

#### Scenario: Reactions come from the user only
- **WHEN** the client receives reaction updates, reloads history or reconnects
- **THEN** it sends no reaction of its own
