//! Shared free-text rendering helpers. `ArgsForm` and its bespoke two-panel
//! `render_run_form` retired once both launch surfaces moved onto the shared
//! `view::form::FormState` engine (`Mode::Form` centered modal + `Mode::DefArgs`
//! two-panel picker). What remains are the pure wrap/caret primitives both the
//! shared form and the `MultilineInput` editor reuse: [`wrap_value_cursor`]
//! (char-wrap a value + locate the caret) and [`caret_line`] (draw one display
//! line with a reversed caret cell).

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::view::theme::Palette;

/// Char-wrap `value` (per logical line) to `width`, returning the display lines
/// plus the caret's (row, col) within them. Reserving one extra display column
/// for the caret is the caller's job (the caret can sit one past the last char
/// of a full row).
pub(crate) fn wrap_value_cursor(
    value: &str,
    cursor: usize,
    width: usize,
) -> (Vec<String>, usize, usize) {
    let w = width.max(1);
    let mut rows: Vec<String> = vec![String::new()];
    let mut col = 0usize; // column on the current row
    let mut cur_row = 0usize;
    let mut cur_col = 0usize;
    let mut found = false;
    for (idx, ch) in value.chars().enumerate() {
        // Wrap before a non-newline char that would overflow the current row.
        if ch != '\n' && col == w {
            rows.push(String::new());
            col = 0;
        }
        if idx == cursor {
            cur_row = rows.len() - 1;
            cur_col = col;
            found = true;
        }
        if ch == '\n' {
            rows.push(String::new());
            col = 0;
        } else {
            rows.last_mut().unwrap().push(ch);
            col += 1;
        }
    }
    if !found {
        cur_row = rows.len() - 1;
        cur_col = col; // caret at the value's end (fits in the reserved column)
    }
    (rows, cur_row, cur_col)
}

/// One display line of a free-text value with a reversed caret cell at `col`
/// (a trailing caret renders as a reversed space).
pub(crate) fn caret_line(text: &str, col: usize, p: &Palette) -> Line<'static> {
    let chars: Vec<char> = text.chars().collect();
    let col = col.min(chars.len());
    let before: String = chars[..col].iter().collect();
    let (caret, after): (String, String) = if col < chars.len() {
        (chars[col].to_string(), chars[col + 1..].iter().collect())
    } else {
        (" ".to_string(), String::new())
    };
    Line::from(vec![
        Span::styled(before, Style::default().fg(p.fg)),
        Span::styled(caret, Style::default().add_modifier(Modifier::REVERSED)),
        Span::styled(after, Style::default().fg(p.fg)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_value_cursor_positions() {
        // Single logical line, char-wrapped at width 3.
        let (rows, r, c) = wrap_value_cursor("abcdef", 4, 3);
        assert_eq!(rows, vec!["abc".to_string(), "def".to_string()]);
        assert_eq!((r, c), (1, 1)); // caret before 'e'
        // Caret at the very end sits one past the last full row's chars.
        let (rows, r, c) = wrap_value_cursor("abc", 3, 3);
        assert_eq!(rows, vec!["abc".to_string()]);
        assert_eq!((r, c), (0, 3));
        // Hard newlines split logical lines; caret at the end sits after "bb".
        let (rows, r, c) = wrap_value_cursor("a\nbb", 4, 8);
        assert_eq!(rows, vec!["a".to_string(), "bb".to_string()]);
        assert_eq!((r, c), (1, 2));
        // Empty value → one empty row, caret at origin.
        let (rows, r, c) = wrap_value_cursor("", 0, 8);
        assert_eq!(rows, vec![String::new()]);
        assert_eq!((r, c), (0, 0));
    }

    #[test]
    fn caret_line_reverses_the_char_at_col() {
        let p = Palette::default();
        // Mid-value caret reverses the char under it, leaving the rest plain.
        let line = caret_line("abc", 1, &p);
        assert_eq!(line.spans.len(), 3);
        assert_eq!(line.spans[0].content, "a");
        assert_eq!(line.spans[1].content, "b");
        assert!(line.spans[1].style.add_modifier.contains(Modifier::REVERSED));
        assert_eq!(line.spans[2].content, "c");
        // A trailing caret renders as a reversed space past the last char.
        let end = caret_line("abc", 3, &p);
        assert_eq!(end.spans[0].content, "abc");
        assert_eq!(end.spans[1].content, " ");
        assert!(end.spans[1].style.add_modifier.contains(Modifier::REVERSED));
    }
}
