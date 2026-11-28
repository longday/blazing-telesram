# Tasks

Contracts (types, signatures, gestures, formats) are fixed in `design.md` D1-D13, so tasks can be coded against them. Each task names the files it owns; the rules of `openspec/changes/DEPENDENCIES.md` apply (one owner per file at task granularity, every merge to `main` compiles and passes tests with no stubs and no warnings, cargo, `bt-probe` and the app only in the Tart VM `bt-vm`, no `#[allow(dead_code)]`, live tasks one at a time, writes obey `BT_WRITE_ONLY_CHAT`). Unit tests of a task belong to that task; checks run per merge unit (see "Merge units") and task 4.1 is the final full run. `[live]` = needs the signed-in session; `[approval]` = needs the user's approval of the target first (the "needs-approval" tasks of the brief).

Open questions Q1-Q6 of `proposal.md` carry recorded defaults; no task is blocked by a question except through the gates below. **Gates:** research 0.1 gates the UI tasks 1.3, 1.5 (the gpui mechanisms of D3); research 0.7 gates the whole quote unit TS-Q (2.x). TS-A and TS-M do not depend on any live or wire result.

Order at a glance: 0.1, 0.2, 0.5 start at once; 0.3 follows a free live slot; 0.4 only if 0.3 finds too few fragment quotes and the user approves; 0.6 follows 0.5 and the approval; 0.7 follows 0.1-0.6. TS-A: 1.1, 1.2, 1.4 start at once (pure code), 1.3 follows 1.1 and 0.1, 1.5 follows 1.3, 1.4, 1.6 follows 1.1 and 1.5, 1.7 follows 1.5, 1.8 follows 1.3-1.7. TS-Q starts after TS-A is on `main` and 0.7: 2.1 and 2.3 (only if 0.7 says so) start at once, 2.2 and 2.4 follow 2.1 (2.4 also 2.2), 2.5 follows 1.7, 2.1, 2.2, 2.6 follows 2.1, 2.7 follows 2.1-2.6. TS-M starts after TS-Q is on `main`: 3.1, 3.2 start at once, 3.3 follows 3.1, 3.2, 3.4 follows 3.1, 3.5 follows 3.3, 3.4, 3.6 follows 3.3, 3.5, 3.7 follows 3.3-3.6. 4.1 per unit; 4.2 follows TS-M (and the approval); 4.3 last.

Compile coupling: `selection.rs` (1.1) and the `format.rs` helper (1.2) have no caller until 1.3-1.5; the selectable text (1.3) calls `Desktop` methods that 1.4 and 1.5 define; the model fields of 2.1 break every `ReplyQuote`/`ReplyTarget` literal until 2.4, 2.5, 2.7 fix theirs; `Pick::Messages` and the mode methods of TS-M have no caller until 3.6. None of these tasks can reach `main` alone, so each slice is one merge unit. `Pick` (the enum over text and message selection) is introduced by 3.1; in TS-A the state holds an `Option<TextSelection>` (a variant nobody constructs would be a `dead_code` warning).

Overlap: the open change `add-notifications-and-badge` NB-N holds `store.rs`, chat-core `model.rs` and `client.rs` (NB 1.2), `desktop.rs` (NB 4.2), `main.rs` (NB 2.2, 5.1), `tests.rs` (NB 6.2) once started; rule 6: whoever starts a file first keeps it until the unit merges. PIN and CM have no open task that edits any app file (PIN 6.x and CM 5.x are live checks). Tests are appended as new sections of `tests.rs`, headed `// ---- text selection (TS 1.8) ----`, `// ---- quotes (TS 2.7) ----`, `// ---- message selection (TS 3.7) ----`.

## Merge units

Work happens on integration branches; a unit reaches `main` only after the checks below pass on the unit's tree, run in `bt-vm` in its own `~/bt/src-select` (never the shared `~/bt/src`), and after `git pull --rebase`.

