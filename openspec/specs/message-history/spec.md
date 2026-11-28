# message-history Specification

## Purpose
Lets the user read a chat's real messages in a Telegram Desktop style timeline, scrolling back through older history, without changing anything on the server.

## Requirements

### Requirement: Open a chat
Selecting a chat SHALL show its newest messages, scrolled to the bottom, in the web client's history chunk size.

#### Scenario: Open chat
- **WHEN** the user selects a chat
- **THEN** the timeline shows its latest messages with the newest at the bottom

### Requirement: Load older history
Scrolling to the top of the loaded messages SHALL load the previous chunk and keep the message the user was looking at in place.

#### Scenario: Scroll to the top
- **WHEN** the user reaches the oldest loaded message and older messages exist
- **THEN** the previous chunk appears above it without moving the visible message

#### Scenario: Start of history
- **WHEN** the user reaches the first message of the chat
- **THEN** no further loading happens and no loading indicator remains

### Requirement: Message content
Each message SHALL show its author (in groups), text as received with line breaks kept, send time, edited marker, reply quote and forwarded-from line; deleted messages SHALL show a deleted placeholder.

#### Scenario: Group message
- **WHEN** a group message from another user is shown
- **THEN** it shows the author's name and avatar

#### Scenario: Own message
- **WHEN** a message was sent by the signed-in account
- **THEN** it is shown on the right in the outgoing bubble style

#### Scenario: Unsupported message type
- **WHEN** a message has a type the client does not render
- **THEN** it shows a placeholder naming the type instead of being hidden

### Requirement: Attachments preview
Image attachments SHALL show a thumbnail; a message with several images (a gallery) SHALL show all of them as a grid of thumbnails; a voice message SHALL show a player and a video a video card (see `media-playback`); other files SHALL show a card with name and size and a Download action (see `attachment-download`). Previews SHALL load without a click; originals and playback audio or video SHALL NOT.

#### Scenario: Image message
- **WHEN** a message contains an image
- **THEN** a thumbnail is shown at the image's aspect ratio

#### Scenario: File message
- **WHEN** a message contains a non-image file
- **THEN** a card with the file name, size and a Download action is shown

#### Scenario: Gallery message
- **WHEN** a message contains several images
- **THEN** every image is shown as a cell of a grid inside the bubble, in message order, and the caption, if any, is shown below the grid

#### Scenario: Preview not loaded yet
- **WHEN** the preview of an image or grid cell has not arrived
- **THEN** a placeholder of the same size is shown, so the row height does not change when the preview arrives

#### Scenario: Voice message
- **WHEN** a message contains a voice message
- **THEN** a player with a play control, a track and the duration is shown instead of an unsupported-type placeholder, and nothing is fetched for it

#### Scenario: Video message
- **WHEN** a message contains a video attachment
- **THEN** a video card with a play control, the name, the size and a Download action is shown, with the duration when the server states it, and nothing is fetched for it

#### Scenario: Voice message cached by an earlier version
- **GIVEN** the cache holds a voice message stored as an unsupported type
- **WHEN** the chat is opened
- **THEN** the message shows the placeholder without an error until the server's copy of that part of the history replaces it

### Requirement: Image viewer
Choosing an image or a grid cell SHALL open a viewer over the chat that shows that image large, with the position among the message's images, previous and next controls when the message has more than one, and a Download action. Left and Right arrow keys SHALL move between the message's images and Escape or the close control SHALL close the viewer. The viewer SHALL cover the chat while open, so no message is marked as read and the composer ignores input until it closes.

#### Scenario: Open from a gallery
- **WHEN** the user chooses the third cell of a four-image gallery
- **THEN** the viewer shows the third image with the position "3 / 4"

#### Scenario: Navigate
- **GIVEN** the viewer shows the third of four images
- **WHEN** the user presses Right
- **THEN** the fourth image is shown, and pressing Right again changes nothing

#### Scenario: Single image
- **WHEN** the user chooses the image of a message that holds one image
- **THEN** the viewer shows it without previous or next controls

#### Scenario: Close
- **WHEN** the user presses Escape in the viewer
- **THEN** the viewer closes and the chat is shown as before

#### Scenario: Message removed while open
- **GIVEN** the viewer shows an image of a message
- **WHEN** that message is deleted
- **THEN** the viewer closes
