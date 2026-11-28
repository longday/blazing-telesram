//! The text selection model of the timeline (D1, D7 of `add-text-selection-and-quotes`): pure, no gpui.
//!
//! A selection is keyed by message timestamp and byte offset, so it survives scrolling, rows leaving
//! the viewport and older history loading above. Offsets are bytes on grapheme boundaries. The messages
//! are always the loaded vector of the open chat, oldest first.

use bt_chat_core::model::{Delivery, Message, MessageBody, MessageTs};
use std::ops::Range;
use std::sync::Arc;
use unicode_segmentation::UnicodeSegmentation;

/// The one selectable text of a message: `Text`, or the caption of an image or a gallery; nothing for the rest.
pub fn selectable_text(m: &Message) -> Option<&str> {
    match &m.body {
        MessageBody::Text(text) => Some(text),
        MessageBody::Image { caption, .. } | MessageBody::Gallery { caption, .. } => {
            caption.as_deref()
        }
        _ => None,
    }
}

/// The position of the message with timestamp `ts` in `messages` (sorted by timestamp).
fn index_of(messages: &[Message], ts: MessageTs) -> Option<usize> {
    messages.binary_search_by_key(&ts, |m| m.timestamp).ok()
}

/// The selectable text of the message with timestamp `ts`.
pub fn text_of(messages: &[Message], ts: MessageTs) -> Option<&str> {
    selectable_text(&messages[index_of(messages, ts)?])
}

/// A caret: a message and a byte offset into its selectable text (always on a grapheme boundary).
/// The derived order is the timeline order: the message first, then the offset.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Pos {
    pub ts: MessageTs,
    pub offset: usize,
}

/// What a drag extends by; from the click count of the gesture.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Unit {
    #[default]
    Char,
    Word,
    Paragraph,
}

/// `anchor` is where the gesture started, `focus` where the pointer is now; either may be the earlier one.
/// `source` holds the selectable text of each endpoint message as it was when the endpoint was set (one
/// entry, or two when the endpoints lie in different messages), so an edit that keeps the length is
/// noticed by `validate`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TextSelection {
    pub anchor: Pos,
    pub focus: Pos,
    pub unit: Unit,
    pub source: Vec<(MessageTs, Arc<str>)>,
}

impl TextSelection {
    /// A selection from `anchor` to `focus` with the endpoint texts snapshotted from `messages`. `None`
    /// when an endpoint is not on a message with selectable text, or its offset is past the text or
    /// inside a character.
    pub fn new(messages: &[Message], anchor: Pos, focus: Pos, unit: Unit) -> Option<Self> {
        let mut source: Vec<(MessageTs, Arc<str>)> = Vec::with_capacity(2);
        for pos in [anchor, focus] {
            let text = text_of(messages, pos.ts)?;
            if pos.offset > text.len() || !text.is_char_boundary(pos.offset) {
                return None;
            }
            if !source.iter().any(|(ts, _)| *ts == pos.ts) {
                source.push((pos.ts, Arc::from(text)));
            }
        }
        Some(Self {
            anchor,
            focus,
            unit,
            source,
        })
    }

    /// The earlier and the later endpoint.
    pub fn ends(&self) -> (Pos, Pos) {
        if self.anchor <= self.focus {
            (self.anchor, self.focus)
        } else {
            (self.focus, self.anchor)
        }
    }

    /// The selected byte range inside `m`: the whole text for a message strictly between the ends, the
    /// tail from the start, the head up to the end, or the part between them. `None` outside the
    /// selection, for a message without selectable text and for an empty range.
    fn range_for(&self, m: &Message) -> Option<Range<usize>> {
        let (start, end) = self.ends();
        if m.timestamp < start.ts || m.timestamp > end.ts {
            return None;
        }
        let text = selectable_text(m)?;
        let from = if m.timestamp == start.ts {
            start.offset
        } else {
            0
        };
        let to = if m.timestamp == end.ts {
            end.offset
        } else {
            text.len()
        };
        let (from, to) = (from.min(text.len()), to.min(text.len()));
        (from < to).then_some(from..to)
    }

    /// The selected byte range inside message `ts`; `None` outside the selection, when the message is
    /// not in `messages` or has no selectable text, and when nothing of it is selected.
    pub fn range_in(&self, messages: &[Message], ts: MessageTs) -> Option<Range<usize>> {
        self.range_for(&messages[index_of(messages, ts)?])
    }

    /// The messages from the earlier endpoint's to the later one's.
    fn span<'a>(&self, messages: &'a [Message]) -> impl Iterator<Item = &'a Message> + use<'a> {
        let (start, end) = self.ends();
        let first = messages.partition_point(|m| m.timestamp < start.ts);
        messages[first..]
            .iter()
            .take_while(move |m| m.timestamp <= end.ts)
    }

    /// True when no character is selected: equal ends, or no message in between has text to select.
    pub fn is_empty(&self, messages: &[Message]) -> bool {
        self.anchor == self.focus || self.span(messages).all(|m| self.range_for(m).is_none())
    }

    /// `Some(ts)` when the selection lies inside one message.
    pub fn single(&self) -> Option<MessageTs> {
        (self.anchor.ts == self.focus.ts).then_some(self.anchor.ts)
    }

    /// Whether the selection still means what the user selected: both endpoint messages exist, their
    /// current selectable text **equals the snapshot** (a same-length edit keeps the offsets in range
    /// but changes their meaning), and the offsets are on char boundaries. The selection is dropped
    /// when this is false.
    pub fn validate(&self, messages: &[Message]) -> bool {
        [self.anchor, self.focus].into_iter().all(|pos| {
            let Some((_, snapshot)) = self.source.iter().find(|(ts, _)| *ts == pos.ts) else {
                return false;
            };
            text_of(messages, pos.ts).is_some_and(|text| {
                text == &**snapshot && pos.offset <= text.len() && text.is_char_boundary(pos.offset)
            })
        })
    }
}

