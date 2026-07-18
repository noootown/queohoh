//! Definition-picker and run-form input handling for `App`.
//!
//! Key navigation and click routing for the task/def picker (`DefPick`) and the
//! per-arg run form (`DefArgs`). Split out of `app/mod.rs` verbatim (no behavior
//! change).

use super::*;
use crate::ipc::types::ModelRef;
use crate::view::form::{Field, FieldKind, FocusKind, FormState};

/// Initial value for one arg when building its form field: `fixed` wins, then
/// `initial`, then the declared `default`, then (for an enum) its first option,
/// else empty. Mirrors the retired `ArgsForm::initial_value` precedence.
fn initial_arg_value(
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
    if let Some(first) = arg.options.as_ref().and_then(|o| o.first()) {
        return first.clone();
    }
    String::new()
}

/// Resolve a worktree-combobox field value to a canonical target ref: an exact
/// existing-worktree name → `worktree:<name>` (this wins so a worktree that
/// happens to look like a PR/ticket still targets the worktree); else the typed
/// PR/ticket classification ([`crate::ref_classify::classify_ref`]); else a
/// literal `worktree:<value>` (create-or-reuse a worktree by that name).
pub(super) fn resolve_target_ref(value: &str, worktrees: &[String]) -> String {
    if worktrees.iter().any(|w| w == value) {
        format!("worktree:{value}")
    } else if let Some(r) = crate::ref_classify::classify_ref(value) {
        r
    } else {
        format!("worktree:{value}")
    }
}

