# Spec Delta

## Purpose

Shows the reactions people put on messages, keeps them current, and lets the user add, change and remove their own reaction on a message.

## ADDED Requirements

### Requirement: Reaction chips
A message that has reactions SHALL show one chip per reaction type below its content, inside the bubble, with the type's emoji and the number of people who chose it, in the order the server reports them. The chip of the user's own reaction SHALL be highlighted. A reaction type the client has no confirmed emoji for SHALL show a neutral glyph with its count. A message without reactions and a deleted message SHALL show no chips.

#### Scenario: Message with reactions
- **WHEN** a message has two people on one type and one person on another
- **THEN** two chips are shown, one with count 2 and one with count 1

#### Scenario: Own reaction
- **WHEN** the user's own reaction is among a message's reactions
- **THEN** its chip is highlighted and the others are not

#### Scenario: Unknown reaction type
- **WHEN** the server reports a reaction type the client cannot map to an emoji
- **THEN** a chip with the neutral glyph and the count is shown instead of hiding the reaction

#### Scenario: Deleted message
- **WHEN** a message with reactions is deleted
- **THEN** it shows the deleted placeholder and no chips

### Requirement: Reaction source
A message SHALL show the reactions the client has been told about: reaction updates received live, reactions kept in the offline cache, and reactions carried by a history row when the server includes them. A message whose reactions the client was never told about SHALL show none. A copy of a message that carries no reaction state (an edit, or a history row without one) SHALL keep the reactions already known.

#### Scenario: Reaction update for a loaded message
- **WHEN** a reaction update arrives for a message of a loaded chat
- **THEN** the message's chips change to the reported reactions

#### Scenario: Edit keeps reactions
- **GIVEN** a message shows reactions
- **WHEN** it is edited and the edit carries no reaction state
- **THEN** the new text appears and the chips stay

#### Scenario: Edit while history loads newer reactions
- **GIVEN** a message shows reactions and a history request for its chat is in flight
- **WHEN** it is edited, the edit carries no reaction state, and the history answer carries newer reactions for it
- **THEN** the new text and the newer reactions are shown

#### Scenario: Update for a message that is not loaded
- **WHEN** a reaction update arrives for a message the client has not loaded or cached
- **THEN** nothing is shown for it and no error appears

### Requirement: One own reaction per message
The user SHALL have at most one reaction on a message. Choosing a type other than the current one SHALL replace the current reaction; choosing the current type SHALL remove it.

#### Scenario: First reaction
- **GIVEN** the user has no reaction on a message
- **WHEN** the user chooses a type
- **THEN** the message shows that type's chip with the user counted and highlighted

#### Scenario: Change reaction
- **GIVEN** the user's reaction on a message is type A
- **WHEN** the user chooses type B
- **THEN** the count of A drops by one, the count of B rises by one, and only B is highlighted

#### Scenario: Remove reaction
- **GIVEN** the user's reaction on a message is type A
- **WHEN** the user chooses type A again
- **THEN** the user is no longer counted on A and the chip disappears if nobody else chose A

### Requirement: Reacting is a user action
The user SHALL be able to react to a sent message that is not deleted: by choosing a type in the reaction row of the message's context menu, or by clicking an existing chip, which chooses that chip's type. The change SHALL appear at once and the server's state SHALL replace it when it arrives. Messages that are pending or failed SHALL NOT accept reactions. The client SHALL send a reaction only as a direct result of such a choice.

#### Scenario: Pick from the menu
- **WHEN** the user right-clicks a sent message and chooses a reaction type in the menu
- **THEN** the menu closes and the chosen chip appears at once

#### Scenario: Click a chip
- **GIVEN** a message shows a chip the user has not chosen
- **WHEN** the user clicks it
- **THEN** the user's reaction becomes that type

#### Scenario: Click the own chip
- **GIVEN** the user's own chip is highlighted
- **WHEN** the user clicks it
- **THEN** the user's reaction is removed

#### Scenario: Pending message
- **WHEN** the user right-clicks a message that is still pending
- **THEN** the menu offers no reaction row

