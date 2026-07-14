//! Reusable bordered form kit. A single accent-bordered popup stacking typed
//! fields — a one-row **Input**, a 3-row **Textarea**, and a **Dropdown** (a
//! value with a `▾`, opening inline as an option list). Each field is its own bordered box
//! titled with its label; the FOCUSED box gets an accent border + bold label so
//! it is always obvious which component you are in. The bottom line is the shared
//! `[ {primary} ] [ Cancel ]` button row. Focus (Tab) cycles field₀…fieldₙ →
//! Primary → Cancel; Enter fires the focused button (or opens/picks a dropdown);
//! nothing submits on a stray keystroke. Consumed by `Mode::Form`.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

use crate::hit::{ButtonKind, HitMap, HitTarget};
use crate::view::args_form::{caret_line, wrap_value_cursor};
use crate::view::modal::{render_button_row, DIALOG_WIDTH, MODAL_PADDING};
use crate::view::multiline_input::MultilineInput;
use crate::view::theme::{GLYPH_CHEVRON_DOWN, Palette};

/// The three field shapes. Shape alone signals the type — a one-row box is an
/// input, a three-row box a textarea, a `▾` a dropdown (no label tags needed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldKind {
    Input,
    Textarea,
    Dropdown { options: Vec<String> },
}

/// One form field: a `label` (rendered as the box's border title), its `kind`,
/// the current `value`, and whether it must be non-empty to submit.
#[derive(Debug, Clone)]
pub struct Field {
    pub label: String,
    pub kind: FieldKind,
    pub value: String,
    pub required: bool,
}

impl Field {
    pub fn input(label: &str, value: &str, required: bool) -> Self {
        Field { label: label.into(), kind: FieldKind::Input, value: value.into(), required }
    }
    pub fn textarea(label: &str, value: &str, required: bool) -> Self {
        Field { label: label.into(), kind: FieldKind::Textarea, value: value.into(), required }
    }
    pub fn dropdown(label: &str, options: Vec<String>, value: &str) -> Self {
        Field {
            label: label.into(),
            kind: FieldKind::Dropdown { options },
            value: value.into(),
            required: false,
        }
    }
    fn is_text(&self) -> bool {
        matches!(self.kind, FieldKind::Input | FieldKind::Textarea)
    }
    fn box_content_height(&self) -> u16 {
        match self.kind {
            FieldKind::Textarea => 3,
            _ => 1,
        }
    }
}

/// Which focus stop is active: a field by index, the primary button, or Cancel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusKind {
    Field(usize),
    Primary,
    Cancel,
}

/// The form's interactive state. `focus` runs `0..fields.len()` (a field), then
/// `fields.len()` (Primary) and `fields.len()+1` (Cancel). `caret` is the char
/// caret into the focused text field. `dropdown_*` track an open select; `error`
/// flags the field that failed validation.
#[derive(Debug, Clone)]
pub struct FormState {
    pub title: String,
    pub primary_label: String,
    pub fields: Vec<Field>,
    pub focus: usize,
    pub caret: usize,
    pub dropdown_open: bool,
    pub dropdown_index: usize,
    pub error: Option<usize>,
}

impl FormState {
    /// Build a form; focus starts on the first field with the caret at its end.
    pub fn new(title: &str, primary_label: &str, fields: Vec<Field>) -> Self {
        let caret = fields.first().map(|f| f.value.chars().count()).unwrap_or(0);
        FormState {
            title: title.into(),
            primary_label: primary_label.into(),
            fields,
            focus: 0,
            caret,
            dropdown_open: false,
            dropdown_index: 0,
            error: None,
        }
    }

    fn stops(&self) -> usize {
        self.fields.len() + 2
    }

    pub fn focus_kind(&self) -> FocusKind {
        if self.focus < self.fields.len() {
            FocusKind::Field(self.focus)
        } else if self.focus == self.fields.len() {
            FocusKind::Primary
        } else {
            FocusKind::Cancel
        }
    }

