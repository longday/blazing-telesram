# Spec Delta

## MODIFIED Requirements

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
- **THEN** a video card with a play control, the name, the duration, the size and a Download action is shown, and nothing is fetched for it

#### Scenario: Voice message cached by an earlier version
- **GIVEN** the cache holds a voice message stored as an unsupported type
- **WHEN** the chat is opened
- **THEN** the message shows the placeholder without an error until the server's copy of that part of the history replaces it
