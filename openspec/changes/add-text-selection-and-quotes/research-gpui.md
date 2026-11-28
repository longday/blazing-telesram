# gpui 0.2.2 spike for text selection (task 0.1)

Evidence for the mechanisms of `design.md` D3 and D5, points (a)-(g) of `research.md` "gpui 0.2.2" plus the extra
`index_for_position` question. Everything below was **observed** unless it is marked `[INFERENCE]`.

## Method and limits

- Scratch crate `~/bt/spike-select/` in the Tart VM `bt-vm` (deleted at the end, not committed): gpui `=0.2.2` with
  `runtime_shaders` (the version of `Cargo.lock`), release build, one window.
  - A header `div`, a `gpui::list` of 60 `StyledText` rows (`ListAlignment::Top`, overdraw 600 px), a footer `div`.
    Rows: one line, three lines with bold and red runs, five wrapped lines, a row with `\n` and an empty paragraph, an
    emoji row (ZWJ family, flag, VS16 heart, many single emoji), mixed Cyrillic/Latin. Row text 15 px, line height 21 px,
    bubble 420 px wide, as `text_block` in `timeline.rs`.
  - An `Entity<SelState { anchor, focus, drag, version }>`; every change does `cx.notify()` on the root.
  - Probes: four registries (differing in where they are cleared and filled), a per-frame call-order log, counters of
    every handler, heights of measured rows per frame, and a `catch_unwind` around `index_for_position`.
- Real input: macOS `CGEvent`s posted through `osascript` (JXA) into the VM session, screenshots by `screencapture`
  and a pixel scan on the host. Test context: a `#[gpui::test]` in the same crate (`test-support`).
- Limits: the VM display is 1440x900 at scale 1 (points = pixels); no Retina scale factor, no physical mouse (so no
  OS-computed double click, see (f)), a virtual GPU (timings in (g) are CPU-side numbers of a VM, not of the host).
  The screen is shared with other agents, so GUI runs were short bursts.
- A **second scratch crate** (same VM directory, rebuilt and deleted again) answered a review of the first version of
  this report: it shapes samples through gpui's real text system with the same runs `with_highlights` builds, sweeps
  the pointer over layouts in a real window and in the test platform, and draws the same selection three ways.

## Summary

| Point | Answer | D3/D5 |
| --- | --- | --- |
| (a) drag leaves row/list/window | Element handlers stop; **window-level handlers registered in a `canvas` paint keep firing everywhere**, also outside the window and for the up | Capture confirmed; **register always**, not only while `drag` is `Some` |
| (b) registry order | Order is `pre-prepaint`, renders, row prepaints, `pre-paint`, row paints, `post-paint`; clear in the pre-list **prepaint**, fill from row paint or prepaint, never from the render closure; previous frame's rows are still there until cleared | Confirmed with three refinements; frame stamp not needed |
| (c) selection background | The background of `with_highlights` is seam-free, **but it splits the text runs, and with three runs (before, selected, after) shaping changes**: Arabic ligatures break (visible), widths move by up to 8.6 px (Arabic) and 0.07-0.73 px (Cyrillic); a cut inside a ZWJ/flag cluster also moves wraps | **D3 changes: paint the selection as rectangles behind an unchanged `StyledText`; do not use `with_highlights` for it** |
| (d) re-render on selection change | Visible rows are re-rendered, heights never changed (0 `HEIGHT CHANGED` in all runs) | Confirmed |
| (e) `.cursor(IBeam)` | Works; reverts to the arrow outside the element, also during a drag | Use `Window::set_window_cursor_style` while dragging |
| (f) `click_count` | Real events: 1/2/3 verbatim from `NSEvent`; test context: helpers give 1, `simulate_event` with a hand-built event gives 2/3 | Confirmed |
| (g) 120 `cx.notify()`/s, 60 rows | Coalesced to the frame rate; 0.14-0.45 ms CPU per frame for 9 visible rows | Confirmed: notify on every focus change is fine |
| extra: unpainted layout | `index_for_position` **panics**; the registry never hands out such a layout | Confirmed |

## (a) The drag leaves the row, the list, the window

Run: mouse down in row 2, drag through rows 3-8, the footer, below the window, release outside the window at
(900, 810) on screen. Handlers: `on_mouse_move`/`on_mouse_up`/`on_mouse_up_out` on every row, on the header and the
footer, and two `window.on_mouse_event` handlers (capture and bubble phase) registered in the paint closure of a `canvas`
placed after the list.

