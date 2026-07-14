//! Minimal multiline text-entry state shared by every text modal (the
//! app-wide input unification seam). One string + a char-index caret;
//! rendering reuses `args_form::wrap_value_cursor` / `caret_line` so all
//! inputs look identical. Editing is char-based (the caret is a char index,
//! converted to a byte offset at the edit point).

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
}
