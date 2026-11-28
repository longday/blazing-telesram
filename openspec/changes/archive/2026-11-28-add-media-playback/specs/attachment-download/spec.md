# Spec Delta

## MODIFIED Requirements

### Requirement: Explicit download action
A file card, a voice message, a video card and the image viewer SHALL each offer a Download action for its original, and a download SHALL start only when the user chooses it. Opening a chat, loading or scrolling history, receiving a message and opening the viewer SHALL NOT download any original; the client SHALL fetch on its own only avatars, image previews and a playback copy on a play choice (`media-playback`). Downloading SHALL NOT change anything on the server.

#### Scenario: Download a file
- **WHEN** the user chooses Download on a file card
- **THEN** the file is saved to disk and the card shows the result

#### Scenario: Download an image original
- **GIVEN** the viewer shows an image
- **WHEN** the user chooses Download in the viewer
- **THEN** the original image file is saved, not the preview

#### Scenario: Nothing without a click
- **WHEN** a chat with file and image messages is opened and scrolled through
- **THEN** no original file is requested from the server and nothing is written to the Downloads folder

#### Scenario: Download a voice message or a video
- **WHEN** the user chooses Download on a voice message or on a video card
- **THEN** the original file is saved to the Downloads folder under its own name by the same rules as any file, with the same progress, cancel, retry and result, and playback is not started

#### Scenario: Nothing without a click for voice and video
- **WHEN** a chat with voice messages and videos is opened and scrolled through
- **THEN** no original is requested, no playback copy is fetched and nothing is written to the Downloads folder

### Requirement: Authenticated and private transfer
A download SHALL use the signed-in account's session, sending the session cookie only to the host it belongs to, never to a redirect target. Cookie values, tokens, URL query strings, attachment names and saved paths SHALL NOT appear in log output above `trace`. The client SHALL NOT open, execute or index a saved file, and only selects it in Finder on «Показать в Finder»; it opens only a video playback copy, in the system player, on play (`media-playback`).

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

#### Scenario: A downloaded video is never opened
- **GIVEN** a video was downloaded to the Downloads folder
- **WHEN** the user chooses play on its card
- **THEN** a separate playback copy is opened and the downloaded file is neither opened nor changed

#### Scenario: Playback fetch redirected
- **WHEN** the server answers the fetch of a playback copy with a redirect to a different host
- **THEN** the client follows it without sending the session cookie to that host
