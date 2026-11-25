// Adapted from GPUI 0.2.2 examples/input.rs (Apache-2.0). See THIRD_PARTY.md.
use std::ops::Range;

use crate::theme::ACCENT;
use gpui::{
    App, AvailableSpace, Bounds, ClipboardItem, ContentMask, Context, CursorStyle, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable,
    GlobalElementId, KeyBinding, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, PaintQuad, Pixels, Point, ScrollWheelEvent, SharedString, Style, TextAlign,
    TextRun, UTF16Selection, UnderlineStyle, Window, WrappedLine, actions, div, fill, point,
    prelude::*, px, relative, rgb, rgba, size,
};
use unicode_segmentation::*;

actions!(
    text_input,
    [
        Backspace,
        Delete,
        Left,
        Right,
        Up,
        Down,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        SelectAll,
        Home,
        End,
        Submit,
        Newline,
        ShowCharacterPalette,
        Paste,
        Cut,
        Copy,
        Quit,
    ]
);

/// Rows a multi-line field grows to before it scrolls inside.
const MAX_ROWS: usize = 8;

/// What a field tells its owner. Programmatic changes (`set_text`) are not reported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputEvent {
    /// Enter in a multi-line field.
    Submit,
    /// The user changed the text.
    Edited,
    /// Up in an empty multi-line field.
    UpInEmpty,
}

pub struct TextInput {
    focus_handle: FocusHandle,
    pub content: SharedString,
    placeholder: SharedString,
    multiline: bool,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    layout: Option<LastLayout>,
    is_selecting: bool,
    /// Distance the multi-line content is scrolled up; clamped when painted.
    scroll_y: Pixels,
    /// The next paint scrolls the caret into view.
    reveal_caret: bool,
    /// Column kept while moving the caret over shorter rows with Up and Down.
    preferred_x: Option<Pixels>,
}

impl EventEmitter<InputEvent> for TextInput {}

/// One visual row: a whole logical line, or a piece of one that was wrapped.
#[derive(Clone, Debug, Default)]
struct Row {
    line: usize,
    /// Byte offset of the logical line in the whole text.
    line_start: usize,
    /// Byte range of the row in the whole text.
    start: usize,
    end: usize,
    /// Position of `start` along the unwrapped line.
    x0: Pixels,
    last_in_line: bool,
}

/// Text shaped into logical lines and cut into visual rows of equal height.
struct Shaped {
    lines: Vec<WrappedLine>,
    rows: Vec<Row>,
    line_height: Pixels,
}

impl Shaped {
    fn new(
        text: SharedString,
        font_size: Pixels,
        runs: &[TextRun],
        wrap: Option<Pixels>,
        line_height: Pixels,
        window: &Window,
    ) -> Self {
        let lines: Vec<WrappedLine> = window
            .text_system()
            .shape_text(text, font_size, runs, wrap, None)
            .map(|lines| lines.into_iter().collect())
            .unwrap_or_default();
        let mut rows = Vec::new();
        let mut line_start = 0;
        for (line_ix, line) in lines.iter().enumerate() {
            let unwrapped = &line.unwrapped_layout;
            let mut starts = vec![0];
            starts.extend(
                line.wrap_boundaries
                    .iter()
                    .map(|b| unwrapped.runs[b.run_ix].glyphs[b.glyph_ix].index),
            );
            for (k, &start) in starts.iter().enumerate() {
                rows.push(Row {
                    line: line_ix,
                    line_start,
                    start: line_start + start,
                    end: line_start + starts.get(k + 1).copied().unwrap_or(line.len()),
                    x0: unwrapped.x_for_index(start),
                    last_in_line: k + 1 == starts.len(),
                });
            }
            line_start += line.len() + 1;
        }
        if rows.is_empty() {
            rows.push(Row {
                last_in_line: true,
                ..Row::default()
            });
        }
        Self {
            lines,
            rows,
            line_height,
        }
    }

    fn height(&self) -> Pixels {
        self.line_height * self.rows.len() as f32
    }

