//! Per-def run form. `ArgsForm` holds one positional value per arg plus a
//! hand-rolled text-editor cursor (a `String` + char-index caret) for the
//! free-text rows, so the form supports Claude-Code-style multiline input
//! (soft-wrap, hard newlines, in-line editing). Enum rows keep the cycle /
//! dropdown behavior; fixed rows are display-only. `render_run_form` draws the
//! full two-panel picker shell (inputs left, the def's prompt right), reusing
//! `menu`'s layout + preview helpers.

use std::collections::HashMap;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear};

use crate::hit::{ButtonKind, HitMap, HitTarget};
use crate::ipc::types::{ArgSpec, TaskDefinition};
use crate::view::menu::{picker_layout, render_preview, PreviewMetrics};
use crate::view::theme::Palette;

/// Per-arg form state. `values` are the positional submission values (a
/// free-text value may embed `\n` hard newlines); `cursor` is the char-index
/// caret into the focused row's value; `preview_scroll` is the right (prompt)
/// panel's first visible wrapped line.
#[derive(Debug, Clone)]
pub struct ArgsForm {
    pub repo: String,
    pub def_name: String,
    pub args: Vec<ArgSpec>,
    pub values: Vec<String>,
    pub fixed: HashMap<String, String>,
    pub initial_worktree: Option<String>,
    pub focus: usize,
    /// Char-index caret into `values[focus]` (free-text rows only; enum/fixed
    /// rows ignore it). Reset to the value's end whenever focus changes.
    pub cursor: usize,
    pub error: Option<usize>,
    pub dropdown: Option<usize>, // highlighted option index while a dropdown is open
    pub preview_scroll: usize,
}

/// True when the arg carries a non-empty `options` list. Shared by `new` and the
/// public `is_enum`.
pub(crate) fn arg_is_enum(arg: &ArgSpec) -> bool {
    arg.options.as_ref().is_some_and(|o| !o.is_empty())
}

/// Initial value for one arg: `fixed` wins, then `initial`, then the declared
/// `default`, then (enums) the first option, else empty.
fn initial_value(
    arg: &ArgSpec,
    fixed: &HashMap<String, String>,
    initial: &HashMap<String, String>,
) -> String {
    if let Some(v) = fixed.get(&arg.name) {
        return v.clone();
    }
    if let Some(v) = initial.get(&arg.name) {
        return v.clone();
    }
    if let Some(d) = &arg.default {
        return d.clone();
    }
    if arg_is_enum(arg)
        && let Some(first) = arg.options.as_ref().and_then(|o| o.first())
    {
        return first.clone();
    }
    String::new()
}

/// Byte offset of char index `ci` in `s` (== `s.len()` at/after the end).
fn byte_at(s: &str, ci: usize) -> usize {
    s.char_indices().nth(ci).map(|(b, _)| b).unwrap_or(s.len())
}

impl ArgsForm {
    pub fn new(
        repo: String,
        def_name: String,
        args: Vec<ArgSpec>,
        fixed: HashMap<String, String>,
        initial: HashMap<String, String>,
        worktree: Option<String>,
    ) -> Self {
        let values: Vec<String> = args.iter().map(|a| initial_value(a, &fixed, &initial)).collect();
        let first_editable = args.iter().position(|a| !fixed.contains_key(&a.name));
        let focus = first_editable.unwrap_or(0);
        let cursor = values.get(focus).map(|v| v.chars().count()).unwrap_or(0);
        ArgsForm {
            repo,
            def_name,
            args,
            values,
            fixed,
            initial_worktree: worktree,
            focus,
            cursor,
            error: None,
            dropdown: None,
            preview_scroll: 0,
        }
    }

