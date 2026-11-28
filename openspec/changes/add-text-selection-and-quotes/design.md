# Design

## Context

`proposal.md` states the motivation, the decisions (numbered 1-5), and the open questions Q1-Q6; `research.md` the evidence and what a task must still confirm (**[probe]**). Contracts below are written for the defaults of the open questions.

Code this change extends: `timeline.rs` (`message_row`, `bubble`, `text_block`, `reply_quote`, `RowContext`, `RowBounds`), `desktop.rs` (the `gpui::list`, the probe `canvas`, the root div and its actions), `desktop/composer.rs` (menu, reply, `dismiss`, the quote bar, the delete dialog), `chat-core::model` (`ReplyQuote`, `ReplyTarget`, `Draft`), `yandex_backend.rs::message` (the reply mapping), `input.rs` (the existing selection and hit-testing over `WrappedLine`, used as the pattern, not reused: its layout is private to the input).

## Goals / Non-Goals

**Goals:** a text selection that feels like Telegram Desktop's (drag, word, paragraph, Shift, across messages, Cmd+C, Cmd+A); a fragment quote that round-trips with the web client; a message selection mode with copy and delete. All testable without a window where possible: the model is pure.

**Non-Goals:** the proposal's non-goals (forward, rich text, jump to the original, a menu bar).

## Decisions

### D1. Selection model (pure, `crates/desktop/src/selection.rs`)

```rust
/// The one selectable text of a message: `Text`, or the caption of an image or a gallery; nothing for the rest.
pub fn selectable_text(m: &Message) -> Option<&str>;

/// A caret: a message and a byte offset into its selectable text (always on a grapheme boundary).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Pos { pub ts: MessageTs, pub offset: usize }

/// `anchor` is where the gesture started, `focus` where the pointer is now; either may be the earlier one.
pub struct TextSelection { pub anchor: Pos, pub focus: Pos, pub unit: Unit, pub source: Vec<(MessageTs, Arc<str>)> }  // Unit: Char | Word | Paragraph, from the click count; `source` = the selectable text of each endpoint message as it was when the endpoint was set (one entry, or two when the endpoints differ)

pub struct MessageSet { /* sorted unique timestamps, `last: Option<MessageTs>` for Shift ranges */ }

pub enum Pick { None, Text(TextSelection), Messages(MessageSet) }  // introduced by TS-M (task 3.1); in TS-A the state holds an Option<TextSelection>

impl TextSelection {
    /// The selected byte range inside message `ts`: the whole text for a message strictly between the ends,
    /// the tail from the start, the head up to the end, or the part between them; `None` outside the selection.
    pub fn range_in(&self, messages: &[Message], ts: MessageTs) -> Option<Range<usize>>;
    pub fn is_empty(&self, messages: &[Message]) -> bool;
    /// `Some(ts)` when the selection lies inside one message.
    pub fn single(&self) -> Option<MessageTs>;
}
pub fn word_at(text: &str, offset: usize) -> Range<usize>;        // UAX #29 word bounds (unicode-segmentation, already a dependency); a run of spaces or punctuation is its own word
pub fn paragraph_at(text: &str, offset: usize) -> Range<usize>;   // between `\n`; the line break is not included
pub fn extract_text(messages: &[Message], sel: &TextSelection, header: impl Fn(&Message) -> String) -> String;  // D7; TS-M adds extract_messages for a MessageSet
pub fn select_all(messages: &[Message]) -> Option<TextSelection>;
```

The order of messages is the order of the loaded `messages` vector (oldest first). Endpoints are looked up by `ts` with `binary_search`. `validate(&messages)` drops the selection (D8) when an endpoint's message is gone, or its current `selectable_text` is **not equal to the snapshot in `source`**, or the offset is not on a char boundary. Comparing the text, not only the offsets, is what catches an edit that keeps the length (`hello world` → `hello there`): the offsets would still be in range and a quote would silently cite other text than the user selected. The snapshot holds only the endpoint texts (at most two, each a message of at most 4096 characters), so a Cmd+A over thousands of messages copies two strings; a message strictly between the ends is selected whole and is copied from the store as it is at copy time. `extract_text` and `fragment_quote` take their text from the validated snapshot or from the validated message, never from a stale one. Pending and failed messages are not special: a text selection on them works but is dropped when the server's copy replaces the row (its `ts` changes).