- **TS-research** = 0.1, 0.2, 0.3, 0.4, 0.6, 0.7: docs only (`research-gpui.md`, `research-bundle.md`, `research-wire.md`, `research.md`, `design.md`, `tasks.md`, specs); each merges as it finishes, no checks beyond `openspec validate add-text-selection-and-quotes --strict`.
- **TS-probe** = 0.5 alone (branch `integ/ts-probe`): the probe command is its own consumer. Checks: `cargo build --workspace --all-targets --locked` with zero warnings, `cargo test -p yandex-proto --locked`, `cargo clippy -p yandex-proto --all-targets --locked` (no new warnings), `cargo fmt --all --check`. Merges before 0.6.
- **TS-A** (text selection) = 1.1-1.8 (branch `integ/add-text-selection-and-quotes`). Needs 0.1; independent of the wire results. Checks: `cargo build --workspace --all-targets --locked` zero warnings, `cargo test --workspace --locked`, `cargo clippy --workspace --all-targets --locked` (no new warnings over the 10-warning baseline), `cargo fmt --all --check`, `openspec validate add-text-selection-and-quotes --strict`.
- **TS-Q** (quotes) = 2.1-2.7, on the same integration branch after TS-A is on `main`; needs 0.7. Same checks.
- **TS-M** (message selection) = 3.1-3.7, after TS-Q is on `main` (shared files: `timeline.rs`, `desktop.rs`, `composer.rs`, `selecting.rs`, `selection.rs`, `format.rs`, `tests.rs`). Same checks. TS-M needs none of 0.2-0.7, so the main session may swap the order (TS-M before TS-Q) if the quote research stalls: no task of TS-M reads a TS-Q result; only the `selection.rs`, `desktop.rs`, `composer.rs` hunks would merge in the other order.

## 0. Research (evidence for the open points)

- [x] 0.1 gpui spike: the mechanisms of D3 (research.md "gpui 0.2.2", points (a)-(g))
  - Owns: new `openspec/changes/add-text-selection-and-quotes/research-gpui.md`; a **scratch crate in the VM** (`~/bt/spike-select/`, not committed, deleted at the end).
  - Depends on: nothing. No account, no live slot (the VM needs no session).
  - Do: a scratch gpui 0.2.2 app (the version of `Cargo.lock`) with a `gpui::list` of 60 rows of wrapped `StyledText` (some with several runs, one with `\n`), an `Entity` that holds a selection, and: (a) the drag whose pointer leaves the row, the list and the window — do `on_mouse_move`/`on_mouse_up` on the row still fire; do `window.on_mouse_event` handlers registered in a `canvas` paint fire; `on_mouse_up_out`; (b) a `canvas` before the list that clears a registry the rows fill while painting: the order and whether rows painted in the previous frame are still in it; (c) `with_highlights` background over a wrapped line and over a run boundary: seams, line-height clipping, emoji; (d) a selection change re-renders rows already measured, their heights unchanged; (e) `.cursor(CursorStyle::IBeam)`; (f) `click_count` 1/2/3 from a real double and triple click, and in the test context (`simulate_mouse_down` with a hand-built event); (g) 120 `cx.notify()` per second with 60 rows. Also `index_for_position` called on a layout from a row that was never painted (confirm the panic and that it is avoided by the registry).
  - Tests: none (research). Write each point's observed answer and the chosen mechanism (or the fallback of D3) in `research-gpui.md`; delete the scratch crate.
  - Done when: points (a)-(g) each have an answer and D3's registry and drag-capture mechanisms are confirmed or replaced by a stated alternative.

- [x] 0.2 Public web client bundle: the quote request (unknowns (1), (6))
  - Owns: new `research-bundle.md`.
  - Depends on: nothing. VM network, anonymous GET only, no account.
  - Do: fetch the public build of `https://yastatic.net/s3/chat-static/telemessenger/_/<build>/web/app.js` (build number from the page; 3.6.3 at the time of writing) and its chunks in the VM; search `Quote`, `ForwardedMessageStyles`, `ForwardedMessageRefs`, `Цитировать`, selection or range handling. Record, without copying the code: the shape of the request the «Цитировать» action builds (extra keys in the style entry or in `Plain`), the quote length limit, newline and trim handling, how a fragment quote is rendered (line count, truncation).
  - Tests: none (research).
  - Done when: `research-bundle.md` answers unknowns (1) and (6), or states they cannot be read from the bundle (then 0.3-0.6 decide).

- [x] 0.3 [live] Read-only history scan for fragment quotes (unknown (5), (3))
  - Owns: new `research-wire.md`.
  - Depends on: a free live slot (rule 5). Reads only, no user.
  - Do: in the VM, per `docs/vm.md`, with `BT_LOG=yandex_proto::xiva=trace`: `bt-probe chats`, then `bt-probe history <chat>` over every chat to its start (as MP 0.1 did). A throwaway script in the VM (not committed) counts, over all replies, how many have a `Quote` equal to the beginning of the original (≤200 chars), a different substring (fragment), a non-substring, a quote above 200 characters, a quote with `\n`; and prints the **shapes only** of up to five fragment replies (every key of `ForwardedMessageStyles[0]`, of `Plain`, lengths; no names, no message text). Names, guids and text stay in the VM.
  - Tests: none (research).
  - Done when: `research-wire.md` has the counts and the shapes, or states that no fragment quote exists in the history (then 0.4 runs).

