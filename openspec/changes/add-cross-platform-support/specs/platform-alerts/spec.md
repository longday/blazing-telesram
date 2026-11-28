# Spec Delta

## Purpose

On Windows and Linux the client tells the user about incoming messages from outside its window and shows the unread count where the platform has a place for it, with the same rules as on macOS.

## ADDED Requirements

### Requirement: Notifications on Windows and Linux
On Windows and Linux the client SHALL post a desktop notification for an incoming message by the same rules as `incoming-notifications` gives on macOS (when it is posted, its content, which chats are silent, no message text in a log), through the platform's notification service, and choosing it SHALL bring the window forward and open that chat. When the platform cannot deliver a notification the client SHALL carry on without one.

#### Scenario: Message while the window is in the background
- **GIVEN** the window is not the active window and a chat is not muted
- **WHEN** an incoming message arrives in it
- **THEN** one notification is posted by the platform's service with the same title and text `incoming-notifications` defines

#### Scenario: Choosing the notification
- **GIVEN** a notification of a chat is shown
- **WHEN** the user chooses it
- **THEN** the window comes forward and that chat is open

#### Scenario: Muted chat
- **WHEN** an incoming message arrives in a muted chat
- **THEN** no notification is posted

#### Scenario: Service not available
- **GIVEN** the platform has no notification service running
- **WHEN** an incoming message arrives
- **THEN** the client keeps working and posts nothing

### Requirement: Unread badge where the platform has one
On Windows the client SHALL show the unread count (the same number as `dock-badge` defines) as an overlay on its task bar button, and on a Linux desktop that implements the launcher entry interface it SHALL show it on the application's launcher icon; at zero the badge SHALL be removed. On a platform or desktop that has no such place the client SHALL show no badge and SHALL NOT replace it with another signal.

#### Scenario: Unread messages on Windows
- **GIVEN** unmuted chats hold 7 unread messages
- **WHEN** the chat list is shown
- **THEN** the task bar button of the client shows the count 7

#### Scenario: All read
- **GIVEN** a badge is shown
- **WHEN** every unmuted chat has been read
- **THEN** the badge is removed

#### Scenario: Desktop without a badge
- **GIVEN** a Linux desktop that does not implement the launcher entry interface
- **WHEN** messages are unread
- **THEN** the client shows no badge