    /// The row that shows the caret at `index`: a wrap point belongs to the row that starts there.
    fn row_of(&self, index: usize) -> usize {
        self.rows
            .partition_point(|row| row.start <= index)
            .saturating_sub(1)
    }

    fn x_in_row(&self, row: &Row, index: usize) -> Pixels {
        self.lines.get(row.line).map_or(px(0.), |line| {
            line.unwrapped_layout
                .x_for_index(index.saturating_sub(row.line_start))
                - row.x0
        })
    }

    /// Top-left of the caret at `index`, relative to the text origin.
    fn position(&self, index: usize) -> Point<Pixels> {
        let ix = self.row_of(index);
        point(
            self.x_in_row(&self.rows[ix], index),
            self.line_height * ix as f32,
        )
    }

    /// Where the caret goes at the end of a row: before a wrap point, since after it the caret would show on the next row.
    fn end_of_row(&self, ix: usize, text: &str) -> usize {
        let row = &self.rows[ix];
        if row.last_in_line {
            return row.end;
        }
        let last = text[row.start..row.end]
            .graphemes(true)
            .next_back()
            .map_or(0, str::len);
        row.end - last
    }

    fn index_in_row(&self, ix: usize, x: Pixels, text: &str) -> usize {
        let row = &self.rows[ix];
        let Some(line) = self.lines.get(row.line) else {
            return 0;
        };
        let index = (row.line_start + line.unwrapped_layout.closest_index_for_x(x + row.x0))
            .clamp(row.start, row.end);
        if index >= row.end {
            self.end_of_row(ix, text)
        } else {
            index
        }
    }

    /// The closest caret position to `point`, relative to the text origin; outside the text it clamps to the nearest row.
    fn index_at(&self, point: Point<Pixels>, text: &str) -> usize {
        let row = (point.y / self.line_height).floor().max(0.) as usize;
        self.index_in_row(row.min(self.rows.len() - 1), point.x, text)
    }
}

/// The layout of the last paint, for hit-testing and caret movement between paints.
struct LastLayout {
    /// The text this was shaped from; the layout is stale once the content differs.
    content: SharedString,
    shaped: Shaped,
    bounds: Bounds<Pixels>,
    scroll_y: Pixels,
}

impl TextInput {
    pub fn single_line(placeholder: &str, cx: &mut Context<Self>) -> Self {
        Self::with_mode(placeholder, false, cx)
    }

    /// Wraps long lines, grows with its text up to `MAX_ROWS` rows and then scrolls. Enter submits,
    /// Shift+Enter inserts a line break.
    pub fn multi_line(placeholder: &str, cx: &mut Context<Self>) -> Self {
        Self::with_mode(placeholder, true, cx)
    }

