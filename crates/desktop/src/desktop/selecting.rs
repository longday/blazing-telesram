//! Text selection in the timeline: the state machine (D2, D4, D5) over the model of `crate::selection`.
//!
//! `PickState` holds the selection and the drag in progress; its transitions are pure over the loaded
//! messages of the open chat and are tested without a window. `Desktop` hands the gestures to it, copies
//! the selection to the clipboard and moves focus to the root, so Cmd+C reaches the selection.

use super::Desktop;
use crate::format::copy_header;
use crate::selection::{
    Pos, TextSelection, Unit, extract_text, paragraph_at, select_all, snap, text_of, word_at,
};
use bt_chat_core::model::{ChatId, Message, MessageTs};
use gpui::{Bounds, ClipboardItem, Context, Pixels, Point, TextLayout, Window, px};
use std::ops::Range;

/// Pixels per tick of the autoscroll for each pixel the pointer is outside the viewport.
const AUTOSCROLL_GAIN: f32 = 0.25;
/// The fastest autoscroll, in pixels per tick (one tick is 16 ms).
const AUTOSCROLL_MAX: f32 = 48.;

/// A drag in progress. `origin` is what the gesture started on: the caret (both ends equal), or the
/// word or the paragraph of a double or triple click, which the drag extends by whole units.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Drag {
    unit: Unit,
    origin: (Pos, Pos),
}

/// The selection of the open chat. A plain click leaves no selection, only the drag that follows it.
#[derive(Debug, Default)]
pub(crate) struct PickState {
    pub(crate) text: Option<TextSelection>,
    pub(crate) drag: Option<Drag>,
}

fn unit_of(click_count: usize) -> Unit {
    match click_count {
        0 | 1 => Unit::Char,
        2 => Unit::Word,
        _ => Unit::Paragraph,
    }
}

/// The span the unit covers around `offset`: the caret itself for a character.
fn unit_range(text: &str, unit: Unit, offset: usize) -> Range<usize> {
    match unit {
        Unit::Char => offset..offset,
        Unit::Word => word_at(text, offset),
        Unit::Paragraph => paragraph_at(text, offset),
    }
}

impl PickState {
    /// Mouse down on the text of message `ts` at byte `offset` (as a hit test returns it, snapped here to
    /// a grapheme boundary). One click puts the caret and clears the selection; with `shift` it moves the
    /// focus of the selection and keeps the anchor. Two clicks select the word, three or more the
    /// paragraph. The drag that follows extends by the click's unit. A message without text clears.
    pub(crate) fn down(
        &mut self,
        messages: &[Message],
        ts: MessageTs,
        offset: usize,
        click_count: usize,
        shift: bool,
    ) {
        let Some(text) = text_of(messages, ts) else {
            self.clear();
            return;
        };
        let at = Pos {
            ts,
            offset: snap(text, offset),
        };
        let unit = unit_of(click_count);
        if shift
            && unit == Unit::Char
            && let Some(anchor) = self.text.as_ref().map(|s| s.anchor)
        {
            self.drag = Some(Drag {
                unit,
                origin: (anchor, anchor),
            });
            self.text =
                TextSelection::new(messages, anchor, at, unit).filter(|s| !s.is_empty(messages));
            return;
        }
        let span = unit_range(text, unit, at.offset);
        let from = Pos {
            ts,
            offset: span.start,
        };
        let to = Pos {
            ts,
            offset: span.end,
        };
        self.drag = Some(Drag {
            unit,
            origin: (from, to),
        });
        self.text = TextSelection::new(messages, from, to, unit).filter(|s| !s.is_empty(messages));
    }

    /// The pointer of a drag is now at `pos`: the focus moves there, extended by the click's unit
    /// (dragging by word or paragraph keeps the whole unit of the start selected, in either direction).
    /// Returns whether the selection changed. Without a drag, or when `pos` is not on text, nothing happens.
    pub(crate) fn drag_to(&mut self, messages: &[Message], pos: Pos) -> bool {
        let Some(drag) = &self.drag else {
            return false;
        };
        let Some(text) = text_of(messages, pos.ts) else {
            return false;
        };
        let pos = Pos {
            ts: pos.ts,
            offset: snap(text, pos.offset),
        };
        let span = unit_range(text, drag.unit, pos.offset);
        let (anchor, focus) = if pos >= drag.origin.0 {
            (
                drag.origin.0,
                Pos {
                    ts: pos.ts,
                    offset: span.end,
                },
            )
        } else {
            (
                drag.origin.1,
                Pos {
                    ts: pos.ts,
                    offset: span.start,
                },
            )
        };
        let selection = TextSelection::new(messages, anchor, focus, drag.unit)
            .filter(|s| !s.is_empty(messages));
        let changed = selection != self.text;
        self.text = selection;
        changed
    }