#### Scenario: Second choice while one is unconfirmed
- **GIVEN** a reaction on a message has been sent and not yet confirmed
- **WHEN** the user chooses another type on the same message
- **THEN** the choice is ignored

### Requirement: Offered reaction types
The reaction row of the context menu SHALL offer only the reaction types whose emoji is confirmed, in a fixed order, with the user's own type marked. If no type is confirmed, the row SHALL NOT be shown; chips and their click behaviour stay.

#### Scenario: Own type marked
- **GIVEN** the user's reaction on a message is a type offered in the row
- **WHEN** the user opens the menu
- **THEN** that type is marked in the row

### Requirement: Failed reactions
When the server refuses a reaction, the message SHALL return to the reactions it showed before the choice and the client SHALL show the reason. A reaction chosen while offline SHALL be refused at once with the offline reason and SHALL NOT be queued.

#### Scenario: Rejected reaction
- **WHEN** the server refuses a reaction
- **THEN** the chips return to their previous state and an error with the reason is shown

#### Scenario: Offline
- **GIVEN** the client is offline
- **WHEN** the user chooses a reaction
- **THEN** the chips do not change and an error says there is no connection

### Requirement: Stale reaction updates
A reaction update SHALL replace the message's known reactions only when its version is newer than the known one; an update with an older or equal version SHALL be ignored (an update without a version is covered by "Reaction input without a version"). While a reaction of the user is unconfirmed, an update for the same message SHALL replace the unconfirmed state.

#### Scenario: Older update
- **GIVEN** a message shows reactions of version 5
- **WHEN** an update of version 4 arrives for it
- **THEN** the chips do not change

#### Scenario: Newer update during an own request
- **GIVEN** the user's reaction on a message is unconfirmed
- **WHEN** a newer update for that message arrives
- **THEN** the chips show the update and the unconfirmed state is dropped

### Requirement: History does not undo newer reaction state
A history row SHALL NOT replace known reactions of a newer version. The user's confirmed choice SHALL survive a history row of the same or an older version and SHALL NOT replace a newer state, whichever of the two finishes first.

#### Scenario: History snapshot older than a confirmed choice
- **GIVEN** a history request was started before the user's reaction on a message was confirmed
- **WHEN** its answer, which does not contain the choice, arrives after the confirmation
- **THEN** the chips keep the confirmed choice

#### Scenario: Newer history state during an own request
- **GIVEN** the user's reaction on a message is unconfirmed and history delivers a newer version of its reactions
- **WHEN** the request is confirmed
- **THEN** the chips keep the newer state from history

#### Scenario: Several history requests and an edit
- **GIVEN** two history requests are out for a chat, and the second one has delivered a newer reaction version for a message
- **WHEN** the message is edited and then the first request, taken before both, delivers its older snapshot
- **THEN** the chips keep the newer reaction version and the message shows the edit

#### Scenario: History page above the cache and a reaction update
- **GIVEN** a history request is out whose page lies above every cached message, and a cached message is edited and then receives a newer reaction version while it is out
- **WHEN** the page arrives
- **THEN** the edited message is still shown, with the newer reaction version

#### Scenario: Reaction update while another history request stores newer messages
- **GIVEN** a history request started before the server got a newer message, and another request stored that message and a reaction update reached an older one meanwhile
- **WHEN** the first request arrives, its rows ending below the newer message
- **THEN** the newer message stays shown

### Requirement: Reaction input without a version
Reaction input without a version SHALL NOT replace a state that has one and SHALL NOT give a state a version. While a message has no known version, such input SHALL be applied as it arrives. (Conservative rule: the reaction samples on record are all versioned, so a versionless input from the server is not expected; ignoring it can only leave an older versioned state in place until the next versioned update.)

#### Scenario: Versionless update over a versioned state
- **GIVEN** a message shows reactions of version 5
- **WHEN** an update without a version arrives for it, and later a history row without reactions
- **THEN** the chips keep version 5 and its counts

#### Scenario: Versionless history row
- **GIVEN** a message shows reactions of version 5 and its history row carries counts but no version
- **WHEN** the row is stored
- **THEN** the chips keep version 5 and its counts, and the row's other fields (an edit) apply