    pub fn is_enum(&self, i: usize) -> bool {
        self.args.get(i).is_some_and(arg_is_enum)
    }
    pub fn is_fixed(&self, i: usize) -> bool {
        self.args.get(i).is_some_and(|a| self.fixed.contains_key(&a.name))
    }
    fn first_editable(&self) -> Option<usize> {
        (0..self.args.len()).find(|&i| !self.is_fixed(i))
    }
    /// Char length of row `i`'s value.
    fn value_len(&self, i: usize) -> usize {
        self.values.get(i).map(|v| v.chars().count()).unwrap_or(0)
    }
    fn clear_error(&mut self, i: usize) {
        if self.error == Some(i) {
            self.error = None;
        }
    }
    fn step_focus(&mut self, delta: i32) {
        let n = self.args.len();
        if n == 0 || self.first_editable().is_none() {
            return;
        }
        let mut next = self.focus;
        for _ in 0..n {
            next = (((next as i32 + delta).rem_euclid(n as i32)) as usize) % n;
            if !self.is_fixed(next) {
                break;
            }
        }
        self.focus = next;
        self.cursor = self.value_len(next); // caret lands at the value's end
    }
    pub fn next_focus(&mut self) {
        self.step_focus(1);
    }
    pub fn prev_focus(&mut self) {
        self.step_focus(-1);
    }
    /// Focus a specific (non-fixed) row and park the caret at its value's end.
    /// Used by click-to-focus (`FormField` hit).
    pub fn focus_field(&mut self, i: usize) {
        if self.is_fixed(i) {
            return;
        }
        self.focus = i;
        self.cursor = self.value_len(i);
    }

    pub fn cycle_option(&mut self, i: usize, delta: i32) {
        if !self.is_enum(i) {
            return;
        }
        let opts = match self.args[i].options.as_ref() {
            Some(o) if !o.is_empty() => o.clone(),
            _ => return,
        };
        let len = opts.len() as i32;
        let cur = opts
            .iter()
            .position(|o| Some(o) == self.values.get(i))
            .map(|p| p as i32)
            .unwrap_or(0);
        let next = ((cur + delta).rem_euclid(len)) as usize;
        self.values[i] = opts[next].clone();
    }

    // --- Text editing (free-text rows only). All caret-relative. ---

