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
}
