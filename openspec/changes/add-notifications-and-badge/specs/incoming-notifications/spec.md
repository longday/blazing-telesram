# Spec Delta

## Purpose

Tells the user through a macOS notification when a message from someone else arrives in a chat they are not looking at, and lets them open that chat from it, without changing anything on the server.

## ADDED Requirements

### Requirement: Notification for an incoming message
A message from another person that arrives while the client is connected, in a chat that is not muted, SHALL show a system notification, unless one of the exclusions of the next two requirements applies. When one update raises a chat's unread count by more than one message, one notification for the newest SHALL be shown, not one per message.

#### Scenario: Private chat
- **GIVEN** the window is in the background
- **WHEN** another person sends «See you at the call» in a private chat
- **THEN** a notification titled with that person's name and reading «See you at the call» is shown

#### Scenario: A chat that was not in the list
- **WHEN** a person who has never written to the user sends the first message of a new private chat
- **THEN** the notification is shown as for any other chat once the chat list holds the chat

### Requirement: Notification content
The notification SHALL be titled with the chat's name and its text SHALL be the one-line preview the chat row shows for that message, with the author's name in front in a group chat or channel and without it in a private chat. A text longer than 140 characters SHALL be cut with «…», and line breaks SHALL read as spaces. A message without text SHALL show the same wording as the chat row («Фото», «Голосовое сообщение», «Видео», a file name).

#### Scenario: Group chat
- **GIVEN** the window is in the background
- **WHEN** «Boris» sends «done» in the group «Team chat»
- **THEN** a notification titled «Team chat» and reading «Boris: done» is shown

#### Scenario: Message without text
- **WHEN** a voice message arrives in a private chat
- **THEN** the notification reads «Голосовое сообщение»

#### Scenario: Long text
- **WHEN** a message of 400 characters arrives
- **THEN** the notification shows its first 140 characters followed by «…», on one line

### Requirement: What never notifies
No notification SHALL be shown for: a message of the user's own account, including one sent from another client; an edit of a message; a deletion; a reaction; a typing indicator; a pin change; a muted chat; a message that was already in the chat list when the client started or that a reconnect or a refresh of the chat list brings in; a message in an older position than the chat's newest one.

#### Scenario: Own message from another device
- **WHEN** the user sends a message to a chat from the web client
- **THEN** no notification is shown

#### Scenario: Edit, deletion, reaction
- **WHEN** a message is edited, deleted or reacted to
- **THEN** no notification is shown

#### Scenario: Muted chat
- **GIVEN** a chat is muted
- **WHEN** another person sends a message in it
- **THEN** no notification is shown and its unread counter still grows

#### Scenario: Backlog after a reconnect
- **GIVEN** messages arrived in several chats while the client was offline
- **WHEN** the connection returns and the chat list is reloaded
- **THEN** no notification is shown for them

#### Scenario: Start of the app
- **GIVEN** chats with unread messages
- **WHEN** the app starts and loads its chat list
- **THEN** no notification is shown

### Requirement: Suppression changes no state
Suppressing a notification SHALL NOT change any unread counter and SHALL NOT stop the Dock badge from being recomputed from the chat list's own unread state: the badge follows `dock-badge`, independently of whether a notification was shown.

#### Scenario: Backlog moves the badge without a notification
- **GIVEN** the badge reads «2»
- **WHEN** a reconnect reload brings three more unread messages in an unmuted chat and no notification is shown
- **THEN** the unread counters are as the server reports them and the badge reads «5»

### Requirement: Open chat and the key window
No notification SHALL be shown for a message in the chat that is open while the chat window is the key window, because the user is looking at it. A notification SHALL be shown for the open chat when the chat window is not the key window (another app in front, the window minimised or hidden), and for any other chat whether or not the window is the key window.

#### Scenario: Looking at the chat
- **GIVEN** a chat is open and the window is the key window
- **WHEN** another person sends a message in that chat
- **THEN** no notification is shown

#### Scenario: Window in the background
- **GIVEN** a chat is open and another app is in front
- **WHEN** another person sends a message in that chat
- **THEN** a notification is shown and the chat stays unread

#### Scenario: Another chat while the window is in front
- **GIVEN** the window is the key window with one chat open
- **WHEN** another person sends a message in a different chat that is not muted
- **THEN** a notification is shown for that chat

#### Scenario: Window minimised
- **GIVEN** a chat is open and the window is minimised
- **WHEN** another person sends a message in that chat
- **THEN** a notification is shown

### Requirement: A notification opens its chat
Choosing a notification SHALL bring the app and its window to the front and open the chat it is about. A chat that is no longer in the list SHALL only bring the window to the front. Choosing a notification SHALL NOT mark anything as read by itself: whether the chat is read follows `read-markers` (the window is focused and the newest message is visible).

#### Scenario: Open from the banner
- **GIVEN** a notification for a chat is shown while another chat is open
- **WHEN** the user chooses it
- **THEN** the app comes to the front and that chat is open

#### Scenario: App in the background
- **GIVEN** another app is in front
- **WHEN** the user chooses a notification
- **THEN** the window comes to the front with the chat open, and the chat becomes read once its newest message is visible in the focused window

#### Scenario: Chat gone
- **GIVEN** the chat of a notification has left the chat list
- **WHEN** the user chooses the notification
- **THEN** the window comes to the front and the open chat does not change

### Requirement: Notifications retire when read
A delivered notification of a chat SHALL be removed from the notification centre when that chat's unread counter drops to zero, whether the user read it here or in another client. When the session ends or the user signs out, every notification of the app SHALL be removed.

#### Scenario: Read here
- **GIVEN** a notification of a chat is on the screen
- **WHEN** the user opens that chat in the focused window and reads it
- **THEN** the notification is removed

#### Scenario: Read in the web client
- **GIVEN** a notification of a chat is in the notification centre
- **WHEN** the user reads the chat in the web client
- **THEN** the notification is removed without a restart

#### Scenario: Sign out
- **GIVEN** notifications of several chats are in the notification centre
- **WHEN** the user signs out
- **THEN** all of them are removed

### Requirement: Permission
The client SHALL ask the system for permission to show notifications once, when the chat window opens, and SHALL NOT ask again in the same run after an answer. When permission is refused, or the system cannot show notifications to this app, the client SHALL show none, SHALL show no error, and SHALL keep working, the Dock badge included.

#### Scenario: First launch
- **WHEN** the chat window opens and the system has not been asked yet
- **THEN** the system's permission prompt is shown

#### Scenario: Refused
- **GIVEN** the user refused notifications
- **WHEN** another person sends a message in a chat that is not open
- **THEN** no notification is shown, no error is shown, and the badge and the unread counter still change

### Requirement: Privacy of notifications
A notification's text, a chat's name and an author's name SHALL go only to the system's notification centre. They SHALL NOT be written to any log at `debug` or above, to a file, or to the notification's own payload beyond the chat identifier needed to open the chat on a click. Showing a notification SHALL NOT send anything to the server and SHALL NOT mark anything as read.

#### Scenario: Debug log
- **GIVEN** `BT_LOG=debug`
- **WHEN** notifications are shown
- **THEN** the log shows at most that a notification was shown or skipped, and contains no message text, chat name or author name

#### Scenario: No write
- **WHEN** a notification is shown, chosen or removed
- **THEN** no request is made because of it
