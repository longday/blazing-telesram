# live-updates Specification

## Purpose
Keeps the chat list and open chat current while the app runs by applying server events in real time and recovering from network loss.

## Requirements

### Requirement: Incoming messages appear live
While connected, a new message in any chat SHALL appear without user action: in the open chat's timeline and in that chat's row preview, time, unread counter and position.

#### Scenario: Message in the open chat
- **WHEN** another user sends a message to the open chat
- **THEN** it appears at the bottom of the timeline, and the view follows it only if it was already at the bottom

#### Scenario: Message in another chat
- **WHEN** a message arrives in a chat that is not open
- **THEN** that chat's row updates its preview and unread counter and moves by the ordering rule

### Requirement: Edits and deletions apply live
Edits and deletions of loaded messages SHALL update them in place while connected.

#### Scenario: Message edited elsewhere
- **WHEN** a loaded message is edited from another client
- **THEN** its new text and edited marker appear

#### Scenario: Message deleted elsewhere
- **WHEN** a loaded message is deleted from another client
- **THEN** it is replaced by the deleted placeholder

### Requirement: Connection status
The client SHALL show when it is connecting or offline, and hide the indicator once connected.

#### Scenario: Network lost
- **WHEN** the connection drops
- **THEN** the window shows a connecting state

### Requirement: Reconnection and catch-up
After the connection drops the client SHALL reconnect automatically with backoff, then reload the chat list and the open chat's newest chunk so missed messages appear.

#### Scenario: Network restored
- **GIVEN** messages arrived while the client was offline
- **WHEN** the network returns
- **THEN** the client reconnects and shows those messages without a restart

### Requirement: Reactions apply live
Reaction changes of loaded messages SHALL update their chips in place while connected, whether another user or one of the user's other clients made them. An update SHALL NOT scroll the timeline, SHALL NOT change the chat row's preview, time, unread counter or position, and SHALL NOT clear the author's typing indicator.

#### Scenario: Reaction from another user
- **WHEN** another user reacts to a loaded message
- **THEN** its chips update without user action and the view does not scroll

#### Scenario: Own reaction from another device
- **WHEN** the user reacts to a loaded message from another client
- **THEN** the chips update and the user's own chip is highlighted

#### Scenario: Chat row unchanged
- **WHEN** a reaction update arrives for a message in a chat that is not open
- **THEN** that chat's row keeps its preview, unread counter and position

#### Scenario: Typing indicator stays
- **GIVEN** the author of a message is shown as typing
- **WHEN** someone reacts to one of that author's messages
- **THEN** the typing indicator stays

### Requirement: Pin and importance changes apply live
Changes of a chat's pinned message and of a loaded message's importance SHALL be applied while connected as `message-pins` and `message-importance` require. Neither SHALL move the timeline's scroll position, change a chat row's preview, unread counter or position, or clear a typing indicator, and a pin or importance event for a message the client has not loaded SHALL cause no error.

#### Scenario: Pin change in the open chat
- **WHEN** another user pins a message in the open chat
- **THEN** the bar updates without the timeline scrolling

#### Scenario: Importance change of an unloaded message
- **WHEN** an importance change arrives for a message that is neither loaded nor cached
- **THEN** nothing is shown for it and no error appears

#### Scenario: Typing indicator stays
- **GIVEN** a user is shown as typing
- **WHEN** a pin or importance change arrives in that chat
- **THEN** the typing indicator stays