### D2. Where the state lives

`pub(crate) struct Picking(Entity<PickState>)` held by `Desktop`; `PickState { pick: Pick, drag: Option<Drag>, clicks: ClickTracker }`. `RowContext` gets `picking: Entity<PickState>` and `texts: TextLayouts` (D3); rows only read. User input calls back into `Desktop` through the `WeakEntity<Desktop>` that rows already carry (as the chips do): `text_down(chat, ts, offset, clicks, shift, cx)`, `drag_to(point, cx)`, `drag_end(cx)`, `toggle_message(chat, ts, shift, cx)`. The state machine itself lives in `desktop/selecting.rs` as methods of `PickState`/`Desktop`, unit-tested with synthetic events, no window. A change notifies `Desktop` (`cx.notify()`), which re-renders the visible rows; the highlight changes no height.

### D3. Text element, hit-testing, highlight

`text_block` becomes `selectable_text(ctx, m, text)`: a `div` with the existing size rules whose child is `StyledText::new(text).with_highlights(ranges)`, `ranges` = `[(range, HighlightStyle { background_color: Some(SELECTION), .. })]` from `range_in`; its `TextLayout` is cloned into a **registry** `TextLayouts = Rc<RefCell<Vec<(MessageTs, TextLayout)>>>` in `RowContext`. Hit-testing is `layout.index_for_position(point)`: `Ok(i)` or `Err(i)` both give the caret index `i`, snapped to a grapheme boundary; a layout is trusted only if its row was painted in the latest frame (the registry is cleared by a `canvas` painted **before** the list, and rows push while painting; spike 0.1 confirms the order, and a frame stamp is the fallback) and only after its prepaint (the method panics otherwise, so the registry holds only measured layouts). The selection colour is a new `theme.rs` constant (the accent at 35 % over the bubble); text colour is unchanged. The text div sets `CursorStyle::IBeam`.

**Gestures on the text** (`on_mouse_down(Left)`): `click_count` 1 puts the caret (clears the selection; with Shift it moves `focus` and keeps `anchor`); 2 selects the word, 3 (and more) the paragraph; the drag that follows extends by that unit (word by word, paragraph by paragraph), like a text field. **Drag capture:** a mouse move or up over another row or outside the list must still reach the state machine, so while `drag` is `Some` a `canvas` after the list registers `window.on_mouse_event` handlers for `MouseMoveEvent` and `MouseUpEvent` (the same canvas pattern the timeline already uses); mouse up ends the drag, also outside the window's content (`on_mouse_up_out` as a backup). Spike 0.1 (a) decides whether the element-level handlers suffice.

### D4. Keyboard and focus

New actions `CopySelection` and `SelectAllText` in `desktop.rs`'s `actions!`; bindings `secondary-c` and `secondary-a` in context `BlazingTelesram` (`main.rs`). **Primary modifier:** wherever this change says Cmd (Cmd+C, Cmd+A, Cmd+click) it means the platform's primary modifier, `secondary` in gpui 0.2.2 (`Keystroke::parse` and `Modifiers::secondary()`): Cmd on macOS, Ctrl on Linux and Windows, so `add-cross-platform-support` needs no change here. Ctrl+click is **not** the entry on macOS (there it is the secondary mouse click), and Cmd+click is not on the other platforms. A focused `TextInput` has deeper-context bindings and keeps its own copy and select-all. Starting a text selection (mouse down on text) and entering message mode **move focus to the root focus handle** (`window.focus(&self.focus)`), so the next Cmd+C goes to the selection, not to a composer that holds focus. A plain click on empty timeline space clears the selection and leaves focus alone. Cmd+C with nothing selected does nothing (no write of an empty clipboard). Q3: with the root focused and no selection, Cmd+A selects all loaded selectable text of the open chat (message mode: all selectable loaded messages). Escape order, inside `dismiss`: log, delete dialog, sign-out dialog, viewer, profile, new-chat dialog, reactors popover, message menu, **message mode or text selection**, info panel, reply/edit mode (so a selection is cleared before the info panel closes, and the reply bar is only cancelled when nothing is selected).