- [ ] 0.4 [live] [approval] Fragment quotes from the other party's web client (only if 0.3 found fewer than three distinct fragment shapes, or none with a newline or over 200 characters)
  - Owns: `research-wire.md` (after 0.3).
  - Depends on: 0.3, the user's approval of the target, a free live slot. **Target proposed:** the private chat between the admin account and the app account.
  - Do: the admin web client (`WebLive`, the other party; **a write by another account, nothing is written from the app account**) selects a fragment in an existing message of the app account and sends «Цитировать» replies: (i) a word from the middle, (ii) a three-line fragment, (iii) a fragment over 200 characters, (iv) the whole text. `bt-probe history <chat>` (reads) and `listen --watch <chat>` on the app account capture the raw shapes. Record the style entry's keys, the stored lengths, newline handling, and how the web client displays the quote.
  - Tests: none (research).
  - Done when: unknowns (1), (3), (4), (6) are answered from observed shapes.

- [ ] 0.5 `bt-probe send-reply`: a guarded send with a custom quote
  - Owns: `crates/yandex-proto/src/bin/bt-probe.rs`, `crates/yandex-proto/tests/probe_write_guard.rs`, `crates/yandex-proto/src/write.rs` (the encoder cuts every quote to `QUOTE_MAX_CHARS` = 200 in `quote()`/`add_reply`, so without this file the probe would measure the client's cut, not the server's limit). TS-probe merges before 2.3, which owns `write.rs` after it.
  - Depends on: nothing (the allow-listed send path and `ReplyRef` exist).
  - Do: `send-reply <chat> <text> --reply <micros> [--quote <text>]`: a text reply to message `<micros>` of `<chat>`; the quote is `--quote` or, without it, the whole original as the app does today. **Refused with exit code 2 unless `BT_WRITE_ONLY_CHAT` is set and equals `<chat>`** (the same `chat_write_guard` as `send-self` uses for its resolved chat; the chat is a required argument, never defaulted, and `create-*` rules are untouched). **Uncut quote path:** `write.rs` gets one explicit entry point that encodes a reply with the quote **not** cut (for example the frame builder taking the character limit as a parameter, `None` = no cut), used **only** by `send-reply --quote`; `ReplyRef`, every existing literal of it and the app's send and edit path stay byte-identical (the app keeps the 200 cut), so `yandex_backend.rs` and the later tasks need no change. `USAGE` and the module docs say it is a write guarded by `BT_WRITE_ONLY_CHAT`, that `--quote` is sent verbatim and may exceed 200 characters, and that only the private chat of the live check is meant.
  - Tests: argument parsing in the file's style (missing chat, missing `--reply`, bad number, unknown flag are usage errors); `probe_write_guard.rs`: unset variable refuses, another chat refuses, the named chat passes the guard; `write.rs`: **the encoded wire frame** (the JSON body that goes out, through the real encoder, not the built request) of an uncut 250-character quote contains all 250 characters in `ForwardedMessageStyles[0].Quote`, while the ordinary encoder path still cuts the same text to 200 (the existing `quote_is_cut_to_200_characters_not_bytes` and every other existing test pass unchanged).
  - Done when: these tests exist and the command is in the write set tested by `probe_write_guard.rs`.