- Element `on_mouse_move` fires only on the element under the pointer (hit-gated by `hitbox.is_hovered`): the start
  row's handler stopped when the pointer left it (18 of 429 moves); the header saw 6 and the footer 122 of them; nothing
  sees a move outside the window or beside the bubbles.
- Element `on_mouse_up` does not fire for an up outside the element. **`on_mouse_up_out`** fired on every row that was
  painted (9 of 9, the start row included) for the up outside the window. It exists only for rows currently painted, so
  it cannot be the only mechanism (the start row may have been scrolled away by autoscroll), but as a backup it works.
- **Window-level handlers fire for all 429 moves** (capture 429, bubble 429), 110 of them outside the window rectangle,
  and for the up outside the window (`position` outside the viewport). `Window::dispatch_mouse_event` runs every
  window-level listener of the rendered frame in a capture pass and a bubble pass without hit-testing.
- **Registration timing.** `window.on_mouse_event` handlers are registered while painting and live for one frame.
  Variant A (registered on every frame, `drag` checked inside) received the up in 8 of 8 quick clicks (down and up posted
  back to back); variant B (registered only when `drag` was `Some` at paint time) received 0 of 8, because the down and
  the up arrive before the next frame is painted. With a slow click (30 ms) B also worked.
  **Mechanism: register always, test `drag` inside.** The handler cost is one closure per frame.
- **Autoscroll tick (D5).** While a button is held, gpui's macOS platform re-sends the last drag `MouseMoveEvent` every
  16 ms (`synthetic_drag` in `platform/mac/window.rs`; found only under `mac/`). With the pointer still, 79 repeats in
  1.37 s and 107 in 1.94 s were counted (about 58 Hz), also with the pointer outside the window. They carry the same
  position and cannot be told from real moves. D5's 16 ms timer is therefore redundant on macOS and still needed on other
  platforms `[INFERENCE: no other backend has it]`. Keep **one** driver, the timer, and compute the scroll distance from
  elapsed time (`Instant`), so the extra macOS repeats neither double the speed nor matter; the window handler only
  stores the pointer position (and maps it to a caret).
- Frames are only drawn when something notifies: 114 moves with a changing focus produced about 10 frames.

## (b) Registry cleared by a `canvas` before the list

Call order of an ordinary frame (9 visible rows, logged by the spike):

```
pre-canvas prepaint -> render x N -> row prepaint x 9 -> pre-canvas paint -> row paint x 9 -> post-canvas paint
```

| Registry | Cleared in | Filled in | Result |
| --- | --- | --- | --- |
| `reg_paint` | pre-canvas **prepaint** | row paint | exactly the 9 painted rows, 0 stale, `index_for_position` OK 9/9 |
| `reg_pre` | pre-canvas prepaint | row prepaint | same |
| `reg_pp` | pre-canvas **paint** | row prepaint | **empty**: paint of the pre-canvas runs after the rows' prepaint and wipes them |
| `reg_render` | pre-canvas prepaint | render closure | 18 entries on the first frame: 9 OK, **9 panic** ("prepaint has not been performed"), the overdraw rows that were only measured |

- Rows painted in the previous frame are **still in the registry** when the next frame starts (n=9 at the `pre-prepaint`
  of frame 2, n=18 for `reg_render`), so the clear is needed; after the clear a registry holds only the rows of the
  current frame (`stale=0` in every logged frame, including after scrolling up and down, where the painted span moved
  from rows 3..13 to 2..11).
- The `list` renders and measures the visible rows on every frame and the overdraw rows only when their size is not
  cached (no prepaint for those). So a registry filled from the **render closure** is wrong.
- **Mechanism:** a `canvas` before the list clears in its **prepaint** callback. The row pushes its layout from a
  zero-size `.absolute().size_0()` `canvas` placed **after** the `StyledText` in the same bubble (the prepaint or the
  paint callback; siblings prepaint in order, so the text has been prepainted by then). The frame stamp of D3 is not
  needed; if kept, it is a debug assertion only.

## (c) `with_highlights` over a wrapped line and a run boundary

- Pixel scan of a screenshot with all 9 visible rows selected (highlight (47, 93, 150) on bubble (43, 51, 64)):
  every highlighted line is a band exactly `line_height` (21 px) tall; the bands of the lines of one paragraph abut
  with **no empty pixel row** between them (3-, 4- and 5-line rows) and **no empty pixel column** along a line,
  including across the bold/red run boundaries of the multi-run row and around the emoji. The text colour of the runs
  is unchanged. The highlight starts at the text origin and ends at the last glyph of the line (not at the bubble
  edge).