    /// Insert `c` at the caret. No-op on enum/fixed rows.
    pub fn input_char(&mut self, c: char) {
        let i = self.focus;
        if self.is_fixed(i) || self.is_enum(i) {
            return;
        }
        if let Some(v) = self.values.get_mut(i) {
            let b = byte_at(v, self.cursor);
            v.insert(b, c);
            self.cursor += 1;
        }
        self.clear_error(i);
    }
    /// Insert `s` verbatim at the caret (bracketed paste). Returns whether
    /// anything was inserted (enum/fixed rows and empty input insert nothing).
    pub fn insert_str(&mut self, s: &str) -> bool {
        let i = self.focus;
        if self.is_fixed(i) || self.is_enum(i) || s.is_empty() {
            return false;
        }
        if let Some(v) = self.values.get_mut(i) {
            let b = byte_at(v, self.cursor);
            v.insert_str(b, s);
            self.cursor += s.chars().count();
        }
        self.clear_error(i);
        true
    }
    /// Insert a hard newline at the caret (Shift+Enter / Alt+Enter).
    pub fn insert_newline(&mut self) {
        self.input_char('\n');
    }
    /// Delete the char before the caret. No-op on enum/fixed rows or at the
    /// value's start.
    pub fn backspace(&mut self) {
        let i = self.focus;
        if self.is_fixed(i) || self.is_enum(i) || self.cursor == 0 {
            return;
        }
        if let Some(v) = self.values.get_mut(i) {
            let start = byte_at(v, self.cursor - 1);
            let end = byte_at(v, self.cursor);
            v.replace_range(start..end, "");
            self.cursor -= 1;
        }
        self.clear_error(i);
    }
    pub fn move_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }
    pub fn move_right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.value_len(self.focus));
    }
    pub fn move_home(&mut self) {
        let (line, _) = self.cursor_line_col();
        self.cursor = self.index_at_line_col(line, 0);
    }
    pub fn move_end(&mut self) {
        let (line, _) = self.cursor_line_col();
        let end = self.logical_line_len(line);
        self.cursor = self.index_at_line_col(line, end);
    }
    /// Move the caret up one logical line (same column). Returns `false` (no
    /// move) on the first line or on an enum/fixed row, so the caller can step
    /// focus instead.
    pub fn try_move_up(&mut self) -> bool {
        if self.is_enum(self.focus) || self.is_fixed(self.focus) {
            return false;
        }
        let (line, col) = self.cursor_line_col();
        if line == 0 {
            return false;
        }
        self.cursor = self.index_at_line_col(line - 1, col);
        true
    }
    /// Move the caret down one logical line (same column). Returns `false` on
    /// the last line or on an enum/fixed row.
    pub fn try_move_down(&mut self) -> bool {
        if self.is_enum(self.focus) || self.is_fixed(self.focus) {
            return false;
        }
        let (line, col) = self.cursor_line_col();
        if line + 1 >= self.logical_line_count() {
            return false;
        }
        self.cursor = self.index_at_line_col(line + 1, col);
        true
    }

    /// (logical line, column) of the caret within the focused row's value.
    fn cursor_line_col(&self) -> (usize, usize) {
        let v = self.values.get(self.focus).map(String::as_str).unwrap_or("");
        let mut line = 0usize;
        let mut col = 0usize;
        for (idx, ch) in v.chars().enumerate() {
            if idx == self.cursor {
                break;
            }
            if ch == '\n' {
                line += 1;
                col = 0;
            } else {
                col += 1;
            }
        }
        (line, col)
    }
    fn logical_line_count(&self) -> usize {
        self.values.get(self.focus).map(|v| v.split('\n').count()).unwrap_or(1)
    }
    fn logical_line_len(&self, line: usize) -> usize {
        self.values
            .get(self.focus)
            .and_then(|v| v.split('\n').nth(line))
            .map(|l| l.chars().count())
            .unwrap_or(0)
    }
    /// Char index of (logical line, column), clamped to the value.
    fn index_at_line_col(&self, line: usize, col: usize) -> usize {
        let v = self.values.get(self.focus).map(String::as_str).unwrap_or("");
        let lines: Vec<&str> = v.split('\n').collect();
        let target = line.min(lines.len().saturating_sub(1));
        let mut idx = 0;
        for l in &lines[..target] {
            idx += l.chars().count() + 1; // + the '\n' separator
        }
        idx + col.min(lines[target].chars().count())
    }

    /// First arg with no default and an empty value blocks submit: sets `error`,
    /// focuses it when editable, returns `Err(index)`. Otherwise returns the
    /// positional values (fixed rows included).
    pub fn validate(&mut self) -> Result<Vec<String>, usize> {
        let missing = self
            .args
            .iter()
            .enumerate()
            .find(|(i, a)| a.default.is_none() && self.values.get(*i).map(String::as_str) == Some(""))
            .map(|(i, _)| i);
        if let Some(i) = missing {
            if !self.is_fixed(i) {
                self.focus = i;
                self.cursor = 0;
            }
            self.error = Some(i);
            return Err(i);
        }
        Ok(self.values.clone())
    }

    pub fn open_dropdown(&mut self, i: usize) {
        if !self.is_enum(i) || self.is_fixed(i) {
            return;
        }
        self.focus = i;
        let opts = self.args[i].options.as_ref();
        let cur = opts
            .and_then(|o| o.iter().position(|v| Some(v) == self.values.get(i)))
            .unwrap_or(0);
        self.dropdown = Some(cur);
    }
    pub fn close_dropdown(&mut self) {
        self.dropdown = None;
    }
    pub fn dropdown_move(&mut self, delta: i32) {
        let Some(cur) = self.dropdown else { return };
        let len = self
            .args
            .get(self.focus)
            .and_then(|a| a.options.as_ref())
            .map(|o| o.len())
            .unwrap_or(0);
        if len == 0 {
            return;
        }
        let next = (cur as i32 + delta).clamp(0, len as i32 - 1) as usize;
        self.dropdown = Some(next);
    }
    pub fn dropdown_pick(&mut self) {
        let Some(hl) = self.dropdown else { return };
        let i = self.focus;
        if let Some(opt) = self
            .args
            .get(i)
            .and_then(|a| a.options.as_ref())
            .and_then(|o| o.get(hl))
            .cloned()
        {
            self.values[i] = opt;
            self.clear_error(i);
        }
        self.dropdown = None;
    }
}

