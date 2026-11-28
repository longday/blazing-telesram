# Spec Delta

## Purpose

The playback copy's rule about the reveal action names the platform's file manager, not only Finder.

## MODIFIED Requirements

### Requirement: Playback copy
The audio or video fetched for playback SHALL be kept as a private local copy in the account's media area, not in the Downloads folder, readable only by the current OS user, named without the attachment's own name. A copy SHALL NOT be presented as a download: no «Сохранено» and no reveal action («Показать в Finder», «Показать в Проводнике», «Показать в папке», see `attachment-download`). Copies SHALL be removed when the client starts and when the user signs out.

#### Scenario: Not in Downloads
- **WHEN** a voice message or video is played
- **THEN** nothing is written to the Downloads folder

#### Scenario: Restart
- **GIVEN** a voice message was played in an earlier run
- **WHEN** the client is started and the user plays it again
- **THEN** the audio is fetched again, because the copy was removed at start

#### Scenario: Sign out
- **GIVEN** voice messages and videos were played
- **WHEN** the user signs out
- **THEN** every playback copy of the account is deleted

#### Scenario: Not a saved download
- **WHEN** a video card whose playback copy is on disk is shown
- **THEN** it does not offer the reveal action unless the original was downloaded

#### Scenario: A copy that cannot be removed yet
- **GIVEN** a system player still holds a playback copy open (Windows does not delete an open file)
- **WHEN** the client starts or the user signs out
- **THEN** the client carries on, removes what it can, and removes the rest at the next start
