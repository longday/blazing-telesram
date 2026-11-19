# Spec Delta

## Purpose

Keeps the chat list and open chat current while the app runs by applying server events in real time and recovering from network loss.

## ADDED Requirements

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
