# Spec Delta

## Purpose

Persists chats and loaded history locally so the app shows the last known state immediately on start and remains readable without network.

## ADDED Requirements

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