### D5. Across messages, scrolling, virtualisation

A selection is keyed by `(ts, offset)`, so it survives scrolling, rows leaving the viewport and older history loading above (timestamps do not change). While `drag` is active the point is mapped to a caret like this: over a registered layout of a painted row, `index_for_position`; between or beside bubbles of a painted row, the nearest row by the row's bounds (`RowContext.rows` is drained by `check_read`, so the registry keeps its own bounds), clamped to that row's text start or end by the y position (above the middle: start; below: end); above the first painted row: the start of that row; below the last: the end of it. When the pointer leaves the viewport vertically, a timer task (`cx.spawn`, 16 ms, dropped on mouse up) calls `ListState::scroll_by` with a speed proportional to the distance outside (capped), and re-maps the pointer after each step; the focus end moves to the newly painted rows. Rows without selectable text (a voice message, a photo without caption, a deleted placeholder) between the ends are skipped. The highlight of a message strictly between the ends is its whole text. Copying (D7) is built from the store's messages by the model, so rows that are not painted, or not even loaded as layouts, are copied correctly.

### D6. Menu changes (`composer.rs`)

`menu_actions` takes the current selection. For the message that holds the selection (`single() == Some(ts)` or a cross-message selection touching the message):

- `Quote` («Цитировать», selector `menu-quote`, first entry before `Reply`) only when `single() == Some(ts)`, the selected text is not blank, the message passes the same conditions as `Reply` (sent, with content, session not blocked). Not offered for a cross-message selection.
- `Copy` is labelled «Скопировать выделенное» (selector `menu-copy`, unchanged) and copies the selection (D7) when the selection touches this message; otherwise «Скопировать текст» as today.
- `Select` («Выбрать», `menu-select`, after `Copy`, icon none) for a sent message that is not deleted: enters message mode with this message selected (D11).

Right click on a message **keeps** the selection when it lies in that message (or touches it), and **clears** it otherwise (the menu opens for the clicked message with no selection entries), as in Telegram. A right click does not extend or move the selection.

### D7. Copy format

`extract` returns: for a text selection inside one message, the bare selected text; for a selection over several messages, and for a message set, per message with selectable text a block `header(m)\n{text}`, blocks separated by one blank line, in timeline order; the first block of a text selection has the tail from its start offset, the last the head up to its end; messages without copyable text contribute nothing. `header(m)` is `{author}, {ДД.ММ.ГГГГ ЧЧ:ММ}` (author name, or «Неизвестный», in the app's clock); `format.rs` gets the date-time helper (Q2). Message mode copies the whole `copy_text(m)` of each message (a file name for a file, as the menu's Copy does today); one message copies bare text. Line endings are `\n`. The clipboard gets plain text only.

### D8. Lifetime of a selection

Cleared by: Escape (D4), a plain left click on empty timeline space or on a text with `click_count` 1 and no drag, switching the chat, opening the viewer or a dialog (the overlays are above the list), sign-out, the account switch, starting message mode, entering Edit mode for the selected message. Dropped by `validate` (each render): an endpoint whose message was deleted or replaced, or whose selectable text is no longer equal to the snapshot taken when the endpoint was set (any edit, **including one that keeps the length**, since the offsets would still be in range but mean other text). **Kept** on a new incoming message, an upsert that leaves the text unchanged, scrolling, loading older history, a reaction change, a read marker. A drag in progress ends if the chat changes. Selection never writes: no `Command` is sent.

