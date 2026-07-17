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
use crate::view::multiline_input::{sanitize_paste, MultilineInput};
use crate::view::theme::{GLYPH_CHEVRON_DOWN, GLYPH_CHEVRON_RIGHT, Palette};

/// One selectable dropdown option: the `value` is what gets stored on the field
/// and submitted (e.g. a `provider/label` model ref, or `""` for a "leave unset"
/// head option); the `label` is what the closed field and the open list render
/// (e.g. `opus (claude)` or `default (…)`). For a plain dropdown the two are
/// equal — see [`Field::dropdown`]; the model picker uses distinct display via
/// [`Field::dropdown_labeled`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DropdownOption {
    pub value: String,
    pub label: String,
}

impl DropdownOption {
    /// A `value == label` option (the plain, self-describing case).
    pub fn plain(value: impl Into<String>) -> Self {
        let value = value.into();
        DropdownOption { label: value.clone(), value }
    }
}

/// The three field shapes. Shape alone signals the type — a one-row box is an
/// input, a three-row box a textarea, a `▾` a dropdown (no label tags needed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldKind {
    Input,
    Textarea,
    Dropdown { options: Vec<DropdownOption> },
    /// An editable text value (the typed filter/value) with an openable,
    /// FILTERED option list — type a worktree name to filter the seeded rows,
    /// or type a bare PR number / ticket id and pick the synthetic "use <ref>"
    /// row `combobox_filtered` offers. Renders like an Input with a `▾`.
    Combobox { options: Vec<String> },
    /// A non-editable, focus-STOP field whose value is a display label and whose
    /// activation (Enter / click) opens a SEPARATE modal — not an inline
    /// dropdown. The engine only renders it (label + a right `▸`) and treats it
    /// as a focus stop; the owning mode intercepts activation (the adhoc-create
    /// form uses it for session continuation → `Mode::SessionPick`). Typing/
    /// paste/caret are inert on it; `validate` never blocks on it.
    Picker,
}

/// One form field: a `label` (rendered as the box's border title), its `kind`,
/// the current `value`, whether it must be non-empty to submit, and whether it
/// is `readonly` — a display-only field (a fixed launch context value) that is
/// focus-skipped, never edited, and rendered dimmed.
#[derive(Debug, Clone)]
pub struct Field {
    pub label: String,
    pub kind: FieldKind,
    pub value: String,
    pub required: bool,
    pub readonly: bool,
}

impl Field {
    pub fn input(label: &str, value: &str, required: bool) -> Self {
        Field { label: label.into(), kind: FieldKind::Input, value: value.into(), required, readonly: false }
    }
    pub fn textarea(label: &str, value: &str, required: bool) -> Self {
        Field { label: label.into(), kind: FieldKind::Textarea, value: value.into(), required, readonly: false }
    }
    pub fn dropdown(label: &str, options: Vec<String>, value: &str) -> Self {
        Field {
            label: label.into(),
            kind: FieldKind::Dropdown { options: options.into_iter().map(DropdownOption::plain).collect() },
            value: value.into(),
            required: false,
            readonly: false,
        }
    }
    /// A dropdown whose options carry a display `label` distinct from their
    /// stored `value` — the model picker (value `provider/label`, display
    /// `label (provider)`; a `""`-valued head option displayed `default (…)`).
    /// `value` selects the option whose `value` matches.
    pub fn dropdown_labeled(label: &str, options: Vec<DropdownOption>, value: &str) -> Self {
        Field {
            label: label.into(),
            kind: FieldKind::Dropdown { options },
            value: value.into(),
            required: false,
            readonly: false,
        }
    }
    /// A type-or-pick field: an editable value seeded with an option list
    /// (e.g. the repo's worktree names) that filters as you type and offers a
    /// synthetic ref row for a typed PR number / ticket id. Never required.
    pub fn combobox(label: &str, options: Vec<String>, value: &str) -> Self {
        Field {
            label: label.into(),
            kind: FieldKind::Combobox { options },
            value: value.into(),
            required: false,
            readonly: false,
        }
    }
    /// A display-only field pre-filled with a fixed launch value (e.g. a source
    /// worktree the launch context nailed down). Focus skips it, edits/paste
    /// ignore it, validation never blocks on it, and it renders dimmed.
    pub fn readonly(label: &str, value: &str) -> Self {
        Field { label: label.into(), kind: FieldKind::Input, value: value.into(), required: false, readonly: true }
    }
    /// A focus-stop [`FieldKind::Picker`] whose activation opens a modal (handled
    /// by the owning mode). `value` is the display label. Never required; never
    /// text-edited.
    pub fn picker(label: &str, value: &str) -> Self {
        Field { label: label.into(), kind: FieldKind::Picker, value: value.into(), required: false, readonly: false }
    }
    /// Text-editable field kinds (Input, Textarea, and Combobox's value); a
    /// Dropdown value is set by picking, never typed.
    fn is_text(&self) -> bool {
        matches!(self.kind, FieldKind::Input | FieldKind::Textarea | FieldKind::Combobox { .. })
    }
    /// Fixed (non-value/width-aware) box content height — the floor a
    /// Textarea starts at and the height every other field kind keeps.
    /// `render_form` computes a Textarea's real auto-grow height separately
    /// via `textarea_rows` (which needs the rendered wrap width, unavailable
    /// here).
    fn box_content_height(&self) -> u16 {
        match self.kind {
            FieldKind::Textarea => 3,
            _ => 1,
        }
    }
}

