//! Minimal multiline text-entry state shared by every text modal (the
//! app-wide input unification seam). One string + a char-index caret;
//! rendering reuses `args_form::wrap_value_cursor` / `caret_line` so all
//! inputs look identical. Editing is char-based (the caret is a char index,
//! converted to a byte offset at the edit point).

use crate::view::args_form::wrap_value_cursor;

/// Normalize a paste payload for a multiline text field. Terminals translate
/// line breaks to `\r` in bracketed paste (they emulate the Enter key), so a
/// multiline paste arrives CR-separated: CRLF/CR become `\n` (preserving the
/// line structure), tabs expand to 4 spaces, and every other control char
/// (e.g. ESC from an ANSI-colored dump) is dropped. The renderer skips control
/// chars it cannot draw, so letting them into a value desyncs the wrap/caret
/// math from what is on screen — the "pasted text renders garbled" bug.
pub(crate) fn sanitize_paste(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next(); // CRLF is ONE line break
                }
                out.push('\n');
            }
            '\n' => out.push('\n'),
            '\t' => out.push_str("    "),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

#[derive(Debug, Clone, Default)]
pub struct MultilineInput {
    pub text: String,
    pub cursor: usize,
}

impl MultilineInput {
    fn byte_at(&self, char_idx: usize) -> usize {
        self.text
            .char_indices()
            .nth(char_idx)
            .map(|(b, _)| b)
            .unwrap_or(self.text.len())
    }

    pub fn insert_char(&mut self, c: char) {
        let at = self.byte_at(self.cursor);
        self.text.insert(at, c);
        self.cursor += 1;
    }

    pub fn insert_str(&mut self, s: &str) {
        let at = self.byte_at(self.cursor);
        self.text.insert_str(at, s);
        self.cursor += s.chars().count();
    }

    pub fn insert_newline(&mut self) {
        self.insert_char('\n');
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let start = self.byte_at(self.cursor - 1);
        let end = self.byte_at(self.cursor);
        self.text.replace_range(start..end, "");
        self.cursor -= 1;
    }

    pub fn move_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn move_right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.text.chars().count());
    }

    pub fn move_home(&mut self) {
        self.cursor = 0;
    }

    pub fn move_end(&mut self) {
        self.cursor = self.text.chars().count();
    }

    /// `(line_index, column)` of the caret in LOGICAL lines (split on `\n`),
    /// both char-based. Column is the char offset from the line's start.
    fn line_col(&self) -> (usize, usize) {
        let mut line = 0;
        let mut col = 0;
        for c in self.text.chars().take(self.cursor) {
            if c == '\n' {
                line += 1;
                col = 0;
            } else {
                col += 1;
            }
        }
        (line, col)
    }

    /// Char index of the first char on logical line `line` (clamped to the last
    /// line). Lines are the `\n`-split segments of `text`.
    fn line_start(&self, line: usize) -> usize {
        let mut idx = 0;
        let mut seen = 0;
        for c in self.text.chars() {
            if seen == line {
                return idx;
            }
            idx += 1;
            if c == '\n' {
                seen += 1;
            }
        }
        idx
    }

    /// Char length (excluding the trailing `\n`) of logical line `line`.
    fn line_len(&self, line: usize) -> usize {
        self.text.split('\n').nth(line).map(|l| l.chars().count()).unwrap_or(0)
    }

    /// Move the caret one logical line up, keeping the column where possible
    /// (clamped to the shorter target line). Inert on the first line.
    pub fn move_up(&mut self) {
        let (line, col) = self.line_col();
        if line == 0 {
            return;
        }
        let target = line - 1;
        self.cursor = self.line_start(target) + col.min(self.line_len(target));
    }

    /// Move the caret one logical line down, keeping the column where possible
    /// (clamped to the shorter target line). Inert on the last line.
    pub fn move_down(&mut self) {
        let (line, col) = self.line_col();
        let last_line = self.text.chars().filter(|&c| c == '\n').count();
        if line >= last_line {
            return;
        }
        let target = line + 1;
        self.cursor = self.line_start(target) + col.min(self.line_len(target));
    }

    /// Char index of the caret after moving `delta` visual rows at `width`,
    /// preserving the visual column (clamped to the target row's length).
    /// Inert (returns the current cursor) past the first/last visual row.
    fn visual_target(&self, width: usize, delta: isize) -> usize {
        let w = width.max(1);
        let (rows, cur_row, cur_col) = wrap_value_cursor(&self.text, self.cursor, w);
        let target = cur_row as isize + delta;
        if target < 0 || target as usize >= rows.len() {
            return self.cursor;
        }
        let target = target as usize;
        // Char index of the start of visual row `target` = sum of prior rows'
        // char lengths, minus a `\n` that was consumed at a hard-line boundary.
        // Simpler + robust: recompute by walking the wrap rows and counting the
        // consumed source characters (each visual row consumes its own chars;
        // a hard newline consumes one extra `\n` not present in any row string).
        // Use `wrap_row_char_starts` to get exact source indices.
        let starts = wrap_row_char_starts(&self.text, w);
        let base = starts[target];
        let row_len = rows[target].chars().count();
        // `wrap_value_cursor` reports `cur_col == w` for a caret at end-of-text
        // on a completely full row (the end-of-text reserve column). That value
        // is the soft-wrap BOUNDARY index, which belongs to the row below —
        // cap it to the last real column of the row so up/down lands on this
        // row instead of overshooting onto the next one.
        let cur_col = cur_col.min(w.saturating_sub(1));
        base + cur_col.min(row_len)
    }

    /// Move the caret one VISUAL (wrapped) row up at `width`, preserving the
    /// visual column. Inert at the first visual row.
    pub fn move_up_visual(&mut self, width: usize) {
        self.cursor = self.visual_target(width, -1);
    }
    /// Move the caret one VISUAL (wrapped) row down at `width`, preserving the
    /// visual column. Inert at the last visual row.
    pub fn move_down_visual(&mut self, width: usize) {
        self.cursor = self.visual_target(width, 1);
    }
}