### D9. Quote on the wire and in the model

**Wire:** the unchanged `WriteRequest::SendText`/`Edit` with `ReplyRef { timestamp, quote }`: `ForwardedMessageRefs` names the whole original (so the web client shows its author and can jump to it), `ForwardedMessageStyles[0].Quote` is the fragment. `quote()` already cuts at 200 characters. If research 0.2-0.6 finds one more key (an offset, a length, a flag), task 2.3 adds it to `ReplyRef`, writes it in `add_reply` and reads it in `wire.rs::relation`; otherwise 2.3 has no work. If the server **rejects or rewrites** a quote that is not the original's text (0.6), **the quote slice (TS-Q) stops** and the main session decides between dropping the feature and a client-side substitute (the fragment as the reply's own text, which loses the link semantics); the web client has no markdown `>` quote to imitate. The text-selection and message-selection slices do not depend on this.

**Model:** `ReplyQuote.partial: bool` and `ReplyTarget.partial: bool`, both `#[serde(default)]` (a cached message, a draft or an outbox row written before this change loads as a whole-message quote and renders as today). `partial` is not on the wire; it is derived by one pure function in `selection.rs`:

```rust
/// True when `quote` is a fragment of `original`, i.e. it is **not** the whole original: after normalising both (`\r\n` → `\n`, trimmed), `quote != first QUOTE_CHARS characters of original`. The one rule, used by the composer, the adapter and the timeline. So the quote of a reply action (the whole text, cut at 200) is not a fragment; a **shorter beginning is** (`hello` quoted from `hello world` is a fragment: the user chose to quote just that part, and it renders as such); the middle, the end and any non-substring are fragments.
pub fn is_fragment(original: &str, quote: &str) -> bool;
/// The quote text of a selected fragment: line endings `\n`, trimmed, cut to `QUOTE_CHARS` characters on a char boundary; `None` when blank.
pub fn fragment_quote(selected: &str) -> Option<String>;
```

The composer sets `partial = is_fragment(whole message text, fragment)` (selecting the whole text is a whole-message reply: `partial` false); the adapter (`yandex_backend.rs::message`) sets it from the wire's `quote` and the original's text, `false` when the original is absent (deleted, or not sent); `client.rs` copies the target's flag into the pending message's `reply_to`. The rule is refined by research 0.3 (what other clients send: a whole quote above the cut, normalised whitespace). A fragment that the server stores differently from what was sent shows what the server returns, as for any message.

### D10. Quote UX

**Composer:** `start_quote(ts, fragment)` does what `start_reply` does (cancels an edit, schedules the draft save, focuses the input) with the fragment as `text` and `partial` set; then clears the selection. The bar keeps selector `reply-bar` and the reply icon; for `partial` its title is «Цитата · {author}» («Цитата» alone without an author) and the text part shows up to **two** lines of the fragment (`line_clamp(2)`, newlines kept) instead of `one_line`; selector `reply-bar-quote` on the text. Escape, the close button, send and the draft rules are the reply's. **Timeline:** `reply_quote` renders a `partial` quote as the author and the text with `line_clamp(3)` (newlines kept, ellipsis at the end) inside the same bordered block; a non-partial quote is unchanged (one truncated line). Selector `msg-{ts}-reply` unchanged, `msg-{ts}-reply-fragment` on the text of a fragment. Edit of an own reply keeps the quote as shown (D9, existing rule); a refused send puts the reply target back with its flag (`with_refused`, existing path).

### D11. Message selection mode

