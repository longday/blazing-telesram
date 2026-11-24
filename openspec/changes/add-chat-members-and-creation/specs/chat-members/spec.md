# chat-members Specification

## Purpose

Lets the user see who is in a chat and see their own profile, read-only, so they know who they are talking to without leaving the client.

## ADDED Requirements

### Requirement: Chat info panel
The client SHALL show a chat info panel for the open chat, opened from the chat header and closed by the user, that lists the chat's participants: users, and for group chats also groups and departments. Each user row SHALL show the avatar (or initials) and the name.

#### Scenario: Open the panel of a group
- **WHEN** the user opens the info panel of a group chat
- **THEN** the panel lists the group's users, groups and departments as the server reports them

#### Scenario: Open the panel of a private chat
- **WHEN** the user opens the info panel of a private chat
- **THEN** the panel shows the chat's participants without any group or department sections

#### Scenario: Close the panel
- **WHEN** the user closes the panel or opens another chat
- **THEN** the panel disappears and the chat timeline keeps its scroll position

### Requirement: Admin role is shown
In a group chat the panel SHALL mark participants that the server reports as administrators, and SHALL NOT mark anyone when the server reports no role.

#### Scenario: Group with administrators
- **WHEN** the server reports a participant with the admin role
- **THEN** that participant's row carries an administrator marker and the others do not

#### Scenario: Role not reported
- **WHEN** the server's member list carries no role information
- **THEN** no participant is marked as administrator

### Requirement: Member list states
The panel SHALL show a loading state while the list is requested, an error state with the reason and a retry action when the request fails, and an offline notice instead of a stale list when the client is offline. Members SHALL NOT be stored in the offline cache.

#### Scenario: Loading
- **WHEN** the user opens the panel and the list has not arrived
- **THEN** the panel shows a loading state, not an empty list

#### Scenario: Request fails
- **WHEN** the server rejects the request or the network fails
- **THEN** the panel shows the reason and a retry action, and the chat itself keeps working

#### Scenario: Offline
- **GIVEN** the client is offline
- **WHEN** the user opens the panel
- **THEN** the panel says the member list is unavailable offline

### Requirement: Partial lists are never shown as complete
When the server reports that more members exist than it returned, the panel SHALL either load the remaining members or state that the list is partial; it SHALL NOT present a truncated list as the full one.

#### Scenario: Server returns a page
- **WHEN** the server's reply indicates that more members exist
- **THEN** the remaining members are loaded, or the panel states how many are shown out of how many

### Requirement: Start a private chat from a member
Each user row other than the current user's SHALL offer a "Написать" action that starts the private chat flow of `chat-creation` for that user, and SHALL NOT start anything without the user's action.

#### Scenario: Write to a member
- **WHEN** the user chooses "Написать" on a member who has no private chat with the account
- **THEN** the confirmation dialog of `chat-creation` opens for that user

#### Scenario: Own row
- **WHEN** the panel lists the current user
- **THEN** the row has no "Написать" action

### Requirement: Current user's profile
The client SHALL let the user open their own profile from the account menu. The profile SHALL be read-only and SHALL show the display name and avatar the server reports for the account, and the account's status text when the server's status information carries one, verbatim and without interpretation as availability or presence.

#### Scenario: Open the profile
- **WHEN** the user chooses the profile entry in the account menu
- **THEN** the profile shows the account's name and avatar, and the status text if the server gave one

#### Scenario: No status information
- **WHEN** the server's reply carries no status text
- **THEN** the profile shows no status line

#### Scenario: Read-only
- **WHEN** the profile is open
- **THEN** it has no control that changes the name, avatar or status

### Requirement: Viewing changes nothing
Opening the info panel or the profile SHALL send only read requests; it SHALL NOT change server state, and the client SHALL NOT send any membership, role, invite, chat-info or profile management request.

#### Scenario: Panel and profile opened
- **WHEN** the user opens the info panel and the profile
- **THEN** no request that changes server state is sent
