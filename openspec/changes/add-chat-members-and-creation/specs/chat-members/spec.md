# chat-members Specification

## Purpose

Lets the user see who is in a chat and see their own profile, read-only, so they know who they are talking to without leaving the client.

## ADDED Requirements

### Requirement: Chat info panel
The client SHALL show a chat info panel for the open chat, opened from the chat header and closed by the user, that lists the chat's participants. For a group chat the panel SHALL list the users and, when the server reports any, the groups and departments, as the server's member list gives them. Each user row SHALL show the avatar (or initials) and the name.

#### Scenario: Open the panel of a group
- **WHEN** the user opens the info panel of a group chat
- **THEN** the panel lists the group's users, and its groups and departments when the server reports any

#### Scenario: Close the panel
- **WHEN** the user closes the panel or opens another chat
- **THEN** the panel disappears and the chat timeline keeps its scroll position

### Requirement: Private chats need no member request
For a private chat the panel SHALL show the other participant known from the chat list entry and SHALL NOT send a member request, because the server does not provide a member list for private chats.

#### Scenario: Private chat panel needs no request
- **WHEN** the user opens the info panel of a private chat
- **THEN** no member request is sent and the panel shows the other participant named by the chat list entry, without any group or department sections

### Requirement: Admin role is shown
In a group chat the panel SHALL mark participants that the server reports with the admin role, and SHALL NOT mark anyone else: a participant with any other role, or with no role, is unmarked. The panel SHALL NOT assume administrators come first in the list.

#### Scenario: Group with administrators
- **WHEN** the server reports a participant with the admin role
- **THEN** that participant's row carries an administrator marker and the others do not

#### Scenario: Role not reported
- **WHEN** the participants carry no role information (a private chat's partner, or a member list without roles)
- **THEN** no participant is marked as administrator

### Requirement: Member list states
The panel SHALL show a loading state while the list is requested, an error state with the reason and a retry action when the request fails, an offline notice instead of a stale list when the client is offline, and a notice that the list is not available to the account when the server refuses it for lack of permission. Members SHALL NOT be stored in the offline cache.

#### Scenario: Loading
- **WHEN** the user opens the panel and the list has not arrived
- **THEN** the panel shows a loading state, not an empty list

#### Scenario: Request fails
- **WHEN** the server rejects the request or the network fails
- **THEN** the panel shows the reason and a retry action, and the chat itself keeps working

#### Scenario: Server refuses the list
- **WHEN** the server answers that the account has no permission to list the members of the chat (observed for channels)
- **THEN** the panel says the member list of this chat is not available to this account, does not show an empty list as if the chat had no members, and the chat itself keeps working

#### Scenario: Offline
- **GIVEN** the client is offline
- **WHEN** the user opens the panel
- **THEN** the panel says the member list is unavailable offline

### Requirement: Partial lists are never shown as complete
The server returns members in pages and reports neither a total nor a "more" flag; a page shorter than the requested size is the last one. The client SHALL load the following pages until the last page or a fixed upper bound, and when the bound is reached while the last page was full it SHALL state that the list may be incomplete and how many members are shown; it SHALL NOT present a bounded list as the full one.

#### Scenario: Group larger than one page
- **WHEN** the group has more members than one page holds and the bound is not reached
- **THEN** every page is loaded and the panel lists all members as complete

#### Scenario: Bound reached
- **WHEN** the upper bound of pages is reached and the last page was full
- **THEN** the panel states that the list may be incomplete and how many members are shown

#### Scenario: A later page fails
- **WHEN** a request for a later page fails
- **THEN** the panel shows the error state of the member list, not a shortened list presented as complete

### Requirement: Start a private chat from a member
Each user row other than the current user's SHALL offer a "Написать" action that starts the private chat flow of `chat-creation` for that user, and SHALL NOT start anything without the user's action.

#### Scenario: Write to a member
- **WHEN** the user chooses "Написать" on a member who has no private chat with the account
- **THEN** the confirmation dialog of `chat-creation` opens for that user

#### Scenario: Own row
- **WHEN** the panel lists the current user
- **THEN** the row has no "Написать" action

### Requirement: Current user's profile
The client SHALL let the user open their own profile from the account menu. The profile SHALL be read-only and SHALL show the display name and avatar the server reports for the account, and the account's status text when the server's status information carries a non-blank text, verbatim and without interpretation as availability or presence. When the profile cannot be read (offline or the request fails) the view SHALL say so instead of showing stale data.

#### Scenario: Open the profile
- **WHEN** the user chooses the profile entry in the account menu
- **THEN** the profile shows the account's name and avatar, and the status text if the server gave one

#### Scenario: No status information
- **WHEN** the server's status information is empty (the server sends an empty status when none is set) or carries no text
- **THEN** the profile shows no status line

#### Scenario: Read-only
- **WHEN** the profile is open
- **THEN** it has no control that changes the name, avatar or status

#### Scenario: Profile unavailable
- **WHEN** the client is offline or the profile request fails
- **THEN** the view says the profile could not be loaded and shows no name, avatar or status from an earlier session

### Requirement: Viewing changes nothing
Opening the info panel or the profile SHALL send only read requests; it SHALL NOT change server state, and the client SHALL NOT send any membership, role, invite, chat-info or profile management request.

#### Scenario: Panel and profile opened
- **WHEN** the user opens the info panel and the profile
- **THEN** no request that changes server state is sent