/// A Textarea's auto-grow content height for `value` wrapped to `width`: its
/// wrapped row count, floored at the original fixed height (3) and capped at
/// `AUTOGROW_CAP`. Past the cap the field scrolls internally (the existing
/// caret-row windowing in `render_form`).
const AUTOGROW_CAP: u16 = 12;

pub(crate) fn textarea_rows(value: &str, width: usize) -> u16 {
    let rows = wrap_value_cursor(value, 0, width.max(1)).0.len() as u16;
    rows.clamp(3, AUTOGROW_CAP)
}

/// Human hint labeling a synthetic combobox ref row in the open popup:
/// `pr:45` → "use PR #45", `ticket:JUS-1756` → "use ticket JUS-1756".
fn ref_hint(r: &str) -> String {
    if let Some(n) = r.strip_prefix("pr:") {
        format!("use PR #{n}")
    } else if let Some(id) = r.strip_prefix("ticket:") {
        format!("use ticket {id}")
    } else {
        format!("use {r}")
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
    /// The last-rendered inner text width of the focused Textarea (the wrap
    /// width used for visual-line caret navigation). Set during layout by
    /// `render_form`/`render_fields`; read by `move_up`/`move_down`. Default
    /// `40` keeps navigation sane before the first render.
    pub content_width: usize,
}

impl FormState {
    /// Build a form; focus starts on the first NON-readonly field (readonly
    /// fields are focus-skipped) with the caret at its end.
    pub fn new(title: &str, primary_label: &str, fields: Vec<Field>) -> Self {
        let focus = fields.iter().position(|f| !f.readonly).unwrap_or(0);
        let caret = fields.get(focus).map(|f| f.value.chars().count()).unwrap_or(0);
        FormState {
            title: title.into(),
            primary_label: primary_label.into(),
            fields,
            focus,
            caret,
            dropdown_open: false,
            dropdown_index: 0,
            error: None,
            content_width: 40,
        }
    }

    /// Cache the focused Textarea's rendered content width, driving visual-line
    /// `move_up`/`move_down` navigation. Called during layout.
    pub fn set_content_width(&mut self, w: usize) {
        self.content_width = w.max(1);
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

    /// Whether `focus` (a `0..stops()` index) is a valid focus stop: the two
    /// buttons always are; a field only when it is not readonly.
    fn is_stop(&self, focus: usize) -> bool {
        focus >= self.fields.len() || !self.fields[focus].readonly
    }

    /// Focus the field at `i` (clamped to a real field), parking the caret and
    /// closing any open dropdown. A readonly field is inert (focus stays put).
    /// Used by click routing.
    pub fn focus_field(&mut self, i: usize) {
        if self.fields.is_empty() {
            return;
        }
        let i = i.min(self.fields.len() - 1);
        if self.fields[i].readonly {
            return;
        }
        self.focus = i;
        self.land_caret();
    }

    pub fn focus_next(&mut self) {
        let n = self.stops();
        let mut next = (self.focus + 1) % n;
        for _ in 0..n {
            if self.is_stop(next) {
                break;
            }
            next = (next + 1) % n;
        }
        self.focus = next;
        self.land_caret();
    }

    pub fn focus_prev(&mut self) {
        let n = self.stops();
        let mut next = (self.focus + n - 1) % n;
        for _ in 0..n {
            if self.is_stop(next) {
                break;
            }
            next = (next + n - 1) % n;
        }
        self.focus = next;
        self.land_caret();
    }

    /// The focused text field, if the focus is on an editable Input/Textarea
    /// (readonly fields are never editable).
    fn focused_text_field(&mut self) -> Option<&mut Field> {
        match self.focus_kind() {
            FocusKind::Field(i) if self.fields[i].is_text() && !self.fields[i].readonly => {
                Some(&mut self.fields[i])
            }
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
    /// Vertical caret movement within the focused Textarea — moves by one
    /// VISUAL (wrapped) row at the cached `content_width`. Inert off a
    /// Textarea — a single-line Input has no rows to move between.
    pub fn move_up(&mut self) {
        if self.is_textarea_focused() {
            let w = self.content_width;
            self.edit(|mi| mi.move_up_visual(w));
        }
    }
    pub fn move_down(&mut self) {
        if self.is_textarea_focused() {
            let w = self.content_width;
            self.edit(|mi| mi.move_down_visual(w));
        }
    }

    /// Whether the focused field is a Textarea (the only field with rows).
    fn is_textarea_focused(&self) -> bool {
        matches!(self.focus_kind(), FocusKind::Field(i) if matches!(self.fields[i].kind, FieldKind::Textarea))
    }
    /// Insert a pasted string into the focused text field, sanitized via
    /// [`sanitize_paste`] (CR/CRLF → `\n`, tabs expanded, other control chars
    /// dropped — unrenderable chars in a value garble the wrap math). A
    /// Textarea keeps the line structure; an Input then flattens each line
    /// break to a space so a multiline paste can't smuggle a newline into a
    /// one-line field. Inert off a text field.
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
        let mut payload = sanitize_paste(s);
        if !is_textarea {
            payload = payload.replace('\n', " ");
        }
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

    /// Overwrite field `i`'s value, parking the caret at its end. Used by the
    /// combobox key path's helpers and tests.
    pub fn set_field_value(&mut self, i: usize, value: &str) {
        if let Some(f) = self.fields.get_mut(i) {
            f.value = value.into();
            if self.focus == i {
                self.caret = f.value.chars().count();
            }
        }
    }

    /// The focused field's dropdown options, if the focus is on a Dropdown.
    fn focused_options(&self) -> Option<&[DropdownOption]> {
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

    /// Whether the focused field is a Combobox (a type-or-pick select).
    pub fn is_combobox_focused(&self) -> bool {
        matches!(self.focus_kind(), FocusKind::Field(i) if matches!(self.fields[i].kind, FieldKind::Combobox { .. }))
    }

    /// Whether the focused field is a [`FieldKind::Picker`] (modal-opening).
    pub fn is_picker_focused(&self) -> bool {
        matches!(self.focus_kind(), FocusKind::Field(i) if matches!(self.fields[i].kind, FieldKind::Picker))
    }

    /// The FILTERED option rows for the focused Combobox, in display order:
    /// every seeded option whose text contains the typed value (case-
    /// insensitive), each paired with its original option index, PLUS a
    /// synthetic ref row `(usize::MAX, "<ref>")` when `classify_ref(value)` is
    /// `Some` and no seeded option already equals that ref. `usize::MAX` marks
    /// the synthetic row so the renderer can label it ("← use PR #45"). Empty
    /// (or an empty vec) off a Combobox.
    pub fn combobox_filtered(&self) -> Vec<(usize, String)> {
        let FocusKind::Field(i) = self.focus_kind() else { return Vec::new() };
        let FieldKind::Combobox { options } = &self.fields[i].kind else { return Vec::new() };
        let needle = self.fields[i].value.to_ascii_lowercase();
        let mut out: Vec<(usize, String)> = options
            .iter()
            .enumerate()
            .filter(|(_, o)| o.to_ascii_lowercase().contains(&needle))
            .map(|(idx, o)| (idx, o.clone()))
            .collect();
        if let Some(r) = crate::ref_classify::classify_ref(&self.fields[i].value)
            && !options.contains(&r)
        {
            out.push((usize::MAX, r));
        }
        out
    }

    /// Open the focused select. A Dropdown highlights its current value; a
    /// Combobox highlights the first FILTERED row (the list changes as you
    /// type, so there is no stable "current" row).
    pub fn open_dropdown(&mut self) {
        if let FocusKind::Field(i) = self.focus_kind() {
            match &self.fields[i].kind {
                FieldKind::Dropdown { options } => {
                    self.dropdown_index =
                        options.iter().position(|o| o.value == self.fields[i].value).unwrap_or(0);
                    self.dropdown_open = true;
                }
                FieldKind::Combobox { .. } => {
                    self.dropdown_index = 0;
                    self.dropdown_open = true;
                }
                _ => {}
            }
        }
    }

    pub fn close_dropdown(&mut self) {
        self.dropdown_open = false;
    }

    /// Number of rows the open select highlight ranges over: a Dropdown's option
    /// count, else a Combobox's FILTERED row count.
    fn open_list_len(&self) -> usize {
        if let Some(opts) = self.focused_options() {
            opts.len()
        } else if self.is_combobox_focused() {
            self.combobox_filtered().len()
        } else {
            0
        }
    }

    /// Move the open-select highlight (clamped, non-wrapping) over a Dropdown's
    /// options or a Combobox's filtered rows.
    pub fn dropdown_move(&mut self, delta: i32) {
        let len = self.open_list_len();
        if len == 0 {
            return;
        }
        let next = (self.dropdown_index as i64 + delta as i64).clamp(0, len as i64 - 1) as usize;
        self.dropdown_index = next;
    }

    /// Commit the highlighted row to the focused select's value and close: a
    /// Dropdown writes the option; a Combobox writes the highlighted FILTERED
    /// string (a seeded option, or the classified ref for the synthetic row).
    pub fn dropdown_pick(&mut self) {
        let idx = self.dropdown_index;
        let FocusKind::Field(i) = self.focus_kind() else {
            self.dropdown_open = false;
            return;
        };
        match &self.fields[i].kind {
            FieldKind::Dropdown { options } => {
                if let Some(opt) = options.get(idx) {
                    self.fields[i].value = opt.value.clone();
                }
            }
            FieldKind::Combobox { .. } => {
                if let Some((_, s)) = self.combobox_filtered().get(idx) {
                    let s = s.clone();
                    self.fields[i].value = s;
                    self.caret = self.fields[i].value.chars().count();
                }
            }
            _ => {}
        }
        self.dropdown_open = false;
    }

    /// Validate: the first required field with an empty (trimmed) value fails,
    /// setting `error` and moving focus to it and returning `Err(index)`. On
    /// success returns the field values in declaration order.
    pub fn validate(&mut self) -> Result<Vec<String>, usize> {
        for (i, f) in self.fields.iter().enumerate() {
            if f.readonly {
                continue; // display-only: never blocks submit
            }
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

/// Per-field CONTENT heights (rows inside each box, excluding its border) for a
/// fields area `avail` rows tall and `wrap_w` text width. Non-textarea fields
/// keep their fixed content height; every Textarea grows from its 3-row floor
/// toward its wrapped row count, but only into the space left after the OTHER
/// fields' boxes, gaps, and the button row — so a large textarea never clips a
/// sibling field or the button row (the Phase 1 auto-grow carry-forward rule).
/// With generous `avail` (the render_form sizing pass) every textarea reaches
/// its full desired height; with a tight `avail` (a fixed panel) the leftover
/// rows are shared out.
fn distribute_field_content_heights(fields: &[Field], wrap_w: usize, avail: u16) -> Vec<u16> {
    let n = fields.len() as u16;
    // Each field box costs 2 border rows + 1 trailing gap row on top of its
    // content, so the rows available for CONTENT are `avail − 3·n`.
    let overhead = 3u16.saturating_mul(n);
    let budget = avail.saturating_sub(overhead);
    let mut heights: Vec<u16> = fields
        .iter()
        .map(|f| match f.kind {
            FieldKind::Textarea => 3, // floor; grown below
            _ => f.box_content_height(),
        })
        .collect();
    let used: u16 = heights.iter().sum();
    let mut slack = budget.saturating_sub(used);
    for (i, f) in fields.iter().enumerate() {
        if matches!(f.kind, FieldKind::Textarea) && slack > 0 {
            let grow = textarea_rows(&f.value, wrap_w).saturating_sub(3).min(slack);
            heights[i] += grow;
            slack -= grow;
        }
    }
    heights
}

/// Render the form popup and register hit targets (`Modal` over the body,
/// `FormField(i)` per field box, `Button` via the row, `DropdownItem(i)` over an
/// open select's options — the option popup is drawn last so it is topmost).
/// Modal chrome + button row live here; the field boxes and the open dropdown
/// popup are drawn by the shared [`render_fields`]/[`render_open_dropdown`],
/// which the two-panel def-args shell reuses. Takes `state` mutably so
/// `render_fields` can cache the focused Textarea's rendered content width
/// (`FormState::set_content_width`) for visual-line `move_up`/`move_down`.
pub fn render_form(frame: &mut ratatui::Frame, hit: &mut HitMap, state: &mut FormState) {
    let p = Palette::default();
    let area = frame.area();

    let width = DIALOG_WIDTH.clamp(50.min(area.width.max(1)), area.width.saturating_sub(4).max(1));

    // Every field box shares the same inner content width — a fixed function
    // of the dialog width (outer border+padding, then the field's own
    // Rounded border). It doesn't depend on the dialog's height, so measure
    // it once via a scratch rect (width-only geometry, no title/border-style
    // needed) BEFORE the dialog height — which a Textarea's auto-grow content
    // height feeds into — is known.
    let scratch_inner_w =
        Block::default().borders(Borders::ALL).padding(MODAL_PADDING).inner(Rect {
            x: 0,
            y: 0,
            width,
            height: 3,
        }).width;
    let field_content_w = scratch_inner_w.saturating_sub(2); // the field box's own border
    let wrap_w = (field_content_w as usize).saturating_sub(1).max(1); // caret reserve

    // Size the dialog to the fields' DESIRED heights (a generous `avail` lets
    // every textarea reach its wrapped row count, capped only by `AUTOGROW_CAP`
    // inside `textarea_rows`). `render_fields` re-distributes within the final
    // (possibly frame-clamped) interior, so a too-tall dialog shrinks rather
    // than clips.
    let content_heights =
        distribute_field_content_heights(&state.fields, wrap_w, area.height);
    // Each field box: 1 label/top border + content + 1 bottom border, then a
    // 1-row gap. Interior = Σ(box_h + gap) + button row.
    let field_h = |i: usize| content_heights[i] + 2;
    let fields_h: u16 = (0..state.fields.len()).map(|i| field_h(i) + 1).sum();
    let inner_h = fields_h + 1; // + button row
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

    // Fields fill the interior above the bottom button row.
    let fields_area = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: inner.height.saturating_sub(1),
    };
    let open_anchor = render_fields(frame, hit, state, fields_area);

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

    // Open dropdown popup last so it is topmost.
    if let Some((anchor, options)) = open_anchor {
        render_open_dropdown(frame, hit, state, area, anchor, options);
    }
}

/// Draw every field box top-to-bottom into `inner` (reserving no button row —
/// the caller does that), registering a `FormField(i)` hit target per box and
/// painting the caret on the focused text field. Caches the focused Textarea's
/// wrap width onto `state` (visual-line nav). Returns the focused-and-open
/// dropdown's anchor box + its option list (else `None`) so the caller can draw
/// the option popup last (topmost). Shared by [`render_form`] (centered modal)
/// and the two-panel def-args shell.
pub(crate) fn render_fields(
    frame: &mut ratatui::Frame,
    hit: &mut HitMap,
    state: &mut FormState,
    inner: Rect,
) -> Option<(Rect, Vec<String>)> {
    let p = Palette::default();
    if inner.width == 0 || inner.height == 0 {
        return None;
    }
    // The field box's own border eats 2 columns; reserve 1 more for the caret.
    let wrap_w = (inner.width as usize).saturating_sub(2).saturating_sub(1).max(1);
    let content_heights =
        distribute_field_content_heights(&state.fields, wrap_w, inner.height);
    let field_h = |i: usize| content_heights[i] + 2;

    let mut cursor_y = inner.y;
    let mut open_anchor: Option<(Rect, Vec<String>)> = None;
    let mut focused_wrap_w: Option<usize> = None;
    for (i, f) in state.fields.iter().enumerate() {
        let focused = state.focus == i;
        let box_h = field_h(i);
        if cursor_y + box_h > inner.y + inner.height {
            break;
        }
        let box_rect = Rect { x: inner.x, y: cursor_y, width: inner.width, height: box_h };
        let is_err = state.error == Some(i);
        let border_col = if is_err {
            p.error
        } else if focused {
            p.accent
        } else if f.readonly {
            p.dim
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
                // The selected option's DISPLAY label left (falling back to the raw
                // value, then `—` when empty and unmatched), `▾` right-aligned.
                let val = options
                    .iter()
                    .find(|o| o.value == f.value)
                    .map(|o| o.label.as_str())
                    .unwrap_or(if f.value.is_empty() { "—" } else { f.value.as_str() });
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
                    // The open list shows each option's display label; the row
                    // order matches `options`, so the highlight index and the
                    // picked value stay aligned.
                    let labels = options.iter().map(|o| o.label.clone()).collect();
                    open_anchor = Some((box_rect, labels));
                }
            }
            FieldKind::Picker => {
                // `value` (display label) left, a right-`▸` on the right — the
                // affordance that activating opens a modal, not an inline list.
                let val = if f.value.is_empty() { "—" } else { f.value.as_str() };
                let chev = GLYPH_CHEVRON_RIGHT.to_string();
                let gap = (content.width as usize)
                    .saturating_sub(val.chars().count() + chev.chars().count());
                let line = Line::from(vec![
                    Span::styled(val.to_string(), Style::default().fg(p.fg)),
                    Span::raw(" ".repeat(gap)),
                    Span::styled(chev, Style::default().fg(p.accent)),
                ]);
                frame.render_widget(Paragraph::new(line), content);
            }
            FieldKind::Combobox { .. } => {
                // Text-field path (value + caret) with the right-aligned `▾`; the
                // rightmost 2 cols are reserved for " ▾" so the chevron never
                // overlaps the value or its caret.
                let chev = GLYPH_CHEVRON_DOWN.to_string();
                let text_w = content.width.saturating_sub(2);
                let wrap_w = (text_w as usize).saturating_sub(1).max(1);
                if focused {
                    focused_wrap_w = Some(wrap_w);
                }
                let (lines, cur_row, cur_col) = wrap_value_cursor(&f.value, state.caret, wrap_w);
                let rows = content.height as usize;
                let start = cur_row.saturating_sub(rows.saturating_sub(1));
                for (ri, line) in lines.iter().enumerate().skip(start).take(rows) {
                    let ly = content.y + (ri - start) as u16;
                    let lrect = Rect { x: content.x, y: ly, width: text_w, height: 1 };
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
                let chev_rect = Rect {
                    x: content.x + content.width.saturating_sub(1),
                    y: content.y,
                    width: 1,
                    height: 1,
                };
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(chev, Style::default().fg(p.accent)))),
                    chev_rect,
                );
                if focused && state.dropdown_open {
                    // The FILTERED rows; label the synthetic ref row so the value
                    // ("pr:45") reads with its meaning ("← use PR #45"). The
                    // labeled display order matches `combobox_filtered`, so the
                    // highlight index and the pick value stay aligned.
                    let labeled: Vec<String> = state
                        .combobox_filtered()
                        .into_iter()
                        .map(|(idx, s)| {
                            if idx == usize::MAX {
                                format!("{s}   ← {}", ref_hint(&s))
                            } else {
                                s
                            }
                        })
                        .collect();
                    open_anchor = Some((box_rect, labeled));
                }
            }
            _ => {
                // Text: wrap the value, window so the caret row stays visible, and
                // paint the caret on the focused field's caret row.
                let wrap_w = (content.width as usize).saturating_sub(1).max(1);
                if focused {
                    focused_wrap_w = Some(wrap_w);
                }
                let (lines, cur_row, cur_col) =
                    wrap_value_cursor(&f.value, state.caret, wrap_w);
                let text_style =
                    if f.readonly { p.dim_style() } else { Style::default().fg(p.fg) };
                let rows = content.height as usize;
                let start = cur_row.saturating_sub(rows.saturating_sub(1));
                for (ri, line) in lines.iter().enumerate().skip(start).take(rows) {
                    let ly = content.y + (ri - start) as u16;
                    let lrect = Rect { x: content.x, y: ly, width: content.width, height: 1 };
                    if focused && ri == cur_row {
                        frame.render_widget(caret_line(line, cur_col, &p), lrect);
                    } else {
                        frame.render_widget(
                            Paragraph::new(Line::from(Span::styled(line.clone(), text_style))),
                            lrect,
                        );
                    }
                }
            }
        }
        cursor_y += box_h + 1;
    }
    if let Some(w) = focused_wrap_w {
        state.set_content_width(w);
    }
    open_anchor
}

/// Draw the open dropdown's bordered option popup just below its `anchor` box,
/// topmost (`Clear` + a `Modal` region so clicks can't leak), registering a
/// `DropdownItem(i)` per option row and highlighting `state.dropdown_index`.
/// `area` is the frame rect (clamps the popup height). Shared by the form and
/// def-args shells.
pub(crate) fn render_open_dropdown(
    frame: &mut ratatui::Frame,
    hit: &mut HitMap,
    state: &FormState,
    area: Rect,
    anchor: Rect,
    options: Vec<String>,
) {
    let p = Palette::default();
    let list_h =
        (options.len() as u16 + 2).min(area.height.saturating_sub(anchor.y + anchor.height));
    if list_h < 3 {
        return;
    }
    let pop = Rect { x: anchor.x, y: anchor.y + anchor.height, width: anchor.width, height: list_h };
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
        let rr = Rect { x: popinner.x, y: popinner.y + row as u16, width: popinner.width, height: 1 };
        let style = if row == state.dropdown_index { p.selection() } else { Style::default().fg(p.fg) };
        hit.push(rr, HitTarget::DropdownItem(row));
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(format!(" {opt}"), style))),
            rr,
        );
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
    fn reversed_symbols(state: &mut FormState) -> String {
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
        let rev = reversed_symbols(&mut f);
        assert!(!rev.contains("Create"), "primary must not be focused while a field is: {rev:?}");
        assert!(!rev.contains("Cancel"), "cancel must not be focused while a field is: {rev:?}");
        // Focus the Primary button → now it (and only it) reverses.
        f.focus = f.fields.len();
        assert!(reversed_symbols(&mut f).contains("Create"), "primary reverses when focused");
        // Focus Cancel → it reverses, primary does not.
        f.focus = f.fields.len() + 1;
        let rev = reversed_symbols(&mut f);
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
    fn combobox_filters_and_accepts_typed_ref() {
        let mut f = FormState::new("t","OK", vec![Field::combobox(
            "target", vec!["JUS-1756".into(),"acme".into()], "")]);
        f.focus = 0;
        for c in "ac".chars() { f.insert_char(c); }
        let view = f.combobox_filtered();
        assert!(view.iter().any(|(_, s)| s == "acme"));
        // typing a bare number offers a pr ref row even with no matching worktree
        f.set_field_value(0, "45");
        let view = f.combobox_filtered();
        assert!(view.iter().any(|(_, s)| s == "pr:45"));
    }

    #[test]
    fn picker_is_a_focus_stop_but_not_text_editable() {
        // A Picker field participates in Tab focus (so it can be activated) but
        // typing/newline never mutate it, and validate never blocks on it.
        let mut f = FormState::new("t", "OK", vec![
            Field::combobox("target", vec![], ""),
            Field::picker("session", "New session"),
            Field::textarea("prompt", "", true),
        ]);
        assert_eq!(f.focus_kind(), FocusKind::Field(0));
        f.focus_next(); // → the Picker (a focus stop)
        assert_eq!(f.focus_kind(), FocusKind::Field(1));
        assert!(f.is_picker_focused());
        f.insert_char('x'); // inert on a Picker
        assert_eq!(f.fields[1].value, "New session");
        f.insert_newline(); // inert
        assert_eq!(f.fields[1].value, "New session");
        // Only the required empty prompt blocks validation, never the Picker.
        assert_eq!(f.validate(), Err(2));
    }

    #[test]
    fn picker_renders_value_and_right_chevron() {
        let mut f = FormState::new("t", "OK", vec![Field::picker("session", "↻ Fix parser")]);
        let (s, _hit) = render(&mut f, 64, 12);
        assert!(s.contains("session"), "picker label renders");
        assert!(s.contains("Fix parser"), "picker value renders");
        assert!(s.contains('▸'), "picker draws the right chevron affordance");
    }

    #[test]
    fn readonly_fields_are_focus_skipped_and_not_edited() {
        let mut f = FormState::new("t", "OK", vec![
            Field::readonly("target", "JUS-1"),
            Field::input("name", "", true),
        ]);
        assert_eq!(f.focus_kind(), FocusKind::Field(1)); // starts past the readonly
        f.insert_char('x'); // edits field 1, not the readonly
        assert_eq!(f.fields[1].value, "x");
        f.focus_next(); // → Primary (skips back over readonly on wrap too)
        assert_eq!(f.focus_kind(), FocusKind::Primary);
        f.focus_next(); // → Cancel
        f.focus_next(); // → wraps to field 1 (skips readonly field 0)
        assert_eq!(f.focus_kind(), FocusKind::Field(1));
    }

    #[test]
    fn textarea_vertical_nav_is_visual_at_cached_width() {
        let mut f = FormState::new("t", "OK", vec![Field::textarea("p", "abcdefghij", true)]);
        f.focus = 0;
        f.set_content_width(4); // rows: abcd/efgh/ij
        f.caret = 9;            // 'j', visual row 2 col 1
        f.move_up(); // → visual row1 col1 → index 5
        assert_eq!(f.caret, 5);
    }

    #[test]
    fn textarea_autogrows_with_content() {
        // helper: content rows for a value at width w
        assert_eq!(textarea_rows("a\nb\nc\nd\ne\nf", 40), 6); // 6 logical lines
        assert_eq!(textarea_rows("", 40), 3);                 // floor at 3
        assert_eq!(textarea_rows("x", 40), 3);
    }

    #[test]
    fn paste_normalizes_cr_line_endings_and_tabs_in_textarea() {
        // Terminals translate `\n` → `\r` in bracketed paste (they emulate the
        // Enter key), so a multiline paste arrives CR-separated. The textarea
        // must keep the LINE STRUCTURE (CR/CRLF → `\n`), expand tabs, and drop
        // other control chars — the renderer skips control chars it can't
        // draw, so letting them into the value garbles the wrap math.
        let mut f = sample();
        f.focus = 2; // the prompt textarea
        f.land_caret();
        f.insert_str("line one\r\nline two\rline three\tend\u{1b}[31m");
        assert_eq!(f.fields[2].value, "line one\nline two\nline three    end[31m");
        assert_eq!(f.caret, f.fields[2].value.chars().count());
    }

    #[test]
    fn paste_collapses_a_crlf_to_one_space_in_input() {
        // A single-line Input flattens line breaks; a CRLF pair is ONE break,
        // so it must become one space, not two.
        let mut f = sample(); // focus starts on field 0 (an input)
        f.insert_str("a\r\nb\rc");
        assert_eq!(f.fields[0].value, "a b c");
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

    fn render(f: &mut FormState, cols: u16, rows: u16) -> (String, HitMap) {
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
        let mut f = sample();
        let (s, hit) = render(&mut f, 70, 24);
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
        let (s, hit) = render(&mut f, 70, 24);
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
        let (s, _hit) = render(&mut f, 64, 22);
        insta::assert_snapshot!("form_create_worktree", s);
    }

    #[test]
    fn combobox_open_typed_ref() {
        // A worktree-seeded combobox with `45` typed: the option list filters to
        // the worktree containing "45" PLUS the synthetic labeled `pr:45` row.
        let mut f = FormState::new(
            "＋ Run · platform",
            "Run",
            vec![Field::combobox("target", vec!["feat-45".into(), "acme".into()], "")],
        );
        f.focus = 0;
        for c in "45".chars() {
            f.insert_char(c);
        }
        f.open_dropdown();
        let (s, _hit) = render(&mut f, 64, 16);
        assert!(s.contains('▾'), "combobox renders the chevron: {s}");
        assert!(s.contains("feat-45"), "the matching worktree lists");
        assert!(s.contains("pr:45"), "the synthetic ref row lists");
        assert!(s.contains("use PR #45"), "the synthetic ref row is labeled");
        insta::assert_snapshot!("combobox_open_typed_ref", s);
    }

    #[test]
    fn form_autogrow_snapshot() {
        // A multi-line prompt grows the Textarea box past its 3-row floor —
        // pins the taller rendered height (vs `form_snapshot`'s single line).
        let mut f = sample();
        f.focus = 2; // prompt focused
        f.insert_str("first line\nsecond line\nthird line\nfourth line\nfifth line");
        let (s, _hit) = render(&mut f, 64, 26);
        insta::assert_snapshot!("form_autogrow", s);
    }
}
