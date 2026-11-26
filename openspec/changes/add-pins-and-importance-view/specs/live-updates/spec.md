# Spec Delta

## ADDED Requirements

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