`Pick::Messages(MessageSet)`: **eligible** messages are `Sent` and not `Deleted` (a placeholder and an unsent message cannot be acted on). Entry: the menu entry «Выбрать» (D6) or **Cmd+click** (the primary modifier, D4: Cmd on macOS, Ctrl on Linux and Windows) on a bubble. While the mode is on: a left click anywhere on an eligible row toggles it (a full-row transparent layer swallows the click, so a chip, a picture, a play button, «Повторить» and a link do **not** act, and a text drag does not start); Shift+click selects the inclusive range between the last toggled message and the clicked one (the range follows the loaded order, ineligible ones skipped); Cmd+A selects all eligible loaded messages; Escape or the bar's ✕ leaves the mode; a right click does nothing (the menu is closed to avoid two surfaces); leaving the chat leaves the mode. The mode ends by itself when the selection becomes empty by toggling the last message off.

**Row:** a 28 px gutter on the left holds a round check (○, ✓ in the accent colour; text glyphs, no new graphic), the row gets a tint (a new `theme.rs` colour). **Bar** (`desktop/selecting.rs`, rendered by `composer_view` instead of the composer while the mode is on, the input and its draft untouched): `selection-bar`, `selection-count` «Выбрано: N», `selection-copy` («Скопировать», enabled when at least one selected message has copyable text), `selection-delete` («Удалить», **enabled only when every selected message is the user's own sent message**), `selection-cancel` (✕). **Delete** shows one confirmation «Удалить N сообщений?» («Удалить сообщение?» for one; Russian plural by a `format.rs` helper) with the existing text «…для всех участников чата.»; confirming sends one `Command::Delete` per message through `Store::delete_message`, in timeline order, closes the mode, and each failure is reported by the existing «Failed edits and deletions» path (a toast with the reason; the message keeps its state). The delete dialog's state becomes a list of `(chat, ts)` (`delete_pending: Vec<(ChatId, MessageTs)>`, `desktop.rs`). Copy uses D7 and leaves the mode on.

### D12. Forward is not here

The selection bar has no «Переслать». A forward needs a write request (`ForwardedMessageRefs` without own text, in another chat), a command, a chat picker (`8-forward.svg`) and cross-chat rules; none exist. It is a follow-up that will add the button to this bar.

### D13. Order and units (see `tasks.md` "Merge units")

TS-A first (text selection, the most visible gap and the base of the quote), then TS-Q (needs TS-A's model and menu code and the research outcome 0.7), then TS-M (needs only TS-A's files; ordered after TS-Q because both edit `timeline.rs`, `composer.rs`, `desktop.rs`, `tests.rs`, and quotes are the user's second ask). Research 0.1 gates TS-A's UI tasks; 0.7 gates TS-Q. The research tasks merge as docs; 0.5 (the probe command) merges alone.

## Risks / Trade-offs

- **gpui.** No selectable-text primitive; the drag-capture and registry mechanisms are assumptions until 0.1 runs. Fallback for drag capture: window-level handlers in the `canvas`; fallback for the registry: per-row bounds already reported by `on_children_prepainted`.
- **`cx.notify()` per mouse move** re-renders the whole `Desktop`; `input.rs` already does the same per move. If the spike shows jank on a long chat, notification is coalesced per frame.
- **Offsets and grapheme clusters.** Offsets are bytes on grapheme boundaries (emoji with joiners, combining marks); `index_for_position` returns char-boundary byte indexes, which the model snaps.
- **Wrapped text height with a highlight.** The highlight changes runs, not metrics; the text div's `max_w` workaround (`text_block` comment) stays.
- **The quote may be validated by the server** (D9). Cheapest to learn first: 0.6.
- **Cmd+A in the timeline selects potentially thousands of loaded messages;** the copy is built once, from the store, on Cmd+C only.
- **The 200-character quote bound** cuts a long selection silently (Q4); the reply bar shows what will be sent, so the user sees the cut before sending.
- **Overlap.** `add-notifications-and-badge` NB-N (`desktop.rs`, `main.rs`, `tests.rs`, chat-core `model.rs`, `client.rs`) and TS tasks share files; rule 6 applies, no task here assumes an order with NB-N.
