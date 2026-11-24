# Spec Delta

## MODIFIED Requirements

### Requirement: Writes only from user actions
The client SHALL change server state only as a direct result of the user's actions defined by the sending, editing, read-marker, typing and chat-creation capabilities. It SHALL NOT send reactions, presence heartbeats or status changes.

#### Scenario: Idle client
- **WHEN** the client runs without user input
- **THEN** no request that changes server state is sent

#### Scenario: Server rejects writes to a chat
- **WHEN** the server refuses a message because the user may not write to that chat
- **THEN** the message is shown as failed with the reason, and no further automatic retries happen
