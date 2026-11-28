# Spec Delta

## ADDED Requirements

### Requirement: New chat entry point
The sidebar SHALL offer a "Новый чат" action next to the search field that opens the new chat dialog of `chat-creation`. The action SHALL be available only while the client is online.

#### Scenario: Open the dialog
- **WHEN** the user presses "Новый чат" while online
- **THEN** the new chat dialog opens and the chat list is unchanged

#### Scenario: Offline
- **GIVEN** the client is offline
- **WHEN** the user looks at the sidebar
- **THEN** "Новый чат" is disabled

### Requirement: Created chat appears in the list
A chat created by the user SHALL appear in the chat list through the normal list refresh, ordered like any other chat, and SHALL be opened for the user.

#### Scenario: Created chat shows up
- **WHEN** a creation succeeds
- **THEN** the new chat is listed, ordered by its last activity, and the timeline of the new chat is shown
