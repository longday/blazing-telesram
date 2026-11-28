# user-card Specification

## Purpose

Lets the user see who another person is: a read-only card with the name, avatar and the directory details the server gives, opened from the places where that person appears in a chat, with copy and write actions and no effect on the account.

## ADDED Requirements

### Requirement: Open the card from a person
The client SHALL open the card of a person when the user clicks that person's avatar or author name on a message in a group chat's timeline, the avatar or name part of the person's row in the chat info panel, or the avatar in the header of a private chat whose chat list entry names that person as the partner. A click that opens the card SHALL cause no other action. In message selection mode, and on a Cmd+click, the row's own behaviour wins and no card opens.

#### Scenario: Timeline avatar
- **WHEN** the user clicks the avatar next to a message of another person in a group chat
- **THEN** the card of that message's author opens

#### Scenario: Timeline author name
- **WHEN** the user clicks the author name above the first message of a run in a group chat
- **THEN** the card of that author opens, and the message is not selected or otherwise acted on

#### Scenario: Message selection mode
- **GIVEN** the message selection mode of `add-text-selection-and-quotes` is on
- **WHEN** the user clicks the avatar or the author name of a message
- **THEN** the message is toggled as by any click on its row and no card opens

#### Scenario: Cmd+click with selection mode off
- **GIVEN** the message selection mode is off
- **WHEN** the user Cmd+clicks (the platform's primary modifier) the avatar or the author name of a sent message
- **THEN** the mode starts with that message selected, as for a Cmd+click anywhere on the row, and no card opens

#### Scenario: Member row
- **WHEN** the user clicks the avatar or the name in a person's row of the info panel
- **THEN** the card of that person opens

#### Scenario: Member row action keeps its own meaning
- **WHEN** the user clicks «Написать» in a member row
- **THEN** the write flow starts as before and the card does not open

#### Scenario: Private chat header
- **WHEN** the user clicks the avatar in the header of a private chat with a named partner
- **THEN** the card of the partner opens, and the info panel does not toggle

#### Scenario: Private chat header outside the avatar
- **WHEN** the user clicks the title area of the same header
- **THEN** the info panel toggles as it does without this capability

#### Scenario: No person to open
- **WHEN** the user clicks the avatar of a message whose author has no identity (the sender carries no id), or the avatar of a chat that has no named partner
- **THEN** nothing opens

#### Scenario: Other chats are unchanged
- **WHEN** the user views a channel or a private chat timeline
- **THEN** no avatar or author name is added to the timeline because of this capability

### Requirement: The card of the current account
The card of the current user SHALL be the same card (the account's name, avatar and the details the server gives about it) without the «Написать» action. It SHALL NOT replace or change the profile view of the account menu.

#### Scenario: Own row in the panel
- **WHEN** the user clicks the avatar or the name in the current user's row of the info panel
- **THEN** the card opens with the account's name and avatar and has no «Написать»

#### Scenario: Own chat
- **WHEN** the user clicks the header avatar of the account's own chat and its chat list entry names the account as the partner
- **THEN** the card opens with no «Написать»

#### Scenario: Account menu unchanged
- **WHEN** the user opens «Профиль» from the account menu
- **THEN** the existing profile view opens, not the card

### Requirement: What the card shows
From a read of the server the card SHALL show the status text, the position, the department, the email and the nickname, each on its own labelled line, verbatim, and only when the server gave a non-blank value. A person with several employments SHALL show one position and department, the server's current one.

#### Scenario: Full record
- **GIVEN** the server's record of the person has a position, a department, an email and a nickname
- **WHEN** the card has loaded
- **THEN** the card shows the name and avatar and one labelled line for each of the four, with the values as the server spelled them

#### Scenario: Fields the server did not send
- **WHEN** the record has no department, no nickname or a blank value for either
- **THEN** the card shows no line, no label and no placeholder for it

#### Scenario: Status text
- **GIVEN** the server's status information for the person carries a non-blank text
- **WHEN** the card has loaded
- **THEN** the card shows that text verbatim under the name, without reading it as availability

#### Scenario: No status information
- **WHEN** the server sends an empty status or none for the person
- **THEN** the card shows no status line

### Requirement: Name and avatar
The card SHALL show the person's avatar (or initials) and name at once, from what the opening place already knew, replace them by the name and avatar the server reports once they arrive, and show «Без имени» when neither has a name.

#### Scenario: Name from the server replaces the opening name
- **WHEN** the card has loaded and the server's name differs from the one the opening place showed
- **THEN** the card shows the server's name

#### Scenario: No name anywhere
- **WHEN** neither the opening place nor the server has a name for the person
- **THEN** the card shows «Без имени» as the name and nothing invented

### Requirement: What the card never shows
The card SHALL NOT show presence (online, last seen), a phone number, gender, any internal flag or category, or the person's raw id, and SHALL NOT infer any of them from other fields.

#### Scenario: Record with presence-like and personal fields
- **GIVEN** the server's record carries a connection flag, a gender, a category and an opaque phone id
- **WHEN** the card has loaded
- **THEN** none of them appears in the card

### Requirement: Deleted accounts
When the server marks the person's account as deleted, the card SHALL say «Аккаунт удалён» in place of the detail lines and SHALL NOT offer «Написать» or «Копировать почту».

#### Scenario: Deleted account
- **GIVEN** the record of the person is marked as a deleted account
- **WHEN** the card has loaded
- **THEN** the card shows the name and avatar, the line «Аккаунт удалён», and no position, department, email, nickname or status

### Requirement: Card states
The card SHALL open at once, show a loading state for the details while they are requested, and when they cannot be read show a retry action with the offline notice, the server's or network's reason, or a not-found notice. The name and avatar the opening place knew SHALL stay visible in every state.

#### Scenario: Loading
- **WHEN** the user opens a card and the details have not arrived
- **THEN** the card shows the name and avatar and a loading line, not empty detail fields

#### Scenario: Request fails
- **WHEN** the server rejects the read or the network fails
- **THEN** the card shows the reason and a retry action, the name and avatar stay, and the chat itself keeps working

#### Scenario: Retry
- **WHEN** the user chooses the retry action
- **THEN** the details are requested again and the card shows the loading state until they arrive

#### Scenario: Offline
- **GIVEN** the client is offline
- **WHEN** the user opens a card
- **THEN** the card says the details are unavailable without a connection, and no request is sent

#### Scenario: Connection drops while the card is shown
- **GIVEN** the card shows loaded details
- **WHEN** the client goes offline
- **THEN** the card replaces the details by the offline notice, details that arrive while the client is not online are not shown, and when it is online again the card requests them anew

#### Scenario: Person not found
- **WHEN** the server has no record of the person
- **THEN** the card says the person was not found, with the name and avatar from the opening place and a retry action

### Requirement: No stale details
Details from an earlier opening or an earlier read SHALL never be shown as current: each opening and each retry reads the server anew, and an answer that belongs to another person or to a read that was superseded is dropped.

#### Scenario: Reopened card starts clean
- **GIVEN** the user has closed the card of a person
- **WHEN** the user opens the card of the same person again
- **THEN** the card shows the loading state first and shows only details of the new read

#### Scenario: Reopened before the first request completes
- **GIVEN** the user opened the card of person A, closed it and opened it again before A's first read was answered
- **WHEN** the first read's answer arrives, and then the second's
- **THEN** the first answer is not shown, a second read was sent, and the card shows the second answer

#### Scenario: Connection returns while loading
- **GIVEN** the card is loading and the connection drops and comes back
- **WHEN** an answer to the read sent before the drop arrives
- **THEN** it is not shown and the card asks again, showing only the new answer

#### Scenario: Another card while one is loading
- **GIVEN** the card of person A is loading
- **WHEN** the user closes it and opens the card of person B, and A's details arrive afterwards
- **THEN** A's details are not shown in B's card

### Requirement: Copy actions
The card SHALL offer «Копировать почту» when it shows an email and «Копировать ID» for the person, each putting exactly that value on the system clipboard and changing nothing else.

#### Scenario: Copy the email
- **WHEN** the user chooses «Копировать почту» on a card that shows an email
- **THEN** the clipboard holds that email exactly as shown

#### Scenario: Copy the id
- **WHEN** the user chooses «Копировать ID»
- **THEN** the clipboard holds the person's id, which the card does not display

#### Scenario: No email
- **WHEN** the card shows no email
- **THEN** it offers no «Копировать почту»

### Requirement: Write to the person
Once the details have loaded, the card SHALL offer «Написать» for every person other than the current user and other than a deleted account (before that the server has not said who the person is). Choosing it SHALL close the card and start the private chat flow of `chat-creation`: an existing private chat is opened, otherwise the confirmation dialog of `chat-creation` opens. The card SHALL NOT create a chat or send anything by itself.

#### Scenario: Person with a private chat
- **GIVEN** the account has a private chat with the person
- **WHEN** the user chooses «Написать»
- **THEN** the card closes and that chat is open

#### Scenario: Person without a private chat
- **GIVEN** the account has no private chat with the person
- **WHEN** the user chooses «Написать»
- **THEN** the card closes and the confirmation dialog of `chat-creation` opens for that person, and nothing is sent until the user confirms there

#### Scenario: Card of the chat partner
- **GIVEN** the card was opened from the header of the open private chat with that person
- **WHEN** the user chooses «Написать»
- **THEN** the card closes and the chat stays open

#### Scenario: Person outside the dialog's known people
- **GIVEN** the card was opened from the timeline of a group chat, the info panel is closed, and the account has no private chat with that person, who is therefore in no list of the new chat dialog
- **WHEN** the user chooses «Написать»
- **THEN** the card closes and the confirmation dialog opens with that person chosen, named as on the card, and nothing is sent until the user confirms there

### Requirement: Closing the card
The card SHALL close with its «Закрыть» action and with Escape, SHALL close when the session is no longer active, and SHALL block the chat underneath like the own profile view does.

#### Scenario: Close
- **WHEN** the user chooses «Закрыть» or presses Escape
- **THEN** the card disappears and the timeline keeps its scroll position

#### Scenario: Session ends
- **GIVEN** a card is open
- **WHEN** the session expires or the user signs out
- **THEN** the card closes and its details are forgotten

#### Scenario: Chat underneath
- **WHEN** the card is open
- **THEN** a click behind it reaches nothing, and no second card, profile view or dialog opens over it

### Requirement: Privacy of the card
The client SHALL keep a person's card details in memory only, for the card on screen: not in the offline cache, not in any table that outlives the card, and forgotten when the card closes, the session ends or the account is switched. No name, email, id, status text, position, department or clipboard text SHALL be written to the log at `debug` level or above, only counts and outcomes.

#### Scenario: Nothing is stored
- **WHEN** a card has been opened and closed
- **THEN** the offline cache holds nothing from it and a later opening asks the server again

#### Scenario: Log content
- **WHEN** a card loads, fails or is copied from with `debug` logging on
- **THEN** the log lines carry at most a result and a count, never a person's data

#### Scenario: Trace level is not covered
- **GIVEN** the developer enabled the request layer's `trace` level, which records raw reply bodies of every read
- **WHEN** a card loads
- **THEN** the card's replies are logged there as for any other read, and this capability changes nothing about that level

### Requirement: Viewing changes nothing
Opening a card SHALL send only read requests from the client's closed read set; it SHALL NOT change server state, send a presence or heartbeat message, or send any write.

#### Scenario: Card opened
- **WHEN** the user opens the card of a person
- **THEN** no request that changes server state is sent, and every method named in a request is on the read allowlist
