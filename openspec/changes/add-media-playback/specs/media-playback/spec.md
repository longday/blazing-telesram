# Spec Delta

## Purpose

Lets the user play the voice messages and videos that arrive in history: voice inline in the timeline with play, pause and progress, video in the system's player, always on an explicit action, without changing anything on the server.

## ADDED Requirements

### Requirement: Voice message player
A voice message SHALL be shown as a player in its bubble: a play/pause control, a progress track and the time. The bubble SHALL keep the same size in every state, so starting, pausing or finishing playback does not move the timeline. The time SHALL show the duration while the message is not playing and the elapsed time while it plays or is paused.

#### Scenario: Voice message in history
- **WHEN** a chat with a voice message of 14 seconds is opened
- **THEN** the message shows a play control, a track and the time «0:14» instead of a placeholder

#### Scenario: Progress while playing
- **GIVEN** a voice message is playing
- **WHEN** five seconds have passed
- **THEN** the control shows pause, the time shows «0:05» and about a third of the track is marked as played

#### Scenario: Size does not change
- **WHEN** a voice message starts, pauses, fails or ends
- **THEN** its bubble keeps its width and height and the timeline does not scroll

#### Scenario: Chat row preview
- **WHEN** the latest message of a chat is a voice message
- **THEN** the chat row, a search hit and a reply quote show «Голосовое сообщение» and not a bracketed placeholder

### Requirement: Playback only on request
The client SHALL fetch the audio of a voice message only when the user chooses its play control. Opening a chat, loading or scrolling history, receiving a voice message and a message being shown SHALL NOT fetch audio, store it or make a sound.

#### Scenario: Nothing without a click
- **WHEN** a chat with voice messages is opened, scrolled through and a new voice message arrives
- **THEN** no audio is requested from the server, no file is written for playback and nothing is played

#### Scenario: First play
- **WHEN** the user chooses play on a voice message that has not been played in this run
- **THEN** the control shows that the audio is loading and, when it has arrived, playback starts by itself

#### Scenario: Play again
- **GIVEN** a voice message was played earlier in this run
- **WHEN** the user chooses play again
- **THEN** it plays from the start at once and no request is made, also when the client is offline

### Requirement: Play, pause and one at a time
Choosing the control of a playing voice message SHALL pause it keeping its position, and choosing it again SHALL resume from that position. Only one voice message SHALL play at a time: starting another SHALL stop the current one, which returns to its start. A voice message that reaches its end SHALL return to its start, ready to play again.

#### Scenario: Pause and resume
- **GIVEN** a voice message plays and is paused at 0:05
- **WHEN** the user chooses its control
- **THEN** it continues from 0:05

#### Scenario: Start another
- **GIVEN** a voice message is playing
- **WHEN** the user chooses play on another voice message
- **THEN** the first stops and shows its start, and the second plays

#### Scenario: End of the message
- **WHEN** a voice message plays to its end
- **THEN** it shows its full duration, no sound continues and choosing play starts it from the beginning

### Requirement: Playback ends with its context
Voice playback SHALL stop when the user opens another chat, when the playing message is deleted or removed from the timeline, when the session ends or the user signs out, and when the window closes. Playback SHALL NOT continue where the user has no control to stop it.

#### Scenario: Switch chat
- **GIVEN** a voice message is playing
- **WHEN** the user opens another chat
- **THEN** the sound stops

#### Scenario: Message deleted while playing
- **GIVEN** a voice message is playing
- **WHEN** the server reports the message as deleted
- **THEN** the sound stops and the message shows the deleted placeholder

#### Scenario: Connection lost
- **GIVEN** a voice message is playing from a copy already on disk
- **WHEN** the connection is lost
- **THEN** it keeps playing

### Requirement: Loading, cancel and failure of playback
While the audio is fetched, the play control SHALL show that it is loading and choosing it SHALL cancel the fetch. A failed fetch or an unplayable file SHALL show a short reason in place of the time (no connection, file no longer on the server, format not supported, no sound output, other) and choosing the control SHALL try again. Without a connection and without a copy already on disk play SHALL be shown as unavailable instead of starting a request that must fail.

#### Scenario: Cancel loading
- **GIVEN** the audio of a voice message is loading
- **WHEN** the user chooses the control
- **THEN** the fetch stops, nothing remains on disk for it and the control offers play again

