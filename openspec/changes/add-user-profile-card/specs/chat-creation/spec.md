# Spec Delta

## MODIFIED Requirements

### Requirement: Participants come from known people
The dialog SHALL offer only people the client already knows: the other participants of the account's private chats and the members of the group whose info panel is open (private chats and channels have no readable member list). It SHALL NOT offer the current user. A private chat started with «Написать» from a member row or a user card SHALL open with that person already chosen, even when the person is in none of those lists, because the user was just shown that person by name.

#### Scenario: Pick participants
- **WHEN** the user opens the new chat dialog
- **THEN** the choice lists names of known people, excluding the current user

#### Scenario: Person from a user card
- **GIVEN** the user opened the user card of a person who has no private chat with the account and is in no list of the dialog (the info panel is closed)
- **WHEN** the user chooses «Написать» on the card
- **THEN** the dialog opens with that person chosen, named as on the card, and the confirmation lists exactly that person

#### Scenario: The current user is never chosen
- **WHEN** the dialog is started with the current user as the person
- **THEN** no person is chosen and nothing can be created
