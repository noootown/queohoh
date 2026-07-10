//! Per-def argument entry form. This task (18) introduces the `ArgsForm` struct
//! + constructor (its first consumer is the def-pick / RunNamedDef flow); the
//! interaction methods (focus/cycle/edit/validate/dropdown) and the real render
//! land in Task 19/20.

use std::collections::HashMap;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear};

use crate::hit::{ButtonKind, HitMap, HitTarget};
use crate::ipc::types::ArgSpec;
use crate::view::modal::modal_frame;
use crate::view::theme::Palette;

/// Per-arg form state. Interaction methods (focus/cycle/edit/validate/dropdown)
/// are added in Task 19; this file introduces the struct + constructor.
#[derive(Debug, Clone)]
pub struct ArgsForm {
    pub repo: String,
    pub def_name: String,
    pub args: Vec<ArgSpec>,
    pub values: Vec<String>,
    pub fixed: HashMap<String, String>,
    pub initial_worktree: Option<String>,
    pub focus: usize,
    pub error: Option<usize>,
    pub dropdown: Option<usize>, // highlighted option index while a dropdown is open
}

/// True when the arg carries a non-empty `options` list. Shared by `new` and
/// (Task 19) the public `is_enum`.
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
    if arg_is_enum(arg) {
        if let Some(first) = arg.options.as_ref().and_then(|o| o.first()) {
            return first.clone();
        }
    }
    String::new()
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
        let values = args.iter().map(|a| initial_value(a, &fixed, &initial)).collect();
        let first_editable = args.iter().position(|a| !fixed.contains_key(&a.name));
        ArgsForm {
            repo,
            def_name,
            args,
            values,
            fixed,
            initial_worktree: worktree,
            focus: first_editable.unwrap_or(0),
            error: None,
            dropdown: None,
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
    }
    pub fn next_focus(&mut self) {
        self.step_focus(1);
    }
    pub fn prev_focus(&mut self) {
        self.step_focus(-1);
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

    pub fn input_char(&mut self, c: char) {
        let i = self.focus;
        if self.is_fixed(i) || self.is_enum(i) {
            return;
        }
        if let Some(v) = self.values.get_mut(i) {
            v.push(c);
        }
        if self.error == Some(i) {
            self.error = None;
        }
    }
    pub fn backspace(&mut self) {
        let i = self.focus;
        if self.is_fixed(i) || self.is_enum(i) {
            return;
        }
        if let Some(v) = self.values.get_mut(i) {
            v.pop();
        }
        if self.error == Some(i) {
            self.error = None;
        }
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
            if self.error == Some(i) {
                self.error = None;
            }
        }
        self.dropdown = None;
    }
}

/// Right-pad (or truncate) `s` to exactly `width` display columns.
fn pad(s: &str, width: usize) -> String {
    let mut out: String = s.chars().take(width).collect();
    while out.chars().count() < width {
        out.push(' ');
    }
    out
}

/// Dimmed hint for a row: `opt1 | opt2 — description` (enum) or the description.
fn row_hint(arg: &ArgSpec) -> String {
    if arg_is_enum(arg) {
        let opts = arg.options.as_ref().map(|o| o.join(" | ")).unwrap_or_default();
        match &arg.description {
            Some(d) => format!("{opts} — {d}"),
            None => opts,
        }
    } else {
        arg.description.clone().unwrap_or_default()
    }
}

