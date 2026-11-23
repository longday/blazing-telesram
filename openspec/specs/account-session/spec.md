# account-session Specification

## Purpose
Lets the user sign in to a Yandex account once and keeps that session available to the client until it expires or the user signs out.

## Requirements

### Requirement: Sign in through an embedded web page
When no valid session exists, the client SHALL show a sign-in window that loads the Yandex Telemost web page and lets the user complete the standard Yandex sign-in, including two-factor and organization (Yandex 360) steps, without leaving the app.

#### Scenario: First launch without a session
- **WHEN** the app starts and no stored session exists
- **THEN** the sign-in window opens instead of the chat window

#### Scenario: Sign-in completes
- **WHEN** the user finishes signing in and the page reports a signed-in account
- **THEN** the sign-in window closes and the chat window opens for that account

#### Scenario: User closes the sign-in window
- **WHEN** the user closes the sign-in window before signing in
- **THEN** the app quits without storing anything

### Requirement: Session reuse across launches
The client SHALL store the session in the project's git-ignored `.runtime/` directory, readable only by the current OS user, and SHALL reuse it on later launches without showing the sign-in window.

#### Scenario: Relaunch with a stored session
- **GIVEN** a session stored by a previous sign-in
- **WHEN** the app starts
- **THEN** the chat window opens without the sign-in window

#### Scenario: Stored session file permissions
- **WHEN** the client writes the session
- **THEN** the file is readable and writable only by its owner

### Requirement: Session expiry is detected
The client SHALL treat a server response that rejects the session as expiry: it SHALL discard the stored session and show the sign-in window again.

#### Scenario: Server rejects the stored session
- **GIVEN** a stored session that the server no longer accepts
- **WHEN** the client uses it
- **THEN** the stored session is deleted and the sign-in window opens

### Requirement: Sign out
The client SHALL let the user sign out; signing out SHALL delete the stored session and the local cache for that account.

#### Scenario: User signs out
- **WHEN** the user chooses sign out
- **THEN** the session and that account's cache are deleted and the sign-in window opens

### Requirement: Session secrets stay local
The client SHALL NOT write session cookies or tokens to logs, standard output or any location other than the stored session.

#### Scenario: Diagnostics output
- **WHEN** the client or the developer probe logs a request
- **THEN** cookie and token values are absent from the output
