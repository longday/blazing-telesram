# Spec Delta

## MODIFIED Requirements

### Requirement: Message content
Each message SHALL show its author (in groups), text as received with line breaks kept and selectable with the mouse (text-selection), send time, edited marker, reply quote and forwarded-from line; a reply quote of a fragment SHALL show up to three lines (message-quotes), any other reply quote one line; deleted messages SHALL show a deleted placeholder.

#### Scenario: Group message
- **WHEN** a group message from another user is shown
- **THEN** it shows the author's name and avatar

#### Scenario: Own message
- **WHEN** a message was sent by the signed-in account
- **THEN** it is shown on the right in the outgoing bubble style

#### Scenario: Unsupported message type
- **WHEN** a message has a type the client does not render
- **THEN** it shows a placeholder naming the type instead of being hidden

#### Scenario: Selectable text
- **WHEN** the user drags over the text of a message
- **THEN** the dragged text is highlighted, with the layout and the row height unchanged

#### Scenario: Fragment quote
- **WHEN** a reply quotes a fragment of its original
- **THEN** its quote block shows up to three lines of the fragment
