# attachment-download Specification

## Purpose
Lets the user save the original of a message's file or image to disk by an explicit action, with visible progress, cancellation and retry, without ever saving anything on its own.

## Requirements

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

### Requirement: Destination and file name
A download SHALL be saved in the user's Downloads folder under the attachment's own name. The name SHALL be reduced to a single file name: path separators, control characters and leading dots SHALL NOT survive, an over-long name SHALL be shortened keeping its extension, and a missing or empty name SHALL be replaced by a generic one. The client SHALL NOT overwrite an existing file: when the name is taken it SHALL add a numeric suffix before the extension.

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

#### Scenario: Downloads folder unavailable
- **GIVEN** the Downloads folder does not exist or cannot be written
- **WHEN** the user chooses Download
- **THEN** the download fails with a reason that names the folder problem and nothing is saved

### Requirement: Progress and cancel
While a download runs, its card (and the viewer for an image) SHALL show the received size and, when the server states it, the total size, and a Cancel action. Cancelling SHALL stop the transfer and remove the unfinished data. Downloads SHALL NOT block history, previews, chat switching or other downloads, and starting several at once SHALL be allowed, with only a few transferring at a time.

#### Scenario: Progress
- **WHEN** a large file is downloading
- **THEN** the card shows how much has been received and updates as more arrives

#### Scenario: Cancel
- **WHEN** the user chooses Cancel during a download
- **THEN** the transfer stops, nothing remains in the Downloads folder for that download, and the Download action is offered again

#### Scenario: Browse while downloading
- **WHEN** a download is running and the user opens another chat
- **THEN** that chat loads normally and the download continues

### Requirement: Complete or absent
A file SHALL appear in the Downloads folder under its final name only when all its bytes have arrived; a failed, cancelled or interrupted download SHALL NOT leave a file under the final name. A response shorter than the size the server announced SHALL count as a failure.

#### Scenario: Connection drops mid-transfer
- **WHEN** the network fails while a file is downloading
- **THEN** the download is shown as failed and no file with the attachment's name exists in the Downloads folder

#### Scenario: Truncated body
- **WHEN** the server ends the body before the announced size
- **THEN** the download fails instead of being saved as if complete

### Requirement: Failure and retry
A failed download SHALL show a short reason (network error, file no longer on the server, folder or disk problem) and a Retry action that starts the download again from the beginning. An expired session SHALL end the session like any other request. A failure SHALL NOT affect other downloads or the rest of the timeline.

#### Scenario: File removed on the server
- **WHEN** the server reports the file as not found
- **THEN** the card shows that the file is no longer available and offers Retry

#### Scenario: Disk full
- **WHEN** the disk runs out of space during a download
- **THEN** the download fails with a disk reason and the unfinished data is removed

#### Scenario: Retry
- **GIVEN** a download failed
- **WHEN** the user chooses Retry and the transfer succeeds
- **THEN** the file is saved and the card shows the result

#### Scenario: Session expired
- **WHEN** the server rejects the session during a download
- **THEN** the client handles it as an expired session (sign-in is requested again) and the unfinished data is removed

### Requirement: Result and repeat
When a download finishes the card SHALL show that the file was saved, with its final file name. Choosing Download again SHALL save a new copy under a non-clashing name. The client SHALL NOT delete saved files, including on sign-out.

#### Scenario: Saved
- **WHEN** a download finishes
- **THEN** the card shows that the file was saved and the name it was saved under

#### Scenario: Download twice
- **GIVEN** a file was saved as `report.pdf`
- **WHEN** the user downloads the same attachment again
- **THEN** a second copy `report (1).pdf` is saved and the first stays

#### Scenario: Sign out
- **WHEN** the user signs out
- **THEN** running downloads are stopped and their unfinished data is removed, while completed downloads stay in the Downloads folder

### Requirement: Offline
Without a connection the Download action SHALL be shown as unavailable instead of starting a request that must fail. A download interrupted by losing the connection SHALL fail as described above and be retryable once the connection is back.

#### Scenario: Offline card
- **GIVEN** the client is offline
- **WHEN** a file card is shown
- **THEN** its Download action is unavailable and says that the client is offline

#### Scenario: Back online
- **GIVEN** a download failed because the connection was lost
- **WHEN** the client is online again and the user chooses Retry
- **THEN** the download starts

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
