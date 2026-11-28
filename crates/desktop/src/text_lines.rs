//! The visual lines of a shaped text, as far as selecting it needs them: where a click puts the caret and
//! which rectangles a selected range covers (D3, D5 of `add-text-selection-and-quotes`, spike 0.1).
//!
//! The rules are pure functions over `Para`, a plain model of one paragraph (a `\n`-separated line of the
//! text) with its glyph starts and wrap points, so they are tested without a window. `paragraphs_of` fills
//! the model from gpui's `TextLayout`, which is the only gpui-dependent part.
//!
//! Left-to-right text only: the glyph indices of a right-to-left run descend with x, and partial ranges and
//! carets of such a run are wrong (a whole-text selection is right).

use crate::selection::snap;
use gpui::{Point, TextLayout};
use std::ops::Range;
use unicode_segmentation::GraphemeCursor;

/// One paragraph of a shaped text. Offsets inside are relative to the paragraph, `base` locates it in the text.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Para {
    /// Byte offset of the paragraph in the whole text.
    pub(crate) base: usize,
    pub(crate) len: usize,
    /// Glyph starts in logical order: the byte offset of the glyph and its x on the unwrapped line.
    pub(crate) glyphs: Vec<(usize, f32)>,
    /// The offset where each wrapped line after the first starts.
    pub(crate) wraps: Vec<usize>,
    /// The width of the unwrapped line.
    pub(crate) width: f32,
}

impl Para {
    /// The x of the first glyph at or after `index`, or the line's end (gpui's `LineLayout::x_for_index`).
    fn x_for(&self, index: usize) -> f32 {
        self.glyphs
            .iter()
            .find(|(i, _)| *i >= index)
            .map_or(self.width, |(_, x)| *x)
    }

    /// The visual line holding `index`: a wrap point belongs to the line that ends there, as gpui's
    /// `position_for_index` resolves it.
    fn line_of(&self, index: usize) -> usize {
        self.wraps.iter().take_while(|w| **w < index).count()
    }

    fn line_start(&self, line: usize) -> usize {
        line.checked_sub(1).map_or(0, |j| self.wraps[j])
    }

    /// The x inside its visual line and the line of `index` (gpui's `position_for_index`).
    fn position(&self, index: usize) -> (f32, usize) {
        let line = self.line_of(index);
        (self.x_for(index) - self.x_for(self.line_start(line)), line)
    }

    fn lines(&self) -> usize {
        self.wraps.len() + 1
    }
}

fn is_boundary(text: &str, offset: usize) -> bool {
    GraphemeCursor::new(offset, text.len(), true)
        .is_boundary(text, 0)
        .unwrap_or(true)
}

/// The byte offset of the caret nearest to the pointer, for a hit test `hit` (gpui's `index_for_position`)
/// and the pointer's `x` relative to the text's left edge.
///
/// `Err` (outside the text, or at a wrap point) is already a line start or end. For `Ok(i)`, `i` is the start
/// of the glyph under the pointer, so the caret is the nearer of the stops around it, by the middle of the
/// gap: a stop is a glyph start that is also a grapheme boundary (so never inside a ligature), the start of
/// the paragraph and its end. The first glyph of a wrapped line measures from the line's left edge, because
/// `position_for_index` of a wrap point is the end of the line above. Each paragraph carries its own base,
/// so the offsets of a second paragraph never collide with those of the first, and the end of one paragraph
/// and the start of the next stay distinct stops.
pub(crate) fn caret_in(text: &str, paras: &[Para], hit: Result<usize, usize>, x: f32) -> usize {
    let index = match hit {
        Err(index) => return snap(text, index),
        Ok(index) => index,
    };
    let Some(para) = paras
        .iter()
        .find(|p| p.base <= index && index <= p.base + p.len)
    else {
        return snap(text, index);
    };
    let local = index - para.base;
    let mut stops: Vec<usize> = para
        .glyphs
        .iter()
        .map(|(i, _)| *i)
        .filter(|i| *i < para.len && is_boundary(text, para.base + i))
        .chain([0, para.len])
        .collect();
    stops.sort_unstable();
    stops.dedup();
    let Some(&start) = stops.iter().rfind(|s| **s <= local) else {
        return snap(text, index);
    };
    let Some(&next) = stops.iter().find(|s| **s > local) else {
        return para.base + start;
    };
    let (x0, line0) = para.position(start);
    let (x1, line1) = para.position(next);
    let from = if line1 > line0 { 0. } else { x0 };
    para.base + if x >= (from + x1) / 2. { next } else { start }
}