/// How a run of a text is treated by a double click.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Run {
    Word,
    Space,
    Newline,
    /// Punctuation, merged with the punctuation next to it.
    Punct,
    /// Emoji and other symbols; each its own run.
    Symbol,
}

impl Run {
    fn of(segment: &str) -> Self {
        if segment.chars().any(char::is_alphanumeric) {
            Self::Word
        } else if segment.chars().all(|c| c == '\n' || c == '\r') {
            Self::Newline
        } else if segment.chars().all(char::is_whitespace) {
            Self::Space
        } else if segment.chars().all(is_punctuation) {
            Self::Punct
        } else {
            Self::Symbol
        }
    }

    /// A run of spaces or of punctuation is one word; a word, a line break and a symbol are not extended.
    fn merges(self) -> bool {
        matches!(self, Self::Space | Self::Punct)
    }
}

/// ASCII punctuation, Latin-1 punctuation (`«»¿¡`) and general punctuation (dashes, quotes, `…`).
fn is_punctuation(c: char) -> bool {
    c.is_ascii_punctuation() || matches!(c, '\u{a1}'..='\u{bf}' | '\u{2010}'..='\u{2027}')
}

/// The word around `offset` by UAX #29 word bounds; a run of spaces or of punctuation is its own word.
/// At the end of the text, the last word. An empty text gives `0..0`.
pub fn word_at(text: &str, offset: usize) -> Range<usize> {
    let offset = text.floor_char_boundary(offset);
    let mut current: Option<(Range<usize>, Run)> = None;
    for (start, segment) in text.split_word_bound_indices() {
        let run = Run::of(segment);
        let end = start + segment.len();
        match &mut current {
            Some((range, previous)) if *previous == run && run.merges() => range.end = end,
            _ => {
                if let Some((range, _)) = current.take()
                    && range.end > offset
                {
                    return range;
                }
                current = Some((start..end, run));
            }
        }
    }
    current.map_or(0..0, |(range, _)| range)
}

/// The paragraph around `offset`: the text between `\n`s, the line break not included. At the end of a
/// line, that line.
pub fn paragraph_at(text: &str, offset: usize) -> Range<usize> {
    let offset = text.floor_char_boundary(offset);
    let start = text[..offset].rfind('\n').map_or(0, |i| i + 1);
    let end = text[offset..].find('\n').map_or(text.len(), |i| offset + i);
    let end = if end > start && text[..end].ends_with('\r') {
        end - 1
    } else {
        end
    };
    start..end
}

/// The grapheme boundary nearest to `offset` (a tie goes to the earlier one), clamped to the text. What
/// a hit test returns is on a char boundary, which may be inside an emoji sequence or before a combining mark.
pub fn snap(text: &str, offset: usize) -> usize {
    if offset >= text.len() {
        return text.len();
    }
    let (mut previous, mut next) = (0, text.len());
    for (i, _) in text.grapheme_indices(true) {
        if i <= offset {
            previous = i;
        } else {
            next = i;
            break;
        }
    }
    if offset - previous <= next - offset {
        previous
    } else {
        next
    }
}

/// Everything selectable in the loaded messages: from the start of the first message with text to the
/// end of the last. `None` when no message has text.
pub fn select_all(messages: &[Message]) -> Option<TextSelection> {
    let has_text = |m: &&Message| selectable_text(m).is_some_and(|text| !text.is_empty());
    let first = messages.iter().find(has_text)?;
    let last = messages.iter().rfind(has_text)?;
    TextSelection::new(
        messages,
        Pos {
            ts: first.timestamp,
            offset: 0,
        },
        Pos {
            ts: last.timestamp,
            offset: selectable_text(last)?.len(),
        },
        Unit::Char,
    )
}

/// The text a copy puts on the clipboard (D7). Inside one message: the bare selected text. Over several
/// messages: per message with selected text a block `header(m)\n{text}`, blocks separated by one blank
/// line, the first cut at the start offset, the last at the end. Messages without selectable text
/// contribute nothing. Line endings are `\n`. A selection that does not `validate` yields an empty
/// string: the text is never taken from a stale snapshot.
pub fn extract_text(
    messages: &[Message],
    sel: &TextSelection,
    header: impl Fn(&Message) -> String,
) -> String {
    if !sel.validate(messages) {
        return String::new();
    }
    let parts = sel.span(messages).filter_map(|m| {
        let text = selectable_text(m)?.get(sel.range_for(m)?)?;
        Some((m, text))
    });
    let text = if sel.single().is_some() {
        parts
            .map(|(_, text)| text.to_owned())
            .next()
            .unwrap_or_default()
    } else {
        parts
            .map(|(m, text)| format!("{}\n{text}", header(m)))
            .collect::<Vec<_>>()
            .join("\n\n")
    };
    if text.contains("\r\n") {
        text.replace("\r\n", "\n")
    } else {
        text
    }
}