/// Render the args form inside a centered modal: one row per arg (label ·
/// value · hint columns), a key hint, and `[ Run ] [ Cancel ]`. Registers a
/// `Modal` body (opaque to the panes beneath), a `FormField(i)` per editable
/// row, `Button` targets, and — when open — the dropdown popup last (topmost).
pub fn render_args_form(frame: &mut Frame, hit: &mut HitMap, p: &Palette, form: &ArgsForm) {
    let title = format!("{} args", form.def_name);
    // Interior lines: one per arg, then a key-hint line, then the buttons line.
    let mut height = form.args.len() as u16 + 2;
    // Grow to fit an open dropdown so its whole option list stays inside the
    // interior (its box hangs below the focused row: `focus+1` .. `+opts+2`).
    if form.dropdown.is_some() {
        if let Some(opts) = form.args.get(form.focus).and_then(|a| a.options.as_ref()) {
            height = height.max(form.focus as u16 + opts.len() as u16 + 3);
        }
    }
    let inner: Rect = modal_frame(frame, frame.area(), &title, height);

    // Popup body (interior grown by the border ring) so clicks are opaque to the
    // panes beneath. Registered first so FormField/Button/Dropdown targets win.
    let body = Rect {
        x: inner.x.saturating_sub(1),
        y: inner.y.saturating_sub(1),
        width: inner.width + 2,
        height: inner.height + 2,
    };
    hit.push(body, HitTarget::Modal);

    let width = inner.width as usize;
    let hint_col = (width / 2).min(40);
    let main_col = width.saturating_sub(hint_col).max(1);
    let label_w = form.args.iter().map(|a| a.name.chars().count()).max().unwrap_or(0);

    for (i, arg) in form.args.iter().enumerate() {
        if i as u16 >= inner.height.saturating_sub(2) {
            break; // reserve the last two interior lines for the hint + buttons
        }
        let row = Rect { x: inner.x, y: inner.y + i as u16, width: inner.width, height: 1 };
        let fixed = form.is_fixed(i);
        let focused = i == form.focus && !fixed && form.dropdown.is_none();
        let value = form.values.get(i).cloned().unwrap_or_default();
        let label = pad(&format!("{}>", arg.name), label_w + 1);
        let shown = if form.is_enum(i) && !fixed { format!("‹{value}›") } else { value };
        let cursor = if focused { "█" } else { "" };
        let main = format!(" {label} {shown}{cursor}");
        let main_style = if focused {
            Style::default().add_modifier(Modifier::REVERSED)
        } else if fixed {
            Style::default().fg(p.dim).add_modifier(Modifier::DIM)
        } else {
            Style::default().fg(p.fg)
        };
        let hint = if form.error == Some(i) {
            " required".to_string()
        } else if hint_col > 0 {
            format!(" {}", row_hint(arg))
        } else {
            String::new()
        };
        let hint_style = if form.error == Some(i) {
            Style::default().fg(p.error)
        } else {
            Style::default().fg(p.dim).add_modifier(Modifier::DIM)
        };
        let mut spans = vec![Span::styled(pad(&main, main_col), main_style)];
        if hint_col > 0 {
            spans.push(Span::styled(pad(&hint, hint_col), hint_style));
        }
        frame.render_widget(Line::from(spans), row);
        if !fixed {
            hit.push(row, HitTarget::FormField(i));
        }
    }

    // Key hint on the second-to-last interior line.
    let hint_y = inner.y + inner.height.saturating_sub(2);
    frame.render_widget(
        Line::from(Span::styled(
            " tab/↓ next · ←/→ cycle · enter run · esc cancel",
            Style::default().fg(p.dim).add_modifier(Modifier::DIM),
        )),
        Rect { x: inner.x, y: hint_y, width: inner.width, height: 1 },
    );

    // [ Run ] [ Cancel ] on the last interior line.
    let btn_y = inner.y + inner.height.saturating_sub(1);
    let run = Rect { x: inner.x + 1, y: btn_y, width: 7, height: 1 };
    let cancel = Rect { x: inner.x + 10, y: btn_y, width: 10, height: 1 };
    frame.render_widget(Line::from(Span::styled("[ Run ]", Style::default().fg(p.accent))), run);
    frame.render_widget(Line::from(Span::styled("[ Cancel ]", Style::default().fg(p.dim))), cancel);
    hit.push(run, HitTarget::Button(ButtonKind::Confirm));
    hit.push(cancel, HitTarget::Button(ButtonKind::Cancel));

    if form.dropdown.is_some() {
        render_dropdown(frame, hit, p, form, inner, label_w, main_col);
    }
}

/// Option-list popup anchored under the focused enum row. Registered last so it
/// is topmost in the hit map (`DropdownItem` clicks win over the field beneath).
fn render_dropdown(
    frame: &mut Frame,
    hit: &mut HitMap,
    p: &Palette,
    form: &ArgsForm,
    inner: Rect,
    label_w: usize,
    main_col: usize,
) {
    let Some(hl) = form.dropdown else { return };
    let Some(opts) = form.args.get(form.focus).and_then(|a| a.options.as_ref()) else { return };
    let x = inner.x + 1 + label_w as u16 + 2;
    let y = inner.y + form.focus as u16 + 1;
    let w = (main_col as u16).min(inner.width.saturating_sub(3)).max(6);
    let h = (opts.len() as u16 + 2).min(inner.height.saturating_sub(form.focus as u16 + 1)).max(3);
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
        let style = if i == hl {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default().fg(p.fg)
        };
        frame.render_widget(Line::from(Span::styled(pad(&format!(" {opt}"), row.width as usize), style)), row);
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
    fn focus_starts_on_first_editable_row() {
        let args = vec![arg("fixed"), arg("editable")];
        let form = ArgsForm::new("r".into(), "d".into(), args, m(&[("fixed", "v")]), HashMap::new(), None);
        assert_eq!(form.focus, 1); // row 0 is fixed
        assert_eq!(form.error, None);
        assert_eq!(form.dropdown, None);
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
    fn focus_wraps_and_skips_fixed() {
        let mut form = ArgsForm::new(
            "r".into(), "d".into(),
            vec![arg("a"), arg("b"), arg("c")],
            m(&[("b", "x")]), // b fixed
            HashMap::new(),
            None,
        );
        assert_eq!(form.focus, 0);
        form.next_focus(); // 0 -> (skip 1) -> 2
        assert_eq!(form.focus, 2);
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
    fn text_edit_appends_and_backspaces_clearing_error() {
        let mut form = ArgsForm::new("r".into(), "d".into(), vec![arg("pr")], HashMap::new(), HashMap::new(), None);
        assert!(form.validate().is_err()); // required + empty
        assert_eq!(form.error, Some(0));
        form.input_char('5'); // typing clears the row error
        assert_eq!(form.error, None);
        form.input_char('7');
        assert_eq!(form.values[0], "57");
        form.backspace();
        assert_eq!(form.values[0], "5");
        assert_eq!(form.validate().unwrap(), vec!["5".to_string()]);
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
        form.input_char('x'); // focus starts on target (source fixed) -> "mainx"
        assert_eq!(form.validate().unwrap(), vec!["wt-a".to_string(), "mainx".to_string()]);
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
