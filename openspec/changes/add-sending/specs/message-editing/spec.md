# Spec Delta

## Purpose

Lets the user act on messages through a context menu and change or remove the messages they sent.

## ADDED Requirements

### Requirement: Message context menu
Right-clicking a message SHALL open a menu with Reply and Copy text for every message, and Edit and Delete for the user's own sent messages. Escape or a click outside SHALL close it.

#### Scenario: Own message
- **WHEN** the user right-clicks their own sent message
- **THEN** the menu offers Reply, Edit, Copy text and Delete

#### Scenario: Other user's message
- **WHEN** the user right-clicks another user's message
- **THEN** the menu offers Reply and Copy text only

#### Scenario: Copy text
- **WHEN** the user chooses Copy text
- **THEN** the message text is on the clipboard

### Requirement: Edit own message
Choosing Edit, or pressing Up in an empty composer, SHALL load the user's message into the composer under an edit bar; Enter SHALL save the new text, and Escape SHALL cancel without changes. Saving SHALL update the message in place with the edited marker.

#### Scenario: Save an edit
- **WHEN** the user edits their message and presses Enter
- **THEN** the message shows the new text and the edited marker, here and in other clients

#### Scenario: Up arrow
- **WHEN** the composer is empty and the user presses Up
- **THEN** the newest own sent message of the chat opens for editing

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
