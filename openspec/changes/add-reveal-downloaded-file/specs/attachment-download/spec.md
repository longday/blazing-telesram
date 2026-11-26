# Spec Delta

## ADDED Requirements

### Requirement: Reveal in Finder
A card whose file or image was saved by a download of the current session, and the image viewer for such an image, SHALL offer «Показать в Finder», which selects the saved file in a Finder window without opening it. The action SHALL be local (no request, no server change, no connection needed) and SHALL NOT be offered otherwise. If the saved file no longer exists, choosing it SHALL reveal nothing and the card SHALL offer Download again.

#### Scenario: Reveal a saved file
- **GIVEN** a file was downloaded and the card shows that it was saved
- **WHEN** the user chooses «Показать в Finder» on the card
- **THEN** Finder shows the Downloads folder with that file selected and the file is not opened

#### Scenario: Reveal from the viewer
- **GIVEN** the viewer shows an image whose original was downloaded
- **WHEN** the user chooses «Показать в Finder» in the viewer
- **THEN** the saved original is selected in Finder

#### Scenario: Offline
- **GIVEN** a file was downloaded and the client is now offline
- **WHEN** the user chooses «Показать в Finder»
- **THEN** the file is revealed and no request is made

#### Scenario: Only when saved
- **WHEN** a file card is shown before any download, while a download runs, or after a download failed
- **THEN** it does not offer «Показать в Finder»

#### Scenario: Not after a restart
- **GIVEN** a file was downloaded in an earlier run of the client
- **WHEN** its card is shown after the client was restarted
- **THEN** the card offers Download only, and no file in the Downloads folder is looked up or indexed

#### Scenario: File removed meanwhile
- **GIVEN** a file was downloaded and the user then deleted or moved it in Finder
- **WHEN** the user chooses «Показать в Finder»
- **THEN** nothing is revealed and the card offers Download again

#### Scenario: Download again
- **GIVEN** a card shows a saved file
- **WHEN** the user chooses Download again
- **THEN** a new copy is saved under a non-clashing name and «Показать в Finder» reveals the new copy

## MODIFIED Requirements

### Requirement: Authenticated and private transfer
A download SHALL use the signed-in account's session, sending the session cookie only to the host it belongs to and never to a host the server redirects to. Cookie values, tokens, URL query strings, attachment names and saved paths SHALL NOT appear in log output above `trace`, and the client SHALL NOT open, execute or index a saved file; the one thing it does with a saved file after the transfer is to select it in Finder when the user chooses «Показать в Finder».

#### Scenario: Redirect to another host
- **WHEN** the server answers a download with a redirect to a different host
- **THEN** the client follows it without sending the session cookie to that host

#### Scenario: Debug log
- **GIVEN** `BT_LOG=debug`
- **WHEN** a file is downloaded
- **THEN** the log shows the request's host, path, status and duration and contains no cookie value, query string or file name

#### Scenario: Reveal leaves no trace in the log
- **GIVEN** `BT_LOG=debug`
- **WHEN** the user chooses «Показать в Finder»
- **THEN** the log contains no file name or path of the saved file