/// Char-wrap `value` (per logical line) to `width`, returning the display lines
/// plus the caret's (row, col) within them. Reserving one extra display column
/// for the caret is the caller's job (the caret can sit one past the last char
/// of a full row).
fn wrap_value_cursor(value: &str, cursor: usize, width: usize) -> (Vec<String>, usize, usize) {
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

/// Right-pad (or truncate) `s` to exactly `width` display columns.
fn pad(s: &str, width: usize) -> String {
    let mut out: String = s.chars().take(width).collect();
    while out.chars().count() < width {
        out.push(' ');
    }
    out
}

const RUN_FORM_HINT: &str =
    " tab/↑↓ move · ←/→ enum · shift/alt+enter newline · enter run · esc cancel ";

/// Render the run form: the full two-panel picker shell with the arg inputs on
/// the left and the definition's prompt (scrollable) on the right. `full` is the
/// cached `TaskDefinition` for the prompt (`None` → a "loading" placeholder).
/// Registers both panels as `Modal`, one `FormField(i)` per editable row, the
/// `Button` targets, the right panel's `MenuPreview` (wheel scroll), and — when
/// open — the dropdown popup last (topmost). Returns the preview scroll metrics.
pub fn render_run_form(
    frame: &mut Frame,
    hit: &mut HitMap,
    p: &Palette,
    form: &ArgsForm,
    full: Option<&TaskDefinition>,
) -> PreviewMetrics {
    let layout = picker_layout(frame.area());
    for r in [layout.left, layout.right] {
        frame.render_widget(Clear, r);
        hit.push(r, HitTarget::Modal); // both panels opaque to clicks
    }

    let title_style = Style::default().fg(p.fg).add_modifier(Modifier::BOLD);
    let left_block = Block::default()
        .title(Span::styled(format!(" {} args ", form.def_name), title_style))
        .title_bottom(Line::from(Span::styled(RUN_FORM_HINT, p.dim_style())))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(p.accent));
    frame.render_widget(left_block, layout.left);
    let right_block = Block::default()
        .title(Span::styled(format!(" {} ", form.def_name), title_style))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(p.accent));
    frame.render_widget(right_block, layout.right);

    let dropdown_anchor = render_fields(frame, hit, p, form, layout.left_inner);
    render_buttons(frame, hit, p, layout.left_inner);

    // Right panel: the def's full prompt (or a placeholder until it arrives).
    let mut content: Vec<(String, Style)> = Vec::new();
    match full {
        Some(td) => {
            for l in td.prompt.lines() {
                content.push((l.to_string(), Style::default().fg(p.fg)));
            }
        }
        None => content.push(("(loading definition…)".into(), p.dim_style())),
    }
    let metrics = render_preview(frame, hit, &layout, &content, form.preview_scroll);

    // Dropdown popup last so it is topmost (Clear + `DropdownItem` clicks win).
    if let Some(anchor_y) = dropdown_anchor {
        render_dropdown(frame, hit, p, form, layout.left_inner, anchor_y);
    }
    metrics
}

