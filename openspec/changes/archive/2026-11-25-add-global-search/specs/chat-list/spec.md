# Spec Delta

## MODIFIED Requirements

### Requirement: Local filtering
The client SHALL filter the loaded list by name as the user types and by kind (all, private, groups, channels, unread) without server requests; this filtering SHALL work offline and SHALL NOT depend on, or wait for, server search results (see `message-search`).

#### Scenario: Search by name
- **WHEN** the user types part of a chat name
- **THEN** only chats whose name contains it, case-insensitively, remain visible

#### Scenario: Unread filter
- **WHEN** the user selects the unread filter
- **THEN** only chats with unread messages remain visible

#### Scenario: Server search unavailable
- **GIVEN** the client is offline or the server search failed
- **WHEN** the user types part of a chat name
- **THEN** the local name matches are still shown immediately

## ADDED Requirements

### Requirement: Server results below the list
For a non-empty query the sidebar SHALL show the local matches first, unchanged in content and order, and the server search results of `message-search` below them; the kind filter SHALL narrow only the local matches.

#### Scenario: Local matches stay on top
- **WHEN** the user types a query that matches a chat name and the server returns message results
- **THEN** the matching chat rows appear above the server result sections

#### Scenario: Kind filter with server results
- **GIVEN** the groups filter is selected
- **WHEN** server results are shown for a query
- **THEN** only group chats remain among the local matches and the server sections are not narrowed

#### Scenario: Empty query
- **WHEN** the search field is empty
- **THEN** the sidebar shows the filtered chat list only, with no server result sections