    /// The button the current focus maps to, or `None` when a field is focused —
    /// so the button row highlights NEITHER (a field box and a button must never
    /// look focused at once). Drives `render_button_row`.
    pub fn button_focus(&self) -> Option<ButtonKind> {
        match self.focus_kind() {
            FocusKind::Primary => Some(ButtonKind::Confirm),
            FocusKind::Cancel => Some(ButtonKind::Cancel),
            FocusKind::Field(_) => None,
        }
    }

    fn land_caret(&mut self) {
        // Landing on a text field parks the caret at its end; leaving a dropdown
        // closes it.
        self.dropdown_open = false;
        if let FocusKind::Field(i) = self.focus_kind()
            && self.fields[i].is_text()
        {
            self.caret = self.fields[i].value.chars().count();
        }
    }

    /// Focus the field at `i` (clamped to a real field), parking the caret and
    /// closing any open dropdown. Used by click routing.
    pub fn focus_field(&mut self, i: usize) {
        if self.fields.is_empty() {
            return;
        }
        self.focus = i.min(self.fields.len() - 1);
        self.land_caret();
    }

    pub fn focus_next(&mut self) {
        self.focus = (self.focus + 1) % self.stops();
        self.land_caret();
    }

    pub fn focus_prev(&mut self) {
        self.focus = (self.focus + self.stops() - 1) % self.stops();
        self.land_caret();
    }

    /// The focused text field, if the focus is on an Input/Textarea.
    fn focused_text_field(&mut self) -> Option<&mut Field> {
        match self.focus_kind() {
            FocusKind::Field(i) if self.fields[i].is_text() => Some(&mut self.fields[i]),
            _ => None,
        }
    }

    /// Apply an edit to the focused text field via [`MultilineInput`] (the shared
    /// caret/text engine) and write the result back.
    fn edit(&mut self, op: impl FnOnce(&mut MultilineInput)) {
        let caret = self.caret;
        if let Some(field) = self.focused_text_field() {
            let mut mi = MultilineInput { text: std::mem::take(&mut field.value), cursor: caret };
            op(&mut mi);
            field.value = mi.text;
            let cur = mi.cursor;
            self.caret = cur;
            self.error = None; // any edit clears a stale required-field flag
        }
    }

    pub fn insert_char(&mut self, c: char) {
        self.edit(|mi| mi.insert_char(c));
    }
    pub fn backspace(&mut self) {
        self.edit(|mi| mi.backspace());
    }
    pub fn move_left(&mut self) {
        self.edit(|mi| mi.move_left());
    }
    pub fn move_right(&mut self) {
        self.edit(|mi| mi.move_right());
    }
    pub fn move_home(&mut self) {
        self.edit(|mi| mi.move_home());
    }
    pub fn move_end(&mut self) {
        self.edit(|mi| mi.move_end());
    }
    /// Vertical caret movement within the focused Textarea (logical lines).
    /// Inert off a Textarea — a single-line Input has no rows to move between.
    pub fn move_up(&mut self) {
        if self.is_textarea_focused() {
            self.edit(|mi| mi.move_up());
        }
    }
    pub fn move_down(&mut self) {
        if self.is_textarea_focused() {
            self.edit(|mi| mi.move_down());
        }
    }

    /// Whether the focused field is a Textarea (the only field with rows).
    fn is_textarea_focused(&self) -> bool {
        matches!(self.focus_kind(), FocusKind::Field(i) if matches!(self.fields[i].kind, FieldKind::Textarea))
    }
    /// Insert a pasted string into the focused text field. A Textarea takes it
    /// verbatim (newlines preserved); an Input collapses control chars to spaces
    /// so a multiline paste can't smuggle a newline into a one-line field. Inert
    /// off a text field.
    pub fn insert_str(&mut self, s: &str) {
        let (is_text, is_textarea) = match self.focus_kind() {
            FocusKind::Field(i) => (
                self.fields[i].is_text(),
                matches!(self.fields[i].kind, FieldKind::Textarea),
            ),
            _ => (false, false),
        };
        if !is_text {
            return;
        }
        let payload: String = if is_textarea {
            s.to_string()
        } else {
            s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect()
        };
        self.edit(|mi| mi.insert_str(&payload));
    }