/// The text of a message as Copy takes it: the text or the caption, the file name of a file or a video;
/// nothing for a voice message, a deleted placeholder and an unsupported type.
pub fn copy_text(m: &Message) -> Option<&str> {
    match &m.body {
        MessageBody::Text(text) => Some(text),
        MessageBody::Image { caption, .. } | MessageBody::Gallery { caption, .. } => {
            caption.as_deref()
        }
        MessageBody::File { name, .. } | MessageBody::Video { name, .. } => Some(name),
        MessageBody::Voice { .. } | MessageBody::Deleted | MessageBody::Unsupported(_) => None,
    }
}

/// A message the user can act on (D11): sent, not pending or failed, and not a deleted placeholder.
pub fn eligible(m: &Message) -> bool {
    m.delivery == Delivery::Sent && !matches!(m.body, MessageBody::Deleted)
}

/// Whether every one of `messages` is the user's own eligible message, which a batch delete needs.
/// False for no messages.
pub fn all_deletable(messages: &[&Message]) -> bool {
    !messages.is_empty() && messages.iter().all(|m| m.outgoing && eligible(m))
}

/// The messages marked in message mode (D11): sorted unique timestamps of eligible messages, and the one
/// toggled last, which a Shift click ranges from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MessageSet {
    marked: Vec<MessageTs>,
    last: Option<MessageTs>,
}

impl MessageSet {
    /// A set with message `ts` marked; `None` when it is not loaded or not eligible.
    pub fn new(messages: &[Message], ts: MessageTs) -> Option<Self> {
        let mut set = Self::default();
        set.toggle(messages, ts).then_some(set)
    }

    pub fn contains(&self, ts: MessageTs) -> bool {
        self.marked.binary_search(&ts).is_ok()
    }

    pub fn is_empty(&self) -> bool {
        self.marked.is_empty()
    }

    /// Marks `ts` if it is not marked and unmarks it if it is, and makes it the message a Shift click
    /// ranges from. Returns false (and changes nothing) for a message that is not loaded or not eligible.
    pub fn toggle(&mut self, messages: &[Message], ts: MessageTs) -> bool {
        if !index_of(messages, ts).is_some_and(|ix| eligible(&messages[ix])) {
            return false;
        }
        match self.marked.binary_search(&ts) {
            Ok(ix) => {
                self.marked.remove(ix);
            }
            Err(ix) => self.marked.insert(ix, ts),
        }
        self.last = Some(ts);
        true
    }

    /// Marks every eligible message from the one toggled last to `ts`, both included, in either
    /// direction; the ineligible ones between are skipped. `ts` becomes the message to range from. With
    /// no earlier toggle, or one that is no longer loaded, it marks `ts` alone. Returns whether the set
    /// changed; false for a `ts` that is not loaded or not eligible.
    pub fn extend_to(&mut self, messages: &[Message], ts: MessageTs) -> bool {
        let Some(to) = index_of(messages, ts).filter(|ix| eligible(&messages[*ix])) else {
            return false;
        };
        let Some(from) = self.last.and_then(|last| index_of(messages, last)) else {
            self.last = Some(ts);
            return match self.marked.binary_search(&ts) {
                Ok(_) => false,
                Err(ix) => {
                    self.marked.insert(ix, ts);
                    true
                }
            };
        };
        let before = self.marked.len();
        for m in &messages[from.min(to)..=from.max(to)] {
            if eligible(m)
                && let Err(ix) = self.marked.binary_search(&m.timestamp)
            {
                self.marked.insert(ix, m.timestamp);
            }
        }
        self.last = Some(ts);
        self.marked.len() != before
    }

    /// Every eligible message of `messages`; `None` when there is none.
    pub fn select_all(messages: &[Message]) -> Option<Self> {
        let marked: Vec<_> = messages
            .iter()
            .filter(|m| eligible(m))
            .map(|m| m.timestamp)
            .collect();
        (!marked.is_empty()).then_some(Self { marked, last: None })
    }

    /// Drops the marks of messages that are gone or no longer eligible (deleted, replaced). Returns
    /// whether anything was dropped.
    pub fn prune(&mut self, messages: &[Message]) -> bool {
        let before = self.marked.len();
        self.marked
            .retain(|ts| index_of(messages, *ts).is_some_and(|ix| eligible(&messages[ix])));
        if self
            .last
            .is_some_and(|last| !index_of(messages, last).is_some_and(|ix| eligible(&messages[ix])))
        {
            self.last = None;
        }
        self.marked.len() != before
    }

    /// The marked messages of `messages` in timeline order.
    pub fn messages<'a>(&self, messages: &'a [Message]) -> Vec<&'a Message> {
        self.marked
            .iter()
            .filter_map(|ts| index_of(messages, *ts).map(|ix| &messages[ix]))
            .collect()
    }
}

/// What the timeline has selected: nothing, a range of text (D1) or whole messages (D11).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Pick {
    #[default]
    None,
    Text(TextSelection),
    Messages(MessageSet),
}