- **Empty lines and `\n` are not painted**: the empty paragraph of the `\n` row has a 21 px gap in the highlight, and the
  newline itself has no visible mark. Selecting across a paragraph break therefore looks like two separated blocks
  (accepted by D3, noted for 1.x tests that look at the highlight).
- **`with_highlights` changes shaping (this corrects the first version of this report, which said it did not).**
  `shape_text` starts a new font run whenever the decoration (the background) changes, also when the font is the same
  (`text_system.rs` `shape_text`, the `decoration_changed` test), and the macOS backend inserts a ZWNJ between font runs
  of the same font so that CoreText does not ligate across the boundary (`platform/mac/text_system.rs`
  `layout_line`, `needs_zwnj`). A selection gives up to three runs, `[before][selected][after]`. Observed in the
  real macOS shaper (`.SystemUIFont` 15 px, unwrapped), runs `[0,a) [a,b) [b,len)` with a background on the middle one,
  glyph indices and x positions plus the width compared with the single-run shaping, over **all** `(a, b)` pairs on
  char boundaries (`differing / pairs`, pairs whose both ends are on grapheme boundaries):

  | Sample | differing / pairs | Examples (width change) |
  | --- | --- | --- |
  | Arabic «لالا» | 3 / 9 | +0.09 px, +5.0 px |
  | Arabic words «مرحبا بالعالم» | 40 / 90 | -0.69 px ... +8.6 px |
  | Arabic «لا لا الله» | 24 / 54 | +0.07 px ... +6.8 px |
  | Cyrillic «Привет, мир» | 12 / 65 | +0.07 px ... +0.73 px |
  | Arabic «لا», Latin «office fi ffl fl», U+FB01/FB02, Hangul, Thai, Devanagari, «é», ZWJ emoji | 0 | none |

  With only **two** runs (one cut) nothing differed in any sample, so a selection that touches the start or the end of
  the text is harmless and one in the middle is not `[INFERENCE from the source: the ZWNJ is appended after the text of
  the later run, so with two runs it lands at the end of the string]`. With three runs, a cut **inside** a grapheme
  cluster changes the shaping far more (pairs with at least one end inside a cluster: ZWJ family emoji 21 of 39 differ,
  by up to 63 px; Thai 12 of 27; Devanagari 8 of 30).
  On screen the same text with the same selection through `with_highlights` shows the «لا» ligature **split into two
  letters**, while the unselected copy and the rectangle variant below keep it whole.
  The first version's wrap check (16 807 highlighted shapings of the 60 rows; 6 differed, all with a cut inside a ZWJ
  sequence or flag; 0 with grapheme-boundary cuts) compared only line breaks and height on Cyrillic, Latin and emoji,
  so it could not see the sub-pixel width shifts above (or a script with ligatures): its "0" does not prove that shaping
  is unchanged. Text that moves by up to 0.73 px while a selection is dragged is also a visible jitter, and a shift
  can move a wrap boundary `[INFERENCE: not observed]`.
- **Conclusion and mechanism: the selection must not touch the text runs.** Paint selection rectangles **behind** the
  unchanged `StyledText`: a zero-size `.absolute()` `canvas` placed **before** the `StyledText` in the bubble; its paint
  closure reads the layout of the text (all prepaints of the frame are done before any paint, so the bounds are set),
  and per visual line fills `window.paint_quad(fill(rect, SELECTION))`. The rectangles come from the layout: the visual
  lines start at `0` and at the glyph index of every `wrap_boundaries` entry of `line_layout_for_index(0)`; for a range
  `[a, b)` clipped to a line `[s, e)` the rect spans `x_for_index(a) - x_for_index(s)` to `x_for_index(b) - x_for_index(s)`
  (the line's `width` when `b` is the end of the last line), `line_height` tall at `y = k * line_height` (integer line
  height keeps the bands abutting). Observed in the spike: the three variants side by side (with_highlights, rectangles,
  none): the rectangle variant has exactly the line breaks and glyph positions of the unselected text, its bands cover
  the wrapped lines like the gpui highlight, and the whole-row rectangles of the sweeps match the line widths. The
  registry of (b) can be filled by the same canvas (its paint closure also sees the prepainted layout).
  Limits: tested on a single paragraph; several paragraphs (`\n`) need `line_layout_for_index` per paragraph and the
  paragraph's y from `position_for_index` `[INFERENCE, not run]`. **Right-to-left text is not supported for partial
  ranges:** the glyph indices of an Arabic run descend with x (`مرحبا`: 8, 6, 6, 4, 2, 0 from left to right) and
  `x_for_index` assumes logical order equals visual order, so partial rectangles (and carets, see below) are wrong; a
  selection of the whole text is right. Devanagari glyphs are also reordered (indices 0, 18, 9, 21).
