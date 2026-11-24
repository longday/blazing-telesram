# Spec Delta

## Purpose

Lets the user find messages, people and chats on the server from the sidebar search field, beyond what the loaded chat list contains, and jump to the chat of a result.

## ADDED Requirements

### Requirement: Server search for a typed query
While the search field holds a query of at least 2 characters (after trimming surrounding whitespace) and the client is online, the client SHALL ask the server for messages, people and chats matching it, and SHALL show the results below the local name matches.

#### Scenario: Query long enough
- **GIVEN** the client is online
- **WHEN** the user types "отчёт" and stops typing
- **THEN** the sidebar shows the local name matches first and the server results below them

#### Scenario: Query too short
- **WHEN** the search field holds one character, or only whitespace
- **THEN** no server request is made and only the local name matches are shown

#### Scenario: Field cleared
- **GIVEN** server results are shown
- **WHEN** the user clears the search field
- **THEN** the server results disappear and the normal chat list is shown

### Requirement: Query leaves the machine only when needed
The client SHALL send the query text to the server only after the user has paused typing for about 300 ms, SHALL NOT send it while offline, and SHALL NOT keep it or its results in the offline cache.

#### Scenario: Typing continues
- **WHEN** the user types a query one character at a time, each within 300 ms of the previous
- **THEN** one request is made for the final text, not one per character

#### Scenario: Offline
- **GIVEN** the client is offline
- **WHEN** the user types a query of at least 2 characters
- **THEN** no request is made and the search area states that server search needs a connection

#### Scenario: Restart
- **WHEN** the app is restarted after a search
- **THEN** neither the query nor its results are shown or restored

### Requirement: Results grouped by kind
Server results SHALL be shown in separate sections for chats, people and messages, each shown only when it has results. A chat already shown among the local name matches SHALL NOT be repeated in the chats section.

#### Scenario: Sections shown
- **WHEN** the server returns results of all three kinds
- **THEN** the sidebar shows a chats section, a people section and a messages section in that order

#### Scenario: Empty section hidden
- **WHEN** the server returns no people
- **THEN** no people section is shown

#### Scenario: Chat already listed locally
- **GIVEN** a chat matches the query by name locally
- **WHEN** the server also returns that chat
- **THEN** it appears once, among the local matches

### Requirement: Result row content
A chat or person result SHALL show the avatar (or initials) and the name; a message result SHALL show the name of its chat, the author, the time and the matching message text, truncated to one or two lines.

#### Scenario: Message result
- **WHEN** a message result is shown
- **THEN** its row shows the chat name, the author, the message time and the message text

#### Scenario: Result without avatar
- **WHEN** a chat or person result has no avatar
- **THEN** its row shows initials on a colored circle

### Requirement: Opening a result
Choosing a chat result or a message result SHALL open that chat. Choosing a person result SHALL open the private chat with that person when it is in the loaded chat list; a person without such a chat SHALL be shown but not openable. Searching and choosing a result SHALL NOT themselves report anything as read; an opened chat is marked read only as `read-markers` requires.

#### Scenario: Message result opens its chat
- **WHEN** the user clicks a message result
- **THEN** the chat of that message opens in the main area

#### Scenario: Person with an existing private chat
- **GIVEN** a private chat with a person is in the chat list
- **WHEN** the user clicks that person in the results
- **THEN** that private chat opens

#### Scenario: Person without a chat
- **GIVEN** no private chat with a person is in the chat list
- **WHEN** that person appears in the results
- **THEN** the row is shown without a click action

### Requirement: Search states
While a request is in flight the search area SHALL show a loading state; a successful reply with no results SHALL show an empty state; a failed request SHALL show an error state that does not remove the local name matches. Typing a new query SHALL leave the previous results visible until the new ones arrive or the query becomes shorter than 2 characters.

#### Scenario: Loading
- **WHEN** the request for a query is in flight
- **THEN** the search area shows that results are loading

#### Scenario: Nothing found
- **WHEN** the server replies with no messages, people or chats
- **THEN** the search area shows that nothing was found

#### Scenario: Request fails
- **WHEN** the request fails because of a network or server error
- **THEN** the search area shows an error and the local name matches stay visible

#### Scenario: Expired session
- **WHEN** the server rejects the session during a search
- **THEN** the client handles it as an expired session, as for any other request

### Requirement: Stale results are discarded
A reply to a query that is no longer the current one SHALL NOT change what is shown.

#### Scenario: Slow reply to an old query
- **GIVEN** the reply to "аб" has not arrived
- **WHEN** the user changes the query to "абв" and the reply to "аб" arrives afterwards
- **THEN** the results shown are those of "абв" only

#### Scenario: Reply after clearing
- **GIVEN** a request is in flight
- **WHEN** the user clears the field and the reply arrives
- **THEN** nothing from that reply is shown

### Requirement: First page only
The client SHALL request and show only the first page of results for each kind, up to a fixed limit, and SHALL NOT claim that the shown results are complete when the server reports more.

#### Scenario: More results exist
- **WHEN** the server reports more matches of a kind than were returned
- **THEN** the section shows the returned rows and no "show more" control

### Requirement: Search is read-only
The search request SHALL NOT change account state, and the query text and result message text SHALL appear in logs only at the `trace` level.

#### Scenario: Debug-level run
- **GIVEN** `BT_LOG=debug`
- **WHEN** the user searches for a word
- **THEN** a record shows the search request with its status and duration and no query text or result text
