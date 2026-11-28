# Proposal

## Why

User report: in the app one cannot select messages, nor select part of a message's text to quote that part. Both are true today. A message's text is a plain gpui `div` child (`timeline.rs::text_block`): no hit-testing, no selection, no cursor; the only way to get text out is the context menu's «Скопировать текст», which copies the whole message. A reply quotes the whole message, cut to 200 characters (`composer.rs::quote_text`), and a message cannot be marked in the timeline at all, so nothing can be copied or deleted in bulk.

The web client already does the second part: select text, right-click, **«Цитировать»** — "the interlocutor sees that you answer exactly that part of the message" (Yandex 360 help, «Ответить на сообщение с цитированием»; quoted in `research.md`). The wire form of a reply already carries the quote as a free string next to the reference to the whole message (`ForwardedMessageRefs` + `ForwardedMessageStyles[].Quote`, `write.rs::add_reply`), so a partial quote is very likely only a different string; that is the one protocol point to verify before the quote tasks start (research 0.2-0.6). `research.md` (next to this file) holds the evidence from the code, gpui 0.2.2 and the public help.

## What Changes

Three slices, in this order of delivery (design D13), each its own merge unit:

- **Text selection (TS-A).** Mouse selection inside the text of message bodies and captions: drag, double click selects a word, triple click a paragraph, Shift+click extends; the selection may run across messages of one chat; Cmd+C copies it (a message with a partial selection copies the selected part, several messages are copied with a header per message); Cmd+A selects all loaded text of the chat when the timeline has focus; Escape and a click elsewhere clear it. «Скопировать текст» in the menu becomes «Скопировать выделенное» for a message that holds the selection. Selection is purely local state: no request, no write.
- **Quotes (TS-Q).** With a selection inside one message the menu offers «Цитировать» (before «Ответить»). It starts a reply whose quote is the selected fragment, not the message start: the reply bar shows «Цитата · author» and up to two lines of the fragment; the sent message shows the fragment as a quote block of up to three lines (a whole-message reply keeps today's one line). Quotes of other clients that are fragments are recognised and rendered the same way. The fragment is trimmed and cut to the existing 200-character bound; edit, drafts, the offline outbox and restarts keep it.
- **Message selection (TS-M).** The menu entry «Выбрать» (it is in `docs/mockups/5-menus.svg`) and Cmd+click enter a selection mode: a click toggles a message, Shift+click selects the range, a bar replaces the composer with «Выбрано: N», **Скопировать**, **Удалить** (only when every selected message is the user's own sent message, one confirmation for all) and a close button; Escape or the close button leaves the mode. **Forward is not offered: the app has no forward (no write request, no command; the mockup `8-forward.svg` is not implemented)**, so it is a follow-up.

Protocol and probe: `yandex-proto` needs **no change if the research confirms** that `Quote` is a free string. `bt-probe` gains one guarded write command (`send-reply <chat> <text> --reply <micros> [--quote <text>]`, refused unless `BT_WRITE_ONLY_CHAT` names `<chat>`) so the live send check does not need the app. `chat-core`: `ReplyQuote` and `ReplyTarget` gain `partial: bool` (`serde(default)`).

## Capabilities

### New Capabilities
- `text-selection`: selecting and copying text inside message bodies, within and across messages.
- `message-quotes`: replying to a selected fragment of a message and showing fragment quotes.
- `message-selection`: selecting whole messages and acting on the selection (copy, delete own).

### Modified Capabilities
- `message-editing`: «Message context menu» gains «Цитировать», «Скопировать выделенное» and «Выбрать».
- `message-sending`: «Reply» (the bar shows a quote, the quote survives drafts and the outbox).
- `message-history`: «Message content» (a fragment quote shows up to three lines).

## Impact

- `crates/desktop`: new `selection.rs` (pure model: positions, words, paragraphs, extraction, copy format, quote fragment rules), new `desktop/selecting.rs` (the `Desktop` side: state machine, mouse and keys, selection mode, selection bar), `timeline.rs` (selectable text, quote block, mode gutter), `desktop.rs` (state, canvas for drag capture, actions), `desktop/composer.rs` (menu entries, `dismiss`, quote bar, composer replaced by the selection bar), `format.rs`, `theme.rs`, `main.rs` (module line, key bindings), `yandex_backend.rs` (the `partial` flag), `tests.rs`.
- `crates/chat-core`: `model.rs`, `fixtures.rs`, `client.rs` (pending copy of a quote), `cache.rs` (test literal).
- `crates/yandex-proto`: `bin/bt-probe.rs`, `tests/probe_write_guard.rs` and `write.rs` (task 0.5: the probe-only uncut-quote encoder entry point; the app's path keeps the 200 cut); `write.rs`, `wire.rs` again only if the research finds extra quote keys (task 2.3).
- Specs: three new, three modified (above). No change to `message-reactions`, `reaction-list`, `offline-cache` (a legacy row without `partial` loads as a whole-message quote: `serde(default)`, scenario in `message-quotes`).
- Risks: (1) gpui 0.2.2 has no selectable text, so everything is built over `StyledText`/`TextLayout` (design D3); its details (drag outside the element, ordering of prepaint, highlight over wrapped lines) are confirmed by the spike 0.1 before the UI tasks start. (2) The server may validate the quote against the original; then the whole quote slice stops at 0.7 and the user decides (design D9). (3) Overlap with the open change `add-notifications-and-badge` (NB-N) on `desktop.rs`, `main.rs`, `tests.rs`, chat-core `model.rs` and `client.rs`: whoever starts a file first keeps it until its unit merges (`DEPENDENCIES.md` rules 1 and 6).

## Non-goals

- Forward (and forward from the selection bar): no write path exists; `8-forward.svg` stays unimplemented. A follow-up change.
- Selecting text outside message bodies: author names, times, reply quotes, file names, chat list, composer (the composer's own selection is unchanged), the info panel.
- Rich text: the text is shown and copied as received (the web client's markup characters stay characters); a quote is plain text, as in the web client ("в цитате отобразится без оформления").
- Jumping from a quote to the original message and highlighting the fragment there.
- Drag and drop of the selection, a selection handle UI, a macOS menu bar (the app has none; Cmd+C and Cmd+A are key bindings).
- Selecting across chats, or across a reload that removes the selected messages.
- Pins, importance, reactions on the selection.

## Follow-ups

- Forward of one or many messages (the selection bar already has the place for it).
- Jump to the quoted message and highlight the fragment.
- A word-level selection with the keyboard (Shift+arrows in the timeline).

## Decisions

Taken from the evidence and Telegram Desktop's behaviour without asking, each reversible in `design.md`; the ones that most need a look are repeated under "Open questions".

1. **Quote = a string; the reference stays the whole message.** `Quote` carries the fragment, `ForwardedMessageRefs` the original (D9). No new wire key unless the research finds one.
2. **Selection is client state keyed by message timestamp and byte offset**, not by pixels, so it survives virtualised rows, scrolling and loading older history (D1, D5). Copying is built from the store, never from layouts.
3. **One selection at a time**: a text selection or a message selection, never both; entering message mode clears the text selection (D11).
4. **No new graphics.** The quote and check marks use existing icons and glyphs; no Telegram Desktop asset is added, so the licence note of `config.yaml` is not triggered.
5. **Writes**: nothing here writes except what the user already does today: a send (a reply with a fragment quote) and a delete (once per selected own message, after one confirmation). Selecting, copying and quoting a fragment send nothing.

## Open questions

Defaults are recorded and the spec is written for them; the main session may change any answer before the tasks they gate start.

- **Q1 Cross-message text selection.** Default **yes** (as in Telegram Desktop). Alternative: one message only (drops task 1.6 and the autoscroll).
- **Q2 Copy format of several messages.** Default: per message a header line `Автор, ДД.ММ.ГГГГ ЧЧ:ММ`, the text, a blank line between messages; a selection inside one message copies the bare text. Alternative: bare texts only.
- **Q3 Cmd+A scope with the timeline focused.** Default: all loaded text of the open chat (message mode: all selectable loaded messages). Alternative: only the message that holds the selection.
- **Q4 Quote length.** Default: the existing 200 characters (cut, not refused). Task 0.6 observes whether the server accepts more; if it does the bound changes.
- **Q5 Selection mode details.** Default: entry by «Выбрать» and Cmd+click; right click does nothing while the mode is on; the bar replaces the composer (draft kept); Delete is disabled unless every selected message is deletable.
- **Q6 Live checks.** Tasks 0.4, 0.6 and 4.2 write to the account (the web client of the other party, then the app): they need the user's approval of the target. The proposed target is the private chat between the admin account and the app account, with `BT_WRITE_ONLY_CHAT` set to it.