- [ ] 0.6 [live] [approval] Send checks from the app account (unknowns (2), (3), (4))
  - Owns: `research-wire.md` (after 0.3, 0.4).
  - Depends on: 0.5 merged, the user's approval of the target, a free live slot. **Target proposed:** the private chat between the admin account and the app account, `BT_WRITE_ONLY_CHAT` set to it; no other chat.
  - Do: in the VM, with `BT_WRITE_ONLY_CHAT=<that chat>`: `send-reply <chat> <text> --reply <micros> --quote <fragment>` for (i) a word from the middle of an own test message, (ii) a three-line fragment, (iii) a quote that is **not** a substring of the original, (iv) a 250-character quote, (v) a quote with leading and trailing spaces. `history <chat>` reads each back; the admin web client (the other party) shows how each is displayed and whether the link to the original works. Each test message is created by the probe for this check (disposable; deleting them is the user's call). Record whether the server keeps, trims, cuts or rejects each form, and which `Quote` string comes back.
  - Tests: none (research).
  - Done when: `research-wire.md` answers unknowns (2), (3), (4) and says which of the outcomes of D9 applies.

- [ ] 0.7 Close the research
  - Owns: `research.md`, `design.md`, `tasks.md`, the specs of this change.
  - Depends on: 0.1-0.6 (0.4 if run).
  - Do: fold the answers into `research.md` "Results"; fix D3 (registry and drag capture), D9 (the wire form, the bound, `is_fragment`), D10 (display lines) and the specs to what was observed; lift or move the gates: tick 2.3 as "no work" or rewrite it for the extra key; if the server rejects or rewrites fragment quotes, stop TS-Q and put the decision to the main session (D9).
  - Tests: `openspec validate add-text-selection-and-quotes --strict`.
  - Done when: no **[probe]** remains in `research.md`, and the TS-A and TS-Q gates are lifted or TS-Q is stopped by a recorded decision.

## 1. Text selection (unit TS-A)

- [x] 1.1 Selection model (D1, D7)
  - Owns: new `crates/desktop/src/selection.rs`; `crates/desktop/src/main.rs` (the `mod selection;` line only).
  - Depends on: nothing.
  - Do: `selectable_text`, `Pos`, `Unit`, `TextSelection` (with the endpoint text snapshot `source`, D1) with `range_in`, `single`, `is_empty`, `validate`, `word_at`, `paragraph_at`, `select_all`, and `extract_text(messages, &TextSelection, header) -> String` (the D7 format). Offsets are bytes on grapheme boundaries (`unicode-segmentation`, already a dependency); `Pos` order is `(message index, offset)`. `validate` compares the endpoint's current text with the snapshot, not only the offsets. No gpui types.
  - Tests: word and paragraph at the start, middle, end, on a space, on punctuation, in Cyrillic, with an emoji and a combining mark, in an empty text; `range_in` for the first, middle, last and outside messages, for an anchor after the focus and equal ends; `single`; `validate` drops the selection when an endpoint's message is gone, when its text shrank, **and when it was edited to a text of the same length** (`hello world` selected at 6..11, edited to `hello there`: offsets in range, snapshot differs, so dropped), and keeps one whose message only moved or whose other messages changed; `extract_text` inside one message (bare), across three with a voice message between (skipped), a header per message, a blank line between, the ends cut at the offsets; `select_all` over a chat with only non-text messages is `None`.
  - Done when: these tests exist.

- [x] 1.2 Date and time label for the copy header (D7)
  - Owns: `crates/desktop/src/format.rs`.
  - Depends on: nothing.
  - Do: `date_time(ts, clock) -> String` as `ДД.ММ.ГГГГ ЧЧ:ММ` in the app's clock (the helper the timeline's time uses decides the zone); `copy_header(author, ts, clock)` = `{author}, {date_time}`, «Неизвестный» without an author.
  - Tests: a fixed `ts` and clock; midnight and year boundaries; no author.
  - Done when: these tests exist.

- [x] 1.3 Selectable text in the timeline (D3)
  - Owns: `crates/desktop/src/timeline.rs`, `crates/desktop/src/theme.rs`.
  - Depends on: 1.1, 0.1.
  - Do: replace `text_block` (body and captions of image and gallery) by `selectable_text`: `StyledText` with `with_highlights` from `range_in`, selector `msg-{ts}-text`, `CursorStyle::IBeam`; `TextLayouts` registry and `picking: Entity<PickState>` in `RowContext`; mouse down (Left) computes the caret from the layout (`Ok`/`Err` both, grapheme-snapped, never on an unmeasured layout) and calls `Desktop::text_down(chat, ts, offset, click_count, shift)`; the quote, author, time, chips and cards stay unselectable; a theme constant `SELECTION` for the highlight. The text div's width rules and the line height stay unchanged (the highlight changes no height).
  - Tests: in 1.8 (the timeline is exercised through the window test harness).
  - Done when: the code is in place and the existing timeline tests pass.

