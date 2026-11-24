# Spec Delta

## MODIFIED Requirements

### Requirement: Attachments preview
Image attachments SHALL show a thumbnail; a message with several images (a gallery) SHALL show all of them as a grid of thumbnails; other files SHALL show a card with name and size and a Download action (see `attachment-download`). Previews SHALL load without a click; originals SHALL NOT.

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

## ADDED Requirements

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