#### Scenario: File removed on the server
- **WHEN** the server reports the voice file as not found
- **THEN** the message shows that the file is not available and offers to try again

#### Scenario: Format not supported
- **WHEN** the downloaded audio cannot be decoded
- **THEN** the message shows that the format is not supported, Download still works, and nothing is played

#### Scenario: Offline without a copy
- **GIVEN** the client is offline and the voice message was not played in this run
- **WHEN** its bubble is shown
- **THEN** play is shown as unavailable and says that the client is offline

#### Scenario: Session expired during the fetch
- **WHEN** the server rejects the session while the audio is fetched
- **THEN** the client handles it as an expired session and nothing is left on disk

### Requirement: Video card
A message with a video attachment SHALL show a video card: a play control, the file name, the duration when the server states it, the size and the Download action. A file counts as a video when its name has a recognised video extension (`mp4`, `mov`); the server's description of the file as a video is optional and only supplies the duration; any other file keeps the ordinary file card. The card SHALL NOT fetch the video, a still frame or anything else by itself.

#### Scenario: Video message
- **WHEN** a message holds a video of 42 seconds and 12 MB named `clip.mp4`
- **THEN** a card with a play control, `clip.mp4`, «0:42 · 12 МБ» and Download is shown

#### Scenario: Not described as a video
- **WHEN** a message holds a file named `clip.mp4` that the server does not describe as a video
- **THEN** a video card with a play control, `clip.mp4`, the size and Download is shown, without a duration

#### Scenario: Unrecognised extension
- **WHEN** a message holds a file whose name has no recognised video extension, whether or not the server describes it as a video
- **THEN** it shows the ordinary file card and no play control

#### Scenario: Nothing without a click
- **WHEN** a chat with video messages is opened and scrolled through
- **THEN** no video data is requested

### Requirement: Video in the system player
Choosing play on a video card SHALL fetch a playback copy, showing the received size and a cancel control, and then open it in the system's video player. The client SHALL NOT decode or draw the video itself. Choosing play again in the same run SHALL open the copy already on disk without a new request. A failure SHALL show its reason on the card and choosing play SHALL try again.

#### Scenario: Play a video
- **WHEN** the user chooses play on a video card
- **THEN** the card shows the progress of the fetch and, when it is complete, the video opens in the system player

#### Scenario: Cancel
- **GIVEN** the playback copy of a video is loading
- **WHEN** the user chooses the card's control
- **THEN** the fetch stops, its unfinished data is removed and nothing is opened

#### Scenario: Play again
- **GIVEN** a video was opened in this run
- **WHEN** the user chooses play again
- **THEN** it opens at once with no request

#### Scenario: Only recognised types are opened
- **GIVEN** the name of a video's file has an extension outside the recognised list
- **WHEN** a play is requested for it
- **THEN** nothing is opened

#### Scenario: Existing copy with an unrecognised extension
- **GIVEN** a playback copy of the same file is already on disk under a name whose extension is outside the recognised list, or has none
- **WHEN** the user chooses play on the video card
- **THEN** that copy is not opened, the video is fetched as a new copy with a recognised extension and that copy is opened

#### Scenario: Download is independent
- **GIVEN** the user downloaded the same video earlier
- **WHEN** the user chooses play
- **THEN** a playback copy is fetched and opened and the downloaded file is not opened

### Requirement: Playback copy
The audio or video fetched for playback SHALL be kept as a private local copy in the account's media area, not in the Downloads folder, readable only by the current OS user, named without the attachment's own name. A copy SHALL NOT be presented as a download: no «Сохранено» and no «Показать в Finder». Copies SHALL be removed when the client starts and when the user signs out.

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
- **THEN** it does not offer «Показать в Finder» unless the original was downloaded

### Requirement: Playback changes nothing on the server
Playing, pausing, fetching or opening a playback copy SHALL NOT send anything to the server: no read or listened mark, no event, no message. The only request is the fetch of the media itself with the session of the signed-in account, and no cookie, token, query string, file name or local path SHALL appear in log output above `trace`.

#### Scenario: Play leaves no mark
- **WHEN** a voice message is played to its end
- **THEN** the only request made was the fetch of its audio and the server state of the message is unchanged

#### Scenario: Debug log
- **GIVEN** `BT_LOG=debug`
- **WHEN** a voice message is played and a video is opened
- **THEN** the log shows the host, path, status and duration of the fetches and contains no cookie value, query string, file name or local path