/// The text a copy of the marked messages puts on the clipboard (D7): per message with copyable text
/// (`copy_text`, so a file gives its name) a block `header(m)\n{text}`, blocks separated by one blank
/// line, in timeline order; one block is the bare text. Messages without copyable text contribute
/// nothing. Line endings are `\n`.
pub fn extract_messages(
    messages: &[Message],
    set: &MessageSet,
    header: impl Fn(&Message) -> String,
) -> String {
    let parts: Vec<(&Message, &str)> = set
        .messages(messages)
        .into_iter()
        .filter_map(|m| Some((m, copy_text(m).filter(|text| !text.is_empty())?)))
        .collect();
    let text = match parts.as_slice() {
        [] => String::new(),
        [(_, text)] => (*text).to_owned(),
        _ => parts
            .iter()
            .map(|(m, text)| format!("{}\n{text}", header(m)))
            .collect::<Vec<_>>()
            .join("\n\n"),
    };
    if text.contains("\r\n") {
        text.replace("\r\n", "\n")
    } else {
        text
    }
}

/// Messages built by hand, for the tests of this module and of the state machine over it.
#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;
    use bt_chat_core::model::{ChatId, Delivery, MediaKind, MediaRef, UserId};

    pub fn message(ts: MessageTs, body: MessageBody) -> Message {
        Message {
            chat: ChatId("g".into()),
            timestamp: ts,
            author: UserId("alice".into()),
            author_name: Some("Алиса".into()),
            author_avatar: None,
            outgoing: false,
            body,
            edited: false,
            reply_to: None,
            forwarded_from: None,
            delivery: Delivery::Sent,
            reactions: Default::default(),
            important: false,
        }
    }

    pub fn text(ts: MessageTs, text: &str) -> Message {
        message(ts, MessageBody::Text(text.into()))
    }

    pub fn voice(ts: MessageTs) -> Message {
        message(
            ts,
            MessageBody::Voice {
                media: MediaRef {
                    id: "v".into(),
                    kind: MediaKind::File,
                },
                duration_ms: Some(1000),
                size: 10,
                name: None,
                waveform: None,
            },
        )
    }

    pub fn image(ts: MessageTs, caption: Option<&str>) -> Message {
        message(
            ts,
            MessageBody::Image {
                media: MediaRef {
                    id: "i".into(),
                    kind: MediaKind::Thumbnail,
                },
                width: 10,
                height: 10,
                caption: caption.map(Into::into),
                name: None,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::{image, message, text, voice};
    use super::*;

    fn pos(ts: MessageTs, offset: usize) -> Pos {
        Pos { ts, offset }
    }

    fn select(messages: &[Message], anchor: Pos, focus: Pos) -> TextSelection {
        TextSelection::new(messages, anchor, focus, Unit::Char).expect("endpoints on text")
    }

    fn header(m: &Message) -> String {
        format!("H{}", m.timestamp)
    }

    #[test]
    fn selectable_text_is_the_text_or_a_caption() {
        assert_eq!(selectable_text(&text(1, "hi")), Some("hi"));
        assert_eq!(selectable_text(&image(1, Some("подпись"))), Some("подпись"));
        assert_eq!(selectable_text(&image(1, None)), None);
        assert_eq!(selectable_text(&voice(1)), None);
        assert_eq!(
            selectable_text(&message(1, MessageBody::Deleted)),
            None,
            "a deleted placeholder"
        );
    }

    #[test]
    fn word_at_the_start_the_middle_and_the_end() {
        let t = "hello world";
        assert_eq!(word_at(t, 0), 0..5);
        assert_eq!(word_at(t, 2), 0..5);
        assert_eq!(word_at(t, 6), 6..11);
        assert_eq!(word_at(t, 8), 6..11);
        assert_eq!(
            word_at(t, 11),
            6..11,
            "the end of the text is the last word"
        );
        assert_eq!(word_at(t, 99), 6..11, "past the end is clamped");
    }

    #[test]
    fn word_at_a_space_and_at_punctuation() {
        assert_eq!(word_at("hello world", 5), 5..6, "the space after a word");
        assert_eq!(word_at("a   b", 2), 1..4, "a run of spaces is one word");
        assert_eq!(word_at("hi, there", 2), 2..3, "the comma");
        assert_eq!(
            word_at("wow!!! ok", 4),
            3..6,
            "a run of punctuation is one word"
        );
        assert_eq!(word_at("what?!", 4), 4..6);
        assert_eq!(
            word_at("can't stop", 2),
            0..5,
            "an apostrophe stays inside the word"
        );
        assert_eq!(word_at("a\nb", 1), 1..2, "a line break is its own run");
        assert_eq!(
            word_at("a  \n  b", 3),
            3..4,
            "spaces do not merge across a line break"
        );
    }

    #[test]
    fn word_at_in_cyrillic() {
        let t = "привет, мир";
        assert_eq!(word_at(t, 0), 0..12);
        assert_eq!(word_at(t, 4), 0..12);
        assert_eq!(
            word_at(t, 5),
            0..12,
            "inside a letter: floored to its start"
        );
        assert_eq!(word_at(t, 12), 12..13, "the comma");
        assert_eq!(word_at(t, 15), 14..20);
    }

    #[test]
    fn word_at_with_an_emoji_and_a_combining_mark() {
        let t = "hi 😀 there";
        assert_eq!(word_at(t, 3), 3..7);
        assert_eq!(word_at(t, 5), 3..7, "inside the emoji");
        assert_eq!(word_at(t, 7), 7..8, "the space after it");
        let family = "a 👨‍👩‍👧 b";
        let end = family.len() - 2;
        assert_eq!(word_at(family, 5), 2..end, "a joined sequence is one run");
        let accent = "cafe\u{301} ok";
        assert_eq!(word_at(accent, 4), 0..6, "the mark belongs to its letter");
        assert_eq!(word_at(accent, 5), 0..6, "between the letter and the mark");
    }

    #[test]
    fn word_at_an_empty_text() {
        assert_eq!(word_at("", 0), 0..0);
        assert_eq!(word_at("", 5), 0..0);
    }

    #[test]
    fn paragraph_at_lines() {
        let t = "one\ntwo\n\nfour";
        assert_eq!(paragraph_at(t, 0), 0..3);
        assert_eq!(paragraph_at(t, 2), 0..3);
        assert_eq!(paragraph_at(t, 3), 0..3, "the end of a line is that line");
        assert_eq!(paragraph_at(t, 4), 4..7, "the start of a line is that line");
        assert_eq!(paragraph_at(t, 8), 8..8, "an empty line");
        assert_eq!(paragraph_at(t, 9), 9..13);
        assert_eq!(paragraph_at(t, 13), 9..13);
        assert_eq!(paragraph_at("", 0), 0..0);
        assert_eq!(paragraph_at("single", 3), 0..6);
        assert_eq!(
            paragraph_at("a\r\nb", 0),
            0..1,
            "CRLF does not leave the CR"
        );
        assert_eq!(paragraph_at("привет\nмир", 13), 13..19);
    }

    #[test]
    fn snap_goes_to_the_nearest_grapheme_boundary() {
        assert_eq!(snap("hello", 2), 2);
        assert_eq!(snap("hello", 9), 5);
        assert_eq!(snap("", 0), 0);
        // `e` + a combining acute is one grapheme of three bytes: 0..3.
        let accent = "e\u{301}x";
        assert_eq!(
            snap(accent, 1),
            0,
            "just after the letter is nearer its start"
        );
        assert_eq!(
            snap(accent, 2),
            3,
            "after the mark's first byte is nearer the end"
        );
        // A four-byte emoji: the middle is a tie and goes to the earlier boundary.
        assert_eq!(snap("😀", 2), 0);
        assert_eq!(snap("😀", 3), 4);
        // A joined family is one grapheme.
        let family = "👨‍👩‍👧x";
        let x = family.len() - 1;
        assert_eq!(snap(family, 5), 0);
        assert_eq!(snap(family, x - 1), x);
    }

    fn five() -> Vec<Message> {
        vec![
            text(1, "first"),
            text(2, "second"),
            voice(3),
            text(4, "fourth"),
            text(5, "fifth"),
        ]
    }

    #[test]
    fn range_in_for_the_first_middle_last_and_outside_messages() {
        let m = five();
        let sel = select(&m, pos(2, 3), pos(4, 2));
        assert_eq!(sel.range_in(&m, 1), None, "before the selection");
        assert_eq!(sel.range_in(&m, 2), Some(3..6), "the tail from the start");
        assert_eq!(sel.range_in(&m, 3), None, "a voice message has no text");
        assert_eq!(sel.range_in(&m, 4), Some(0..2), "the head up to the end");
        assert_eq!(sel.range_in(&m, 5), None, "after the selection");
        assert_eq!(sel.range_in(&m, 99), None, "not a loaded message");
        let wide = select(&m, pos(1, 2), pos(5, 1));
        assert_eq!(
            wide.range_in(&m, 2),
            Some(0..6),
            "strictly between: all of it"
        );
        assert_eq!(wide.range_in(&m, 4), Some(0..6));
    }

    #[test]
    fn range_in_with_the_anchor_after_the_focus() {
        let m = five();
        let forward = select(&m, pos(2, 3), pos(4, 2));
        let backward = select(&m, pos(4, 2), pos(2, 3));
        for ts in 1..=5 {
            assert_eq!(forward.range_in(&m, ts), backward.range_in(&m, ts), "{ts}");
        }
        let one = select(&m, pos(2, 5), pos(2, 1));
        assert_eq!(one.range_in(&m, 2), Some(1..5));
    }

    #[test]
    fn range_in_with_equal_ends_is_nothing() {
        let m = five();
        let caret = select(&m, pos(2, 3), pos(2, 3));
        assert_eq!(caret.range_in(&m, 2), None);
        assert!(caret.is_empty(&m));
    }

    #[test]
    fn is_empty_looks_at_the_text_between_the_ends() {
        let m = five();
        assert!(!select(&m, pos(2, 0), pos(2, 1)).is_empty(&m));
        assert!(!select(&m, pos(1, 5), pos(2, 1)).is_empty(&m));
        // From the end of one message to the start of the next, with a voice message between: nothing.
        assert!(select(&m, pos(2, 6), pos(4, 0)).is_empty(&m));
        // With a text between them, something.
        assert!(!select(&m, pos(1, 5), pos(5, 0)).is_empty(&m));
    }

    #[test]
    fn single_is_the_message_of_a_one_message_selection() {
        let m = five();
        assert_eq!(select(&m, pos(2, 1), pos(2, 4)).single(), Some(2));
        assert_eq!(select(&m, pos(2, 4), pos(2, 1)).single(), Some(2));
        assert_eq!(select(&m, pos(2, 1), pos(4, 1)).single(), None);
    }

    #[test]
    fn a_selection_needs_text_at_both_ends_on_char_boundaries() {
        let m = vec![text(1, "привет"), voice(2)];
        assert!(TextSelection::new(&m, pos(1, 0), pos(1, 12), Unit::Char).is_some());
        assert!(TextSelection::new(&m, pos(1, 0), pos(2, 0), Unit::Char).is_none());
        assert!(TextSelection::new(&m, pos(1, 0), pos(9, 0), Unit::Char).is_none());
        assert!(TextSelection::new(&m, pos(1, 0), pos(1, 13), Unit::Char).is_none());
        assert!(TextSelection::new(&m, pos(1, 0), pos(1, 5), Unit::Char).is_none());
    }

    #[test]
    fn the_snapshot_holds_one_text_per_endpoint_message() {
        let m = five();
        let inside = select(&m, pos(2, 1), pos(2, 3));
        assert_eq!(inside.source, vec![(2, Arc::from("second"))]);
        let across = select(&m, pos(4, 1), pos(1, 3));
        assert_eq!(
            across.source,
            vec![(4, Arc::from("fourth")), (1, Arc::from("first"))],
            "the anchor's and the focus's"
        );
    }

    #[test]
    fn validate_keeps_a_selection_whose_messages_only_moved_or_whose_others_changed() {
        let m = five();
        let sel = select(&m, pos(2, 1), pos(4, 3));
        assert!(sel.validate(&m));

        // Older history loads above: the indexes shift, the timestamps do not.
        let mut older = vec![text(-2, "old"), text(-1, "older")];
        older.extend(m.clone());
        assert!(sel.validate(&older));

        // A message after the end arrives; a message between the ends is edited or removed.
        let mut changed = m.clone();
        changed.push(text(6, "new"));
        changed.remove(2);
        changed[0] = text(1, "edited first");
        assert!(sel.validate(&changed));
    }

    #[test]
    fn validate_drops_a_selection_whose_endpoint_message_is_gone() {
        let m = five();
        let sel = select(&m, pos(2, 1), pos(4, 3));
        let mut without_anchor = m.clone();
        without_anchor.remove(1);
        assert!(!sel.validate(&without_anchor));
        let mut without_focus = m.clone();
        without_focus.remove(3);
        assert!(!sel.validate(&without_focus));
        assert!(!sel.validate(&[]));
    }

    #[test]
    fn validate_drops_a_selection_whose_text_shrank_or_lost_its_text() {
        let m = five();
        let sel = select(&m, pos(2, 1), pos(4, 3));
        let mut shorter = m.clone();
        shorter[1] = text(2, "sec");
        assert!(!sel.validate(&shorter));
        let mut deleted = m.clone();
        deleted[3] = message(4, MessageBody::Deleted);
        assert!(!sel.validate(&deleted));
    }

    #[test]
    fn validate_drops_a_selection_whose_text_was_edited_to_the_same_length() {
        let m = vec![text(1, "hello world")];
        let sel = select(&m, pos(1, 6), pos(1, 11));
        assert!(sel.validate(&m));
        let edited = vec![text(1, "hello there")];
        assert!(
            sel.range_in(&edited, 1).is_some(),
            "the offsets alone are still in range"
        );
        assert!(!sel.validate(&edited), "the snapshot differs");
        assert!(
            extract_text(&edited, &sel, header).is_empty(),
            "nothing is copied from a stale selection"
        );
    }

    #[test]
    fn validate_drops_an_offset_inside_a_character() {
        let m = vec![text(1, "привет")];
        let mut sel = select(&m, pos(1, 0), pos(1, 12));
        sel.focus.offset = 5;
        assert!(!sel.validate(&m));
        sel.focus.offset = 99;
        assert!(!sel.validate(&m));
    }

    #[test]
    fn extract_inside_one_message_is_the_bare_text() {
        let m = five();
        assert_eq!(
            extract_text(&m, &select(&m, pos(2, 1), pos(2, 4)), header),
            "eco"
        );
        assert_eq!(
            extract_text(&m, &select(&m, pos(2, 4), pos(2, 1)), header),
            "eco",
            "backwards"
        );
        assert_eq!(
            extract_text(&m, &select(&m, pos(2, 3), pos(2, 3)), header),
            ""
        );
    }

    #[test]
    fn extract_across_messages_has_a_header_per_message_and_a_blank_line_between() {
        let m = five();
        let sel = select(&m, pos(2, 3), pos(5, 2));
        assert_eq!(
            extract_text(&m, &sel, header),
            "H2\nond\n\nH4\nfourth\n\nH5\nfi",
            "the voice message between is skipped"
        );
    }

    #[test]
    fn extract_across_three_texts_with_a_voice_between_cuts_the_ends() {
        let m = vec![
            text(1, "hello world"),
            voice(2),
            text(3, "middle"),
            text(4, "tail end"),
        ];
        let sel = select(&m, pos(1, 6), pos(4, 4));
        assert_eq!(
            extract_text(&m, &sel, header),
            "H1\nworld\n\nH3\nmiddle\n\nH4\ntail"
        );
        let backwards = select(&m, pos(4, 4), pos(1, 6));
        assert_eq!(
            extract_text(&m, &backwards, header),
            extract_text(&m, &sel, header)
        );
    }

    #[test]
    fn extract_skips_an_end_message_with_nothing_selected_in_it() {
        let m = vec![text(1, "hello"), text(2, "world")];
        // From the very end of the first message: only the second contributes, still with a header.
        let sel = select(&m, pos(1, 5), pos(2, 3));
        assert_eq!(extract_text(&m, &sel, header), "H2\nwor");
    }

    #[test]
    fn extract_includes_the_caption_of_a_picture() {
        let m = vec![
            text(1, "one"),
            image(2, Some("caption")),
            image(3, None),
            text(4, "two"),
        ];
        let sel = select(&m, pos(1, 0), pos(4, 3));
        assert_eq!(
            extract_text(&m, &sel, header),
            "H1\none\n\nH2\ncaption\n\nH4\ntwo"
        );
    }

    #[test]
    fn extract_uses_unix_line_endings() {
        let m = vec![text(1, "a\r\nb")];
        assert_eq!(
            extract_text(&m, &select(&m, pos(1, 0), pos(1, 4)), header),
            "a\nb"
        );
    }

    #[test]
    fn select_all_covers_the_first_to_the_last_text() {
        let m = vec![
            voice(1),
            text(2, "first"),
            voice(3),
            text(4, "last"),
            voice(5),
        ];
        let sel = select_all(&m).expect("there is text");
        assert_eq!((sel.anchor, sel.focus), (pos(2, 0), pos(4, 4)));
        assert_eq!(extract_text(&m, &sel, header), "H2\nfirst\n\nH4\nlast");
        let one = vec![voice(1), text(2, "only")];
        let sel = select_all(&one).unwrap();
        assert_eq!(sel.single(), Some(2));
        assert_eq!(extract_text(&one, &sel, header), "only");
    }

    #[test]
    fn select_all_without_text_is_none() {
        assert!(select_all(&[]).is_none());
        let m = vec![voice(1), image(2, None), message(3, MessageBody::Deleted)];
        assert!(select_all(&m).is_none());
        let empty = vec![text(1, ""), image(2, Some(""))];
        assert!(
            select_all(&empty).is_none(),
            "empty text is nothing to select"
        );
    }

    // ---- message mode (D11) ----

    use bt_chat_core::model::{MediaKind, MediaRef, PayloadId};

    fn own(mut m: Message) -> Message {
        m.outgoing = true;
        m
    }

    fn pending(mut m: Message) -> Message {
        m.delivery = Delivery::Pending(PayloadId("p".into()));
        m
    }

    fn failed(mut m: Message) -> Message {
        m.delivery = Delivery::Failed {
            payload: PayloadId("p".into()),
            reason: "no".into(),
        };
        m
    }

    fn deleted(ts: MessageTs) -> Message {
        message(ts, MessageBody::Deleted)
    }

    fn file(ts: MessageTs, name: &str) -> Message {
        message(
            ts,
            MessageBody::File {
                media: MediaRef {
                    id: "f".into(),
                    kind: MediaKind::File,
                },
                name: name.into(),
                size: 10,
            },
        )
    }

    fn marked(set: &MessageSet) -> Vec<MessageTs> {
        set.marked.clone()
    }

    #[test]
    fn eligible_is_sent_and_not_deleted() {
        assert!(eligible(&text(1, "a")));
        assert!(eligible(&voice(1)));
        assert!(!eligible(&pending(text(1, "a"))));
        assert!(!eligible(&failed(text(1, "a"))));
        assert!(!eligible(&deleted(1)));
    }

    #[test]
    fn a_set_starts_only_on_an_eligible_loaded_message() {
        let m = vec![text(1, "a"), pending(text(2, "b")), deleted(3)];
        assert_eq!(marked(&MessageSet::new(&m, 1).unwrap()), vec![1]);
        assert!(MessageSet::new(&m, 2).is_none());
        assert!(MessageSet::new(&m, 3).is_none());
        assert!(MessageSet::new(&m, 9).is_none(), "not loaded");
    }

    #[test]
    fn toggle_marks_and_unmarks_and_keeps_the_order() {
        let m = vec![text(1, "a"), text(2, "b"), text(3, "c")];
        let mut set = MessageSet::new(&m, 3).unwrap();
        assert!(set.toggle(&m, 1));
        assert_eq!(marked(&set), vec![1, 3]);
        assert!(set.contains(1) && !set.contains(2));
        assert!(set.toggle(&m, 3));
        assert_eq!(marked(&set), vec![1], "off again");
        assert!(set.toggle(&m, 1));
        assert!(set.is_empty(), "the last one off leaves the set empty");
    }

    #[test]
    fn toggle_refuses_an_ineligible_or_unknown_message() {
        let m = vec![text(1, "a"), failed(text(2, "b")), deleted(3)];
        let mut set = MessageSet::new(&m, 1).unwrap();
        for ts in [2, 3, 9] {
            assert!(!set.toggle(&m, ts));
        }
        assert_eq!(marked(&set), vec![1]);
    }

    #[test]
    fn a_range_marks_the_eligible_messages_over_a_gap_of_ineligible_ones() {
        let m = vec![
            text(1, "a"),
            pending(text(2, "b")),
            deleted(3),
            text(4, "c"),
            text(5, "d"),
            text(6, "e"),
        ];
        let mut set = MessageSet::new(&m, 1).unwrap();
        assert!(set.extend_to(&m, 4));
        assert_eq!(
            marked(&set),
            vec![1, 4],
            "the pending and the deleted are skipped"
        );
        assert!(set.extend_to(&m, 6), "ranges on from the clicked one");
        assert_eq!(marked(&set), vec![1, 4, 5, 6]);
        assert!(
            !set.extend_to(&m, 2),
            "an ineligible target changes nothing"
        );
    }

    #[test]
    fn a_range_runs_in_either_direction() {
        let m = vec![text(1, "a"), text(2, "b"), text(3, "c"), text(4, "d")];
        let mut set = MessageSet::new(&m, 4).unwrap();
        assert!(set.extend_to(&m, 2));
        assert_eq!(
            marked(&set),
            vec![2, 3, 4],
            "backwards from the last toggled"
        );
        let mut forward = MessageSet::new(&m, 1).unwrap();
        assert!(forward.extend_to(&m, 3));
        assert_eq!(marked(&forward), vec![1, 2, 3]);
        assert!(!forward.extend_to(&m, 3), "nothing new to mark");
    }

    #[test]
    fn a_range_without_a_loaded_start_marks_the_clicked_message_alone() {
        let m = vec![text(1, "a"), text(2, "b"), text(3, "c")];
        let mut set = MessageSet::new(&m, 1).unwrap();
        let fewer = vec![text(2, "b"), text(3, "c")];
        assert!(set.extend_to(&fewer, 3), "message 1 is no longer loaded");
        assert_eq!(marked(&set), vec![1, 3]);
        let mut all = MessageSet::select_all(&m).unwrap();
        assert!(
            !all.extend_to(&m, 2),
            "no earlier toggle: the message is already marked"
        );
    }

    #[test]
    fn select_all_skips_the_pending_failed_and_deleted() {
        let m = vec![
            text(1, "a"),
            pending(text(2, "b")),
            failed(text(3, "c")),
            deleted(4),
            voice(5),
        ];
        assert_eq!(marked(&MessageSet::select_all(&m).unwrap()), vec![1, 5]);
        assert!(MessageSet::select_all(&[pending(text(1, "a")), deleted(2)]).is_none());
        assert!(MessageSet::select_all(&[]).is_none());
    }

    #[test]
    fn prune_drops_the_marks_of_messages_that_are_gone_or_deleted() {
        let m = vec![text(1, "a"), text(2, "b"), text(3, "c")];
        let mut set = MessageSet::select_all(&m).unwrap();
        assert!(!set.prune(&m));
        let after = vec![text(1, "a"), deleted(2)];
        assert!(set.prune(&after));
        assert_eq!(marked(&set), vec![1]);
    }

    #[test]
    fn the_anchor_survives_an_uncheck_and_a_prune_but_not_its_message() {
        let m = vec![text(1, "a"), text(2, "b"), text(3, "c"), text(4, "d")];
        let mut set = MessageSet::new(&m, 1).unwrap();
        set.toggle(&m, 2);
        set.toggle(&m, 2);
        assert!(!set.prune(&m), "nothing to drop: 2 is unchecked, not gone");
        assert!(set.extend_to(&m, 4));
        assert_eq!(marked(&set), vec![1, 2, 3, 4], "from the unchecked 2");

        let mut set = MessageSet::new(&m, 1).unwrap();
        set.toggle(&m, 3);
        let after = vec![text(1, "a"), text(2, "b"), deleted(3), text(4, "d")];
        assert!(set.prune(&after));
        assert!(set.extend_to(&after, 4));
        assert_eq!(marked(&set), vec![1, 4], "a deleted anchor is gone");
    }

    #[test]
    fn all_deletable_needs_every_message_own_and_eligible() {
        let a = own(text(1, "a"));
        let b = own(voice(2));
        let foreign = text(3, "c");
        assert!(all_deletable(&[&a, &b]));
        assert!(!all_deletable(&[&a, &foreign]), "one foreign message");
        assert!(!all_deletable(&[&own(pending(text(4, "d")))]));
        assert!(!all_deletable(&[]));
    }

    #[test]
    fn extract_messages_of_one_message_is_its_bare_text() {
        let m = vec![text(1, "hello"), text(2, "other")];
        let set = MessageSet::new(&m, 1).unwrap();
        assert_eq!(extract_messages(&m, &set, header), "hello");
    }

    #[test]
    fn extract_messages_of_several_has_a_header_per_block_in_timeline_order() {
        let m = vec![text(1, "first"), text(2, "skip"), text(3, "third\r\nline")];
        let mut set = MessageSet::new(&m, 3).unwrap();
        set.toggle(&m, 1);
        assert_eq!(
            extract_messages(&m, &set, header),
            "H1\nfirst\n\nH3\nthird\nline"
        );
    }

    #[test]
    fn extract_messages_skips_a_voice_and_takes_a_files_name() {
        let m = vec![text(1, "hello"), voice(2), file(3, "report.pdf")];
        let set = MessageSet::select_all(&m).unwrap();
        assert_eq!(
            extract_messages(&m, &set, header),
            "H1\nhello\n\nH3\nreport.pdf"
        );
        let only_voice = MessageSet::new(&m, 2).unwrap();
        assert_eq!(extract_messages(&m, &only_voice, header), "");
        let only_file = MessageSet::new(&m, 3).unwrap();
        assert_eq!(extract_messages(&m, &only_file, header), "report.pdf");
        let with_voice = {
            let mut s = MessageSet::new(&m, 1).unwrap();
            s.toggle(&m, 2);
            s
        };
        assert_eq!(
            extract_messages(&m, &with_voice, header),
            "hello",
            "one message with text is the bare text, the voice contributes nothing"
        );
    }
}
