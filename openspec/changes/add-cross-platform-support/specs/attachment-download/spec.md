# Spec Delta

## Purpose

Saved files get names the host accepts, and the reveal action names the platform's file manager.

## RENAMED Requirements

- FROM: `### Requirement: Reveal in Finder`
- TO: `### Requirement: Reveal in the file manager`

## MODIFIED Requirements

### Requirement: Destination and file name
A download SHALL be saved in the user's Downloads folder (as the platform defines it, see `cross-platform-runtime`) under the attachment's own name. The name SHALL be reduced to a single file name valid on the host: path separators, control characters and leading dots SHALL NOT survive, characters and names the host file system forbids SHALL be replaced (see `cross-platform-runtime`), an over-long name SHALL be shortened keeping its extension, and a missing or empty name SHALL be replaced by a generic one. The client SHALL NOT overwrite an existing file: when the name is taken it SHALL add a numeric suffix before the extension.

#### Scenario: Plain download
- **GIVEN** the Downloads folder has no file named `report.pdf`
- **WHEN** the user downloads an attachment named `report.pdf`
- **THEN** `report.pdf` appears in the Downloads folder with the attachment's exact bytes

#### Scenario: Name already taken
- **GIVEN** the Downloads folder already holds `report.pdf`
- **WHEN** the user downloads another attachment named `report.pdf`
- **THEN** it is saved as `report (1).pdf` and the existing file is unchanged

#### Scenario: Hostile name
- **WHEN** the attachment's name is `../../.ssh/authorized_keys`
- **THEN** the file is saved inside the Downloads folder under a single harmless name and nothing is written elsewhere

#### Scenario: Name the host forbids
- **WHEN** an attachment named `a:b?.txt` is downloaded on Windows
- **THEN** the file is saved inside the Downloads folder under a name Windows accepts

#### Scenario: Downloads folder unavailable
- **GIVEN** the Downloads folder does not exist or cannot be written
- **WHEN** the user chooses Download
- **THEN** the download fails with a reason that names the folder problem and nothing is saved

### Requirement: Reveal in the file manager
A card whose file or image was saved by a download of the current session, and the image viewer for such an image, SHALL offer a «Показать» action named after the platform's file manager: «Показать в Finder» on macOS, «Показать в Проводнике» on Windows and «Показать в папке» on Linux. It SHALL show the saved file in a file-manager window, with the file selected where the platform can select it and otherwise showing its folder, without opening the file. The action SHALL be local (no request, no server change, no connection needed) and SHALL NOT be offered otherwise. If the saved file no longer exists, choosing it SHALL reveal nothing and the card SHALL offer Download again.

#### Scenario: Reveal a saved file
- **GIVEN** a file was downloaded and the card shows that it was saved
- **WHEN** the user chooses the reveal action on the card
- **THEN** the file manager shows the Downloads folder, the file is selected where the platform can select it, and the file is not opened

#### Scenario: Label follows the platform
- **WHEN** a card shows a saved file
- **THEN** the action reads «Показать в Finder» on macOS, «Показать в Проводнике» on Windows and «Показать в папке» on Linux

#### Scenario: Reveal from the viewer
- **GIVEN** the viewer shows an image whose original was downloaded
- **WHEN** the user chooses the reveal action in the viewer
- **THEN** the saved original is shown in the file manager

#### Scenario: Offline
- **GIVEN** a file was downloaded and the client is now offline
- **WHEN** the user chooses the reveal action
- **THEN** the file is revealed and no request is made

#### Scenario: Only when saved
- **WHEN** a file card is shown before any download, while a download runs, or after a download failed
- **THEN** it does not offer the reveal action

#### Scenario: Not after a restart
- **GIVEN** a file was downloaded in an earlier run of the client
- **WHEN** its card is shown after the client was restarted
- **THEN** the card offers Download only, and no file in the Downloads folder is looked up or indexed

#### Scenario: File removed meanwhile
- **GIVEN** a file was downloaded and the user then deleted or moved it in the file manager
- **WHEN** the user chooses the reveal action
- **THEN** nothing is revealed and the card offers Download again

#### Scenario: Download again
- **GIVEN** a card shows a saved file
- **WHEN** the user chooses Download again
- **THEN** a new copy is saved under a non-clashing name and the reveal action shows the new copy