- Line height: the bands are `line_height` tall, the 15 px glyphs (Cyrillic, Latin, colour emoji) are fully inside.
  Scale 2 (Retina) and a fractional line height were not tested `[INFERENCE: whole pixels at 21 px, scale 1]`.

## (d) A selection change re-renders measured rows

Every selection change re-rendered the visible rows (the render closure ran for each of them; `render-row` counters
rose) and the `list` re-measured them. The heights read by `ListState::bounds_for_item` for every measured row were
compared on every frame: **no `HEIGHT CHANGED` in any run** (drags across rows 2-8, forced selection of all rows,
scrolling). Rows outside the viewport that have a cached size are not re-rendered; they pick up the current selection
when they scroll in (the render closure reads a snapshot of the state).

## (e) `.cursor(CursorStyle::IBeam)`

Queried `NSCursor.currentSystemCursor` (image size and hot spot) from JXA: over a bubble with `.cursor(IBeam)` the
cursor was the I-beam (23x22, hot spot 12,11), over the header, the empty list area and the desktop the arrow
(28x40, 5,5). The style belongs to the element's hitbox, so the bubble padding also shows the I-beam; put it on the
text `div` if only the text should. **During a drag** with the pointer over the header or outside the window the
element style is gone (arrow). Registering `window.set_window_cursor_style(CursorStyle::IBeam, ...)` in the paint
closure of the drag-capture `canvas` while `drag` is `Some` held the I-beam over the header and outside the window and
dropped it when the button was released (arrow again). Use it for D5.

## (f) `click_count`

- Real events: `CGEvent` mouse downs with `clickState` 1, 2, 3 arrived as `click_count` 1, 2, 3 on both the down and the
  up (gpui's macOS backend forwards `NSEvent.clickCount` unchanged).
- Events without `clickState` (what the stock click tools post) arrive as `click_count` 1 on the down and 0 on the up,
  even 80 ms apart: macOS does not count clicks of synthetic events. **A physical double click could not be produced in
  the VM**; that it gives 2 is `[INFERENCE]` from the forwarding above. Note a **0** on the up.
- Test context: `simulate_mouse_down`, `simulate_mouse_up` and `simulate_click` hard-code `click_count: 1`. A hand-built
  `vcx.simulate_event(MouseDownEvent { click_count: 2 | 3, .. })` (and the matching `MouseUpEvent`) reaches the handler
  with 2 and 3 (observed). Tests of double and triple click use `simulate_event`.
- In the test context hit-testing and wrapping work, window-level handlers receive moves and the up at points outside
  the window (-50, 5000) and (9000, -50), and the drag ended on that up (a click 30 px into a bubble gave caret offset
  2). **The test platform's text system is a mock, not CoreText** (an earlier version of this report said the real one
  was used): every character advances 9 px (the glyph lists of «Alpha...» are `(0, 0.0), (1, 9.0), (2, 18.0)...`), an
  Arabic pair is two glyphs, the ZWJ family emoji is a glyph per code point. Tests that depend on real shaping
  (ligatures, kerning, clusters) cannot run in `#[gpui::test]`.

## (g) 120 `cx.notify()` per second, 60 rows

A timer notifying the root at the given rate, a draw timer from the pre-list canvas prepaint to the post-list canvas
paint (CPU side, no presentation), the render closure time, and `ps` CPU time. Release build, VM.

