# Spec Delta

## ADDED Requirements

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