    /// Newline — only meaningful for a Textarea (Input stays single-line).
    pub fn insert_newline(&mut self) {
        if let FocusKind::Field(i) = self.focus_kind()
            && matches!(self.fields[i].kind, FieldKind::Textarea)
        {
            self.edit(|mi| mi.insert_newline());
        }
    }

    /// The focused field's dropdown options, if the focus is on a Dropdown.
    fn focused_options(&self) -> Option<&[String]> {
        match self.focus_kind() {
            FocusKind::Field(i) => match &self.fields[i].kind {
                FieldKind::Dropdown { options } => Some(options),
                _ => None,
            },
            _ => None,
        }
    }

    pub fn is_dropdown_focused(&self) -> bool {
        self.focused_options().is_some()
    }

    /// Open the focused dropdown, highlighting its current value.
    pub fn open_dropdown(&mut self) {
        if let FocusKind::Field(i) = self.focus_kind()
            && let FieldKind::Dropdown { options } = &self.fields[i].kind
        {
            self.dropdown_index =
                options.iter().position(|o| *o == self.fields[i].value).unwrap_or(0);
            self.dropdown_open = true;
        }
    }

    pub fn close_dropdown(&mut self) {
        self.dropdown_open = false;
    }

    /// Move the open-dropdown highlight (clamped, non-wrapping).
    pub fn dropdown_move(&mut self, delta: i32) {
        let len = self.focused_options().map(<[String]>::len).unwrap_or(0);
        if len == 0 {
            return;
        }
        let next = (self.dropdown_index as i64 + delta as i64).clamp(0, len as i64 - 1) as usize;
        self.dropdown_index = next;
    }

    /// Commit the highlighted option to the focused dropdown's value and close.
    pub fn dropdown_pick(&mut self) {
        let idx = self.dropdown_index;
        if let FocusKind::Field(i) = self.focus_kind()
            && let FieldKind::Dropdown { options } = &self.fields[i].kind
            && let Some(opt) = options.get(idx)
        {
            let opt = opt.clone();
            self.fields[i].value = opt;
        }
        self.dropdown_open = false;
    }

    /// Validate: the first required field with an empty (trimmed) value fails,
    /// setting `error` and moving focus to it and returning `Err(index)`. On
    /// success returns the field values in declaration order.
    pub fn validate(&mut self) -> Result<Vec<String>, usize> {
        for (i, f) in self.fields.iter().enumerate() {
            if f.required && f.value.trim().is_empty() {
                self.error = Some(i);
                self.focus = i;
                self.dropdown_open = false;
                self.caret = f.value.chars().count();
                return Err(i);
            }
        }
        Ok(self.fields.iter().map(|f| f.value.clone()).collect())
    }
}

