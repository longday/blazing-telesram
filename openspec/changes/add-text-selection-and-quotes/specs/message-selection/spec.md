# Spec Delta

## Purpose
Lets the user select whole messages of the open chat and copy them or delete their own, from one bar, with a single confirmation.

## ADDED Requirements

### Requirement: Enter and leave selection mode
Choosing «Выбрать» in the menu of a sent, non-deleted message, or Cmd+click on such a message (Cmd stands for the platform's primary modifier: Cmd on macOS, Ctrl on Linux and Windows), SHALL start a selection mode in which that message is selected. Escape, the bar's close button, deselecting the last message, and leaving the chat SHALL end it. While the mode is on the text selection SHALL be cleared and disabled, and right-click SHALL open no menu.

#### Scenario: Enter from the menu
- **WHEN** the user right-clicks a sent message and chooses «Выбрать»
- **THEN** the message is marked as selected and the bar shows «Выбрано: 1»

#### Scenario: Enter with Cmd+click
- **WHEN** the user Cmd+clicks a sent message
- **THEN** the mode starts with that message selected

#### Scenario: Not offered
- **WHEN** the user right-clicks a pending, failed or deleted message
- **THEN** the menu does not offer «Выбрать»

#### Scenario: Escape
- **WHEN** the mode is on and the user presses Escape
- **THEN** the mode ends and no message is marked

#### Scenario: Chat switch
- **WHEN** the mode is on and the user opens another chat
- **THEN** the mode is off in both chats

#### Scenario: Text selection cleared
- **WHEN** a text selection exists and the user enters the mode
- **THEN** the text selection is cleared

### Requirement: Choose messages
In the mode, a left click on a message row SHALL toggle that message and SHALL NOT act on what is under the pointer (a picture, a play button, a link, a reaction chip). Shift+click SHALL select every sent, non-deleted message between the last toggled one and the clicked one. Cmd+A SHALL select all of them in the loaded part of the chat. A marked row SHALL show a check at its left and a tint.

#### Scenario: Toggle
- **WHEN** the user clicks an unmarked message, then clicks it again
- **THEN** it is marked, then unmarked, and the count follows

#### Scenario: Range
- **WHEN** the user marks one message and Shift+clicks another five rows below
- **THEN** all six messages and those between them that can be selected are marked

#### Scenario: Click on a picture
- **WHEN** the user clicks the picture of a message in the mode
- **THEN** the message is toggled and the viewer does not open

#### Scenario: Reaction chip
- **WHEN** the user clicks a reaction chip in the mode
- **THEN** the message is toggled and no reaction is sent

#### Scenario: Select all
- **WHEN** the user presses Cmd+A in the mode
- **THEN** every sent, non-deleted loaded message is marked

#### Scenario: Last one off
- **WHEN** the user unmarks the last marked message
- **THEN** the mode ends

### Requirement: Selection bar
In the mode a bar SHALL replace the composer and show «Выбрано: N», «Скопировать» (enabled when a marked message has text to copy), «Удалить» (enabled only when every marked message is the user's own sent message) and a close button. The composer's draft and reply target SHALL be unchanged and come back with the composer.

#### Scenario: Bar shown
- **WHEN** three messages are marked
- **THEN** the bar shows «Выбрано: 3»

#### Scenario: Other user's message marked
- **WHEN** a marked message is another user's
- **THEN** «Удалить» is disabled and «Скопировать» is enabled

#### Scenario: Draft kept
- **WHEN** the user has typed a draft, enters the mode and leaves it
- **THEN** the composer shows the same draft

### Requirement: Copy the marked messages
«Скопировать» SHALL put the texts of the marked messages on the clipboard as plain text in timeline order, each message preceded by a line with its author and date and time (`Автор, ДД.ММ.ГГГГ ЧЧ:ММ`) and separated by a blank line, except that a single message is copied as its bare text. A message without text contributes nothing; a file contributes its name, as the menu's Copy does. The mode SHALL stay on.

#### Scenario: Copy several
- **WHEN** the user marks two text messages and chooses «Скопировать»
- **THEN** the clipboard holds both texts, each under its author and date line, with a blank line between

#### Scenario: Copy one
- **WHEN** one message is marked and the user chooses «Скопировать»
- **THEN** the clipboard holds its text alone

#### Scenario: Message without text
- **WHEN** a marked message is a voice message
- **THEN** it adds nothing to the clipboard

### Requirement: Delete the marked messages
«Удалить» SHALL ask once, «Удалить N сообщений?» (for one: «Удалить сообщение?»), warning that the messages disappear for all chat members. Confirming SHALL delete each marked message as the single delete does, in timeline order, and end the mode. A failed delete SHALL be reported with its reason and leave that message as it was, without stopping the others. Cancelling SHALL leave the mode and the marks as they were.

#### Scenario: Delete three
- **WHEN** three own messages are marked and the user confirms
- **THEN** all three show the deleted placeholder, here and in other clients, and the mode ends

#### Scenario: One fails
- **WHEN** the server refuses the second of three deletions
- **THEN** the first and third are deleted, the second keeps its text, and an error with the reason is shown

#### Scenario: Cancel
- **WHEN** the user cancels the confirmation
- **THEN** the messages are not deleted and stay marked

#### Scenario: Not all own
- **WHEN** a marked message is another user's
- **THEN** the user cannot start the deletion

### Requirement: Forward is not part of selection
The bar SHALL offer no forward until the client can forward messages.

#### Scenario: No forward
- **WHEN** messages are marked
- **THEN** the bar has no forward button