| Run | draws/s | avg draw | max draw | avg root render | CPU of the process |
| --- | --- | --- | --- | --- | --- |
| 1 notify/s, 60 rows | 1 | 0.14 ms | 0.15 ms (8.6 ms first frame) | 0.012 ms | 0.04 s per 10 s |
| 120/s, 60 rows | 57-60 at first, then ~25 | 0.14-0.45 ms | 1-2.4 ms (8.7 ms first) | 0.011-0.028 ms | 0.39 s per 10 s |
| 120/s, 60 rows, all selected | 57-59, then ~25 | 0.37-0.46 ms | 0.7 ms (7.9 ms first) | 0.021-0.026 ms | 0.29 s per 10 s |
| 120/s, 600 rows | 57-58, then ~25 | 0.13-0.17 ms | 0.9 ms | 0.011-0.014 ms | 0.19 s per 10 s |

- The 120 requests per second collapse into at most one draw per display refresh (57-60/s; the ~25/s later in the
  runs is the VM or the window being throttled, not a cost of the code). A highlight over every visible row raises the
  draw from ~0.15 to ~0.4 ms. 600 rows cost the same as 60 (the list only builds the visible rows).
- The spike rows are lighter than the app's (no avatars, reactions, previews), so read this as the overhead of the
  selection path, not as the app's frame time. Mouse moves notify only when the focus changes, and nothing else needs
  rendering per move. After the first frame no draw took longer than 2.5 ms.

## Extra: `index_for_position` on a layout that was never painted

`TextLayout::index_for_position` panics: "measurement has not been performed" when the element was not measured and
"prepaint has not been performed" when it was measured but not prepainted (the overdraw rows of the `list`: observed 9 of
18 layouts of `reg_render`; a `TextLayout::default()` panics too, observed in the test). The layout of an element is
re-created every frame. The registry of (b) (filled from paint or prepaint, cleared every frame) contains only
prepainted layouts, so `index_for_position` never panicked for it in any frame of any run.

## Caret placement: findings the design did not list

1. `index_for_position` returns the glyph that **contains** the point, i.e. the start of its cluster, so the right half
   of a letter puts the caret **before** it (three clicks across the family emoji at 303, 309 and 316 px all gave
   offset 51). A text-field caret needs the nearest boundary. `WrappedLineLayout::closest_index_for_position` exists but
   `TextLayout` does not expose it. The rule that worked (pointer swept over the layouts in 0.5 px steps, in the real
   shaper and in the test platform):
   - `i` from `index_for_position` (`Ok(i)`; `Err(i)` is already a line start or end, return it as is). `i` is a
     **global** byte offset, but `TextLayout::line_layout_for_index(ix)` returns the layout of the **one paragraph**
     (a `\n`-separated line, `elements/text.rs` `line_layout_for_index`: it walks `lines` with `line_start_ix` and
     returns `line.layout` of the paragraph containing `ix`) and the glyph indices inside it are **paragraph-local**
     (`position_for_index` subtracts `line_start_ix`, `ix_within_line = index - line_start_ix`). So work **per
     paragraph**: split the text on `\n`, `base_k` = the sum of `len + 1` of the paragraphs before `k`; the paragraph of
     `i` is the one with `base_k <= i <= base_k + len_k` (an index equal to the end of a paragraph belongs to that
     paragraph, as in gpui), and its layout is `line_layout_for_index(base_k)`;
   - **candidates of paragraph `k`** = `base_k + glyph.index` for the glyph starts of that layout
     (`unwrapped_layout.runs[].glyphs[].index`) that are also grapheme boundaries, plus `base_k` and the explicit
     end `base_k + len_k` (the position before the `\n`; the `\n` is not a glyph and `base_{k+1}` belongs to the next
     paragraph, never to this paragraph's candidates, so the wrap-start fix below cannot fire across a `\n`). An empty
     paragraph has the single candidate `base_k`. `start` = the largest candidate `<= i`, `next` = the smallest
     candidate `> i`; if `i` is already the last candidate (the end of the paragraph) return it. Without the base and the
     per-paragraph walk the glyph indices of the second paragraph collide with those of the first
     `[INFERENCE from the source: the rule was run on single-paragraph layouts only; the multi-paragraph rule has
     not been run, only the offsets of index_for_position across paragraphs, item 3]`);
   - `p0 = position_for_index(start)`, `p1 = position_for_index(next)` (absolute coordinates); **if `p1.y > p0.y`, `start`
     is the first glyph of a wrapped line and `position_for_index(start)` is the END of the previous visual line** (an
     index equal to a wrap boundary resolves to the earlier line, `line_layout.rs` `position_for_index`), so use
     `x0 = tl.bounds().left()` instead of `p0.x`; for the last glyph of a line `next` is the wrap boundary and `p1` is that
     line's end, which is right;
   - pick `next` when `pos.x >= (x0 + p1.x) / 2`, else `start`.
