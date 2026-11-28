# Spec Delta

## MODIFIED Requirements

### Requirement: Chat info panel
The client SHALL show a chat info panel for the open chat, opened from the chat header (except by the avatar of a private chat with a named partner, which opens that partner's user card) and closed by the user, that lists the chat's participants. For a group chat the panel SHALL list the users of the server's member list. Each user row SHALL show the avatar (or initials) and the name; clicking either SHALL open that person's user card.

#### Scenario: Open the panel of a group
- **WHEN** the user opens the info panel of a group chat
- **THEN** the panel lists the group's users

#### Scenario: Close the panel
- **WHEN** the user closes the panel or opens another chat
- **THEN** the panel disappears and the chat timeline keeps its scroll position

#### Scenario: Row opens the card
- **WHEN** the user clicks the avatar or the name of a user row
- **THEN** the user card of that person opens (`user-card`) and the panel stays as it was

#### Scenario: Private chat header
- **WHEN** the user clicks the avatar in the header of a private chat whose chat list entry names a partner
- **THEN** the partner's user card opens and the panel does not toggle; a click on the rest of the header opens or closes the panel as before
