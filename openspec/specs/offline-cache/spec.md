# offline-cache Specification

## Purpose
Persists chats and loaded history locally so the app shows the last known state immediately on start and remains readable without network.

## Requirements

### Requirement: Instant start from cache
On launch with a stored session the client SHALL show the cached chat list and cached history before the server responds, then replace them with server data.

#### Scenario: Launch with cache
- **GIVEN** a previous run loaded chats and history
- **WHEN** the app starts
- **THEN** the cached chat list appears before the connection is established

### Requirement: Offline reading
Without network the client SHALL show cached chats and any cached history, marked as offline.

#### Scenario: Launch offline
- **GIVEN** a cache from a previous run and no network
- **WHEN** the user opens a cached chat
- **THEN** its cached messages are shown with the offline indicator

### Requirement: Cache stays consistent with the server
Data received from the server SHALL replace cached entries, including edits and deletions, so the cache never shows a message the server reports deleted.

#### Scenario: Deleted while offline
- **GIVEN** a cached message deleted on the server while the app was closed
- **WHEN** the client reloads that chunk from the server
- **THEN** the cached copy is replaced by the deleted placeholder

### Requirement: Cache location and isolation
The cache SHALL live in the git-ignored `.runtime/` directory, readable only by the current OS user, separately per account.

#### Scenario: Different account signs in
- **WHEN** a different account signs in
- **THEN** none of the previous account's chats are shown

### Requirement: Reactions are cached
A cached message SHALL keep its reactions, the user's own reaction and the reaction version, so that cached history shows the chips of the last known state before the server answers and without network. Only server-confirmed state SHALL be cached: a reaction of the user that the server has not confirmed SHALL NOT survive a restart. Cache entries written before reactions existed SHALL load as messages without reactions.

#### Scenario: Launch with cached reactions
- **GIVEN** a previous run received reactions for a message
- **WHEN** the app starts offline and the user opens that chat
- **THEN** the message shows the same chips as before

#### Scenario: Unconfirmed reaction and restart
- **GIVEN** the user chose a reaction that the server has not confirmed
- **WHEN** the app is restarted
- **THEN** the message shows the last server-confirmed reactions

#### Scenario: Cache from an older version
- **GIVEN** a cache written before reactions existed
- **WHEN** the app starts
- **THEN** its messages load without chips and without errors

### Requirement: Pins and importance are cached
A cached chat SHALL keep its pinned message and the text last shown for it, and a cached message SHALL keep its importance flag, so that the bar and the markers of the last known state appear before the server answers and without network. A pinned message outside a chat's cached history SHALL NOT be stored as a history message. Entries written before pins and importance existed SHALL load without a pinned message and without markers.

#### Scenario: Launch with cached pin and importance
- **GIVEN** a previous run showed a bar and an important message
- **WHEN** the app starts offline and the user opens that chat
- **THEN** the bar and the marker are shown as before

#### Scenario: Pinned message outside the cached history
- **GIVEN** a bar shows a message older than the cached history of its chat
- **WHEN** the cached history is loaded
- **THEN** the timeline holds only the cached history chunks, with no stray older message
