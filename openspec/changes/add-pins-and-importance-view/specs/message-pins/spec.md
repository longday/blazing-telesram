# Spec Delta

## Purpose

Shows which message of a chat is pinned, keeps that state current while connected and in the offline cache. Display only: the client does not pin or unpin.

## ADDED Requirements

### Requirement: Pinned message bar
When the open chat has a pinned message the client SHALL show a bar between the chat header and the timeline that names the author (in groups and channels) and shows the beginning of the pinned message's text on one line, or the same placeholder a chat row preview uses for a message without text (a photo, a file, a deleted message). A chat without a pinned message SHALL show no bar. The bar SHALL be labelled as a pinned message and SHALL NOT use the chat row's pinned-chat marker.

#### Scenario: Chat with a pinned message
- **GIVEN** a message of the open chat is pinned
- **WHEN** the chat is shown
- **THEN** the bar shows the author and the text of that message

#### Scenario: Pinned photo
- **GIVEN** the pinned message has no text
- **WHEN** the chat is shown
- **THEN** the bar shows the same placeholder a chat row would show for that message

#### Scenario: No pin
- **WHEN** a chat has no pinned message
- **THEN** no bar is shown and the timeline starts directly under the header

### Requirement: Pin source
The client SHALL show the pin the server reports: the pinned message named by the chat's entry in the chat list, and pin changes received live. The text of the pinned message SHALL come from its loaded or cached copy, otherwise be requested once from the server, for the open chat only. The pinned message SHALL NOT be added to the chat's cached history, so that the cache never shows a gap as if it were history.

#### Scenario: Pinned message is loaded
- **GIVEN** the pinned message is among the loaded messages
- **WHEN** the chat is opened
- **THEN** the bar shows its text without an extra server request

#### Scenario: Pinned message is far in the past
- **GIVEN** the pinned message is older than the loaded history
- **WHEN** the chat is opened while connected
- **THEN** the bar shows its text and the timeline still starts at the newest chunk with no gap

#### Scenario: Text cannot be fetched
- **GIVEN** the pinned message is not loaded and the request for it fails
- **WHEN** the chat is shown
- **THEN** the bar shows the pinned-message label with no text, no error dialog appears, and a later opening of the chat tries again

### Requirement: Pin changes apply live
While connected, a pin, an unpin or a change of the pinned message made by another user or another client of the account SHALL update the bar of that chat without user action. The update SHALL NOT scroll the timeline, SHALL NOT change the chat row's preview, time, unread counter or position, and SHALL NOT clear anyone's typing indicator.

#### Scenario: Another user pins a message
- **GIVEN** the chat is open and has no pinned message
- **WHEN** another user pins a message
- **THEN** the bar appears with that message and the view does not scroll

#### Scenario: Pin replaced
- **GIVEN** the bar shows message A
- **WHEN** another user pins message B
- **THEN** the bar shows message B

#### Scenario: Unpinned
- **GIVEN** the bar is shown
- **WHEN** the message is unpinned
- **THEN** the bar disappears

#### Scenario: Chat that is not open
- **WHEN** a pin changes in a chat that is not open
- **THEN** that chat's row keeps its preview, unread counter and position, and opening the chat shows the new state

### Requirement: Pinned message follows edits and deletions
While a message is pinned, an edit of it SHALL change the text in the bar and its deletion SHALL change the bar to the deleted placeholder, in the same way as in the timeline.

#### Scenario: Pinned message edited
- **GIVEN** the bar shows a message's text
- **WHEN** the message is edited
- **THEN** the bar shows the new text

#### Scenario: Pinned message deleted
- **GIVEN** the bar shows a message
- **WHEN** the message is deleted
- **THEN** the bar shows the deleted placeholder

### Requirement: Pin in the offline cache
The last known pin of every chat, with the text last shown for it, SHALL be kept in the cache, so that the bar appears before the server answers and without network. A cache written before pins existed SHALL load as chats without a pinned message.

#### Scenario: Launch offline
- **GIVEN** a previous run showed a bar for a chat
- **WHEN** the app starts offline and the user opens that chat
- **THEN** the bar shows the same text with the offline indicator

#### Scenario: Cache from an older version
- **GIVEN** a cache written before pins existed
- **WHEN** the app starts
- **THEN** chats load without a bar and without errors

#### Scenario: Unpinned while offline
- **GIVEN** a cached bar and a message unpinned while the app was closed
- **WHEN** the chat list is reloaded from the server
- **THEN** the bar is gone

### Requirement: A pinned message is not a pinned chat
The pinned-chat marker and the pinned-first ordering of the chat list SHALL depend only on the chat's own pin, never on whether a message of the chat is pinned.

#### Scenario: Chat with a pinned message
- **GIVEN** a chat is not pinned in the chat list and one of its messages is pinned
- **WHEN** the chat list is shown
- **THEN** the chat has no pinned marker and keeps its place by last activity

### Requirement: Pins are display only
The client SHALL NOT offer pinning or unpinning a message, and SHALL NOT send any pin request. The bar SHALL NOT offer an unpin control.

#### Scenario: Message menu
- **WHEN** the user opens the context menu of a message
- **THEN** it has no pin or unpin entry

#### Scenario: Bar controls
- **WHEN** the bar is shown
- **THEN** it has no unpin control