- [x] 1.4 The selection state machine (D2, D4, D5)
  - Owns: new `crates/desktop/src/desktop/selecting.rs`.
  - Depends on: 1.1, 1.2.
  - Do: `PickState { text: Option<TextSelection>, drag: Option<Drag> }`; pure transitions over `&[Message]`: `down(ts, offset, clicks, shift)` (1 caret/clear, shift extends, 2 word, 3+ paragraph), `drag_to(pos)` extending by the click's unit, `drag_end`, `clear`, `validate`, `select_all`; the caret mapping `caret_at(point, registry) -> Option<Pos>` (D5: over a text, beside a bubble to the nearest row, above/below the painted rows); the autoscroll speed function (distance outside the viewport to pixels per tick, capped, 0 inside); `Desktop` methods `text_down`, `drag_to`, `drag_end`, `copy_selection(cx)` (writes `extract_text` to the clipboard, no-op when empty), `select_all_text(cx)`, `clear_selection(cx)`; focus handed to the root handle on `text_down`.
  - Tests: all transitions with synthetic messages (click, shift-click extends and keeps the anchor, double and triple click units, drag by word, drag backwards, drag across messages, `clear`, `validate` after a delete, a shortening edit and a same-length edit, the snapshot taken at each endpoint); `caret_at` over a layout stub (inside, beside, above the first, below the last, between two rows); the speed function (inside 0, increasing, capped); `copy_selection` with an empty selection writes nothing.
  - Done when: these tests exist.

- [x] 1.5 Wiring in `Desktop` (D2-D5, D8)
  - Owns: `crates/desktop/src/desktop.rs`.
  - Depends on: 1.3, 1.4.
  - Do: `mod selecting;`, the `PickState` entity, `RowContext` fields; a `canvas` before the list that clears the registry, a `canvas` after it that, while a drag is active, registers the window-level mouse move and up handlers and runs the autoscroll task with `ListState::scroll_by`; actions `CopySelection` and `SelectAllText` and their handlers on the root div; `validate` each render; the selection is cleared on chat switch, sign-out, account switch and when an overlay (viewer, dialog) opens. Rows that take no part keep the existing `on_children_prepainted` bounds; this task never drains `RowBounds`.
  - Tests: in 1.8.
  - Done when: the code is in place and the workspace builds on the integrated tree.

- [x] 1.6 Key bindings (D4)
  - Owns: `crates/desktop/src/main.rs`.
  - Depends on: 1.1, 1.5.
  - Do: `KeyBinding::new("secondary-c", CopySelection, Some("BlazingTelesram"))` and `"secondary-a"` with `SelectAllText` (`secondary` = Cmd on macOS, Ctrl on Linux and Windows, D4; the `TextInput` bindings are untouched).
  - Tests: in 1.8 (dispatch the actions with the composer focused and unfocused).
  - Done when: the bindings are registered next to the existing ones.

- [ ] 1.7 Escape order, menu copy, right-click rule (D4, D6, D8)
  - Owns: `crates/desktop/src/desktop/composer.rs`.
  - Depends on: 1.5.
  - Do: `dismiss` clears the text selection after the message menu and before the info panel and reply/edit cancel; `menu_actions` takes the selection: `Copy` reads «Скопировать выделенное» and copies `extract_text` when the selection touches the message; `open_menu` keeps the selection when it touches the clicked message and clears it otherwise; `start_edit` clears it. «Цитировать» and «Выбрать» are added by TS-Q and TS-M.
  - Tests: Escape order (selection before info panel before reply cancel); the menu label and clipboard for a selection inside the message, across messages (the whole selection), and for a message outside it (the selection is cleared and the label is «Скопировать текст»); the existing menu tests pass unchanged.
  - Done when: these tests exist.

- [ ] 1.8 Tests for the unit
  - Owns: `crates/desktop/src/tests.rs`.
  - Depends on: 1.3-1.7.
  - Do: a section `// ---- text selection (TS 1.8) ----`; build `MouseDownEvent { click_count }` by hand where `simulate_mouse_down` cannot set it (0.1 (f)).
  - Tests: drag inside one message highlights exactly the range (assert the state, and the highlight through the debug selector bounds if 0.1 allows); double click selects a word, triple a paragraph; Shift+click extends; a click clears; drag across two messages and Cmd+C gives the D7 text with headers; Cmd+C with the composer focused copies the composer's text, after selecting in the timeline copies the selection; Cmd+A in the timeline then Cmd+C; Escape, chat switch, deletion and edit of the selected message clear it, **also an edit that keeps the length** (then neither Cmd+C nor the menu offers the old range); a new incoming message and loading older history keep it; the quote, the author and a voice card are not selectable; no `Command` is sent by any of these (the fake backend records nothing). The shortcut tests dispatch the actions through the `secondary` binding, so they hold on every platform the tests run on.
  - Done when: these tests exist and pass.