    fn with_mode(placeholder: &str, multiline: bool, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            content: "".into(),
            placeholder: placeholder.to_owned().into(),
            multiline,
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            layout: None,
            is_selecting: false,
            scroll_y: px(0.),
            reveal_caret: false,
            preferred_x: None,
        }
    }

    /// Replaces the text with the caret at its end. Not reported as an edit.
    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.content = text.to_owned().into();
        self.selected_range = text.len()..text.len();
        self.selection_reversed = false;
        self.marked_range = None;
        self.layout = None;
        self.scroll_y = px(0.);
        self.reveal_caret = true;
        self.preferred_x = None;
        cx.notify();
    }

    #[cfg(test)]
    pub fn cursor(&self) -> usize {
        self.cursor_offset()
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx)
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), cx);
        } else {
            self.move_to(self.selected_range.end, cx)
        }
    }

    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        if !self.multiline {
            return cx.propagate();
        }
        if self.content.is_empty() {
            cx.emit(InputEvent::UpInEmpty);
        } else {
            self.vertical(-1, false, cx);
        }
    }

    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        if !self.multiline {
            return cx.propagate();
        }
        self.vertical(1, false, cx);
    }

    fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        if !self.multiline {
            return cx.propagate();
        }
        self.vertical(-1, true, cx);
    }

    fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        if !self.multiline {
            return cx.propagate();
        }
        self.vertical(1, true, cx);
    }

    /// Moves the caret one visual row up or down, keeping its column; past the first or last row it goes to the text's edge.
    fn vertical(&mut self, delta: isize, extend: bool, cx: &mut Context<Self>) {
        let Some(layout) = self.fresh_layout() else {
            return;
        };
        let shaped = &layout.shaped;
        let cursor = self.cursor_offset();
        let current = shaped.row_of(cursor);
        let x = self
            .preferred_x
            .unwrap_or_else(|| shaped.x_in_row(&shaped.rows[current], cursor));
        let target = current as isize + delta;
        let index = if target < 0 {
            0
        } else if target as usize >= shaped.rows.len() {
            self.content.len()
        } else {
            shaped.index_in_row(target as usize, x, &self.content)
        };
        if extend {
            self.select_to(index, cx);
        } else {
            self.move_to(index, cx);
        }
        self.preferred_x = Some(x);
    }

    fn submit(&mut self, _: &Submit, _: &mut Window, cx: &mut Context<Self>) {
        // While an IME composes, Enter commits the composition instead.
        if !self.multiline || self.marked_range.is_some() {
            return cx.propagate();
        }
        cx.emit(InputEvent::Submit);
    }

    fn newline(&mut self, _: &Newline, window: &mut Window, cx: &mut Context<Self>) {
        if !self.multiline {
            return cx.propagate();
        }
        self.replace_text_in_range(None, "\n", window, cx);
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx)
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        let start = self.fresh_layout().map_or(0, |l| {
            l.shaped.rows[l.shaped.row_of(self.cursor_offset())].start
        });
        self.move_to(start, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        let end = self.fresh_layout().map_or(self.content.len(), |l| {
            l.shaped
                .end_of_row(l.shaped.row_of(self.cursor_offset()), &self.content)
        });
        self.move_to(end, cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.previous_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.next_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.is_selecting = true;

        if event.modifiers.shift {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        } else {
            self.move_to(self.index_for_mouse_position(event.position), cx)
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _window: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, window: &mut Window, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(window.line_height()).y;
        self.scroll_y = (self.scroll_y - delta).max(px(0.));
        cx.notify();
    }

    fn show_character_palette(
        &mut self,
        _: &ShowCharacterPalette,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        window.show_character_palette();
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            let text = if self.multiline {
                text.replace("\r\n", "\n").replace('\r', "\n")
            } else {
                text.replace('\n', " ")
            };
            self.replace_text_in_range(None, &text, window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
    }
    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
            self.replace_text_in_range(None, "", window, cx)
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        self.preferred_x = None;
        self.reveal_caret = true;
        cx.notify()
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    /// The last paint's layout while it still matches the text.
    fn current_layout(&self) -> Option<&LastLayout> {
        self.layout.as_ref().filter(|l| l.content == self.content)
    }

    /// `current_layout` for hit-testing and caret movement: with no text there is nothing to hit.
    fn fresh_layout(&self) -> Option<&LastLayout> {
        self.current_layout().filter(|_| !self.content.is_empty())
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        let Some(layout) = self.fresh_layout() else {
            return 0;
        };
        let mut local = position - layout.bounds.origin;
        local.y += layout.scroll_y;
        layout.shaped.index_at(local, &self.content)
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        };
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        self.preferred_x = None;
        self.reveal_caret = true;
        cx.notify()
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;

        for ch in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }

        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;

        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }

        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(idx, _)| (idx < offset).then_some(idx))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(idx, _)| (idx > offset).then_some(idx))
            .unwrap_or(self.content.len())
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.selection_reversed = false;
        self.marked_range.take();
        self.preferred_x = None;
        self.reveal_caret = true;
        cx.emit(InputEvent::Edited);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        if !new_text.is_empty() {
            self.marked_range = Some(range.start..range.start + new_text.len());
        } else {
            self.marked_range = None;
        }
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .map(|new_range| new_range.start + range.start..new_range.end + range.end)
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
        self.selection_reversed = false;
        self.preferred_x = None;
        self.reveal_caret = true;
        cx.emit(InputEvent::Edited);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let layout = self.current_layout()?;
        let range = self.range_from_utf16(&range_utf16);
        let (start, end) = (
            layout.shaped.position(range.start),
            layout.shaped.position(range.end),
        );
        let origin = bounds.origin - point(px(0.), layout.scroll_y);
        Some(Bounds::from_corners(
            origin + start,
            origin + point(end.x, end.y + layout.shaped.line_height),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let layout = self.fresh_layout()?;
        let mut local = layout.bounds.localize(&point)?;
        local.y += layout.scroll_y;
        Some(self.offset_to_utf16(layout.shaped.index_at(local, &self.content)))
    }
}