    /// Mouse up: the drag ends, the selection stays. Returns whether a drag was in progress.
    pub(crate) fn drag_end(&mut self) -> bool {
        self.drag.take().is_some()
    }

    /// Drops the selection and the drag. Returns whether there was a selection.
    pub(crate) fn clear(&mut self) -> bool {
        self.drag = None;
        self.text.take().is_some()
    }

    /// Drops the selection (and the drag) when it no longer means what the user selected (D8): an
    /// endpoint message is gone, or its text is not the snapshot's, even at the same length. Returns
    /// whether it was dropped.
    pub(crate) fn validate(&mut self, messages: &[Message]) -> bool {
        if self.text.as_ref().is_some_and(|s| !s.validate(messages)) {
            self.clear();
            return true;
        }
        false
    }

    /// Selects all loaded selectable text; no text selects nothing. Returns whether the selection changed.
    pub(crate) fn select_all(&mut self, messages: &[Message]) -> bool {
        self.drag = None;
        let selection = select_all(messages);
        let changed = selection != self.text;
        self.text = selection;
        changed
    }

    /// The text a copy puts on the clipboard (D7); `None` for no selection, an empty or a stale one.
    pub(crate) fn selected_text(
        &self,
        messages: &[Message],
        header: impl Fn(&Message) -> String,
    ) -> Option<String> {
        let selection = self.text.as_ref()?;
        if selection.is_empty(messages) || !selection.validate(messages) {
            return None;
        }
        let text = extract_text(messages, selection, header);
        (!text.is_empty()).then_some(text)
    }
}

/// A text laid out on screen, as far as the caret mapping needs it. Implemented for gpui's `TextLayout`
/// (only a measured and prepainted one, which the registry guarantees) and by a stub in tests.
pub(crate) trait TextHit {
    fn bounds(&self) -> Bounds<Pixels>;
    /// The byte index under `point`: `Ok` inside the text, `Err` with the nearest index outside it.
    fn index_for_position(&self, point: Point<Pixels>) -> Result<usize, usize>;
}

impl TextHit for TextLayout {
    fn bounds(&self) -> Bounds<Pixels> {
        TextLayout::bounds(self)
    }

    fn index_for_position(&self, point: Point<Pixels>) -> Result<usize, usize> {
        TextLayout::index_for_position(self, point)
    }
}

/// The caret under `point` among the texts painted in the latest frame (D5). Over a text, and beside it
/// on one of its lines, the hit-tested index. Otherwise the text nearest vertically: above it, its
/// start; below it, its end. So above the first painted text is the start of that one, below the last
/// the end of it, and between two texts the nearer one (the earlier on a tie), by its start or end.
/// Rows without text are not in `texts` and are skipped. The offset is snapped to a grapheme boundary.
/// `None` when no registered text belongs to a loaded message.
pub(crate) fn caret_at<L: TextHit>(
    point: Point<Pixels>,
    texts: &[(MessageTs, L)],
    messages: &[Message],
) -> Option<Pos> {
    let y = f32::from(point.y);
    let mut nearest: Option<(f32, MessageTs, &L, &str)> = None;
    for (ts, layout) in texts {
        let Some(text) = text_of(messages, *ts) else {
            continue;
        };
        let bounds = layout.bounds();
        let distance = (f32::from(bounds.top()) - y)
            .max(y - f32::from(bounds.bottom()))
            .max(0.);
        let closer = nearest.is_none_or(|(best, best_ts, ..)| {
            distance < best || (distance == best && *ts < best_ts)
        });
        if closer {
            nearest = Some((distance, *ts, layout, text));
        }
    }
    let (_, ts, layout, text) = nearest?;
    let bounds = layout.bounds();
    let offset = if point.y < bounds.top() {
        0
    } else if point.y > bounds.bottom() {
        text.len()
    } else {
        match layout.index_for_position(point) {
            Ok(index) | Err(index) => snap(text, index),
        }
    };
    Some(Pos { ts, offset })
}

/// Pixels to scroll per tick while a drag has the pointer at `y` and the list shows `viewport`: zero
/// inside, growing with the distance outside, capped. Negative above the viewport (towards older
/// messages), positive below it, as `ListState::scroll_by` takes it.
pub(crate) fn autoscroll_step(y: Pixels, viewport: Bounds<Pixels>) -> Pixels {
    let (top, bottom) = (f32::from(viewport.top()), f32::from(viewport.bottom()));
    let y = f32::from(y);
    let step = if y < top {
        (y - top) * AUTOSCROLL_GAIN
    } else if y > bottom {
        (y - bottom) * AUTOSCROLL_GAIN
    } else {
        0.
    };
    px(step.clamp(-AUTOSCROLL_MAX, AUTOSCROLL_MAX))
}

