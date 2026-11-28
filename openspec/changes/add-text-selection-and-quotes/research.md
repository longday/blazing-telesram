# Research

Date 2026-11-28. Read-only: repository, the gpui 0.2.2 source in `~/.cargo/registry/src/index.crates.io-*/gpui-0.2.2`, and the public Yandex 360 help pages. No live call, no VM run. Everything marked **[probe]** is a claim that a task below must confirm before the tasks that depend on it start. Later results go into `research-gpui.md` (0.1), `research-bundle.md` (0.2), `research-wire.md` (0.3, 0.4, 0.6) and are folded into this file by 0.7.

## What the app does today (code, at `870e584`)

| Fact | Where |
|---|---|
| A text body is `text_block(text)`: a `div` with `max_w`, 15 px, `.child(text)`. Captions of an image or gallery use the same function. No `StyledText`, no hit-testing, no cursor style. | `crates/desktop/src/timeline.rs:467`, uses at `:387` and `:459` |
| The bubble handles only a right mouse down: it opens the message menu at the pointer. A left click on text does nothing. | `timeline.rs:172` |
| `MessageBody` is `Text(String)`, `Image`/`Gallery` with `caption: Option<String>`, `File`, `Voice`, `Video`, `Deleted`, `Unsupported`; there is no rich text: the text is the wire's `Text.MessageText` as received. | `crates/chat-core/src/model.rs:220` |
| The menu: `Reactors`, `Reply`, `Copy`, `Edit`, `Delete`; Copy writes `copy_text(m)` (the whole text, a caption or a file name) to the clipboard. | `desktop/composer.rs:72-139`, `:628` |
| Reply: `start_reply` sets `compose.reply = ReplyTarget { timestamp, author, text: quote_text(&message) }`; `quote_text` cuts the message text to `QUOTE_CHARS = 200` characters (a voice or video gives its `preview_text`). | `composer.rs:19`, `:166`, `:441` |
| The reply bar shows the author and the text as one line (`one_line`, `truncate`). | `composer.rs:807` |
| A reply in the timeline: `reply_quote` shows the author and `quote.text` as **one truncated line** at most 300 px wide. | `timeline.rs:356` |
| `ReplyTarget {timestamp, author, text}` is in `Draft` and `Command::Send` (serde, so drafts and the outbox persist it); `ReplyQuote {author, text, timestamp}` is the shown quote of a message (serde, cached). | `model.rs:172-185`, `:203` |
| Escape (`Dismiss`) closes the topmost thing: log, delete dialog, sign-out dialog, viewer, profile, new-chat dialog, reactors popover, **message menu**, info panel, then the reply/edit mode. | `composer.rs:540` |
| The timeline is `gpui::list(ListState, |ix| message_row(...))`; rows report their bounds in `RowContext.rows` (`on_children_prepainted`); `check_read` **drains** that vector each frame, so a second consumer must not drain it. A `canvas` after the list "runs after the list is laid out". The list reports no item bounds while it follows the bottom. | `timeline.rs:66-80`, `:122`; `desktop.rs:1545-1569`; `composer.rs:686` |
| The root div has key context `BlazingTelesram`, `track_focus(&self.focus)`, actions `Search`, `ToggleLog`, `Dismiss`, `ViewerPrev`, `ViewerNext`; bindings in `main.rs:68`. `TextInput` binds `cmd-a`, `cmd-c`, `cmd-x`, `cmd-v` (and `ctrl-`) in context `TextInput`, a deeper context, so it wins while an input has focus. **The app sets no macOS menu bar.** | `desktop.rs:1788-1804`; `main.rs:68-77`; `input.rs:1015-1040` |
| `TextInput` is the one place with selection today: byte-range selection over `WrappedLine` shaping, mouse down/drag (`is_selecting`), Shift+click extends; **no double-click word selection**, no triple click. It is private to the input (its layout is the input's own `Shaped`). | `input.rs:55-260`, `:394-417` |
| A delete is `Store::delete_message(chat, ts)` behind one confirmation (`delete_pending: Option<(ChatId, MessageTs)>`, dialog «Удалить сообщение?»). A delete is offered for `outgoing && Sent && !Deleted`. | `store.rs:644`; `composer.rs:118-137`, `:998` |
| **There is no forward**: no `Command`, no `WriteRequest` variant, no UI. `forwarded_from` is read only. `WriteRequest` covers send, edit, delete, seen, typing, reaction. | `model.rs:582`; `write.rs:55-93` |

## gpui 0.2.2: no selectable text

A search of the whole crate for `selectable`, `text_selection`, `SelectionRange`, `select_all`, `user_select` finds nothing: the framework has no selectable text primitive. What it has (all in `src/elements/text.rs`):

- `StyledText::new(text)`, `.with_highlights(ranges)` / `.with_default_highlights(style, ranges)` where `HighlightStyle` has `background_color` (`style.rs:496-507`), and `TextRun` has `background_color` (`text_system.rs:733-741`); `.layout()` returns a `TextLayout`.
- `TextLayout` is `Clone` (`Rc<RefCell<..>>`), so a handle can be captured by event closures. `index_for_position(point) -> Result<usize, usize>` (`Ok` inside a glyph, `Err` nearest index outside; **panics with "measurement has not been performed" / "prepaint has not been performed" if called before the element was laid out and prepainted**), `position_for_index`, `line_layout_for_index`, `bounds`, `line_height`, `text()`, `wrapped_text()`.
- `InteractiveText` (click and hover on ranges, no selection).
- `MouseDownEvent`/`MouseUpEvent` have `click_count` (`interactive.rs:104`, `:131`); `Div::on_mouse_move`, `on_mouse_up_out`, `on_mouse_down_out`, `on_drag_move`; `Window::on_mouse_event` (`window.rs:3421`, registered while painting, so per frame); `Styled::line_clamp(n)`; `ListState::scroll_by`, `scroll_to_reveal_item`, `viewport_bounds`.
- The test context has `simulate_mouse_down/up` and `simulate_click`; whether they set `click_count` above 1 is **[probe]** (task 1.8 may build the event by hand).

So the selection is built over `StyledText` + `TextLayout` hit-testing, in the style of `input.rs` but for read-only text spread over many list rows. Open gpui points for the spike (0.1): (a) do mouse move and mouse up reach a handler while the pointer is outside the element where the drag began (needed for dragging across rows and for ending a drag outside)? (b) can a `canvas` painted **before** the list clear a per-frame registry that the rows fill while painting (prepaint order), or is a frame stamp needed? (c) does `background_color` highlight paint across a wrapped line break without gaps, on a multi-run text? (d) does an entity change re-render list rows already measured (heights unchanged)? (e) the I-beam cursor (`CursorStyle::IBeam`) over text; (f) `click_count` for a double and a triple click on macOS and in the test context; (g) the cost of `cx.notify()` per mouse move on a full timeline.

## Product behaviour (public help)

`https://yandex.ru/support/yandex-360/customers/messenger/ru/actions` (read 2026-11-28):

- «Ответить на сообщение с цитированием»: 1. select the text to quote — "the interlocutor will see that you answer exactly that part of the message"; 2. right-click the message; 3. choose **Цитировать**. (Listed for the desktop client; the Android list has no such entry.)
- The context menu of the web client: **Ответить**, **Скопировать текст**, **Редактировать**, **Переслать**, **Закрепить**, **Добавить в сохранённые**, **Отметить как важное**, **Отметить непрочитанным**, **Просмотрено / … отреагировали**. The mockup `docs/mockups/5-menus.svg` also draws **Выбрать** (☑️), which the app does not have yet.
- "Максимальная длина одного сообщения — 4096 символов."
- `.../formatting`: "При копировании сообщения стили не сохраняются, копируется только текст. При ответе на сообщение текст в цитате отобразится без оформления. При пересылке сообщения форматирование текста сохранится." So copy and quote are plain text.

## Reply on the wire (repository)

- A reply is a `Plain` with `ForwardedMessageRefs: [{ChatId, Timestamp}]` and `ForwardedMessageStyles: [{Quote: <string>}]`; the quote is cut to `QUOTE_MAX_CHARS = 200` characters ("the reference's bound", `write.rs:27`, `:95-107`).
- The reader treats a quote style as a reply, with `quote` as the string and `reference` as the target (`wire.rs:679-708`); the shown text is `quote` or, without one, the original's `preview_text` (`yandex_backend.rs:1447-1473`).
- **The quote is stored independently of the original.** The `add-sending` design (edit section, D5): "a later edit restores only the quote text, not the link (verified live)"; `bt-probe` tests cover "the original was edited after the reply: the quote still shows the old text". So the server keeps `Quote` as a string and does not recompute it from the original, which is the precondition for a fragment quote being only a different string.
- A reply to a message is shown with the **original's author** from `ForwardedMessages[0].ServerMessageInfo.From`, not from the quote (fixture `research-history-page.json` line 15-17).
- Edit repeats `ReplyRef {timestamp, quote}` from the cached `reply_to` (`yandex_backend.rs:948`, `:968`), so an edited own reply keeps whatever quote text it shows, partial or not.
- **Unknown [probe]:** (1) whether the web client's «Цитировать» sends only `Quote` or also another key in the style entry (an offset, a length, a flag) or in `Plain`; (2) whether the server validates the string against the original (substring, length, normalisation) or keeps any string; (3) whether it keeps newlines in a multi-line quote and trims; (4) whether a quote above 200 characters is accepted and what the web client shows; (5) how many replies in a real history already carry a fragment quote and what their style entries look like; (6) how the web client renders a fragment quote (lines, truncation) so the timeline block matches.

## Verification plan

| Task | Question | Method | Needs |
|---|---|---|---|
| 0.1 | gpui points (a)-(g) | VM scratch crate, no account | VM |
| 0.2 | Unknowns (1), (6) | the public web client bundle: `ForwardedMessageStyles`, `Quote`, the «Цитировать» handler; anonymous GET | VM network |
| 0.3 | Unknown (5), (3) | `bt-probe history` over all chats, counts only; the shapes of style entries whose `Quote` differs from the original | VM, the live session, a live slot |
| 0.4 | (1), (3), (4), (6) when 0.3 finds no fragment quote or too few shapes | the other party's web client (admin account) sends fragment quotes in the private chat; the app account reads | **[approval]**, a live slot |
| 0.5 | none (code) | `bt-probe send-reply` with `--quote`, guarded | VM |
| 0.6 | (2), (3), (4) from the app account | run 0.5 in the private chat with `BT_WRITE_ONLY_CHAT` set; read back with `history`; see it in the web client | **[approval]**, a live slot |

## Results

None yet. 0.1-0.7 write them here.