struct TextElement {
    input: Entity<TextInput>,
}

struct PrepaintState {
    shaped: Shaped,
    cursor: Option<PaintQuad>,
    selection: Vec<PaintQuad>,
    scroll_y: Pixels,
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        let input = self.input.read(cx);
        if !input.multiline {
            style.size.height = window.line_height().into();
            return (window.request_layout(style, [], cx), ());
        }
        // The height follows the wrapped text, up to `MAX_ROWS` rows.
        let text = if input.content.is_empty() {
            input.placeholder.clone()
        } else {
            input.content.clone()
        };
        let text_style = window.text_style();
        let font = text_style.font();
        let font_size = text_style.font_size.to_pixels(window.rem_size());
        let line_height = window.line_height();
        let layout = window.request_measured_layout(style, move |known, available, window, _| {
            let width = known.width.or(match available.width {
                AvailableSpace::Definite(width) => Some(width),
                _ => None,
            });
            let run = TextRun {
                len: text.len(),
                font: font.clone(),
                color: Default::default(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let rows = Shaped::new(text.clone(), font_size, &[run], width, line_height, window)
                .rows
                .len();
            size(
                width.unwrap_or_default(),
                line_height * rows.clamp(1, MAX_ROWS) as f32,
            )
        });
        (layout, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let content = input.content.clone();
        let selected_range = input.selected_range.clone();
        let cursor = input.cursor_offset();
        let (multiline, mut scroll_y, reveal) =
            (input.multiline, input.scroll_y, input.reveal_caret);
        let style = window.text_style();

        let (display_text, text_color) = if content.is_empty() {
            (input.placeholder.clone(), rgb(0x8fa3bb).into())
        } else {
            (content, style.color)
        };

        let run = TextRun {
            len: display_text.len(),
            font: style.font(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = if let Some(marked_range) = input.marked_range.as_ref() {
            vec![
                TextRun {
                    len: marked_range.start,
                    ..run.clone()
                },
                TextRun {
                    len: marked_range.end - marked_range.start,
                    underline: Some(UnderlineStyle {
                        color: Some(run.color),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun {
                    len: display_text.len() - marked_range.end,
                    ..run
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect()
        } else {
            vec![run]
        };

        let font_size = style.font_size.to_pixels(window.rem_size());
        let wrap = multiline.then_some(bounds.size.width);
        let shaped = Shaped::new(
            display_text,
            font_size,
            &runs,
            wrap,
            window.line_height(),
            window,
        );

        let line_height = shaped.line_height;
        let caret = shaped.position(cursor);
        if multiline {
            scroll_y = scroll_y.clamp(px(0.), (shaped.height() - bounds.size.height).max(px(0.)));
            if reveal {
                if caret.y < scroll_y {
                    scroll_y = caret.y;
                } else if caret.y + line_height > scroll_y + bounds.size.height {
                    scroll_y = caret.y + line_height - bounds.size.height;
                }
            }
        }
        self.input.update(cx, |input, _| {
            input.scroll_y = scroll_y;
            input.reveal_caret = false;
        });

        let origin = bounds.origin - point(px(0.), scroll_y);
        let (selection, cursor) = if selected_range.is_empty() {
            (
                Vec::new(),
                Some(fill(
                    Bounds::new(origin + caret, size(px(2.), line_height)),
                    rgb(ACCENT),
                )),
            )
        } else {
            let quads = shaped
                .rows
                .iter()
                .enumerate()
                .filter(|(_, row)| {
                    row.start <= selected_range.end && selected_range.start <= row.end
                })
                .map(|(ix, row)| {
                    let from = selected_range.start.max(row.start);
                    let to = selected_range.end.min(row.end);
                    let (left, right) = (shaped.x_in_row(row, from), shaped.x_in_row(row, to));
                    let top = origin.y + line_height * ix as f32;
                    fill(
                        Bounds::from_corners(
                            point(origin.x + left, top),
                            point(origin.x + right, top + line_height),
                        ),
                        rgba(0x6ab2f255),
                    )
                })
                .collect();
            (quads, None)
        };
        PrepaintState {
            shaped,
            cursor,
            selection,
            scroll_y,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        let line_height = prepaint.shaped.line_height;
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for selection in prepaint.selection.drain(..) {
                window.paint_quad(selection);
            }
            let mut origin = bounds.origin - point(px(0.), prepaint.scroll_y);
            for line in &prepaint.shaped.lines {
                line.paint(origin, line_height, TextAlign::Left, None, window, cx)
                    .unwrap();
                origin.y += line.size(line_height).height;
            }
            if focus_handle.is_focused(window)
                && let Some(cursor) = prepaint.cursor.take()
            {
                window.paint_quad(cursor);
            }
        });

        let shaped = std::mem::replace(
            &mut prepaint.shaped,
            Shaped {
                lines: Vec::new(),
                rows: Vec::new(),
                line_height,
            },
        );
        let scroll_y = prepaint.scroll_y;
        self.input.update(cx, |input, _cx| {
            input.layout = Some(LastLayout {
                content: input.content.clone(),
                shaped,
                bounds,
                scroll_y,
            });
        });
    }
}

impl Render for TextInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let field = div()
            .flex()
            .key_context("TextInput")
            .track_focus(&self.focus_handle(cx))
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::submit))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::show_character_palette))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .w_full()
            .overflow_hidden()
            .text_color(rgb(0xe6edf5))
            .text_size(px(15.));
        if self.multiline {
            field
                .line_height(px(22.))
                .on_scroll_wheel(cx.listener(Self::on_scroll))
                .child(TextElement { input: cx.entity() })
        } else {
            field.line_height(px(30.)).child(
                div()
                    .h(px(30. + 4. * 2.))
                    .w_full()
                    .p(px(4.))
                    .bg(rgb(0x202e40))
                    .child(TextElement { input: cx.entity() }),
            )
        }
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

pub fn bind(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some("TextInput")),
        KeyBinding::new("delete", Delete, Some("TextInput")),
        KeyBinding::new("left", Left, Some("TextInput")),
        KeyBinding::new("right", Right, Some("TextInput")),
        KeyBinding::new("up", Up, Some("TextInput")),
        KeyBinding::new("down", Down, Some("TextInput")),
        KeyBinding::new("shift-left", SelectLeft, Some("TextInput")),
        KeyBinding::new("shift-right", SelectRight, Some("TextInput")),
        KeyBinding::new("shift-up", SelectUp, Some("TextInput")),
        KeyBinding::new("shift-down", SelectDown, Some("TextInput")),
        KeyBinding::new("enter", Submit, Some("TextInput")),
        KeyBinding::new("shift-enter", Newline, Some("TextInput")),
        KeyBinding::new("cmd-a", SelectAll, Some("TextInput")),
        KeyBinding::new("cmd-v", Paste, Some("TextInput")),
        KeyBinding::new("cmd-c", Copy, Some("TextInput")),
        KeyBinding::new("cmd-x", Cut, Some("TextInput")),
        KeyBinding::new("ctrl-a", SelectAll, Some("TextInput")),
        KeyBinding::new("ctrl-v", Paste, Some("TextInput")),
        KeyBinding::new("ctrl-c", Copy, Some("TextInput")),
        KeyBinding::new("ctrl-x", Cut, Some("TextInput")),
        KeyBinding::new("home", Home, Some("TextInput")),
        KeyBinding::new("end", End, Some("TextInput")),
        KeyBinding::new("cmd-left", Home, Some("TextInput")),
        KeyBinding::new("cmd-right", End, Some("TextInput")),
        KeyBinding::new("ctrl-cmd-space", ShowCharacterPalette, Some("TextInput")),
    ]);
}
