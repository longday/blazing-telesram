# Spec Delta

## MODIFIED Requirements

### Requirement: Message context menu
Right-clicking a message SHALL open a menu with Reply for a sent message with content, Copy text for a message with text, Delete for the user's own sent messages, and Edit for those the server can rewrite without losing content. A sent, non-deleted message SHALL also show the reaction row (message-reactions) above the other entries and, with at least one reaction, «Кто отреагировал» (reaction-list), and SHALL offer «Выбрать» (message-selection). A pending or failed message offers Copy text only. When a text selection lies inside the message's text, the menu SHALL offer «Цитировать» (message-quotes) before Reply, and Copy text SHALL read «Скопировать выделенное» and copy the selected text only; when the selection covers several messages including this one, Copy text SHALL read «Скопировать выделенное» and copy the selection, and «Цитировать» SHALL not be offered. Right-clicking a message outside the selection SHALL clear the selection. Escape or a click outside SHALL close the menu.

#### Scenario: Own message
- **WHEN** the user right-clicks their own sent message
- **THEN** the menu offers the reaction row, Reply, Edit, Copy text, Select and Delete

#### Scenario: Other user's message
- **WHEN** the user right-clicks another user's message that has no reactions
- **THEN** the menu offers the reaction row, Reply, Copy text and Select only

#### Scenario: Message with reactions
- **WHEN** the user right-clicks a sent message that has reactions
- **THEN** the menu also offers «Кто отреагировал»

#### Scenario: Message without reactions
- **WHEN** the user right-clicks a sent message that has no reactions
- **THEN** the menu does not offer «Кто отреагировал»

#### Scenario: Edit not offered
- **WHEN** the user right-clicks their own sent message that is forwarded, or a reply whose original is outside the chat or has no known id
- **THEN** the menu does not offer Edit

#### Scenario: Copy text
- **WHEN** the user chooses Copy text
- **THEN** the message text is on the clipboard

#### Scenario: Selection inside the message
- **WHEN** the user selects part of a message's text and right-clicks that message
- **THEN** the menu offers «Цитировать» before Reply, and «Скопировать выделенное» copies only the selected part

#### Scenario: Selection over several messages
- **WHEN** the selection covers two messages and the user right-clicks one of them
- **THEN** the menu offers «Скопировать выделенное» for the whole selection and does not offer «Цитировать»

#### Scenario: Right-click outside the selection
- **WHEN** a selection exists and the user right-clicks a message that holds no part of it
- **THEN** the selection is cleared and the menu offers the entries for the clicked message as without a selection

#### Scenario: Pending message
- **WHEN** the user right-clicks a pending or failed message
- **THEN** the menu offers Copy text only

### Requirement: Edit own message
Choosing Edit, or pressing Up in an empty composer, SHALL load the user's message into the composer under an edit bar; Enter SHALL save the new text, and Escape SHALL cancel without changes. Saving SHALL update the message in place with the edited marker and SHALL keep the quote of a reply, including a quoted fragment (message-quotes). Entering Edit mode SHALL clear a text selection.

#### Scenario: Save an edit
- **WHEN** the user edits their message and presses Enter
- **THEN** the message shows the new text and the edited marker, here and in other clients

#### Scenario: Edit a reply
- **WHEN** the user edits their reply to another message
- **THEN** the reply keeps its quote, here and in other clients

#### Scenario: Edit a fragment reply
- **WHEN** the user edits their reply that quotes a fragment of another message
- **THEN** the reply keeps the same fragment as its quote, here and in other clients

#### Scenario: Up arrow
- **WHEN** the composer is empty and the user presses Up
- **THEN** the newest own sent message of the chat that can be edited opens for editing

#### Scenario: Cancel an edit
- **WHEN** the edit bar is shown and the user presses Escape
- **THEN** the message stays unchanged and the composer returns to the previous draft
