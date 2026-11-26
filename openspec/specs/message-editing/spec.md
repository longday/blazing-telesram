# message-editing Specification

## Purpose

Lets the user act on messages through a context menu and change or remove the messages they sent.

## Requirements

### Requirement: Message context menu
Right-clicking a message SHALL open a menu with Reply for every sent message with content, Copy text for every message with text, Delete for the user's own sent messages, and Edit for those of them the server can rewrite without losing content. For every sent message that is not deleted it SHALL also show the reaction row defined by the message-reactions capability above the other entries. A pending or failed message offers Copy text only. Escape or a click outside SHALL close it.

#### Scenario: Own message
- **WHEN** the user right-clicks their own sent message
- **THEN** the menu offers the reaction row, Reply, Edit, Copy text and Delete

#### Scenario: Other user's message
- **WHEN** the user right-clicks another user's message
- **THEN** the menu offers the reaction row, Reply and Copy text only

#### Scenario: Edit not offered
- **WHEN** the user right-clicks their own sent message that is forwarded, or a reply whose original is outside the chat or has no known id
- **THEN** the menu does not offer Edit

#### Scenario: Copy text
- **WHEN** the user chooses Copy text
- **THEN** the message text is on the clipboard

### Requirement: Edit own message
Choosing Edit, or pressing Up in an empty composer, SHALL load the user's message into the composer under an edit bar; Enter SHALL save the new text, and Escape SHALL cancel without changes. Saving SHALL update the message in place with the edited marker and SHALL keep the quote of a reply.

#### Scenario: Save an edit
- **WHEN** the user edits their message and presses Enter
- **THEN** the message shows the new text and the edited marker, here and in other clients

#### Scenario: Edit a reply
- **WHEN** the user edits their reply to another message
- **THEN** the reply keeps its quote, here and in other clients

#### Scenario: Up arrow
- **WHEN** the composer is empty and the user presses Up
- **THEN** the newest own sent message of the chat that can be edited opens for editing

#### Scenario: Cancel an edit
- **WHEN** the edit bar is shown and the user presses Escape
- **THEN** the message stays unchanged and the composer returns to the previous draft

### Requirement: Delete own message
Choosing Delete SHALL ask for confirmation; confirming SHALL delete the message for all chat members, replacing it with the deleted placeholder.

#### Scenario: Confirm deletion
- **WHEN** the user confirms deletion of their message
- **THEN** the message shows the deleted placeholder, here and in other clients

#### Scenario: Cancel deletion
- **WHEN** the user cancels the confirmation
- **THEN** the message stays

### Requirement: Failed edits and deletions
When the server rejects an edit or deletion, the message SHALL keep its previous state and the client SHALL show the reason.

#### Scenario: Rejected edit
- **WHEN** the server refuses an edit
- **THEN** the message keeps its old text and an error with the reason is shown
