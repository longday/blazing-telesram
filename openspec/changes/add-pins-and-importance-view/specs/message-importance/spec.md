# Spec Delta

## Purpose

Shows which messages their author or a participant marked as important, keeps that state current while connected and in the offline cache. Display only: the client neither marks nor unmarks messages as important; the one flag it sends is the existing one, repeated when the user edits an own message that is already important.

## ADDED Requirements

### Requirement: Important marker
A message that the server reports as important SHALL show a marker in its bubble, in the timeline of any chat kind, for incoming and outgoing messages. The marker is a small lightning-bolt glyph next to the message time in the bubble footer. A message that is not important and a deleted message SHALL show no marker. The marker SHALL NOT change the bubble's text, size class or position in the timeline.

#### Scenario: Important incoming message
- **WHEN** another user's message is reported important
- **THEN** its bubble shows the marker

#### Scenario: Important own message
- **WHEN** the user's own message is reported important
- **THEN** its bubble shows the marker

#### Scenario: Ordinary message
- **WHEN** a message is not reported important
- **THEN** its bubble shows no marker

#### Scenario: Deleted message
- **WHEN** an important message is deleted
- **THEN** it shows the deleted placeholder and no marker

### Requirement: Importance source
A message SHALL show the importance the client has been told about: the flag of a history row, the flag of a message pushed live, and the flag kept in the offline cache. A message whose row or push carries no flag SHALL be treated as not important. The flag is shown as the server last sent it for that message; a newer copy of the message replaces an older one in the way edits already do.

#### Scenario: History row with the flag
- **WHEN** a history chunk contains an important message
- **THEN** the message shows the marker

#### Scenario: Message sent as important by another client
- **WHEN** a message arrives live already marked important
- **THEN** it appears with the marker

### Requirement: Importance changes apply live
While connected, a change of a loaded message's importance made elsewhere SHALL update its marker in place without user action. The update SHALL NOT scroll the timeline and SHALL NOT change the chat row's preview, time, unread counter or position.

#### Scenario: Marked important elsewhere
- **GIVEN** a loaded message without a marker
- **WHEN** it is marked important from another client
- **THEN** the marker appears and the view does not scroll

#### Scenario: Importance removed elsewhere
- **GIVEN** a loaded message with a marker
- **WHEN** the importance is removed from another client
- **THEN** the marker disappears

### Requirement: Importance in the offline cache
A cached message SHALL keep its importance flag, so cached history shows the markers of the last known state before the server answers and without network. Cache entries written before importance existed SHALL load as messages that are not important.

#### Scenario: Launch offline
- **GIVEN** a previous run loaded an important message
- **WHEN** the app starts offline and the user opens that chat
- **THEN** the message shows the marker

#### Scenario: Cache from an older version
- **GIVEN** a cache written before importance existed
- **WHEN** the app starts
- **THEN** its messages load without markers and without errors

### Requirement: Importance is shown, not changed
The client SHALL NOT offer marking or unmarking a message as important and SHALL NOT offer an "important" option in the composer. Messages the client sends SHALL be sent as not important. When the user edits one of their own messages that is important, the edit request SHALL carry the important flag so that the edit does not clear it; an edit of a message that is not important SHALL NOT carry it. No other request SHALL carry the flag.

#### Scenario: Message menu
- **WHEN** the user opens the context menu of a message
- **THEN** it has no entry that changes importance

#### Scenario: New message
- **WHEN** the user sends a message
- **THEN** the request does not set the importance flag

#### Scenario: Editing an important own message
- **GIVEN** the user's own message is important
- **WHEN** the user edits its text
- **THEN** the edit request carries the important flag and the message is still shown with the marker afterwards

#### Scenario: Editing an ordinary message
- **GIVEN** the user's own message is not important
- **WHEN** the user edits its text
- **THEN** the edit request does not carry the important flag
