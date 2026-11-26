# Spec Delta

## ADDED Requirements

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