/// The rectangles a selected `range` of the text covers, as `(visual line, left, right)` with `left` and
/// `right` measured from the text's left edge; the line counts from the top of the whole text. The paragraph
/// break itself and empty paragraphs are not painted (as in gpui's own highlight).
pub(crate) fn highlight_rects(paras: &[Para], range: Range<usize>) -> Vec<(usize, f32, f32)> {
    let mut rects = Vec::new();
    let mut first_line = 0;
    for para in paras {
        let lo = range.start.max(para.base);
        let hi = range.end.min(para.base + para.len);
        if lo < hi {
            let (lo, hi) = (lo - para.base, hi - para.base);
            for line in 0..para.lines() {
                let start = para.line_start(line);
                let last = line + 1 == para.lines();
                let end = if last { para.len } else { para.wraps[line] };
                let (from, to) = (lo.max(start), hi.min(end));
                if from >= to {
                    continue;
                }
                let origin = para.x_for(start);
                let left = para.x_for(from) - origin;
                let right = if last && to == end {
                    para.width - origin
                } else {
                    para.x_for(to) - origin
                };
                rects.push((first_line + line, left, right));
            }
        }
        first_line += para.lines();
    }
    rects
}

/// The model of every paragraph of `text`, read from a layout that was measured and prepainted.
pub(crate) fn paragraphs_of(layout: &TextLayout, text: &str) -> Vec<Para> {
    let mut base = 0;
    text.split('\n')
        .map(|line| {
            let mut para = Para {
                base,
                len: line.len(),
                glyphs: Vec::new(),
                wraps: Vec::new(),
                width: 0.,
            };
            if let Some(shaped) = layout.line_layout_for_index(base) {
                let runs = &shaped.unwrapped_layout.runs;
                para.glyphs = runs
                    .iter()
                    .flat_map(|run| run.glyphs.iter())
                    .map(|glyph| (glyph.index, f32::from(glyph.position.x)))
                    .collect();
                para.wraps = shaped
                    .wrap_boundaries
                    .iter()
                    .map(|b| runs[b.run_ix].glyphs[b.glyph_ix].index)
                    .collect();
                para.width = f32::from(shaped.unwrapped_layout.width);
            }
            base += line.len() + 1;
            para
        })
        .collect()
}

