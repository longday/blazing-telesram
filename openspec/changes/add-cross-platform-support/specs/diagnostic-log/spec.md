# Spec Delta

## Purpose

The log view's shortcut follows the platform's primary modifier.

## MODIFIED Requirements

### Requirement: In-app log window
The chat window SHALL offer a log view, opened by a toolbar button and by the primary modifier with Shift+L (Cmd+Shift+L on macOS, Ctrl+Shift+L on Linux and Windows), that lists recent records (at least the last 2000) with time, level and message in a monospace font, updates while open, and copies all shown records to the clipboard on request.

#### Scenario: Open the log
- **WHEN** the user presses the primary modifier with Shift+L in the chat window
- **THEN** a log view shows the recent records, newest at the bottom

#### Scenario: New record while open
- **GIVEN** the log view is open
- **WHEN** the client records an event
- **THEN** the event appears in the view without reopening it

#### Scenario: Copy
- **WHEN** the user chooses copy in the log view
- **THEN** the clipboard holds the shown records as text lines
