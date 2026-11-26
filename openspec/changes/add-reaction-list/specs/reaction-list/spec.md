## ADDED Requirements

### Requirement: Reactor list availability
The user SHALL be able to open a list of the people who reacted to a sent message that is not deleted and has at least one reaction. A message without reactions, a deleted message, and a message that is pending or failed SHALL offer no list.

#### Scenario: Message with reactions
- **GIVEN** a sent message that has reactions
- **WHEN** the user opens its list
- **THEN** the list of the people who reacted is shown

#### Scenario: No reactions
- **WHEN** a message has no reactions, is deleted, or is still pending
- **THEN** no list can be opened for it

### Requirement: Reactor entries
Each entry SHALL show the person's avatar and name, the emoji of the reaction they chose (the neutral glyph for a type the client has no emoji for), and when they reacted. The user's own entry SHALL be marked. Entries SHALL be in the order the server reports them.

#### Scenario: Entries of two people
- **GIVEN** a sent message that two people reacted to, one with 👍 and one with ❤️
- **WHEN** the user opens its list
- **THEN** two entries are shown, one with each person's name and emoji and the time of their reaction

#### Scenario: Own reaction
- **GIVEN** the user's own reaction is among the message's reactions
- **WHEN** the user opens the list
- **THEN** the user's entry is marked as theirs and the others are not

#### Scenario: Unknown reaction type
- **WHEN** an entry's reaction type has no confirmed emoji
- **THEN** the entry shows the neutral glyph instead of being hidden


### Requirement: Complete or labelled
The list SHALL NOT present a partial list as complete. It SHALL either show every person who reacted, or state how many of how many entries it shows, using the message's known reaction counts. Entries the server reports without an identity the client can show SHALL be counted as accounted for and reported as entries the list could not show; they SHALL NOT be dropped silently.

#### Scenario: Complete list
- **GIVEN** the server returns every person who reacted
- **WHEN** the list is shown
- **THEN** it states no shortfall

#### Scenario: Partial list
- **GIVEN** a message with 45 reactions of which the list holds 30
- **WHEN** the list is shown
- **THEN** it says 30 of 45 are shown

#### Scenario: Known count lower than the list
- **GIVEN** the message's known reaction count is lower than the number of people the server has
- **WHEN** the user reaches the end of the loaded entries
- **THEN** the remaining people are still offered, because the end of the list is decided from the server's pages, not from the count

#### Scenario: Known count higher than the list
- **GIVEN** the message's known reaction count is higher than the number of people the server returns
- **WHEN** the server has no more entries
- **THEN** no further page is requested or offered, and the list says how many of how many it shows

#### Scenario: Unidentifiable entry
- **WHEN** the server reports a reaction without any identity for the person
- **THEN** the entry is counted toward the total and the list says how many entries it could not show

### Requirement: Names of reactors
An entry's name SHALL come from the data the server sent with the reaction; when that has none, from the user lookup the client already uses for message authors; when that has none, SHALL be the placeholder "Неизвестный". An entry SHALL NOT show a blank name or a raw user id.

#### Scenario: Name in the reply
- **WHEN** the server sends a display name with the reaction
- **THEN** the entry shows it without any further request

#### Scenario: Name from the lookup
- **WHEN** the server sends no name but the user lookup knows the person
- **THEN** the entry shows the looked-up name

#### Scenario: No name anywhere
- **WHEN** neither the reply nor the lookup has a name
- **THEN** the entry shows "Неизвестный"

### Requirement: Opening the list is a read
Opening the list, and anything it does to show more entries, SHALL only read from the server. It SHALL NOT add, change or remove a reaction, send any other write, or change the reactions the message shows. The list SHALL NOT be written to the offline cache and SHALL NOT survive closing it or restarting the client.

#### Scenario: Reading changes nothing
- **WHEN** the user opens the list of a message and closes it
- **THEN** no reaction was sent and the message's chips are unchanged

#### Scenario: Not persisted
- **WHEN** the client is restarted
- **THEN** no reactor list is available until the user opens one again

### Requirement: Failed and offline list
While the list loads, the user SHALL see that it is loading. When the server refuses or the request fails, the list SHALL show the reason and a way to try again. When the client has no connection, the list SHALL say there is no connection, SHALL NOT show entries from an earlier time as current, and SHALL offer to try again. A failure SHALL NOT change the message's chips.

#### Scenario: Loading
- **WHEN** the user opens the list and the answer has not arrived
- **THEN** a loading state is shown

#### Scenario: Refused
- **WHEN** the server refuses the list
- **THEN** the list shows the reason and a retry, and the chips stay

#### Scenario: Offline
- **GIVEN** the client is offline
- **WHEN** the user opens the list
- **THEN** it says there is no connection and shows no entries

#### Scenario: Message deleted meanwhile
- **GIVEN** the list is loading
- **WHEN** the message is deleted
- **THEN** the list shows that the message is gone and no entries

### Requirement: Answers match the list shown
The list SHALL show only answers to the request that belongs to the list currently open. An answer to a list the user closed, to an earlier opening of the same message, or to a page requested before a refresh SHALL be discarded, whether it carries entries or a failure.

#### Scenario: Reopened message
- **GIVEN** the user opened the list of a message, closed it and opened it again before the first answer arrived
- **WHEN** the first answer arrives
- **THEN** it changes nothing, and the list shows the answer to the second opening

#### Scenario: Refresh with an older page in flight
- **GIVEN** the list is refreshed while a request for an older page is still out
- **WHEN** the older page or its failure arrives
- **THEN** it is discarded and the refreshed first page is not mixed with it