## 2. Quotes (unit TS-Q; starts after TS-A is on `main` and 0.7)

- [ ] 2.1 `partial` in the model
  - Owns: `crates/chat-core/src/model.rs`, `crates/chat-core/src/fixtures.rs`, `crates/chat-core/src/client.rs`, `crates/chat-core/src/cache.rs`, `crates/desktop/src/store.rs`.
  - Depends on: 0.7.
  - Do: `ReplyQuote.partial` and `ReplyTarget.partial`, `bool`, `#[serde(default)]`; every literal of both types in these files updated (found with `lsp` references; the literal sites of `yandex_backend.rs`, `composer.rs` and `tests.rs` belong to 2.4, 2.5, 2.7); `client.rs` copies `target.partial` into the pending message's `reply_to`; a pending send and a refused send keep it.
  - Tests: old JSON of a `Message`, a `Draft` and a `Command::Send` without the field loads with `partial == false`; a cached message and an outbox row round-trip `partial == true`; the pending message of a fragment send carries it; a refused send restores the target with it.
  - Done when: these tests exist.

- [ ] 2.2 Fragment rules (D9)
  - Owns: `crates/desktop/src/selection.rs`.
  - Depends on: 2.1, TS-A merged.
  - Do: `is_fragment(original, quote)` and `fragment_quote(selected)` per D9 (and the rule as refined by 0.7).
  - Tests: the quote of a reply action (the whole text, also a text over 200 characters cut to 200) is **not** a fragment; `hello` from `hello world` **is** a fragment (the one rule of D9: only the whole original, cut at 200, is not); a word from the middle, the end and a non-substring are; a whole short text is not; trimming and `\r\n` on both sides; a 300-character selection is cut to 200 on a char boundary (Cyrillic, an emoji at the cut); a blank selection is `None`; an absent original is handled by the caller (not here). The composer test (2.5), the adapter test (2.4) and the timeline test (2.7) all go through this function.
  - Done when: these tests exist.

- [ ] 2.3 Wire extras (only if 0.7 finds a key beyond `Quote`)
  - Owns: `crates/yandex-proto/src/write.rs`, `crates/yandex-proto/src/wire.rs`, `crates/yandex-proto/tests/wire_fixtures.rs`, a new fixture from the live capture of 0.3/0.4 (headed as a capture).
  - Depends on: 0.7.
  - Do: nothing if `Quote` is the only key (tick as "no work"); otherwise `ReplyRef` gets the key, `add_reply` writes it, `wire.rs::relation` reads it, and the `partial` rule of D9 uses it. If the server rejects or rewrites a fragment quote, this unit stops (D9).
  - Tests: if run: the request body for a fragment, the existing reply tests byte-identical, the fixture decodes.
  - Done when: ticked as no work, or these tests exist.

- [ ] 2.4 Adapter: derive `partial` from the wire
  - Owns: `crates/desktop/src/yandex_backend.rs`.
  - Depends on: 2.1, 2.2.
  - Do: in the reply mapping set `ReplyQuote.partial = original_text.map_or(false, |o| is_fragment(o, quote))`; `Backend::edit` and the send path pass `ReplyTarget.partial` unchanged; every literal in this file updated.
  - Tests: the fixture reply with `"Quote": "plain text"` stays non-partial; a synthetic reply whose quote is a middle word is partial; a reply whose quote is the first 200 characters of a 300-character original (what the reply action sends) is **not** partial, while a 250-character quote of the same original is partial (D9: only the whole original cut at 200 is not a fragment); a shorter beginning (`hello` of `hello world`) is partial; an absent original (deleted) is not partial; an edit of a partial reply sends the same quote text.
  - Done when: these tests exist.

- [ ] 2.5 «Цитировать» and the quote bar (D6, D10)
  - Owns: `crates/desktop/src/desktop/composer.rs`.
  - Depends on: 2.1, 2.2, 1.7.
  - Do: `MenuAction::Quote` (selector `menu-quote`, before `Reply`, offered per D6); `start_quote(ts, fragment)`; the bar for `partial` (title «Цитата · {author}», two lines, selector `reply-bar-quote`); every `ReplyTarget`/`ReplyQuote` literal in this file updated; `start_reply` sets `partial: false`.
  - Tests: «Цитировать» offered for a selection inside one message, not for none, a blank one, two messages, a message that cannot be replied to; choosing it sets the target with the fragment, `partial`, clears the selection, focuses the composer; whole text selected gives `partial == false`; the bar shows title and two lines; Escape cancels and sends nothing; edit mode is cancelled by the quote like by a reply; a draft keeps the fragment.
  - Done when: these tests exist.