/// The caret offset nearest to `point` in a painted `layout` of `text`. The layout must have been measured
/// and prepainted, which the registry of painted texts guarantees (gpui panics otherwise).
pub(crate) fn caret_for(layout: &TextLayout, point: Point<gpui::Pixels>, text: &str) -> usize {
    let hit = layout.index_for_position(point);
    if let Err(index) = hit {
        return snap(text, index);
    }
    let x = f32::from(point.x - layout.bounds().left());
    caret_in(text, &paragraphs_of(layout, text), hit, x)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A paragraph of `text` shaped at `cell` px a char, one glyph per char (the test platform's shaping).
    fn para(base: usize, text: &str, cell: f32, wraps: &[usize]) -> Para {
        Para {
            base,
            len: text.len(),
            glyphs: text
                .char_indices()
                .map(|(i, _)| (i, i as f32 * cell))
                .collect(),
            wraps: wraps.to_vec(),
            width: text.len() as f32 * cell,
        }
    }

    fn caret(text: &str, paras: &[Para], hit: usize, x: f32) -> usize {
        caret_in(text, paras, Ok(hit), x)
    }

    #[test]
    fn the_caret_is_the_nearer_stop_by_the_middle_of_a_glyph() {
        let text = "abc";
        let p = [para(0, text, 9., &[])];
        assert_eq!(caret(text, &p, 0, 4.), 0);
        assert_eq!(
            caret(text, &p, 0, 4.5),
            1,
            "the middle goes to the next stop"
        );
        assert_eq!(caret(text, &p, 1, 13.), 1);
        assert_eq!(caret(text, &p, 1, 14.), 2);
    }

    #[test]
    fn the_right_half_of_the_last_glyph_of_a_line_puts_the_caret_after_it() {
        let text = "abc";
        let p = [para(0, text, 9., &[])];
        assert_eq!(caret(text, &p, 2, 22.), 2);
        assert_eq!(caret(text, &p, 2, 23.), 3);
    }

    #[test]
    fn outside_the_text_the_hit_is_already_a_caret() {
        let text = "abc";
        let p = [para(0, text, 9., &[])];
        assert_eq!(caret_in(text, &p, Err(0), -5.), 0);
        assert_eq!(caret_in(text, &p, Err(3), 500.), 3);
        assert_eq!(caret_in(text, &p, Err(9), 500.), 3, "clamped to the text");
    }

    #[test]
    fn the_first_glyph_of_a_wrapped_line_switches_at_its_own_middle() {
        // "abcd" wrapped before "c": the lines are "ab" and "cd".
        let text = "abcd";
        let p = [para(0, text, 9., &[2])];
        // Measured from the line's left edge: the middle of "c" is 4.5, not 9 as the end of line one would give.
        assert_eq!(caret(text, &p, 2, 4.), 2);
        assert_eq!(caret(text, &p, 2, 5.), 3);
        // The last glyph of the first line: its right half ends at the wrap point.
        assert_eq!(caret(text, &p, 1, 12.), 1);
        assert_eq!(caret(text, &p, 1, 14.), 2);
        // The last glyph of the last line.
        assert_eq!(caret(text, &p, 3, 14.), 4);
        assert_eq!(caret(text, &p, 3, 12.), 3);
    }

    #[test]
    fn a_ligature_has_no_caret_inside_it() {
        // "ab" shaped as one glyph 18 px wide: offset 1 is a grapheme boundary but not a stop.
        let text = "abc";
        let p = [Para {
            base: 0,
            len: 3,
            glyphs: vec![(0, 0.), (2, 18.)],
            wraps: vec![],
            width: 27.,
        }];
        assert_eq!(caret(text, &p, 0, 8.), 0);
        assert_eq!(caret(text, &p, 0, 10.), 2);
        assert_eq!(caret(text, &p, 0, 17.), 2, "never 1");
    }

    #[test]
    fn a_stop_inside_a_grapheme_is_skipped() {
        // "e" and a combining acute are two glyphs here but one grapheme: offset 1 is no stop.
        let text = "e\u{301}x";
        let p = [Para {
            base: 0,
            len: text.len(),
            glyphs: vec![(0, 0.), (1, 9.), (3, 18.)],
            wraps: vec![],
            width: 27.,
        }];
        assert_eq!(caret(text, &p, 1, 10.), 3);
        assert_eq!(caret(text, &p, 0, 2.), 0);
    }

    #[test]
    fn the_second_paragraph_carries_its_base() {
        // "a\nbc": paragraph one is 0..1, paragraph two starts at 2.
        let text = "a\nbc";
        let p = [para(0, "a", 9., &[]), para(2, "bc", 9., &[])];
        assert_eq!(
            caret(text, &p, 2, 3.),
            2,
            "the start of the second paragraph"
        );
        assert_eq!(caret(text, &p, 2, 5.), 3);
        assert_eq!(caret(text, &p, 3, 12.), 3);
        assert_eq!(caret(text, &p, 3, 14.), 4);
        // The end of paragraph one is its own stop, distinct from the start of paragraph two.
        assert_eq!(caret(text, &p, 0, 3.), 0);
        assert_eq!(caret(text, &p, 0, 6.), 1);
        assert_eq!(
            caret(text, &p, 1, 6.),
            1,
            "an index at the end belongs to its paragraph"
        );
    }

    #[test]
    fn the_first_glyph_after_a_line_break_and_an_empty_paragraph() {
        let text = "ab\n\ncd";
        let p = [
            para(0, "ab", 9., &[]),
            para(3, "", 9., &[]),
            para(4, "cd", 9., &[]),
        ];
        assert_eq!(caret(text, &p, 4, 4.), 4);
        assert_eq!(caret(text, &p, 4, 5.), 5);
        assert_eq!(caret(text, &p, 3, 100.), 3, "the empty line has one stop");
    }

    #[test]
    fn a_wrapped_second_paragraph_measures_from_its_own_lines() {
        let text = "x\nabcd";
        let p = [para(0, "x", 9., &[]), para(2, "abcd", 9., &[2])];
        assert_eq!(caret(text, &p, 4, 4.), 4);
        assert_eq!(caret(text, &p, 4, 5.), 5);
        assert_eq!(caret(text, &p, 5, 12.), 5);
    }

    #[test]
    fn a_selection_inside_one_line_is_one_rectangle() {
        let p = [para(0, "abcdef", 9., &[])];
        assert_eq!(highlight_rects(&p, 1..4), vec![(0, 9., 36.)]);
        assert_eq!(highlight_rects(&p, 2..2), vec![]);
        assert_eq!(
            highlight_rects(&p, 0..6),
            vec![(0, 0., 54.)],
            "the whole line reaches its end"
        );
    }

    #[test]
    fn a_selection_over_wrapped_lines_is_a_rectangle_per_line() {
        // "abcdef" in lines "ab", "cd", "ef".
        let p = [para(0, "abcdef", 9., &[2, 4])];
        assert_eq!(
            highlight_rects(&p, 1..5),
            vec![(0, 9., 18.), (1, 0., 18.), (2, 0., 9.)]
        );
        assert_eq!(
            highlight_rects(&p, 2..4),
            vec![(1, 0., 18.)],
            "starts at the wrap point"
        );
        assert_eq!(
            highlight_rects(&p, 0..6),
            vec![(0, 0., 18.), (1, 0., 18.), (2, 0., 18.)]
        );
    }

    #[test]
    fn a_selection_over_paragraphs_counts_the_lines_above_and_skips_empty_ones() {
        // "ab\n\ncde": the empty paragraph is a line with nothing painted.
        let p = [
            para(0, "ab", 9., &[]),
            para(3, "", 9., &[]),
            para(4, "cde", 9., &[]),
        ];
        assert_eq!(highlight_rects(&p, 1..6), vec![(0, 9., 18.), (2, 0., 18.)]);
        assert_eq!(highlight_rects(&p, 3..3), vec![]);
    }
}