2. What the sweeps showed (real shaper unless noted):
   - **Wrap starts:** the rule without the `x0` fix switched to the next caret only at about the full glyph width
     (x = 9.0, 8.5, 4.0, 9.0, 8.0 on the five wrapped lines of the Latin sample, 9.0, 9.0, 9.0, 8.5 on the four of the
     Cyrillic one; the 4.0 is a narrow first glyph), i.e. the right half of the first glyph of a wrapped line kept the
     caret before it. With the fix the switch is at the midpoint (4.5, 4.5, 2.0, 4.5, 4.0 and 4.5, 4.5, 4.5, 4.5) and in
     the test platform 4.5 on all 7 wrapped lines (the rule without the fix: 9.0 on all 7).
   - **Ligature «لا»** (one glyph, indices 0 and 2 are graphemes, 2 is inside the glyph): the grapheme-boundary-only rule
     produced offset 2 for x in 4.5..8.5, a caret in the middle of the ligature; the glyph-cluster rule produced only
     the cluster boundaries (0 and 4). The same for «لالا» (2 inside the first ligature vs 4 and 8). For the **left-to-right**
     samples (Latin, Cyrillic, «é», the ZWJ family emoji, Thai, Devanagari) both rules gave the same carets, so the
     glyph filter only matters where graphemes are finer than clusters.
   - **Right-to-left:** in Arabic the carets are reversed (offsets decrease from the left edge; `Err` at the right edge
     gives `len` although that edge is the logical start). Both rules are wrong for RTL; see (c).
   - Combining marks and the ZWJ emoji are one glyph per grapheme here, so one caret stop; a click across the family
     emoji gave 51 then 76 (GUI), flags 89 then 97.
3. Offsets across paragraph breaks are right: the first glyph of the paragraph after `\n` gave its byte offset, a click on
   the empty line gave the offset of that empty line (between the two `\n`), and a click right of a line's end gave the
   end of that line.

Tests the implementation (1.3) must carry: (i) the rule as a pure function over a plain line model (glyph starts with x,
wrap starts, grapheme boundaries), unit-tested without gpui, including a line whose first glyph is a wrap start, a
cluster with an inner grapheme boundary (the ligature case) and a line end, and a multi-paragraph text (`a\nbc`: the
offsets of the second paragraph carry its base, never collide with the first paragraph's glyph indices, and the end of
paragraph one and the start of paragraph two are distinct stops); (ii) a `#[gpui::test]` with the mock text
system for the wrap starts (left and right half of the first and the last glyph of a wrapped line, the first glyph after
`\n`); (iii) because the test platform cannot shape a ligature, a **manual VM GUI check** on the real shaper
(sweep over «لا», «e` + U+0301», the family emoji) with the expected carets from above.

## Decisions for D3 and D5 (to be folded into `design.md` by task 0.7)

- **Registry (D3): confirmed**, with: clear in the pre-list canvas **prepaint** (not paint), fill from a zero-size
  canvas next to the `StyledText` (after it, or before it when that canvas also paints the selection; never the render
  closure), frame stamp dropped.
- **Drag capture (D3): confirmed with a change.** Window-level `window.on_mouse_event` handlers for `MouseMoveEvent` and
  `MouseUpEvent` in a `canvas` after the list, **registered on every frame** and checking `drag` inside (not "while
  `drag` is `Some`"). Element handlers are not enough; `on_mouse_up_out` on the rows is a possible backup, not needed.
- **D5 autoscroll:** one timer drives the scroll with a speed from elapsed time; macOS also repeats the last move every
  16 ms (no use of it needed). Hold the I-beam with `Window::set_window_cursor_style` while dragging.
- **Selection painting (D3): changed.** Rectangles behind the unchanged `StyledText` (see (c)); `with_highlights` is not
  used for the selection, so the text never reshapes while selecting. Ranges snap to grapheme boundaries (and to glyph
  clusters) for carets, copy and word/paragraph units, no longer for shaping. RTL partial selection is not supported.
- **Caret from a point:** the glyph-cluster midpoint rule with the wrap-start fix (checked in the real shaper and the
  test platform), not the bare `index_for_position` and not the grapheme-only midpoint.
- **Tests:** double and triple click through `simulate_event` with a hand-built `MouseDownEvent { click_count }`; the
  caret tests listed under "Caret placement".
