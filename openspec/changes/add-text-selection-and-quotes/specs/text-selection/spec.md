# Spec Delta

## Purpose
Lets the user select the text of messages with the mouse, inside one message or across messages of the open chat, and copy it, without any request to the server. In this capability and in `message-selection`, Cmd stands for the platform's primary modifier: Cmd on macOS, Ctrl on Linux and Windows.

## ADDED Requirements

### Requirement: Selectable text
The text of a message body and the caption of an image or a gallery SHALL be selectable with the mouse, and the pointer SHALL show the text cursor over it. The author name, the time, the reply quote, the forwarded-from line, file names, reaction chips, voice and video cards and placeholders SHALL NOT be selectable.

#### Scenario: Pointer over text
- **WHEN** the pointer is over the text of a message
- **THEN** it shows the text cursor

#### Scenario: Pointer over a quote
- **WHEN** the user drags over the reply quote or the author name of a message
- **THEN** nothing is selected from them

#### Scenario: Caption
- **WHEN** the user drags over the caption of an image
- **THEN** the caption text is selected

### Requirement: Select by dragging
Pressing the left button on a message text and moving the pointer SHALL select the text between the press point and the pointer, with the selected part highlighted while the button is held and afterwards. Shift+click SHALL extend an existing selection to the clicked point. A plain click without movement SHALL clear the selection.

#### Scenario: Drag inside a message
- **WHEN** the user presses on the third word of a message and drags to the fifth
- **THEN** the characters between the two points are highlighted and nothing else

#### Scenario: Drag backwards
- **WHEN** the user drags from the end of a line to its start
- **THEN** the same characters are selected as for the forward drag

#### Scenario: Wrapped text
- **WHEN** the user drags from a word on the first line of a wrapped message to a word on the third line
- **THEN** the selection covers the text between them across all lines

#### Scenario: Extend with Shift
- **WHEN** a selection exists and the user Shift+clicks later in the same message
- **THEN** the selection grows to the clicked point and keeps its start

#### Scenario: Click clears
- **WHEN** a selection exists and the user clicks a message text without moving
- **THEN** the selection is cleared

### Requirement: Select by word and paragraph
A double click on a message text SHALL select the word under the pointer, and a triple click the paragraph (the text between line breaks). Dragging after the click SHALL extend the selection by whole words or whole paragraphs respectively.

#### Scenario: Double click
- **WHEN** the user double-clicks a word of a message
- **THEN** exactly that word is selected, without the surrounding spaces

#### Scenario: Triple click
- **WHEN** the user triple-clicks a line of a multi-line message
- **THEN** that line is selected without the line break

#### Scenario: Drag after a double click
- **WHEN** the user double-clicks a word and drags to a later word
- **THEN** both words and everything between them are selected

### Requirement: Selection across messages
A selection SHALL be able to start in one message and end in another of the same chat. Messages between the ends SHALL be highlighted whole, the first from the start point and the last up to the end point. While the pointer is held above or below the visible list, the list SHALL scroll and the selection SHALL keep extending.

#### Scenario: Drag over three messages
- **WHEN** the user drags from the middle of one message to the middle of the message two rows below
- **THEN** the first is selected from the start point to its end, the middle one entirely, the last from its start to the end point

#### Scenario: Messages without text between
- **WHEN** the dragged range contains a voice message or a photo without caption
- **THEN** it is skipped and does not break the selection

#### Scenario: Drag past the edge
- **WHEN** the user holds the pointer below the visible list while dragging and more messages exist there
- **THEN** the list scrolls toward that edge and the selection extends over the rows that appear

#### Scenario: Scrolled out of view
- **WHEN** a selected message scrolls out of the viewport and back
- **THEN** its highlight is still there

#### Scenario: Older history loads
- **WHEN** older messages load above a selection
- **THEN** the selection and its highlight do not change

### Requirement: Copy the selection
Cmd+C SHALL copy the selected text as plain text when the timeline, not a text field, has the focus. A selection inside one message SHALL copy exactly the selected text; a selection over several messages SHALL copy, per message with text, a line `Автор, ДД.ММ.ГГГГ ЧЧ:ММ`, the selected text and a blank line between messages. Cmd+C with no selection SHALL change nothing, and copying SHALL keep the selection.

#### Scenario: Copy part of a message
- **WHEN** the user selects "world" in "hello world" and presses Cmd+C
- **THEN** the clipboard holds "world"

#### Scenario: Copy across messages
- **WHEN** the user selects from the middle of one message to the middle of the next and presses Cmd+C
- **THEN** the clipboard holds, for each of the two messages, the author and date line and the selected part, separated by a blank line

#### Scenario: Nothing selected
- **WHEN** nothing is selected and the user presses Cmd+C
- **THEN** the clipboard is unchanged

#### Scenario: Composer keeps its own copy
- **WHEN** the composer has the focus and holds a selection and the user presses Cmd+C
- **THEN** the composer's selected text is copied, not the timeline selection

#### Scenario: Selecting moves the focus
- **WHEN** the composer has the focus and the user selects text in the timeline and presses Cmd+C
- **THEN** the timeline selection is copied

### Requirement: Select all
With the timeline focused, Cmd+A SHALL select all the text of the loaded messages of the open chat. In a text field Cmd+A SHALL keep selecting the field's text.

#### Scenario: Select all in the timeline
- **WHEN** the timeline has the focus and the user presses Cmd+A and then Cmd+C
- **THEN** the clipboard holds the text of every loaded message with the author and date lines

#### Scenario: Select all in the composer
- **WHEN** the composer has the focus and the user presses Cmd+A
- **THEN** only the composer's text is selected

### Requirement: Clearing the selection
Escape SHALL clear the selection before it closes the info panel or cancels a reply or edit; a click on empty timeline space, switching the chat, signing out and starting message selection SHALL clear it too. It SHALL be dropped when its message is deleted or a message holding an end of it changes text in any way, even keeping its length, and SHALL be kept on new messages, older history loading, scrolling and reaction changes.

#### Scenario: Escape
- **WHEN** a selection exists and the user presses Escape
- **THEN** the selection is cleared and no panel closes

#### Scenario: Chat switch
- **WHEN** a selection exists and the user opens another chat
- **THEN** the selection is gone and does not come back on returning

#### Scenario: Selected message edited
- **WHEN** the selected message is edited by its author
- **THEN** the selection is gone

#### Scenario: Same-length edit
- **WHEN** the user selects "world" in "hello world" and the message is edited to "hello there"
- **THEN** the selection is gone, and neither Cmd+C nor «Цитировать» offers "there" or the old range

#### Scenario: New message
- **WHEN** a new message arrives while a selection exists
- **THEN** the selection stays as it was

### Requirement: Selection sends nothing
Selecting, extending, clearing and copying text SHALL NOT send any request to the server.

#### Scenario: Selecting
- **WHEN** the user selects and copies text in the timeline
- **THEN** no request that changes server state is sent
