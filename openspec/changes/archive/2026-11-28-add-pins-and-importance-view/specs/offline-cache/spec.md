# Spec Delta

## ADDED Requirements

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