/// Draw the arg rows top-down into the left interior (reserving the bottom line
/// for the buttons), registering a `FormField(i)` per editable row. Returns the
/// screen `y` of the focused enum row when a dropdown is open (its anchor).
fn render_fields(
    frame: &mut Frame,
    hit: &mut HitMap,
    p: &Palette,
    form: &ArgsForm,
    inner: Rect,
) -> Option<u16> {
    if inner.width == 0 || inner.height == 0 {
        return None;
    }
    let bottom = inner.y + inner.height.saturating_sub(1); // last line = buttons
    let mut y = inner.y;
    let mut dropdown_anchor = None;

    for (i, arg) in form.args.iter().enumerate() {
        if y >= bottom {
            break;
        }
        let fixed = form.is_fixed(i);
        let focused = i == form.focus && form.dropdown.is_none();
        let value = form.values.get(i).cloned().unwrap_or_default();
        let name_style = if i == form.focus {
            Style::default().fg(p.accent).add_modifier(Modifier::BOLD)
        } else if fixed {
            p.dim_style()
        } else {
            Style::default().fg(p.fg)
        };

        if form.is_enum(i) && !fixed {
            let val_style = if focused { p.selection() } else { Style::default().fg(p.fg) };
            let row = Rect { x: inner.x, y, width: inner.width, height: 1 };
            frame.render_widget(
                Line::from(vec![
                    Span::styled(format!(" {} ", arg.name), name_style),
                    Span::styled(format!("‹{value}›"), val_style),
                ]),
                row,
            );
            hit.push(row, HitTarget::FormField(i));
            if form.dropdown.is_some() && i == form.focus {
                dropdown_anchor = Some(y);
            }
            y += 1;
        } else if fixed {
            let row = Rect { x: inner.x, y, width: inner.width, height: 1 };
            frame.render_widget(
                Line::from(vec![
                    Span::styled(format!(" {} ", arg.name), name_style),
                    Span::styled(value, p.dim_style()),
                ]),
                row,
            );
            y += 1; // fixed rows are display-only (no FormField)
        } else {
            // Free-text: a label line, then the wrapped value with the caret.
            let label_y = y;
            let mut spans = vec![Span::styled(format!(" {}", arg.name), name_style)];
            if form.error == Some(i) {
                spans.push(Span::styled("  required", Style::default().fg(p.error)));
            } else if let Some(d) = &arg.description
                && !d.is_empty()
            {
                spans.push(Span::styled(format!("  {d}"), p.dim_style()));
            }
            frame.render_widget(Line::from(spans), Rect { x: inner.x, y, width: inner.width, height: 1 });
            y += 1;
            let indent = 2u16;
            let vx = inner.x + indent;
            let vw = inner.width.saturating_sub(indent).max(1);
            // Reserve one column so the caret can sit past a full row's last char.
            let wrap_w = (vw as usize).saturating_sub(1).max(1);
            let (lines, cur_row, cur_col) = wrap_value_cursor(&value, form.cursor, wrap_w);
            for (ri, line) in lines.iter().enumerate() {
                if y >= bottom {
                    break;
                }
                let row = Rect { x: vx, y, width: vw, height: 1 };
                if focused && ri == cur_row {
                    frame.render_widget(caret_line(line, cur_col, p), row);
                } else {
                    frame.render_widget(
                        Line::from(Span::styled(line.clone(), Style::default().fg(p.fg))),
                        row,
                    );
                }
                y += 1;
            }
            let field_h = (y - label_y).max(1);
            hit.push(
                Rect { x: inner.x, y: label_y, width: inner.width, height: field_h },
                HitTarget::FormField(i),
            );
        }
        if y < bottom {
            y += 1; // blank separator between fields
        }
    }
    dropdown_anchor
}

/// One display line of a free-text value with a reversed caret cell at `col`
/// (a trailing caret renders as a reversed space).
fn caret_line(text: &str, col: usize, p: &Palette) -> Line<'static> {
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

/// `[ Run ] [ Cancel ]` on the left interior's last line, with `Button` targets.
fn render_buttons(frame: &mut Frame, hit: &mut HitMap, p: &Palette, inner: Rect) {
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let btn_y = inner.y + inner.height.saturating_sub(1);
    let run = Rect { x: inner.x + 1, y: btn_y, width: 7, height: 1 };
    let cancel = Rect { x: inner.x + 10, y: btn_y, width: 10, height: 1 };
    frame.render_widget(Line::from(Span::styled("[ Run ]", Style::default().fg(p.accent))), run);
    frame.render_widget(Line::from(Span::styled("[ Cancel ]", Style::default().fg(p.dim))), cancel);
    hit.push(run, HitTarget::Button(ButtonKind::Confirm));
    hit.push(cancel, HitTarget::Button(ButtonKind::Cancel));
}

