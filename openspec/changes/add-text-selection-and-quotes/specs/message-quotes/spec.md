# Spec Delta

## Purpose
Lets the user reply to a selected fragment of a message instead of the whole message, and shows such quoted fragments, from this app or from other clients.

## ADDED Requirements

### Requirement: Quote a selected fragment
With a non-blank text selection inside one message, the context menu of that message SHALL offer «Цитировать» before «Ответить» when the message can be replied to. Choosing it SHALL start a reply to that message whose quote is the selected fragment, SHALL clear the selection and SHALL put the focus in the composer. «Цитировать» SHALL NOT be offered for a selection over several messages, for a blank selection, or for a message that cannot be replied to.

#### Scenario: Quote a fragment
- **WHEN** the user selects "world" in "hello world", right-clicks the message and chooses «Цитировать»
- **THEN** the composer shows a reply bar with the quote "world" and the selection is cleared

#### Scenario: No selection
- **WHEN** the user right-clicks a message with no selection
- **THEN** the menu does not offer «Цитировать»

#### Scenario: Selection over several messages
- **WHEN** the selection covers two messages and the user right-clicks one of them
- **THEN** the menu does not offer «Цитировать»

#### Scenario: Blank selection
- **WHEN** only spaces or line breaks are selected
- **THEN** the menu does not offer «Цитировать»

#### Scenario: Right-click elsewhere
- **WHEN** a selection exists in one message and the user right-clicks another message
- **THEN** the selection is cleared and the menu for the clicked message does not offer «Цитировать»

#### Scenario: Whole text selected
- **WHEN** the user selects the entire text of a short message and chooses «Цитировать»
- **THEN** the reply is a whole-message reply, shown as before

### Requirement: Quote bar for a fragment
While a fragment quote is pending, the bar above the composer SHALL show the title «Цитата · {author}» and the fragment up to two lines with its line breaks, and SHALL be dismissed like a reply bar (Escape, the close button, or sending). The reply bar of a whole message SHALL stay as it is.

#### Scenario: Multi-line fragment
- **WHEN** the user quotes a fragment of three lines
- **THEN** the bar shows its first two lines and an ellipsis

#### Scenario: Cancel
- **WHEN** the quote bar is shown and the user presses Escape
- **THEN** the bar disappears and the next message is sent without a quote

#### Scenario: Whole-message reply
- **WHEN** the user chooses «Ответить» on a message
- **THEN** the bar shows the author and one line of the text, as before

### Requirement: Send a fragment quote
Sending with a fragment quote SHALL send a reply to the whole original message whose quote text is the fragment, trimmed, with line breaks as `\n`, and cut at 200 characters at a character boundary. The sent message SHALL show the fragment as its quote, here and in other clients. The pending message SHALL show the quote before the server confirms it.

#### Scenario: Send
- **WHEN** the user quotes "world" from "hello world" and sends "yes"
- **THEN** the sent message "yes" shows the quote "world" with the original's author, here and in the web client

#### Scenario: Long fragment
- **WHEN** the user quotes a fragment of 300 characters and sends
- **THEN** the quote is the first 200 characters of the fragment, and the bar showed that cut before sending

#### Scenario: Pending
- **WHEN** the user sends a fragment reply while offline
- **THEN** the pending message shows the fragment quote and is delivered with it after connecting

#### Scenario: Send refused
- **WHEN** the server refuses the message
- **THEN** the message is shown as failed with the reason and the quote is restored with its flag when the user retries or cancels

### Requirement: Fragment quote in the timeline
A reply whose quote is a fragment of its original — the trimmed quote differs from the beginning of the trimmed original text — SHALL show the author and the fragment as up to three lines, with line breaks kept and an ellipsis when cut, in the same bordered block as other quotes. A reply whose quote is the beginning of the original, or whose original is not known, SHALL show one truncated line as before.

#### Scenario: Fragment from another client
- **WHEN** another client sends a reply whose quote is a word from the middle of the original
- **THEN** the message shows the author and that word in the quote block

#### Scenario: Three lines
- **WHEN** a fragment quote has five lines
- **THEN** the block shows three and ends with an ellipsis

#### Scenario: Whole-message quote
- **WHEN** a reply quotes the beginning of its original, as the reply action does
- **THEN** the block shows one truncated line

#### Scenario: Original edited later
- **WHEN** the original of a fragment quote is edited
- **THEN** the quote keeps showing the text it had when the reply was sent

### Requirement: The fragment survives
The fragment quote SHALL be kept when the user edits the reply, when a draft with a pending quote is stored and restored, when a pending reply waits in the outbox across an app restart, and in the cache. Data stored before this capability, without the fragment flag, SHALL load as a whole-message quote.

#### Scenario: Edit a fragment reply
- **WHEN** the user edits the text of their fragment reply
- **THEN** the reply keeps the same fragment quote, here and in other clients

#### Scenario: Draft
- **WHEN** the user starts a fragment quote, types, opens another chat and returns
- **THEN** the bar shows the same fragment and the typed text

#### Scenario: Restart with a pending reply
- **GIVEN** a fragment reply is pending
- **WHEN** the app is restarted
- **THEN** it is still pending with the same fragment quote

#### Scenario: Old cache row
- **WHEN** a cached message or draft written before this capability has a quote without the flag
- **THEN** it loads and shows as a whole-message quote

### Requirement: Quoting writes only a send
Selecting a fragment and choosing «Цитировать» SHALL send nothing; the only request is the user's send of the reply.

#### Scenario: Quote and cancel
- **WHEN** the user chooses «Цитировать» and cancels with Escape
- **THEN** no request that changes server state is sent
