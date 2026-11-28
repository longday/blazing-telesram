# Spec Delta

## Purpose

The sign-in window works on Linux and Windows as well as on macOS.

## MODIFIED Requirements

### Requirement: Sign in through an embedded web page
When no valid session exists, the client SHALL show a sign-in window that loads the Yandex Telemost web page and lets the user complete the standard Yandex sign-in, including two-factor and organization (Yandex 360) steps, without leaving the app. On macOS the page is shown inside the client's own window. On Linux and Windows the page SHALL be shown in a separate window of the client opened for the sign-in, while the client's own window shows that it is waiting, with a Cancel action; the page's cookies SHALL reach the client only through the client itself, SHALL NOT be written to any file, log, argument list or environment variable on the way (other processes of the same user are not excluded, as for the stored session), and the separate window SHALL close when the sign-in finishes, is cancelled or the client quits. When the web view cannot be created, the client SHALL say what to install (see `cross-platform-runtime`) instead of showing a blank window; on Linux the run script reports a missing system library before the client starts.

#### Scenario: First launch without a session
- **WHEN** the app starts and no stored session exists
- **THEN** the sign-in window opens instead of the chat window

#### Scenario: Sign-in completes
- **WHEN** the user finishes signing in and the page reports a signed-in account
- **THEN** the sign-in window closes and the chat window opens for that account

#### Scenario: User closes the sign-in window
- **WHEN** the user closes the sign-in window before signing in
- **THEN** the app quits without storing anything

#### Scenario: Separate sign-in window on Linux and Windows
- **GIVEN** the client runs on Linux or Windows and no session exists
- **WHEN** the app starts
- **THEN** a window with the Yandex page opens, the client's own window says it is waiting for the sign-in and offers Cancel

#### Scenario: Cancel while waiting
- **GIVEN** the sign-in page is open in its separate window
- **WHEN** the user chooses Cancel in the client's window
- **THEN** the sign-in page window closes, the app quits and nothing is stored

#### Scenario: The client is closed during the sign-in
- **GIVEN** the sign-in page is open in its separate window
- **WHEN** the client's process ends for any reason
- **THEN** the sign-in page window closes as well

#### Scenario: Cookies stay inside
- **WHEN** a sign-in completes on Linux or Windows
- **THEN** no file other than the stored session, and no log line, argument or environment variable, contains a cookie value

#### Scenario: Web view runtime missing
- **GIVEN** the web view runtime of the platform is not installed
- **WHEN** the sign-in is needed
- **THEN** the client shows what to install and quits when the message is closed, storing nothing