- [ ] 2.6 Fragment block in the timeline (D10)
  - Owns: `crates/desktop/src/timeline.rs`.
  - Depends on: 2.1.
  - Do: `reply_quote` renders `partial` with `line_clamp(3)` and newlines kept, selector `msg-{ts}-reply-fragment`; the non-partial block is unchanged.
  - Tests: in 2.7.
  - Done when: the code is in place and existing timeline tests pass.

- [ ] 2.7 Tests for the unit
  - Owns: `crates/desktop/src/tests.rs`.
  - Depends on: 2.1-2.6.
  - Do: a section `// ---- quotes (TS 2.7) ----`; every `ReplyTarget`/`ReplyQuote` literal in this file updated.
  - Tests: a fragment reply is shown as up to three lines, a whole-message reply as one line; a pending fragment message shows the fragment; a refused send puts the target back with the flag; the draft with a fragment survives a chat switch and a restart (cache round trip); a legacy cached quote shows as whole; the edit of a fragment reply keeps the quote; «Цитировать» then send writes one `Command::Send` with the fragment as the reply text and nothing else.
  - Done when: these tests exist and pass.


## 3. Message selection (unit TS-M; starts after TS-Q is on `main`)

- [ ] 3.1 `Pick` and the message set (D1, D11)
  - Owns: `crates/desktop/src/selection.rs`.
  - Depends on: TS-A merged.
  - Do: `MessageSet` (toggle, range from the last toggled, select all eligible, `eligible(&Message)`: sent and not deleted, `all_deletable(&[&Message])`: own and sent), `Pick` (None, Text, Messages), `extract_messages` (D7 for sets).
  - Tests: toggle on and off, range over a gap of ineligible messages, range in either direction, select all skipping pending and deleted, `all_deletable` with one foreign message, `extract_messages` for one message (bare), two (headers), a voice (skipped) and a file (its name).
  - Done when: these tests exist.

- [ ] 3.2 Count label (D11)
  - Owns: `crates/desktop/src/format.rs`.
  - Depends on: nothing.
  - Do: `messages_count(n)` for «Удалить N сообщений?» with the Russian plural (1 сообщение, 2-4 сообщения, 5-20 сообщений, 21 сообщение, 111-114 сообщений).
  - Tests: those numbers.
  - Done when: these tests exist.

- [ ] 3.3 Mode state and the bar (D11)
  - Owns: `crates/desktop/src/desktop/selecting.rs`.
  - Depends on: 3.1, 3.2.
  - Do: `PickState` holds `Pick`; transitions `enter(ts)`, `toggle(ts, shift)`, `select_all`, `leave`, last-one-off leaves; entering clears the text selection and the drag; `Desktop` methods `enter_mode`, `toggle_message`, `copy_marked` (leaves the mode on), `request_delete_marked`; `selection_bar()` with selectors `selection-bar`, `selection-count`, `selection-copy`, `selection-delete`, `selection-cancel`, the disabled states of D11, no forward button.
  - Tests: transitions; entering clears a text selection; `copy_marked` text; `request_delete_marked` refuses when a foreign message is marked; the bar's enabled flags for the mixed and the all-own sets.
  - Done when: these tests exist.

- [ ] 3.4 Rows in the mode (D11)
  - Owns: `crates/desktop/src/timeline.rs`, `crates/desktop/src/theme.rs`.
  - Depends on: 3.1.
  - Do: while the mode is on: a 28 px gutter with ○ / ✓ (text glyphs), a row tint, a transparent full-row layer that turns every left click into `toggle_message(ts, shift)` and starts no text drag; a primary-modifier click (`event.modifiers.secondary()`: Cmd on macOS, Ctrl on Linux and Windows) on an eligible bubble outside the mode calls `enter_mode`; selectors `msg-{ts}-check`, `msg-{ts}-marked`; new theme colours for the check and the tint; no change to a row's height or to rows outside the mode.
  - Tests: in 3.7.
  - Done when: the code is in place and existing timeline tests pass.

