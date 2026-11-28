# Spec Delta

## MODIFIED Requirements

### Requirement: Reply
The user SHALL be able to reply to a message of the open chat. A reply bar above the composer SHALL show the quoted author and text until the message is sent or the user cancels with Escape or the bar's close button. A reply to a selected fragment («Цитировать», message-quotes) SHALL show the fragment in the bar and send it as the quote.

#### Scenario: Reply to a message
- **WHEN** the user chooses Reply on a message and sends text
- **THEN** the sent message shows a quote of that message

#### Scenario: Reply to a fragment
- **WHEN** the user chooses «Цитировать» on a selected fragment and sends text
- **THEN** the sent message shows that fragment as its quote

#### Scenario: Cancel reply
- **WHEN** the reply bar is shown and the user presses Escape
- **THEN** the bar disappears and the next message is sent without a quote

### Requirement: Drafts
Unsent composer text and a pending reply target, including a quoted fragment, SHALL be kept per chat across chat switches and app restarts, and the chat row SHALL show a draft preview instead of the last message while a draft exists.

#### Scenario: Switch chats
- **WHEN** the user types in one chat, opens another and returns
- **THEN** the typed text and reply target are restored

#### Scenario: Fragment kept in the draft
- **WHEN** the user quotes a fragment, types, opens another chat and returns
- **THEN** the typed text and the same fragment are restored

#### Scenario: Draft in the chat list
- **WHEN** a chat has a draft
- **THEN** its row shows "Черновик:" and the draft text

### Requirement: Writes only from user actions
The client SHALL change server state only as a direct result of the user's actions defined by the sending, editing, read-marker, typing, message-reactions and message-selection capabilities. It SHALL NOT send presence heartbeats or status changes. Selecting text, quoting a fragment and marking messages SHALL send nothing by themselves.

#### Scenario: Idle client
- **WHEN** the client runs without user input
- **THEN** no request that changes server state is sent

#### Scenario: Server rejects writes to a chat
- **WHEN** the server refuses a message because the user may not write to that chat
- **THEN** the message is shown as failed with the reason, and no further automatic retries happen

#### Scenario: Reactions come from the user only
- **WHEN** the client receives reaction updates, reloads history or reconnects
- **THEN** it sends no reaction of its own

#### Scenario: Selecting sends nothing
- **WHEN** the user selects text, quotes a fragment and cancels, or marks messages without choosing an action
- **THEN** no request that changes server state is sent