impl Desktop {
    /// Runs `f` on the selection state with the loaded messages of the open chat (none without one).
    fn with_picking<R>(
        &self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut PickState, &[Message]) -> R,
    ) -> R {
        let chat = self.open.as_ref().map(|o| o.chat.clone());
        let store = self.store.clone();
        self.picking.update(cx, |state, cx| {
            let messages = chat
                .and_then(|chat| store.read(cx).timeline(&chat))
                .map_or(&[][..], |timeline| timeline.messages.as_slice());
            f(state, messages)
        })
    }

    /// Mouse down on the text of message `at.ts` of `chat` at byte `at.offset` (D3): see
    /// `PickState::down`. Focus moves to the root, so the next Cmd+C goes to the selection and not to a
    /// composer that holds focus.
    pub(crate) fn text_down(
        &mut self,
        chat: ChatId,
        at: Pos,
        click_count: usize,
        shift: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.open.as_ref().map(|o| &o.chat) != Some(&chat) {
            return;
        }
        window.focus(&self.focus);
        self.with_picking(cx, |state, messages| {
            state.down(messages, at.ts, at.offset, click_count, shift)
        });
        cx.notify();
    }

    /// The pointer of a drag is at `point` (window coordinates); `texts` are the layouts painted in the
    /// latest frame.
    pub(crate) fn drag_to<L: TextHit>(
        &mut self,
        point: Point<Pixels>,
        texts: &[(MessageTs, L)],
        cx: &mut Context<Self>,
    ) {
        let changed = self.with_picking(cx, |state, messages| {
            state.drag.is_some()
                && caret_at(point, texts, messages).is_some_and(|pos| state.drag_to(messages, pos))
        });
        if changed {
            cx.notify();
        }
    }

    /// Mouse up, also outside the window: the drag ends, the selection stays.
    pub(crate) fn drag_end(&mut self, cx: &mut Context<Self>) {
        if self.picking.update(cx, |state, _| state.drag_end()) {
            cx.notify();
        }
    }

    /// Cmd+C with the timeline focused: puts the selection on the clipboard (D7). Nothing is written for
    /// no selection, an empty one or a stale one.
    pub(crate) fn copy_selection(&mut self, cx: &mut Context<Self>) {
        let clock = self.clock;
        let (dropped, text) = self.with_picking(cx, |state, messages| {
            let dropped = state.validate(messages);
            let text = state.selected_text(messages, |m| {
                copy_header(m.author_name.as_deref(), m.timestamp, clock)
            });
            (dropped, text)
        });
        if let Some(text) = text {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
        if dropped {
            cx.notify();
        }
    }

    /// Cmd+A with the timeline focused: selects all loaded selectable text of the open chat.
    pub(crate) fn select_all_text(&mut self, cx: &mut Context<Self>) {
        if self.with_picking(cx, |state, messages| state.select_all(messages)) {
            cx.notify();
        }
    }

    /// Drops the selection and any drag. Returns whether there was a selection, so Escape can tell
    /// whether it closed something.
    pub(crate) fn clear_selection(&mut self, cx: &mut Context<Self>) -> bool {
        let had = self.picking.update(cx, |state, _| state.clear());
        if had {
            cx.notify();
        }
        had
    }

    /// Drops a selection that no longer matches the messages (D8); called on every render.
    pub(crate) fn validate_selection(&mut self, cx: &mut Context<Self>) {
        if self.with_picking(cx, |state, messages| state.validate(messages)) {
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alerts::fake as alerts_fake;
    use crate::format::Clock;
    use crate::player::fake as player_fake;
    use crate::selection::fixtures::{message, text, voice};
    use crate::store::ChatStore;
    use bt_chat_core::model::MessageBody;
    use bt_chat_core::model::{ChatKind, ChatSummary, ClientEvent, HistoryChunk};
    use gpui::{AppContext as _, Focusable as _, Modifiers, TestAppContext, VisualTestContext};
    use gpui::{point, size};

    fn pos(ts: MessageTs, offset: usize) -> Pos {
        Pos { ts, offset }
    }

    fn header(m: &Message) -> String {
        format!("H{}", m.timestamp)
    }

    fn chat() -> Vec<Message> {
        vec![
            text(1, "hello world"),
            text(2, "second line\nhere"),
            voice(3),
            text(4, "last one"),
        ]
    }

    /// The selection as `(anchor, focus)` positions.
    fn ends(state: &PickState) -> Option<(Pos, Pos)> {
        state.text.as_ref().map(|s| (s.anchor, s.focus))
    }

    #[test]
    fn a_click_puts_a_caret_and_starts_a_drag_without_a_selection() {
        let m = chat();
        let mut state = PickState::default();
        state.down(&m, 1, 3, 1, false);
        assert_eq!(state.text, None);
        assert_eq!(
            state.drag,
            Some(Drag {
                unit: Unit::Char,
                origin: (pos(1, 3), pos(1, 3))
            })
        );
        assert!(state.drag_end());
        assert!(!state.drag_end());
    }

    #[test]
    fn dragging_selects_and_the_drag_end_keeps_the_selection() {
        let m = chat();
        let mut state = PickState::default();
        state.down(&m, 1, 3, 1, false);
        assert!(state.drag_to(&m, pos(1, 8)));
        assert_eq!(ends(&state), Some((pos(1, 3), pos(1, 8))));
        assert!(
            !state.drag_to(&m, pos(1, 8)),
            "the same point changes nothing"
        );
        assert!(state.drag_to(&m, pos(1, 9)));
        assert_eq!(ends(&state), Some((pos(1, 3), pos(1, 9))));
        assert!(state.drag_end());
        assert_eq!(ends(&state), Some((pos(1, 3), pos(1, 9))));
        assert!(!state.drag_to(&m, pos(1, 1)), "no drag, no change");
        assert!(!state.drag_end());
    }

    #[test]
    fn a_drag_backwards_keeps_the_anchor() {
        let m = chat();
        let mut state = PickState::default();
        state.down(&m, 1, 8, 1, false);
        state.drag_to(&m, pos(1, 2));
        assert_eq!(ends(&state), Some((pos(1, 8), pos(1, 2))));
        assert_eq!(state.text.as_ref().unwrap().range_in(&m, 1), Some(2..8));
        // Back to the anchor: nothing is selected, the drag goes on.
        assert!(state.drag_to(&m, pos(1, 8)));
        assert_eq!(state.text, None);
        assert!(state.drag.is_some());
        state.drag_to(&m, pos(1, 10));
        assert_eq!(ends(&state), Some((pos(1, 8), pos(1, 10))));
    }

    #[test]
    fn a_drag_across_messages_skips_the_ones_without_text() {
        let m = chat();
        let mut state = PickState::default();
        state.down(&m, 1, 6, 1, false);
        state.drag_to(&m, pos(4, 4));
        let sel = state.text.as_ref().unwrap();
        assert_eq!(sel.range_in(&m, 1), Some(6..11));
        assert_eq!(sel.range_in(&m, 2), Some(0..16));
        assert_eq!(sel.range_in(&m, 3), None);
        assert_eq!(sel.range_in(&m, 4), Some(0..4));
        assert_eq!(
            state.selected_text(&m, header).as_deref(),
            Some("H1\nworld\n\nH2\nsecond line\nhere\n\nH4\nlast")
        );
        // A position on a message without text does nothing.
        assert!(!state.drag_to(&m, pos(3, 0)));
    }

    #[test]
    fn a_click_clears_the_selection() {
        let m = chat();
        let mut state = PickState::default();
        state.down(&m, 1, 3, 1, false);
        state.drag_to(&m, pos(1, 8));
        state.drag_end();
        assert!(state.text.is_some());
        state.down(&m, 2, 1, 1, false);
        assert_eq!(state.text, None);
        assert_eq!(
            state.drag.as_ref().map(|d| d.origin),
            Some((pos(2, 1), pos(2, 1)))
        );
    }

    #[test]
    fn a_click_on_a_message_without_text_clears_and_starts_no_drag() {
        let m = chat();
        let mut state = PickState::default();
        state.down(&m, 1, 0, 2, false);
        assert!(state.text.is_some());
        state.down(&m, 3, 0, 1, false);
        assert_eq!((&state.text, &state.drag), (&None, &None));
        state.down(&m, 99, 0, 1, false);
        assert_eq!((&state.text, &state.drag), (&None, &None));
    }

    #[test]
    fn a_click_inside_a_grapheme_snaps_to_its_boundary() {
        let m = vec![text(1, "a😀b")];
        let mut state = PickState::default();
        state.down(&m, 1, 3, 1, false);
        assert_eq!(state.drag.as_ref().unwrap().origin.0, pos(1, 1));
        state.drag_to(&m, pos(1, 4));
        assert_eq!(
            ends(&state),
            Some((pos(1, 1), pos(1, 5))),
            "the emoji's far side"
        );
    }

    #[test]
    fn shift_click_extends_and_keeps_the_anchor() {
        let m = chat();
        let mut state = PickState::default();
        state.down(&m, 1, 3, 1, false);
        state.drag_to(&m, pos(1, 8));
        state.drag_end();
        state.down(&m, 1, 10, 1, true);
        assert_eq!(ends(&state), Some((pos(1, 3), pos(1, 10))));
        state.drag_end();
        // Past the other end of the anchor, and into another message.
        state.down(&m, 1, 1, 1, true);
        assert_eq!(ends(&state), Some((pos(1, 3), pos(1, 1))));
        state.drag_end();
        state.down(&m, 4, 4, 1, true);
        assert_eq!(ends(&state), Some((pos(1, 3), pos(4, 4))));
        // The drag that follows moves the focus and keeps the anchor.
        state.drag_to(&m, pos(2, 6));
        assert_eq!(ends(&state), Some((pos(1, 3), pos(2, 6))));
    }

    #[test]
    fn shift_click_without_a_selection_is_a_plain_click() {
        let m = chat();
        let mut state = PickState::default();
        state.down(&m, 1, 3, 1, true);
        assert_eq!(state.text, None);
        assert_eq!(state.drag.as_ref().unwrap().origin, (pos(1, 3), pos(1, 3)));
    }

    #[test]
    fn a_double_click_selects_the_word_and_a_triple_click_the_paragraph() {
        let m = chat();
        let mut state = PickState::default();
        state.down(&m, 1, 8, 2, false);
        assert_eq!(ends(&state), Some((pos(1, 6), pos(1, 11))));
        assert_eq!(state.text.as_ref().unwrap().unit, Unit::Word);
        state.drag_end();

        state.down(&m, 2, 13, 3, false);
        assert_eq!(
            ends(&state),
            Some((pos(2, 12), pos(2, 16))),
            "the second line"
        );
        assert_eq!(state.text.as_ref().unwrap().unit, Unit::Paragraph);
        state.drag_end();
        state.down(&m, 2, 3, 4, false);
        assert_eq!(
            ends(&state),
            Some((pos(2, 0), pos(2, 11))),
            "four clicks are a paragraph too"
        );
        state.drag_end();
        assert_eq!(
            state.selected_text(&m, header).as_deref(),
            Some("second line")
        );
    }

    #[test]
    fn a_drag_after_a_double_click_goes_by_word_in_both_directions() {
        let m = chat();
        let mut state = PickState::default();
        state.down(&m, 1, 8, 2, false);
        // Forward into the middle of the next message's first word: whole words.
        state.drag_to(&m, pos(2, 2));
        assert_eq!(ends(&state), Some((pos(1, 6), pos(2, 6))));
        // Back inside the first word: just that word.
        state.drag_to(&m, pos(1, 9));
        assert_eq!(ends(&state), Some((pos(1, 6), pos(1, 11))));
        // Backwards past the start: the origin word stays selected, to the start of the word reached.
        state.drag_to(&m, pos(1, 2));
        assert_eq!(ends(&state), Some((pos(1, 11), pos(1, 0))));
        assert_eq!(state.text.as_ref().unwrap().range_in(&m, 1), Some(0..11));
    }

    #[test]
    fn a_drag_after_a_triple_click_goes_by_paragraph() {
        let m = chat();
        let mut state = PickState::default();
        state.down(&m, 2, 3, 3, false);
        state.drag_to(&m, pos(4, 2));
        assert_eq!(ends(&state), Some((pos(2, 0), pos(4, 8))));
        state.drag_to(&m, pos(2, 14));
        assert_eq!(
            ends(&state),
            Some((pos(2, 0), pos(2, 16))),
            "the second line joins"
        );
        state.drag_to(&m, pos(1, 3));
        assert_eq!(ends(&state), Some((pos(2, 11), pos(1, 0))));
    }

    #[test]
    fn the_snapshot_is_taken_at_each_endpoint() {
        let m = chat();
        let mut state = PickState::default();
        state.down(&m, 1, 3, 1, false);
        assert!(state.text.is_none());
        state.drag_to(&m, pos(1, 5));
        assert_eq!(state.text.as_ref().unwrap().source.len(), 1);
        state.drag_to(&m, pos(4, 2));
        let source = &state.text.as_ref().unwrap().source;
        assert_eq!(
            source.iter().map(|(ts, t)| (*ts, &**t)).collect::<Vec<_>>(),
            vec![(1, "hello world"), (4, "last one")]
        );
    }

    #[test]
    fn clear_drops_the_selection_and_the_drag() {
        let m = chat();
        let mut state = PickState::default();
        state.down(&m, 1, 3, 1, false);
        state.drag_to(&m, pos(1, 8));
        assert!(state.clear());
        assert_eq!((&state.text, &state.drag), (&None, &None));
        assert!(!state.clear(), "nothing left to clear");
        state.down(&m, 1, 3, 1, false);
        assert!(!state.clear(), "a bare drag is not a selection");
        assert!(state.drag.is_none());
    }

    fn selected(m: &[Message]) -> PickState {
        let mut state = PickState::default();
        state.down(m, 1, 6, 1, false);
        state.drag_to(m, pos(2, 6));
        state.drag_end();
        assert!(state.text.is_some());
        state
    }

    #[test]
    fn validate_keeps_a_selection_that_only_moved_or_whose_other_messages_changed() {
        let m = chat();
        let mut state = selected(&m);
        let mut moved = vec![text(-5, "older")];
        moved.extend(m.clone());
        moved.push(text(9, "newer"));
        moved[3] = message(3, MessageBody::Deleted);
        assert!(!state.validate(&moved));
        assert!(state.text.is_some());
    }

    #[test]
    fn validate_drops_the_selection_after_a_delete() {
        let m = chat();
        let mut state = selected(&m);
        let mut gone = m.clone();
        gone.remove(1);
        assert!(state.validate(&gone));
        assert_eq!((&state.text, &state.drag), (&None, &None));
        assert!(!state.validate(&gone), "nothing to drop twice");
    }

    #[test]
    fn validate_drops_the_selection_after_a_shortening_edit() {
        let m = chat();
        let mut state = selected(&m);
        let mut shorter = m.clone();
        shorter[0] = text(1, "hello");
        assert!(state.validate(&shorter));
        assert!(state.text.is_none());
    }

    #[test]
    fn validate_drops_the_selection_after_a_same_length_edit() {
        let m = vec![text(1, "hello world")];
        let mut state = PickState::default();
        state.down(&m, 1, 6, 1, false);
        state.drag_to(&m, pos(1, 11));
        state.drag_end();
        let edited = vec![text(1, "hello there")];
        assert!(
            state.text.as_ref().unwrap().range_in(&edited, 1).is_some(),
            "the offsets alone would still be in range"
        );
        assert!(state.validate(&edited));
        assert!(state.text.is_none());
        assert_eq!(state.selected_text(&edited, header), None);
    }

    #[test]
    fn select_all_selects_the_loaded_text_and_replaces_a_selection() {
        let m = chat();
        let mut state = selected(&m);
        assert!(state.select_all(&m));
        assert_eq!(ends(&state), Some((pos(1, 0), pos(4, 8))));
        assert!(!state.select_all(&m), "already everything");
        assert_eq!(
            state.selected_text(&m, header).as_deref(),
            Some("H1\nhello world\n\nH2\nsecond line\nhere\n\nH4\nlast one")
        );
        let none = vec![voice(1)];
        assert!(state.select_all(&none));
        assert!(state.text.is_none());
        assert!(!state.select_all(&none));
    }

    #[test]
    fn selected_text_is_none_without_a_real_selection() {
        let m = chat();
        let mut state = PickState::default();
        assert_eq!(state.selected_text(&m, header), None, "no selection");
        state.down(&m, 1, 3, 1, false);
        assert_eq!(state.selected_text(&m, header), None, "a caret");
        state.drag_to(&m, pos(1, 5));
        assert_eq!(state.selected_text(&m, header).as_deref(), Some("lo"));
        // A selection that is empty by construction is not copied either.
        state.text = TextSelection::new(&m, pos(1, 5), pos(1, 5), Unit::Char);
        assert_eq!(state.selected_text(&m, header), None, "equal ends");
    }

    /// One line of text at `x = 10`, every byte `cell` wide.
    struct Line {
        top: f32,
        len: usize,
        cell: f32,
    }

    impl Line {
        fn at(top: f32, len: usize) -> Self {
            Self { top, len, cell: 8. }
        }
    }

    impl TextHit for Line {
        fn bounds(&self) -> Bounds<Pixels> {
            Bounds::new(
                point(px(10.), px(self.top)),
                size(px(self.cell * self.len as f32), px(20.)),
            )
        }

        fn index_for_position(&self, p: Point<Pixels>) -> Result<usize, usize> {
            let index = ((f32::from(p.x) - 10.) / self.cell).round().max(0.) as usize;
            if self.bounds().contains(&p) {
                Ok(index.min(self.len))
            } else {
                Err(index.min(self.len))
            }
        }
    }

    /// Texts 1, 2 and 4 painted at y 0..20, 40..60 and 100..120 (message 3 is a voice row with no text).
    fn painted() -> (Vec<Message>, Vec<(MessageTs, Line)>) {
        let m = vec![
            text(1, "hello world"),
            text(2, "second"),
            voice(3),
            text(4, "last"),
        ];
        let texts = vec![
            (1, Line::at(0., 11)),
            (2, Line::at(40., 6)),
            (4, Line::at(100., 4)),
        ];
        (m, texts)
    }

    fn caret(x: f32, y: f32) -> Option<Pos> {
        let (m, texts) = painted();
        caret_at(point(px(x), px(y)), &texts, &m)
    }

    #[test]
    fn caret_over_a_text_is_the_hit_tested_index() {
        assert_eq!(caret(34., 10.), Some(pos(1, 3)));
        assert_eq!(caret(10., 10.), Some(pos(1, 0)));
        assert_eq!(caret(98., 10.), Some(pos(1, 11)));
        assert_eq!(caret(26., 50.), Some(pos(2, 2)));
        assert_eq!(caret(26., 110.), Some(pos(4, 2)));
    }

    #[test]
    fn caret_beside_a_text_is_the_nearest_index_on_its_line() {
        assert_eq!(caret(0., 10.), Some(pos(1, 0)), "left of the bubble");
        assert_eq!(caret(900., 10.), Some(pos(1, 11)), "right of the bubble");
        assert_eq!(caret(900., 50.), Some(pos(2, 6)));
        assert_eq!(caret(0., 110.), Some(pos(4, 0)));
    }

    #[test]
    fn caret_above_the_first_text_is_its_start_and_below_the_last_its_end() {
        assert_eq!(caret(500., -300.), Some(pos(1, 0)));
        assert_eq!(caret(26., -1.), Some(pos(1, 0)));
        assert_eq!(caret(0., 900.), Some(pos(4, 4)));
        assert_eq!(caret(26., 121.), Some(pos(4, 4)));
    }

    #[test]
    fn caret_between_two_texts_goes_to_the_nearer_one() {
        // Rows 1 and 2 are 20 px apart (y 20..40).
        assert_eq!(
            caret(26., 25.),
            Some(pos(1, 11)),
            "nearer the first: its end"
        );
        assert_eq!(
            caret(26., 35.),
            Some(pos(2, 0)),
            "nearer the second: its start"
        );
        assert_eq!(
            caret(26., 30.),
            Some(pos(1, 11)),
            "a tie goes to the earlier text"
        );
        // Over the voice row between 2 and 4 (y 60..100), which has no text.
        assert_eq!(caret(26., 70.), Some(pos(2, 6)));
        assert_eq!(caret(26., 90.), Some(pos(4, 0)));
    }

    #[test]
    fn caret_skips_texts_of_messages_that_are_not_loaded_and_is_none_without_any() {
        let (m, mut texts) = painted();
        texts.push((7, Line::at(200., 5)));
        assert_eq!(
            caret_at(point(px(26.), px(300.)), &texts, &m),
            Some(pos(4, 4)),
            "message 7 is not loaded"
        );
        assert_eq!(caret_at(point(px(0.), px(0.)), &texts[..0], &m), None);
        assert_eq!(caret_at(point(px(0.), px(0.)), &texts[3..], &m), None);
    }

    #[test]
    fn caret_is_snapped_to_a_grapheme_boundary() {
        let m = vec![text(1, "a😀b")];
        let at = |x: f32| {
            let line = Line {
                top: 0.,
                len: 6,
                cell: 1.,
            };
            caret_at(point(px(x), px(5.)), &[(1, line)], &m)
        };
        // A hit test lands inside the emoji: byte 2 of 1..5, a tie, goes to its start.
        assert_eq!(at(12.), Some(pos(1, 1)));
        assert_eq!(at(14.), Some(pos(1, 5)));
    }

    #[test]
    fn autoscroll_is_zero_inside_and_grows_with_the_distance_outside_up_to_a_cap() {
        let viewport = Bounds::new(point(px(0.), px(100.)), size(px(400.), px(400.)));
        let step = |y: f32| f32::from(autoscroll_step(px(y), viewport));
        assert_eq!(step(100.), 0.);
        assert_eq!(step(300.), 0.);
        assert_eq!(step(500.), 0.);
        assert_eq!(step(510.), 2.5, "below: forward");
        assert_eq!(step(90.), -2.5, "above: back");
        assert!(step(540.) > step(510.) && step(600.) > step(540.));
        assert!(step(40.) < step(90.) && step(-100.) < step(40.));
        assert_eq!(step(5_000.), AUTOSCROLL_MAX);
        assert_eq!(step(-5_000.), -AUTOSCROLL_MAX);
        assert_eq!(
            step(500. + 1_000.),
            AUTOSCROLL_MAX,
            "past the cap stays at it"
        );
    }

    // ---- the Desktop methods, through a real window ----

    fn summary(id: &str) -> ChatSummary {
        ChatSummary {
            id: ChatId(id.into()),
            kind: ChatKind::Group,
            title: "Команда".into(),
            avatar: None,
            last_message: None,
            unread: 0,
            muted: false,
            pinned_rank: None,
            partner: None,
            pinned_message: None,
            pin_action_ts: None,
        }
    }

    fn clipboard(cx: &mut VisualTestContext) -> Option<String> {
        cx.read_from_clipboard().and_then(|item| item.text())
    }

    #[gpui::test]
    fn the_desktop_copies_a_selection_and_writes_nothing_for_an_empty_one(cx: &mut TestAppContext) {
        player_fake::reset();
        cx.update(player_fake::install);
        alerts_fake::reset();
        cx.update(alerts_fake::install);
        let (events, event_rx) = async_channel::unbounded();
        let (command_tx, _commands) = async_channel::unbounded();
        let store = cx.new(|cx| ChatStore::new(event_rx, command_tx, cx));
        let (desktop, cx) = cx.add_window_view({
            let store = store.clone();
            move |window, cx| Desktop::new(store, Clock::fixed(10_000, 0), window, cx)
        });
        cx.simulate_resize(size(px(1200.), px(1400.)));
        events
            .try_send(ClientEvent::ChatsReplaced(vec![summary("g")]))
            .unwrap();
        cx.run_until_parked();
        let row = cx.debug_bounds("chat-g-row").expect("the chat row");
        cx.simulate_click(row.center(), Modifiers::none());
        events
            .try_send(ClientEvent::History(HistoryChunk {
                chat: ChatId("g".into()),
                messages: chat(),
                reached_start: true,
                replaces: None,
                payloads: Vec::new(),
            }))
            .unwrap();
        cx.run_until_parked();
        let g = ChatId("g".into());

        // Nothing selected: Cmd+C leaves the clipboard alone.
        cx.write_to_clipboard(ClipboardItem::new_string("sentinel".into()));
        desktop.update(cx, |d, cx| d.copy_selection(cx));
        assert_eq!(clipboard(cx).as_deref(), Some("sentinel"));

        // A click moves focus from the composer to the root and selects nothing yet.
        desktop.update_in(cx, |d, window, cx| {
            window.focus(&d.compose.input.focus_handle(cx))
        });
        let root_focused = |cx: &mut VisualTestContext| {
            cx.update(|window, cx| desktop.read(cx).focus.is_focused(window))
        };
        assert!(!root_focused(cx));
        desktop.update_in(cx, |d, window, cx| {
            d.text_down(g.clone(), pos(1, 3), 1, false, window, cx)
        });
        assert!(root_focused(cx));
        desktop.update(cx, |d, cx| d.copy_selection(cx));
        assert_eq!(
            clipboard(cx).as_deref(),
            Some("sentinel"),
            "a caret copies nothing"
        );

        // Drag to byte 8 of the first message (8 px a byte from x = 10), then release.
        desktop.update(cx, |d, cx| {
            d.drag_to(point(px(74.), px(10.)), &[(1, Line::at(0., 11))], cx)
        });
        desktop.update(cx, |d, cx| d.drag_end(cx));
        desktop.update(cx, |d, cx| d.copy_selection(cx));
        assert_eq!(clipboard(cx).as_deref(), Some("lo wo"));

        // Clearing reports whether there was something, and a copy after it writes nothing.
        cx.write_to_clipboard(ClipboardItem::new_string("sentinel".into()));
        assert!(desktop.update(cx, |d, cx| d.clear_selection(cx)));
        assert!(!desktop.update(cx, |d, cx| d.clear_selection(cx)));
        desktop.update(cx, |d, cx| d.copy_selection(cx));
        assert_eq!(clipboard(cx).as_deref(), Some("sentinel"));

        // Select all: one block per message with text, headed by the author and the date.
        desktop.update(cx, |d, cx| d.select_all_text(cx));
        desktop.update(cx, |d, cx| d.copy_selection(cx));
        let at = "Алиса, 01.01.1970 00:00";
        assert_eq!(
            clipboard(cx),
            Some(format!(
                "{at}\nhello world\n\n{at}\nsecond line\nhere\n\n{at}\nlast one"
            ))
        );

        // A click for another chat is ignored.
        desktop.update(cx, |d, cx| d.clear_selection(cx));
        desktop.update_in(cx, |d, window, cx| {
            d.text_down(ChatId("other".into()), pos(1, 3), 1, false, window, cx)
        });
        assert!(desktop.read_with(cx, |d, cx| d.picking.read(cx).drag.is_none()));
    }
}
