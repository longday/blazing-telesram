# Spec Delta

## Purpose

Records what the client does and which protocol traffic it exchanges, so the user and the developer can find why something fails without rebuilding or attaching a debugger.

## ADDED Requirements

### Requirement: Structured activity log
The desktop app and the developer probe SHALL record diagnostics as log records with a timestamp, level (`error`, `warn`, `info`, `debug`, `trace`), component and message, plus named fields. Errors that the user does not otherwise see SHALL be recorded at `warn` or `error`.

#### Scenario: Recoverable failure
- **WHEN** the cache cannot be opened and the client falls back to memory
- **THEN** a `warn` or `error` record names the component and the failure

### Requirement: Verbosity filter
The `BT_LOG` environment variable SHALL select which records are produced, using per-component level directives; without it the level SHALL be `info`. An invalid value SHALL stop startup with an error that names `BT_LOG`.

#### Scenario: Default level
- **WHEN** the app starts without `BT_LOG`
- **THEN** `info`, `warn` and `error` records are produced and `debug`/`trace` records are not

#### Scenario: Invalid filter
- **WHEN** `BT_LOG` cannot be parsed
- **THEN** the app exits before opening a window and the error names `BT_LOG`

### Requirement: Network trace
The client SHALL record each HTTP request and each WebSocket request it sends at `debug` with method or operation, host and path, status and duration, and each Xiva connection change and incoming push kind at `debug` or above. Message text and raw frame bodies SHALL appear only at `trace`.

#### Scenario: Request traced
- **GIVEN** `BT_LOG=debug`
- **WHEN** the client loads a chat's history
- **THEN** a record shows the history request with its status and duration and no message text

#### Scenario: Raw frames on demand
- **GIVEN** `BT_LOG=trace` for the protocol component
- **WHEN** a push arrives
- **THEN** a record contains the decoded push body

### Requirement: Terminal output
Every produced record SHALL be written to standard error as one human-readable line.

#### Scenario: Run from a terminal
- **WHEN** the app runs from `./run.sh`
- **THEN** records appear in that terminal as they happen

### Requirement: Machine-readable log file
The desktop app SHALL append every produced record as one JSON object per line to a file in `logs/` under the runtime directory, flushed per record, so another process can read it while the app runs. Each launch SHALL start a new file; the directory SHALL be owner-only and the app SHALL keep at most the 10 newest files.

#### Scenario: Reading a running app's log
- **GIVEN** the app is running
- **WHEN** another process reads the newest file in `logs/`
- **THEN** each line parses as JSON with `timestamp`, `level`, `target` and `fields`, including records from the last second

#### Scenario: Log rotation
- **GIVEN** 10 log files already exist
- **WHEN** the app starts
- **THEN** the oldest file is deleted and a new file is created

### Requirement: In-app log window
The chat window SHALL offer a log view, opened by a toolbar button and by Cmd+Shift+L, that lists recent records (at least the last 2000) with time, level and message, updates while open, and copies all shown records to the clipboard on request.

#### Scenario: Open the log
- **WHEN** the user presses Cmd+Shift+L in the chat window
- **THEN** a log view shows the recent records, newest at the bottom

#### Scenario: New record while open
- **GIVEN** the log view is open
- **WHEN** the client records an event
- **THEN** the event appears in the view without reopening it

#### Scenario: Copy
- **WHEN** the user chooses copy in the log view
- **THEN** the clipboard holds the shown records as text lines

### Requirement: No secrets in the log
No log output SHALL contain cookie values, CSRF or session tokens, or URL query strings, at any level.

#### Scenario: Trace-level run
- **GIVEN** `BT_LOG=trace`
- **WHEN** the client signs in, loads chats and receives pushes
- **THEN** no output contains a stored cookie value or token