- [ ] 3.5 Wiring and the delete state (D11)
  - Owns: `crates/desktop/src/desktop.rs`.
  - Depends on: 3.3, 3.4.
  - Do: `RowContext` gets the mode view; `SelectAllText` in the mode selects all eligible; the mode is left on chat switch; `delete_pending` becomes `Vec<(ChatId, MessageTs)>` (one entry for the single delete); the delete dialog text uses `messages_count`; the existing single-delete path stays the one-element case.
  - Tests: in 3.7.
  - Done when: the workspace builds on the integrated tree.

- [ ] 3.6 Menu entry, composer swap, batch delete (D6, D11)
  - Owns: `crates/desktop/src/desktop/composer.rs`.
  - Depends on: 3.3, 3.5.
  - Do: `MenuAction::Select` («Выбрать», selector `menu-select`, after `Copy`, offered for a sent, non-deleted message); `composer_view` returns the bar while the mode is on (the input and its draft untouched); `dismiss` leaves the mode after the message menu and before the text selection and info panel; a right click does nothing in the mode; `confirm_delete` sends one `Command::Delete` per entry through `Store::delete_message` in timeline order, closes the mode, reports failures through the existing deletion error path; the dialog is not opened for a set that is not all deletable.
  - Tests: «Выбрать» offered for sent, not for pending, failed and deleted; choosing it enters the mode with that message; the composer's draft is intact after the mode; Escape leaves the mode before closing the info panel; confirming deletion of three sends three deletes in order; one refused delete leaves that message and deletes the others; cancel keeps the marks.
  - Done when: these tests exist.

- [ ] 3.7 Tests for the unit
  - Owns: `crates/desktop/src/tests.rs`.
  - Depends on: 3.3-3.6.
  - Do: a section `// ---- message selection (TS 3.7) ----`.
  - Tests: a primary-modifier click (built by hand with `Modifiers::secondary_key()`) enters; click toggles; Shift+click marks the range; a click on a picture or a reaction chip toggles and opens nothing and sends nothing; Cmd+A marks all eligible; deselecting the last message leaves; chat switch leaves; the bar shows «Выбрано: N»; «Скопировать» puts the D7 text on the clipboard; «Удалить» is disabled with a foreign message; deletion of three own messages end to end through the fake backend; selection and mode never send a request on their own.
  - Done when: these tests exist and pass.

## 4. Verification and close-out

- [ ] 4.1 Final run per unit (VM)
  - Owns: nothing (checks only).
  - Depends on: the unit under test.
  - Do: in `bt-vm`, `~/bt/src-select`, `--locked --offline`: `cargo fmt --all --check`; `cargo build --workspace --all-targets` zero warnings; `cargo test --workspace`; `cargo clippy --workspace --all-targets` (no new warning over the 10-warning baseline); `openspec validate add-text-selection-and-quotes --strict` and `--all --strict`. Record the counts in `DEPENDENCIES.md`.
  - Done when: green for TS-probe, TS-A, TS-Q, TS-M, and again on `main` after each merge.

- [ ] 4.2 [live] [approval] Hand check of the app in the VM
  - Owns: nothing (checks only); appends the result to `research-wire.md`.
  - Depends on: TS-M merged (or the units available so far), 4.1, the user's approval of the target, a free live slot. **Target:** the private chat between the admin account and the app account; `BT_WRITE_ONLY_CHAT` set to it; the app account sends only what is listed here.
  - Do: with the app in the VM (screenshots with `screencapture`, `pbpaste` for the clipboard): drag, double and triple click, Shift+click, a drag across three messages with autoscroll, Cmd+C, Cmd+A, Escape; the quote of a fragment sent to the chat, read back with `bt-probe history` and seen in the web client of the admin account (the quote shows the fragment and links to the original); the web client's own fragment quote seen in the app; message mode with copy and the deletion of the test messages this check created (they are deleted by the app, which is the delete path under test); the draft kept across the mode. Screenshots saved to files and their paths reported.
  - Done when: each behaviour has a recorded result; any defect becomes a task here, not a silent fix.

- [ ] 4.3 Close-out
  - Owns: `openspec/changes/DEPENDENCIES.md`, this change's artifacts.
  - Depends on: 4.1, 4.2.
  - Do: update `DEPENDENCIES.md` (units merged, files released); sync the delta specs into the main specs (new `text-selection`, `message-quotes`, `message-selection`; modified `message-editing`, `message-sending`, `message-history`), then `openspec archive add-text-selection-and-quotes`.
  - Done when: the change is archived and the main specs hold the new and modified requirements.
