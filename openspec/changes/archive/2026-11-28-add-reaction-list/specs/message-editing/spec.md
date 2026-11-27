# Spec Delta

## MODIFIED Requirements

### Requirement: Message context menu
Right-clicking a message SHALL open a menu with Reply for every sent message with content, Copy text for every message with text, Delete for the user's own sent messages, and Edit for those of them the server can rewrite without losing content. For every sent message that is not deleted it SHALL also show the reaction row defined by the message-reactions capability above the other entries, and for such a message that has at least one reaction it SHALL also offer «Кто отреагировал», which opens the list defined by the reaction-list capability. A pending or failed message offers Copy text only. Escape or a click outside SHALL close it.

#### Scenario: Own message
- **WHEN** the user right-clicks their own sent message
- **THEN** the menu offers the reaction row, Reply, Edit, Copy text and Delete

#### Scenario: Other user's message
- **WHEN** the user right-clicks another user's message that has no reactions
- **THEN** the menu offers the reaction row, Reply and Copy text only

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