/// Render the form popup and register hit targets (`Modal` over the body,
/// `FormField(i)` per field box, `Button` via the row, `DropdownItem(i)` over an
/// open select's options — the option popup is drawn last so it is topmost).
pub fn render_form(frame: &mut ratatui::Frame, hit: &mut HitMap, state: &FormState) {
    let p = Palette::default();
    let area = frame.area();

    // Each field box: 1 label/top border + content + 1 bottom border, then a
    // 1-row gap. Interior = Σ(box_h + gap) + button row.
    let field_h = |f: &Field| f.box_content_height() + 2;
    let fields_h: u16 = state.fields.iter().map(|f| field_h(f) + 1).sum();
    let inner_h = fields_h + 1; // + button row
    let width = DIALOG_WIDTH.clamp(50.min(area.width.max(1)), area.width.saturating_sub(4).max(1));
    let height = (inner_h + 4).min(area.height.max(1)); // border(2) + padding(2)
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let rect = Rect { x, y, width, height };

    frame.render_widget(Clear, rect);
    hit.push(rect, HitTarget::Modal);

    let block = Block::default()
        .title(Span::styled(
            format!(" {} ", state.title),
            Style::default().fg(p.fg).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(p.accent))
        .padding(MODAL_PADDING);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    // Draw each field box top-to-bottom; remember the focused dropdown's box so
    // its option popup can anchor below it after the loop.
    let mut cursor_y = inner.y;
    let mut open_anchor: Option<(Rect, Vec<String>)> = None;
    for (i, f) in state.fields.iter().enumerate() {
        let focused = state.focus == i;
        let box_h = field_h(f);
        if cursor_y + box_h > inner.y + inner.height {
            break;
        }
        let box_rect = Rect { x: inner.x, y: cursor_y, width: inner.width, height: box_h };
        let is_err = state.error == Some(i);
        let border_col = if is_err {
            p.error
        } else if focused {
            p.accent
        } else {
            p.border
        };
        let label_style = if is_err {
            Style::default().fg(p.error).add_modifier(Modifier::BOLD)
        } else if focused {
            Style::default().fg(p.accent).add_modifier(Modifier::BOLD)
        } else {
            p.dim_style()
        };
        let title = if f.required && is_err {
            format!(" {} — required ", f.label)
        } else {
            format!(" {} ", f.label)
        };
        let fbox = Block::default()
            .title(Span::styled(title, label_style))
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(border_col));
        let content = fbox.inner(box_rect);
        frame.render_widget(fbox, box_rect);
        hit.push(box_rect, HitTarget::FormField(i));

        match &f.kind {
            FieldKind::Dropdown { options } => {
                // `value` left, `▾` right-aligned.
                let val = if f.value.is_empty() { "—" } else { f.value.as_str() };
                let chev = GLYPH_CHEVRON_DOWN.to_string();
                let gap = (content.width as usize)
                    .saturating_sub(val.chars().count() + chev.chars().count());
                let line = Line::from(vec![
                    Span::styled(val.to_string(), Style::default().fg(p.fg)),
                    Span::raw(" ".repeat(gap)),
                    Span::styled(chev, Style::default().fg(p.accent)),
                ]);
                frame.render_widget(Paragraph::new(line), content);
                if focused && state.dropdown_open {
                    open_anchor = Some((box_rect, options.clone()));
                }
            }
            _ => {
                // Text: wrap the value, window so the caret row stays visible, and
                // paint the caret on the focused field's caret row.
                let wrap_w = (content.width as usize).saturating_sub(1).max(1);
                let (lines, cur_row, cur_col) =
                    wrap_value_cursor(&f.value, state.caret, wrap_w);
                let rows = content.height as usize;
                let start = cur_row.saturating_sub(rows.saturating_sub(1));
                for (ri, line) in lines.iter().enumerate().skip(start).take(rows) {
                    let ly = content.y + (ri - start) as u16;
                    let lrect = Rect { x: content.x, y: ly, width: content.width, height: 1 };
                    if focused && ri == cur_row {
                        frame.render_widget(caret_line(line, cur_col, &p), lrect);
                    } else {
                        frame.render_widget(
                            Paragraph::new(Line::from(Span::styled(
                                line.clone(),
                                Style::default().fg(p.fg),
                            ))),
                            lrect,
                        );
                    }
                }
            }
        }
        cursor_y += box_h + 1;
    }

    // Button row on the last interior line.
    let btn_y = inner.y + inner.height.saturating_sub(1);
    render_button_row(
        frame,
        hit,
        Rect { x: inner.x, y: btn_y, width: inner.width, height: 1 },
        &state.primary_label,
        state.button_focus(),
        p.accent,
    );

    // Open dropdown: a bordered option popup just below its field box, topmost.
    if let Some((anchor, options)) = open_anchor {
        let list_h = (options.len() as u16 + 2).min(area.height.saturating_sub(anchor.y + anchor.height));
        if list_h >= 3 {
            let pop = Rect {
                x: anchor.x,
                y: anchor.y + anchor.height,
                width: anchor.width,
                height: list_h,
            };
            frame.render_widget(Clear, pop);
            hit.push(pop, HitTarget::Modal);
            let popblock = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(p.accent));
            let popinner = popblock.inner(pop);
            frame.render_widget(popblock, pop);
            for (row, opt) in options.iter().enumerate() {
                if row as u16 >= popinner.height {
                    break;
                }
                let rr =
                    Rect { x: popinner.x, y: popinner.y + row as u16, width: popinner.width, height: 1 };
                let style = if row == state.dropdown_index {
                    p.selection()
                } else {
                    Style::default().fg(p.fg)
                };
                hit.push(rr, HitTarget::DropdownItem(row));
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(format!(" {opt}"), style))),
                    rr,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hit::{HitMap, HitTarget};
    use ratatui::{backend::TestBackend, Terminal};

    fn sample() -> FormState {
        FormState::new(
            "＋ Create Worktree · platform",
            "Create",
            vec![
                Field::input("branch / worktree name", "", true),
                Field::dropdown(
                    "model",
                    vec!["fable".into(), "opus".into(), "sonnet".into(), "haiku".into()],
                    "opus",
                ),
                Field::textarea("prompt", "", true),
            ],
        )
    }

    /// Symbols of every REVERSED cell in a rendered form (the focused-button
    /// highlight uses REVERSED+BOLD; a text caret also reverses one cell).
    fn reversed_symbols(state: &FormState) -> String {
        let mut term = Terminal::new(TestBackend::new(80, 30)).unwrap();
        let mut hit = HitMap::default();
        term.draw(|f| render_form(f, &mut hit, state)).unwrap();
        let buf = term.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..30 {
            for x in 0..80 {
                let c = &buf[(x, y)];
                if c.modifier.contains(Modifier::REVERSED) {
                    out.push_str(c.symbol());
                }
            }
        }
        out
    }

    #[test]
    fn field_focus_highlights_no_button() {
        // With a field focused, NEITHER button renders as the focused (reversed)
        // button — otherwise the field box and a button both look focused.
        let mut f = sample(); // focus starts on field 0 (an input)
        let rev = reversed_symbols(&f);
        assert!(!rev.contains("Create"), "primary must not be focused while a field is: {rev:?}");
        assert!(!rev.contains("Cancel"), "cancel must not be focused while a field is: {rev:?}");
        // Focus the Primary button → now it (and only it) reverses.
        f.focus = f.fields.len();
        assert!(reversed_symbols(&f).contains("Create"), "primary reverses when focused");
        // Focus Cancel → it reverses, primary does not.
        f.focus = f.fields.len() + 1;
        let rev = reversed_symbols(&f);
        assert!(rev.contains("Cancel"));
        assert!(!rev.contains("Create"));
    }

    #[test]
    fn focus_cycles_fields_then_buttons() {
        let mut f = sample();
        assert_eq!(f.focus_kind(), FocusKind::Field(0));
        f.focus_next();
        assert_eq!(f.focus_kind(), FocusKind::Field(1));
        f.focus_next();
        assert_eq!(f.focus_kind(), FocusKind::Field(2));
        f.focus_next();
        assert_eq!(f.focus_kind(), FocusKind::Primary);
        assert_eq!(f.button_focus(), Some(ButtonKind::Confirm));
        f.focus_next();
        assert_eq!(f.focus_kind(), FocusKind::Cancel);
        assert_eq!(f.button_focus(), Some(ButtonKind::Cancel));
        f.focus_next();
        assert_eq!(f.focus_kind(), FocusKind::Field(0)); // wraps
        f.focus_prev();
        assert_eq!(f.focus_kind(), FocusKind::Cancel); // wraps back
    }

    #[test]
    fn typing_edits_focused_text_field_and_caret_follows() {
        let mut f = sample();
        for c in "feat/x".chars() {
            f.insert_char(c);
        }
        assert_eq!(f.fields[0].value, "feat/x");
        assert_eq!(f.caret, 6);
        f.backspace();
        assert_eq!(f.fields[0].value, "feat/");
        // Newline is inert on an Input, active on a Textarea.
        f.insert_newline();
        assert_eq!(f.fields[0].value, "feat/");
        f.focus = 2; // the prompt textarea
        f.land_caret();
        f.insert_char('a');
        f.insert_newline();
        f.insert_char('b');
        assert_eq!(f.fields[2].value, "a\nb");
    }

    #[test]
    fn dropdown_open_move_pick_sets_value() {
        let mut f = sample();
        f.focus = 1; // model dropdown
        f.land_caret();
        assert!(f.is_dropdown_focused());
        f.open_dropdown();
        assert!(f.dropdown_open);
        assert_eq!(f.dropdown_index, 1); // "opus" is index 1
        f.dropdown_move(1); // → sonnet
        f.dropdown_pick();
        assert_eq!(f.fields[1].value, "sonnet");
        assert!(!f.dropdown_open);
        // Clamp: cannot move below 0 or past the last option.
        f.open_dropdown();
        f.dropdown_move(-10);
        assert_eq!(f.dropdown_index, 0);
        f.dropdown_move(99);
        assert_eq!(f.dropdown_index, 3);
    }

    #[test]
    fn validate_flags_first_empty_required_field() {
        let mut f = sample();
        // branch name (0) and prompt (2) are required; both empty → fails on 0.
        assert_eq!(f.validate(), Err(0));
        assert_eq!(f.error, Some(0));
        assert_eq!(f.focus, 0);
        // Fill the name; now it fails on the prompt (2).
        for c in "feat/x".chars() {
            f.insert_char(c);
        }
        assert_eq!(f.validate(), Err(2));
        // Fill the prompt; validation passes and returns values in order.
        f.focus = 2;
        f.land_caret();
        for c in "do it".chars() {
            f.insert_char(c);
        }
        assert_eq!(f.validate(), Ok(vec!["feat/x".into(), "opus".into(), "do it".into()]));
    }

    fn render(f: &FormState, cols: u16, rows: u16) -> (String, HitMap) {
        let mut term = Terminal::new(TestBackend::new(cols, rows)).unwrap();
        let mut hit = HitMap::default();
        term.draw(|frame| render_form(frame, &mut hit, f)).unwrap();
        let buf = term.backend().buffer().clone();
        let mut s = String::new();
        for y in 0..rows {
            for x in 0..cols {
                s.push_str(buf[(x, y)].symbol());
            }
            s.push('\n');
        }
        (s, hit)
    }

    #[test]
    fn render_shows_fields_chevron_and_button_row() {
        let f = sample();
        let (s, hit) = render(&f, 70, 24);
        assert!(s.contains("Create Worktree"));
        assert!(s.contains("branch / worktree name"));
        assert!(s.contains("model"));
        assert!(s.contains("prompt"));
        assert!(s.contains('▾'), "dropdown chevron renders");
        assert!(s.contains("opus"), "dropdown shows its current value");
        assert!(s.contains("[ Create ]") && s.contains("[ Cancel ]"));
        let (mut f0, mut f1, mut f2, mut modal) = (false, false, false, false);
        for y in 0..24 {
            for x in 0..70 {
                match hit.hit(x, y) {
                    Some(HitTarget::FormField(0)) => f0 = true,
                    Some(HitTarget::FormField(1)) => f1 = true,
                    Some(HitTarget::FormField(2)) => f2 = true,
                    Some(HitTarget::Modal) => modal = true,
                    _ => {}
                }
            }
        }
        assert!(f0 && f1 && f2 && modal, "each field box + modal register hit targets");
    }

    #[test]
    fn render_open_dropdown_lists_options_and_registers_items() {
        let mut f = sample();
        f.focus = 1;
        f.open_dropdown();
        let (s, hit) = render(&f, 70, 24);
        assert!(s.contains("fable"));
        assert!(s.contains("sonnet"));
        assert!(s.contains("haiku"));
        let mut items = 0;
        for y in 0..24 {
            for x in 0..70 {
                if let Some(HitTarget::DropdownItem(_)) = hit.hit(x, y) {
                    items += 1;
                }
            }
        }
        assert!(items > 0, "open dropdown registers DropdownItem targets");
    }

    #[test]
    fn form_snapshot() {
        let mut f = sample();
        f.focus = 2; // prompt focused
        for c in "Redesign the dialogs".chars() {
            f.insert_char(c);
        }
        let (s, _hit) = render(&f, 64, 22);
        insta::assert_snapshot!("form_create_worktree", s);
    }
}