/// Source char index at the start of each visual row for `text` wrapped to
/// `width` — the inverse mapping `wrap_value_cursor` implies. A hard `\n`
/// terminates a row and is itself consumed (not part of any row string).
fn wrap_row_char_starts(text: &str, width: usize) -> Vec<usize> {
    let w = width.max(1);
    let mut starts = vec![0usize];
    let mut col = 0usize;
    for (idx, ch) in text.chars().enumerate() {
        if ch == '\n' {
            starts.push(idx + 1); // next row starts AFTER the newline
            col = 0;
        } else {
            if col == w {
                starts.push(idx); // soft-wrap: next row starts AT this char
                col = 0;
            }
            col += 1;
        }
    }
    starts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ml(text: &str, cursor: usize) -> MultilineInput {
        MultilineInput { text: text.into(), cursor }
    }

    #[test]
    fn insert_char_at_caret_advances_cursor() {
        let mut m = ml("ab", 1);
        m.insert_char('x');
        assert_eq!(m.text, "axb");
        assert_eq!(m.cursor, 2);
    }

    #[test]
    fn insert_newline_is_a_char_insert() {
        let mut m = ml("ab", 2);
        m.insert_newline();
        assert_eq!(m.text, "ab\n");
        assert_eq!(m.cursor, 3);
    }

    #[test]
    fn backspace_removes_char_before_caret() {
        let mut m = ml("abc", 2);
        m.backspace();
        assert_eq!(m.text, "ac");
        assert_eq!(m.cursor, 1);
        let mut at_start = ml("abc", 0);
        at_start.backspace();
        assert_eq!(at_start.text, "abc");
    }

    #[test]
    fn moves_clamp_at_edges_and_are_char_based() {
        let mut m = ml("héllo", 0);
        m.move_left();
        assert_eq!(m.cursor, 0);
        m.move_end();
        assert_eq!(m.cursor, 5); // chars, not bytes
        m.move_right();
        assert_eq!(m.cursor, 5);
        m.move_home();
        assert_eq!(m.cursor, 0);
    }

    #[test]
    fn sanitize_paste_normalizes_line_endings_tabs_and_control_chars() {
        // CRLF and lone CR are each ONE line break; tabs expand; ESC (and any
        // other control char) drops while its printable tail stays.
        assert_eq!(sanitize_paste("a\r\nb\rc\nd"), "a\nb\nc\nd");
        assert_eq!(sanitize_paste("x\ty"), "x    y");
        assert_eq!(sanitize_paste("red\u{1b}[31mtext\u{7}"), "red[31mtext");
        assert_eq!(sanitize_paste("plain"), "plain");
    }

    #[test]
    fn insert_str_pastes_multichar_including_newlines() {
        let mut m = ml("ad", 1);
        m.insert_str("b\nc");
        assert_eq!(m.text, "ab\ncd");
        assert_eq!(m.cursor, 4);
    }

    #[test]
    fn move_up_down_navigate_logical_lines_keeping_column() {
        // "abc\ndefg\nhi"; caret after 'g' — line 1, col 4, char index 8.
        let mut m = ml("abc\ndefg\nhi", 8);
        m.move_up(); // → line 0, col clamped min(4,3)=3 → end of "abc" (index 3)
        assert_eq!(m.cursor, 3);
        m.move_up(); // already on the first line → inert
        assert_eq!(m.cursor, 3);
        // Back down: col 3 clamps onto line 1 (len 4) at index 4+3=7.
        m.move_down();
        assert_eq!(m.cursor, 7);
        // Down again clamps col 3 onto the short last line "hi" (len 2): index 9+2=11.
        m.move_down();
        assert_eq!(m.cursor, 11);
        m.move_down(); // last line → inert
        assert_eq!(m.cursor, 11);
    }

    #[test]
    fn move_up_down_are_inert_on_a_single_line() {
        let mut m = ml("hello", 3);
        m.move_up();
        assert_eq!(m.cursor, 3);
        m.move_down();
        assert_eq!(m.cursor, 3);
    }

    #[test]
    fn visual_move_traverses_wrapped_rows_of_one_logical_line() {
        // width 4, one logical line "abcdefghij" wraps to ["abcd","efgh","ij"].
        // caret at index 9 ('j') → visual row 2, col 1.
        let mut m = ml("abcdefghij", 9);
        m.move_up_visual(4); // → row 1 col 1 → index 5 ('f')
        assert_eq!(m.cursor, 5);
        m.move_up_visual(4); // → row 0 col 1 → index 1 ('b')
        assert_eq!(m.cursor, 1);
        m.move_up_visual(4); // already top → inert
        assert_eq!(m.cursor, 1);
        m.move_down_visual(4); // → row 1 col 1 → index 5
        assert_eq!(m.cursor, 5);
    }

    #[test]
    fn visual_move_clamps_column_onto_short_last_row() {
        // width 4: rows ["abcd","efgh","ij"]; caret at index 2 (row0 col2).
        let mut m = ml("abcdefghij", 2);
        m.move_down_visual(4); // row1 col2 → index 6
        assert_eq!(m.cursor, 6);
        m.move_down_visual(4); // row2 len2, col clamped to 2 → index 10 (end)
        assert_eq!(m.cursor, 10);
    }

    #[test]
    fn wrap_row_char_starts_matches_wrap_value_cursor() {
        let text = "abc\ndefghij\nk";
        for w in [1usize, 2, 3, 4, 7] {
            let starts = wrap_row_char_starts(text, w);
            for cur in 0..=text.chars().count() {
                let (_rows, row, col) = wrap_value_cursor(text, cur, w);
                assert!(starts[row] <= cur, "w={w} cur={cur} row={row}");
                if row + 1 < starts.len() {
                    assert!(cur <= starts[row + 1], "w={w} cur={cur}");
                }
                // For interior carets (not the end-of-text boundary), the
                // reconstruction `starts[row] + col` must round-trip back to
                // `cur` exactly — i.e. the boundary index is attributed to
                // the same row consistently on both sides of the mapping.
                if cur < text.chars().count() {
                    assert_eq!(starts[row] + col, cur, "w={w} cur={cur} row={row} col={col}");
                }
            }
        }
    }

    #[test]
    fn visual_move_up_clamps_off_the_full_row_boundary() {
        // width 4, one logical line "abcdefgh" wraps to exactly ["abcd","efgh"]
        // with no remainder — the caret at end-of-text (index 8) sits at
        // (row 1, col 4), where col 4 == width is the end-of-text reserve
        // column, not a real column on row 1. Up must land on row 0, not
        // re-land on row 1's start (the boundary index).
        let mut m = ml("abcdefgh", 8);
        m.move_up_visual(4);
        // Lands on row 0 (index 3, the last char of "abcd"); in particular
        // NOT index 4 (the start of "efgh" / row 1).
        assert_eq!(m.cursor, 3);
        let (_rows, row, _col) = wrap_value_cursor(&m.text, m.cursor, 4);
        assert_eq!(row, 0);
        // Already on the top visual row → inert.
        m.move_up_visual(4);
        assert_eq!(m.cursor, 3);
    }

    #[test]
    fn visual_move_down_is_inert_at_full_row_boundary_end_of_text() {
        // Same shape as above but exercising move_down_visual from the
        // already-last visual row: must stay inert, not skip past the end.
        let mut m = ml("abcdefgh", 8);
        m.move_down_visual(4);
        assert_eq!(m.cursor, 8);
    }
}