impl App {
    /// Build a [`FormState`] with one field per arg, in declaration order:
    /// a `fixed` arg becomes a read-only field; an arg with `options` a Dropdown;
    /// everything else a free-text Textarea (a worktree-typed arg will become a
    /// Combobox in Phase 3 — the branch is called out below). Focus starts on
    /// the first non-readonly field. The Primary button is labeled `Run`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn form_from_args(
        &self,
        def_name: &str,
        args: &[ArgSpec],
        fixed: &HashMap<String, String>,
        initial: &HashMap<String, String>,
        worktrees: &[String],
        branches: &[String],
        initial_worktree: Option<&str>,
    ) -> FormState {
        let fields = args
            .iter()
            .map(|a| {
                let val = initial_arg_value(a, fixed, initial);
                if fixed.contains_key(&a.name) {
                    Field::readonly(&a.name, &val)
                } else if a.is_worktree() {
                    // A worktree-typed arg is a Combobox seeded with the repo's
                    // worktree names — or, when launched FROM a worktree row, a
                    // read-only field locked to that worktree (no re-targeting).
                    match initial_worktree {
                        Some(wt) => Field::readonly(&a.name, wt),
                        None => {
                            // Required: an empty combobox must block submit
                            // inline (via `FormState::validate`) rather than
                            // resolve to the malformed ref `"worktree:"`.
                            let mut f = Field::combobox(&a.name, worktrees.to_vec(), &val);
                            f.required = true;
                            f
                        }
                    }
                } else if a.is_branch() {
                    // A `type: branch` arg is a dropdown seeded with the repo's
                    // worktree branches (incl. main/master). Prepend the current
                    // value when it names a branch with no local worktree (e.g.
                    // the default `main`) so it stays selectable.
                    let mut opts = branches.to_vec();
                    if !val.is_empty() && !opts.contains(&val) {
                        opts.insert(0, val.clone());
                    }
                    Field::dropdown(&a.name, opts, &val)
                } else if a.options.as_ref().is_some_and(|o| !o.is_empty()) {
                    Field::dropdown(&a.name, a.options.clone().unwrap_or_default(), &val)
                } else if a.is_text() {
                    // `type: text` opts into the multiline, auto-growing textarea
                    // (e.g. autofix's `situation`). Every other free-text arg is a
                    // single-line input.
                    Field::textarea(&a.name, &val, a.default.is_none())
                } else {
                    Field::input(&a.name, &val, a.default.is_none())
                }
            })
            .collect();
        FormState::new(def_name, "Run", fields)
    }

    /// Open the run form. `fixed`/`initial` and `worktree` are caller-decided;
    /// `worktrees` seeds a worktree-typed arg's combobox (the repo's worktree
    /// names, from the call site's `active_worktree_names()`). `def_model` is the
    /// def's authored `model:` (drives the trailing effective-chain model
    /// picker — see [`Self::def_model_field`]). Returns the prompt-fetch
    /// command(s) for the def's right panel (empty when the full definition is
    /// already cached / in flight).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn open_def_args(
        &mut self,
        repo: String,
        name: String,
        args: Vec<ArgSpec>,
        fixed: HashMap<String, String>,
        initial: HashMap<String, String>,
        worktree: Option<String>,
        worktrees: Vec<String>,
        branches: Vec<String>,
        def_model: Option<ModelRef>,
    ) -> Vec<Cmd> {
        let cmds = self.ensure_full_def(&repo, &name);
        // Model picker FIRST (always field 0), then the arg fields in
        // declaration order (their positional order on submit is unchanged —
        // `submit_def_args` peels the leading model off before reading args).
        // Because the model dropdown is never readonly, `FormState::new` always
        // lands initial focus on it.
        let arg_fields = self
            .form_from_args(
                &name,
                &args,
                &fixed,
                &initial,
                &worktrees,
                &branches,
                worktree.as_deref(),
            )
            .fields;
        let mut fields = vec![self.def_model_field(&repo, def_model.as_ref())];
        fields.extend(arg_fields);
        let state = FormState::new(&name, "Run", fields);
        self.mode = Mode::DefArgs {
            state,
            repo,
            def_name: name,
            args,
            initial_worktree: worktree,
            preview_scroll: 0,
        };
        cmds
    }

    /// `Mode::DefPick` key handling (lazyvim-style). Esc closes; Up/Ctrl+k/Ctrl+p
    /// and Down/Ctrl+j/Ctrl+n move circularly over the FILTERED defs; Enter picks
    /// the highlighted def (zero-arg dispatch or open the args form with the
    /// targeted worktree's branch as FIXED context); Backspace/printable edit the
    /// name filter, resetting the highlight to the first match. `q` is no longer a
    /// close key — it types into the filter. Navigation and filter edits prefetch
    /// the newly-highlighted def's prompt for the right pane.
    pub(super) fn def_pick_key(&mut self, ev: &crossterm::event::KeyEvent) -> Update {
        use crossterm::event::{KeyCode::*, KeyModifiers};
        let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
        let alt = ev.modifiers.contains(KeyModifiers::ALT);
        let Mode::DefPick { defs, index, query, .. } = &self.mode else {
            return Update { dirty: false, cmds: vec![] };
        };
        let filtered = crate::selectors::filter_rows(defs, query, |d| d.name.clone());
        let flen = filtered.len();
        let cur = *index;
        let actual = filtered.get(cur).copied();
        match ev.code {
            Esc => {
                self.mode = Mode::List;
                Update { dirty: true, cmds: vec![] }
            }
            Up => self.def_pick_move(cur, flen, -1),
            Down => self.def_pick_move(cur, flen, 1),
            Char('k') | Char('p') if ctrl => self.def_pick_move(cur, flen, -1),
            Char('j') | Char('n') if ctrl => self.def_pick_move(cur, flen, 1),
            Enter => match actual {
                Some(a) => self.def_pick_activate(a),
                None => Update { dirty: false, cmds: vec![] },
            },
            Backspace => {
                if let Mode::DefPick { query, index, preview_scroll, .. } = &mut self.mode {
                    query.pop();
                    *index = 0;
                    *preview_scroll = 0;
                }
                let cmds = self.prefetch_full_def();
                Update { dirty: true, cmds }
            }
            Char(c) if !ctrl && !alt => {
                if let Mode::DefPick { query, index, preview_scroll, .. } = &mut self.mode {
                    query.push(c);
                    *index = 0;
                    *preview_scroll = 0;
                }
                let cmds = self.prefetch_full_def();
                Update { dirty: true, cmds }
            }
            _ => Update { dirty: false, cmds: vec![] },
        }
    }

    /// Move the def-pick highlight circularly over the filtered defs (the
    /// preview scroll resets — it belongs to the outgoing def), then prefetch
    /// the newly-highlighted def's prompt. `cur` is the current filtered index,
    /// `flen` the filtered def count.
    pub(super) fn def_pick_move(&mut self, cur: usize, flen: usize, dir: i32) -> Update {
        let next = if flen == 0 {
            0
        } else if dir < 0 {
            cur.checked_sub(1).unwrap_or(flen - 1)
        } else if cur + 1 >= flen {
            0
        } else {
            cur + 1
        };
        if let Mode::DefPick { index, preview_scroll, .. } = &mut self.mode {
            *index = next;
            *preview_scroll = 0;
        }
        let cmds = self.prefetch_full_def();
        Update { dirty: true, cmds }
    }

    /// Activate the def at `index` in the open picker: open the run form with
    /// the worktree branch driving source/branch/ticket as FIXED, plus the
    /// effective-chain model picker (preselected to chain\[0\]). Zero-arg defs
    /// still open the form so the operator can confirm/override the model —
    /// there is no immediate `runDefinition` hop.
    fn def_pick_activate(&mut self, index: usize) -> Update {
        let Mode::DefPick { defs, worktree, branch, .. } = &self.mode else {
            return Update { dirty: false, cmds: vec![] };
        };
        let Some(def) = defs.get(index).cloned() else {
            return Update { dirty: false, cmds: vec![] };
        };
        let worktree = worktree.clone();
        let branch = branch.clone();
        let fixed = branch
            .as_deref()
            .map(crate::worktree_context::context_arg_values)
            .unwrap_or_default();
        let worktrees = self.active_worktree_names();
        let branches = self.active_worktree_branches();
        let cmds = self.open_def_args(
            def.repo,
            def.name,
            def.args,
            fixed,
            HashMap::new(),
            worktree,
            worktrees,
            branches,
            def.model,
        );
        Update { dirty: true, cmds }
    }

    /// Route a left-click while the def-pick popup is open: a `MenuItem` picks
    /// that row (same path as Enter); the `Modal` body is inert; anything else
    /// closes the popup.
    pub(super) fn route_def_pick_click(&mut self, target: Option<HitTarget>) -> Update {
        match target {
            Some(HitTarget::MenuItem(i)) => {
                // `i` is a FILTERED display index; resolve it to the underlying
                // def index through the same filter before activating.
                let actual = if let Mode::DefPick { defs, query, .. } = &self.mode {
                    crate::selectors::filter_rows(defs, query, |d| d.name.clone()).get(i).copied()
                } else {
                    None
                };
                match actual {
                    Some(a) => self.def_pick_activate(a),
                    None => Update { dirty: false, cmds: vec![] },
                }
            }
            Some(HitTarget::Modal) | Some(HitTarget::MenuPreview) => {
                Update { dirty: false, cmds: vec![] }
            }
            _ => {
                self.mode = Mode::List;
                Update { dirty: true, cmds: vec![] }
            }
        }
    }

    /// `Mode::DefArgs` key handling — mirrors `form_key` (the shared field-engine
    /// standard) exactly, differing only in the mode pattern and that the Primary
    /// button submits via `submit_def_args`. Dropdown-open: ↑/↓ move the
    /// highlight, Enter picks, Esc closes the dropdown only. Dropdown-closed:
    /// Tab/Shift-Tab are the ONLY focus movers; ↑/↓ open a focused dropdown or
    /// move the caret between visual lines in a focused textarea (never stepping
    /// focus); ←/→/Home/End/Backspace/printable edit the focused text field;
    /// Shift+Enter inserts a newline; plain Enter NEVER submits from a field
    /// (a textarea takes a newline, a single-line input advances focus, a
    /// dropdown opens); only the Primary button submits. Esc cancels.
    pub(super) fn def_args_key(&mut self, ev: &crossterm::event::KeyEvent) -> Update {
        use crossterm::event::{KeyCode::*, KeyModifiers};
        let shift = ev.modifiers.contains(KeyModifiers::SHIFT);
        let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
        let alt = ev.modifiers.contains(KeyModifiers::ALT);
        let Mode::DefArgs { state, .. } = &mut self.mode else {
            return Update { dirty: false, cmds: vec![] };
        };
        let dropdown_open = state.dropdown_open;
        // A focused Combobox (type-or-pick) has its own handling whether or not
        // its list is open: printable/Backspace edit the value AND (re)open the
        // filtered list; Up/Down open or move the highlight; Enter picks the
        // highlight (or the typed ref) / opens when closed; Esc closes the list
        // only (else cancels); ←/→/Home/End move the caret; Tab/Shift-Tab move
        // focus (the app-wide standard).
        if state.is_combobox_focused() {
            return match ev.code {
                Esc => {
                    if dropdown_open { state.close_dropdown(); } else { self.mode = Mode::List; }
                    Update { dirty: true, cmds: vec![] }
                }
                Enter => {
                    if dropdown_open { state.dropdown_pick(); } else { state.open_dropdown(); }
                    Update { dirty: true, cmds: vec![] }
                }
                Up => {
                    if dropdown_open { state.dropdown_move(-1); } else { state.open_dropdown(); }
                    Update { dirty: true, cmds: vec![] }
                }
                Down => {
                    if dropdown_open { state.dropdown_move(1); } else { state.open_dropdown(); }
                    Update { dirty: true, cmds: vec![] }
                }
                Left => { state.move_left(); Update { dirty: true, cmds: vec![] } }
                Right => { state.move_right(); Update { dirty: true, cmds: vec![] } }
                Home => { state.move_home(); Update { dirty: true, cmds: vec![] } }
                End => { state.move_end(); Update { dirty: true, cmds: vec![] } }
                Tab if !shift => { state.focus_next(); Update { dirty: true, cmds: vec![] } }
                BackTab => { state.focus_prev(); Update { dirty: true, cmds: vec![] } }
                Tab if shift => { state.focus_prev(); Update { dirty: true, cmds: vec![] } }
                Backspace => {
                    state.backspace();
                    state.open_dropdown(); // re-open + reset the filtered highlight
                    Update { dirty: true, cmds: vec![] }
                }
                Char(c) if !ctrl && !alt => {
                    state.insert_char(c);
                    state.open_dropdown();
                    Update { dirty: true, cmds: vec![] }
                }
                _ => Update { dirty: false, cmds: vec![] },
            };
        }
        if dropdown_open {
            match ev.code {
                Up => { state.dropdown_move(-1); return Update { dirty: true, cmds: vec![] }; }
                Down => { state.dropdown_move(1); return Update { dirty: true, cmds: vec![] }; }
                Enter => { state.dropdown_pick(); return Update { dirty: true, cmds: vec![] }; }
                Esc => { state.close_dropdown(); return Update { dirty: true, cmds: vec![] }; }
                _ => return Update { dirty: false, cmds: vec![] },
            }
        }
        let is_dropdown = state.is_dropdown_focused();
        let fk = state.focus_kind();
        match ev.code {
            Esc => { self.mode = Mode::List; Update { dirty: true, cmds: vec![] } }
            // Newline chord first — must win over the plain-Enter arm; inert off a
            // focused textarea.
            Enter if shift => { state.insert_newline(); Update { dirty: true, cmds: vec![] } }
            // Enter NEVER submits from a text field — only the Primary button
            // does (explicit-commit rule): a focused dropdown opens, a textarea
            // takes a newline, a single-line input advances focus.
            Enter => match fk {
                FocusKind::Field(_) if is_dropdown => {
                    state.open_dropdown();
                    Update { dirty: true, cmds: vec![] }
                }
                FocusKind::Field(i) if matches!(state.fields[i].kind, FieldKind::Textarea) => {
                    state.insert_newline();
                    Update { dirty: true, cmds: vec![] }
                }
                FocusKind::Field(_) => {
                    state.focus_next();
                    Update { dirty: true, cmds: vec![] }
                }
                FocusKind::Primary => self.submit_def_args(),
                FocusKind::Cancel => { self.mode = Mode::List; Update { dirty: true, cmds: vec![] } }
            },
            // Tab/Shift-Tab are the ONLY focus movers; arrows never step focus.
            Tab if !shift => { state.focus_next(); Update { dirty: true, cmds: vec![] } }
            BackTab => { state.focus_prev(); Update { dirty: true, cmds: vec![] } }
            Tab if shift => { state.focus_prev(); Update { dirty: true, cmds: vec![] } }
            Up => {
                if is_dropdown { state.open_dropdown(); } else { state.move_up(); }
                Update { dirty: true, cmds: vec![] }
            }
            Down => {
                if is_dropdown { state.open_dropdown(); } else { state.move_down(); }
                Update { dirty: true, cmds: vec![] }
            }
            Left => { state.move_left(); Update { dirty: true, cmds: vec![] } }
            Right => { state.move_right(); Update { dirty: true, cmds: vec![] } }
            Home => { state.move_home(); Update { dirty: true, cmds: vec![] } }
            End => { state.move_end(); Update { dirty: true, cmds: vec![] } }
            Backspace => { state.backspace(); Update { dirty: true, cmds: vec![] } }
            Char(c) if !ctrl && !alt => { state.insert_char(c); Update { dirty: true, cmds: vec![] } }
            _ => Update { dirty: false, cmds: vec![] },
        }
    }

    /// Validate and dispatch `runDefinition`, or keep the form open on the first
    /// missing field (the row is flagged via `error`, focus moved to it). When
    /// the def has a worktree-typed arg, its field value is resolved to a
    /// canonical ref (`resolve_target_ref`) and sent as `params.ref` (the
    /// worktree param is then suppressed — see `run_definition_cmd`). The
    /// trailing model field (appended by [`Self::open_def_args`]) is peeled off
    /// and sent as a 1-entry exact `params.model` when non-empty.
    fn submit_def_args(&mut self) -> Update {
        // The repo's worktree names for the exact-match branch of the ref
        // resolution — read before the `self.mode` mutable borrow.
        let worktree_names = self.active_worktree_names();
        let Mode::DefArgs { state, repo, def_name, args, initial_worktree, .. } = &mut self.mode else {
            return Update { dirty: false, cmds: vec![] };
        };
        match state.validate() {
            Ok(values) => {
                // `open_def_args` builds the form as `[model, args…]`, so the
                // model picker is field 0 and the positional args follow it. A
                // hand-built `Mode::DefArgs` test fixture may omit the model
                // field entirely; detect that by the presence of the extra
                // leading field (`values.len() > n_args`) rather than assuming a
                // fixed index, so both layouts read args correctly.
                let n_args = args.len();
                let has_model = values.len() > n_args;
                let arg_start = usize::from(has_model);
                let arg_values: Vec<String> =
                    values.iter().skip(arg_start).take(n_args).cloned().collect();
                let model = has_model
                    .then(|| values.first().cloned())
                    .flatten()
                    .filter(|m| !m.is_empty());
                // A worktree-typed arg's value → canonical ref; no such arg keeps
                // the old positional-only behavior (target_ref None).
                let target_ref = args
                    .iter()
                    .position(ArgSpec::is_worktree)
                    .and_then(|i| arg_values.get(i))
                    .map(|value| resolve_target_ref(value, &worktree_names));
                let cmd = Self::run_definition_cmd(
                    repo,
                    def_name,
                    &arg_values,
                    initial_worktree.as_deref(),
                    target_ref.as_deref(),
                    model.as_deref(),
                );
                self.mode = Mode::List;
                Update { dirty: true, cmds: vec![cmd] }
            }
            Err(_) => Update { dirty: true, cmds: vec![] }, // error flagged on the row
        }
    }

    /// Route a left-click while the args form is open: a `DropdownItem` picks it,
    /// a `FormField` focuses (a dropdown field also opens), `Button` Confirm
    /// submits and Cancel closes; the `Modal`/preview body is inert; anything
    /// else (outside the popup) dismisses. Mirrors `form_click`.
    pub(super) fn def_args_click(&mut self, target: &HitTarget) -> Update {
        match target {
            HitTarget::DropdownItem(i) => {
                if let Mode::DefArgs { state, .. } = &mut self.mode {
                    state.dropdown_index = *i;
                    state.dropdown_pick();
                }
                Update { dirty: true, cmds: vec![] }
            }
            HitTarget::FormField(i) => {
                if let Mode::DefArgs { state, .. } = &mut self.mode {
                    state.focus_field(*i);
                    if state.is_dropdown_focused() {
                        state.open_dropdown();
                    }
                }
                Update { dirty: true, cmds: vec![] }
            }
            HitTarget::Button(crate::hit::ButtonKind::Confirm) => self.submit_def_args(),
            HitTarget::Button(crate::hit::ButtonKind::Cancel) => {
                self.mode = Mode::List;
                Update { dirty: true, cmds: vec![] }
            }
            // Left panel body and right (prompt) panel: inert (the wheel scrolls
            // the preview over `MenuPreview`).
            HitTarget::Modal | HitTarget::MenuPreview => Update { dirty: false, cmds: vec![] },
            // Any other target is behind the popup (a pane row/body/tab): a click
            // outside the form dismisses it, same as esc (mirrors def-pick/menu).
            _ => {
                self.mode = Mode::List;
                Update { dirty: true, cmds: vec![] }
            }
        }
    }
}