/// Option-list popup hanging under the focused enum row (`anchor_y`). Registered
/// last so it is topmost in the hit map (`DropdownItem` clicks win).
fn render_dropdown(
    frame: &mut Frame,
    hit: &mut HitMap,
    p: &Palette,
    form: &ArgsForm,
    inner: Rect,
    anchor_y: u16,
) {
    let Some(hl) = form.dropdown else { return };
    let Some(opts) = form.args.get(form.focus).and_then(|a| a.options.as_ref()) else { return };
    let name_w = form.args.get(form.focus).map(|a| a.name.chars().count()).unwrap_or(0);
    // Anchor under the value column: " {name} " then the ‹value›.
    let x = (inner.x + 2 + name_w as u16).min(inner.x + inner.width.saturating_sub(6));
    let y = anchor_y + 1;
    let w = (inner.x + inner.width).saturating_sub(x).clamp(6, inner.width.max(6));
    let room = (inner.y + inner.height).saturating_sub(y);
    let h = (opts.len() as u16 + 2).min(room).max(3);
    let area = Rect { x, y, width: w, height: h };
    frame.render_widget(Clear, area);
    frame.render_widget(
        Block::default().borders(Borders::ALL).border_style(Style::default().fg(p.accent)),
        area,
    );
    for (i, opt) in opts.iter().enumerate() {
        if i as u16 + 1 >= h.saturating_sub(1) {
            break;
        }
        let row = Rect { x: x + 1, y: y + 1 + i as u16, width: w.saturating_sub(2), height: 1 };
        let style = if i == hl { p.selection() } else { Style::default().fg(p.fg) };
        frame.render_widget(
            Line::from(Span::styled(pad(&format!(" {opt}"), row.width as usize), style)),
            row,
        );
        hit.push(row, HitTarget::DropdownItem(i));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::types::ArgSpec;
    use std::collections::HashMap;

    fn arg(name: &str) -> ArgSpec {
        ArgSpec { name: name.into(), default: None, options: None, description: None }
    }
    fn m(p: &[(&str, &str)]) -> HashMap<String, String> {
        p.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }
    fn free(name: &str) -> ArgsForm {
        ArgsForm::new("r".into(), "d".into(), vec![arg(name)], HashMap::new(), HashMap::new(), None)
    }

    #[test]
    fn value_precedence_fixed_initial_default_firstopt_empty() {
        let args = vec![
            ArgSpec { default: Some("d".into()), ..arg("fixedwin") },            // fixed wins
            ArgSpec { default: Some("d".into()), ..arg("initialwin") },          // initial > default
            ArgSpec { default: Some("ready".into()), ..arg("defaultwin") },      // default
            ArgSpec { options: Some(vec!["x".into(), "y".into()]), ..arg("enumfirst") }, // first option
            arg("emptyreq"),                                                     // "" (required, no default)
        ];
        let form = ArgsForm::new(
            "platform".into(),
            "d".into(),
            args,
            m(&[("fixedwin", "F")]),
            m(&[("initialwin", "I")]),
            None,
        );
        assert_eq!(form.values, vec!["F", "I", "ready", "x", ""]);
    }

    #[test]
    fn focus_starts_on_first_editable_row_caret_at_end() {
        let args = vec![arg("fixed"), ArgSpec { default: Some("main".into()), ..arg("editable") }];
        let form = ArgsForm::new("r".into(), "d".into(), args, m(&[("fixed", "v")]), HashMap::new(), None);
        assert_eq!(form.focus, 1); // row 0 is fixed
        assert_eq!(form.cursor, 4); // caret at end of "main"
        assert_eq!(form.error, None);
        assert_eq!(form.dropdown, None);
        assert_eq!(form.preview_scroll, 0);
    }

    #[test]
    fn is_enum_and_is_fixed() {
        let form = ArgsForm::new(
            "r".into(), "d".into(),
            vec![
                ArgSpec { options: Some(vec!["a".into(), "b".into()]), ..arg("mode") },
                arg("pr"),
                arg("src"),
            ],
            m(&[("src", "wt")]),
            HashMap::new(),
            None,
        );
        assert!(form.is_enum(0));
        assert!(!form.is_enum(1));
        assert!(!form.is_fixed(0));
        assert!(form.is_fixed(2));
    }

    #[test]
    fn focus_wraps_skips_fixed_and_resets_caret() {
        let mut form = ArgsForm::new(
            "r".into(), "d".into(),
            vec![ArgSpec { default: Some("aa".into()), ..arg("a") }, arg("b"), arg("c")],
            m(&[("b", "x")]), // b fixed
            HashMap::new(),
            None,
        );
        assert_eq!(form.focus, 0);
        assert_eq!(form.cursor, 2); // "aa"
        form.next_focus(); // 0 -> (skip 1) -> 2
        assert_eq!(form.focus, 2);
        assert_eq!(form.cursor, 0); // "c" is empty
        form.next_focus(); // 2 -> wrap -> 0
        assert_eq!(form.focus, 0);
        form.prev_focus(); // 0 -> wrap -> (skip 2? no: prev is 2) -> 2
        assert_eq!(form.focus, 2);
    }

    #[test]
    fn cycle_option_wraps_only_on_enums() {
        let mut form = ArgsForm::new(
            "r".into(), "d".into(),
            vec![ArgSpec { default: Some("ready".into()), options: Some(vec!["ready".into(), "create".into()]), ..arg("mode") }],
            HashMap::new(), HashMap::new(), None,
        );
        assert_eq!(form.values[0], "ready");
        form.cycle_option(0, 1); // ready -> create
        assert_eq!(form.values[0], "create");
        form.cycle_option(0, 1); // create -> ready (wrap)
        assert_eq!(form.values[0], "ready");
        form.cycle_option(0, -1); // ready -> create (wrap back)
        assert_eq!(form.values[0], "create");
    }

    #[test]
    fn text_edit_inserts_at_caret_and_backspaces_clearing_error() {
        let mut form = free("pr");
        assert!(form.validate().is_err()); // required + empty
        assert_eq!(form.error, Some(0));
        form.input_char('5'); // typing clears the row error
        assert_eq!(form.error, None);
        form.input_char('7');
        assert_eq!(form.values[0], "57");
        assert_eq!(form.cursor, 2);
        form.move_left(); // caret between 5 and 7
        form.input_char('6'); // insert mid-value → "567"
        assert_eq!(form.values[0], "567");
        form.backspace(); // deletes the char before the caret ('6')
        assert_eq!(form.values[0], "57");
    }

    #[test]
    fn text_edit_ignores_enum_and_fixed_rows() {
        let mut form = ArgsForm::new(
            "r".into(), "d".into(),
            vec![ArgSpec { options: Some(vec!["a".into(), "b".into()]), default: Some("a".into()), ..arg("mode") }],
            HashMap::new(), HashMap::new(), None,
        );
        form.input_char('x'); // enum focus: typing ignored
        assert_eq!(form.values[0], "a");
        assert!(!form.insert_str("hi")); // enum: paste ignored
        assert_eq!(form.values[0], "a");
    }

    #[test]
    fn insert_newline_and_paste_multiline() {
        let mut form = free("desc");
        form.input_char('a');
        form.insert_newline(); // hard newline
        form.input_char('b');
        assert_eq!(form.values[0], "a\nb");
        assert_eq!(form.cursor, 3);
        // Paste a multiline blob verbatim at the caret.
        assert!(form.insert_str("\nc\nd"));
        assert_eq!(form.values[0], "a\nb\nc\nd");
        assert_eq!(form.cursor, 7);
        assert_eq!(form.logical_line_count(), 4);
    }

    #[test]
    fn caret_line_movement_home_end_up_down() {
        let mut form = free("desc");
        assert!(form.insert_str("ab\ncde")); // caret at end (index 6, line 1 col 3)
        assert_eq!(form.cursor_line_col(), (1, 3));
        form.move_home(); // start of line 1
        assert_eq!(form.cursor_line_col(), (1, 0));
        assert_eq!(form.cursor, 3);
        form.move_end(); // end of line 1
        assert_eq!(form.cursor_line_col(), (1, 3));
        assert!(form.try_move_up()); // to line 0, col clamped to len (2)
        assert_eq!(form.cursor_line_col(), (0, 2));
        assert!(!form.try_move_up()); // already on the first line → no move
        assert!(form.try_move_down()); // back to line 1
        assert_eq!(form.cursor_line_col().0, 1);
        assert!(!form.try_move_down()); // last line → no move
    }

    #[test]
    fn arrows_do_not_move_lines_on_enum_rows() {
        let mut form = ArgsForm::new(
            "r".into(), "d".into(),
            vec![ArgSpec { options: Some(vec!["a".into(), "b".into()]), default: Some("a".into()), ..arg("mode") }],
            HashMap::new(), HashMap::new(), None,
        );
        assert!(!form.try_move_up()); // enum: never consumes ↑/↓
        assert!(!form.try_move_down());
    }

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
    fn validate_focuses_first_editable_missing() {
        let mut form = ArgsForm::new(
            "r".into(), "d".into(),
            vec![ArgSpec { default: Some("main".into()), ..arg("target") }, arg("pr")],
            HashMap::new(), HashMap::new(), None,
        );
        let err = form.validate().unwrap_err();
        assert_eq!(err, 1); // pr is required-and-empty
        assert_eq!(form.error, Some(1));
        assert_eq!(form.focus, 1);
    }

    #[test]
    fn validate_ok_returns_positional_values_including_fixed() {
        let mut form = ArgsForm::new(
            "r".into(), "d".into(),
            vec![arg("source"), ArgSpec { default: Some("main".into()), ..arg("target") }],
            m(&[("source", "wt-a")]),
            HashMap::new(), None,
        );
        form.input_char('x'); // focus starts on target (source fixed), caret at end -> "mainx"
        assert_eq!(form.validate().unwrap(), vec!["wt-a".to_string(), "mainx".to_string()]);
    }

    #[test]
    fn focus_field_parks_caret_and_skips_fixed() {
        let mut form = ArgsForm::new(
            "r".into(), "d".into(),
            vec![ArgSpec { default: Some("wt".into()), ..arg("source") }, ArgSpec { default: Some("main".into()), ..arg("target") }],
            m(&[("source", "wt")]),
            HashMap::new(), None,
        );
        form.focus_field(0); // fixed → ignored, focus stays on target
        assert_eq!(form.focus, 1);
        form.focus_field(1);
        assert_eq!(form.focus, 1);
        assert_eq!(form.cursor, 4); // end of "main"
    }

    #[test]
    fn dropdown_open_move_pick() {
        let mut form = ArgsForm::new(
            "r".into(), "d".into(),
            vec![ArgSpec { options: Some(vec!["ready".into(), "create".into(), "draft".into()]), default: Some("ready".into()), ..arg("mode") }],
            HashMap::new(), HashMap::new(), None,
        );
        form.open_dropdown(0);
        assert_eq!(form.dropdown, Some(0)); // highlight = index of current value ("ready")
        form.dropdown_move(1);
        assert_eq!(form.dropdown, Some(1));
        form.dropdown_move(5); // clamp at last
        assert_eq!(form.dropdown, Some(2));
        form.dropdown_pick();
        assert_eq!(form.values[0], "draft");
        assert_eq!(form.dropdown, None);
    }
}
