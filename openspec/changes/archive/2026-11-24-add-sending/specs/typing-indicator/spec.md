# Spec Delta

## Purpose

Shows chat members that someone is writing: the user's typing is reported to the chat, and other people's typing is shown in the chat header and the chat list.

## ADDED Requirements

### Requirement: Report own typing
While the user changes the composer text of a new message in a chat (not while editing a sent one), the client SHALL report typing in that chat, repeating no more often than the server's typing lifetime requires, and SHALL stop reporting once the user stops typing.

#### Scenario: User types
- **WHEN** the user types in the composer
- **THEN** other members of the chat see that the user is typing

#### Scenario: User stops
- **WHEN** the user stops typing
- **THEN** no further typing reports are sent and the indicator disappears for others when its lifetime ends

### Requirement: Show others typing
When another member types, the open chat's header SHALL replace its subtitle with "<name> печатает…" (or "печатает…" in a private chat), and that chat's row SHALL show the same text instead of the last message, until the server's typing lifetime ends or their message arrives.

#### Scenario: Partner types in the open chat
- **WHEN** the partner of the open private chat types
- **THEN** the header shows "печатает…"

#### Scenario: Message arrives
- **WHEN** a member who was typing sends their message
- **THEN** the typing text disappears from the header and the chat row
