//! Definition-picker, run-form, and create-worktree input handling for `App`.
//!
//! Key navigation and click routing for the task/def picker (`DefPick`), the
//! per-arg run form (`DefArgs`), and the new-worktree branch prompt
//! (`CreateWorktree`). Split out of `app/mod.rs` verbatim (no behavior change).

use super::*;

impl App {
    /// `Mode::CreateWorktree` key handling. Enter validates the branch name:
    /// invalid keeps the modal open with the inline error; valid dispatches
    /// `createWorktree` and closes immediately (creation fires async and can
    /// take minutes — progress lives on the status line). Esc cancels; every
    /// other key edits the input and clears any prior error.
    pub(super) fn create_worktree_key(&mut self, ev: &crossterm::event::KeyEvent) -> Update {
        use crossterm::event::KeyCode::*;
        let repo = match self.active_repo() {
            Some(r) => r,
            None => return Update { dirty: false, cmds: vec![] },
        };
        match ev.code {
            Esc => {
                self.mode = Mode::List;
                Update { dirty: true, cmds: vec![] }
            }
            Enter => {
                let name = match &self.mode {
                    Mode::CreateWorktree { input, .. } => input.value().to_string(),
                    _ => return Update { dirty: false, cmds: vec![] },
                };
                if let Some(msg) = crate::worktree_context::validate_branch(&name) {
                    if let Mode::CreateWorktree { error, .. } = &mut self.mode {
                        *error = Some(msg);
                    }
                    return Update { dirty: true, cmds: vec![] };
                }
                // Close immediately — creation can take minutes; progress + result
                // live on the status line, not a blocked modal.
                self.mode = Mode::List;
                self.status_line = Some(format!("creating worktree {name}…"));
                Update { dirty: true, cmds: vec![Self::create_worktree_cmd(&repo, &name)] }
            }
            _ => {
                // Feed the key to tui-input; a new keystroke clears any prior
                // validation error. Mouse never reaches here (Task 12 filters it).
                if let Mode::CreateWorktree { input, error } = &mut self.mode {
                    input.handle_event(&crossterm::event::Event::Key(*ev));
                    *error = None;
                }
                Update { dirty: true, cmds: vec![] }
            }
        }
    }

    /// Open the run form. `fixed`/`initial` and `worktree` are caller-decided.
    /// Returns the prompt-fetch command(s) for the def's right panel (empty when
    /// the full definition is already cached / in flight).
    pub(super) fn open_def_args(
        &mut self,
        repo: String,
        name: String,
        args: Vec<ArgSpec>,
        fixed: HashMap<String, String>,
        initial: HashMap<String, String>,
        worktree: Option<String>,
    ) -> Vec<Cmd> {
        let cmds = self.ensure_full_def(&repo, &name);
        self.mode = Mode::DefArgs {
            form: crate::view::args_form::ArgsForm::new(repo, name, args, fixed, initial, worktree),
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

    /// Activate the def at `index` in the open picker: zero-arg defs dispatch
    /// `runDefinition` against the targeted worktree immediately; otherwise open
    /// the args form with the worktree branch driving source/branch/ticket as
    /// FIXED.
    fn def_pick_activate(&mut self, index: usize) -> Update {
        let Mode::DefPick { defs, worktree, branch, .. } = &self.mode else {
            return Update { dirty: false, cmds: vec![] };
        };
        let Some(def) = defs.get(index).cloned() else {
            return Update { dirty: false, cmds: vec![] };
        };
        let worktree = worktree.clone();
        let branch = branch.clone();
        if def.args.is_empty() {
            self.mode = Mode::List;
            return Update {
                dirty: true,
                cmds: vec![Self::run_definition_cmd(&def.repo, &def.name, &[], worktree.as_deref())],
            };
        }
        let fixed = branch
            .as_deref()
            .map(crate::worktree_context::context_arg_values)
            .unwrap_or_default();
        let cmds = self.open_def_args(def.repo, def.name, def.args, fixed, HashMap::new(), worktree);
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

    /// `Mode::DefArgs` key handling. Dropdown-open: ↑/↓ move, Enter picks, Esc
    /// closes the dropdown only. Dropdown-closed: Tab/Shift-Tab move focus; ↑/↓
    /// move the cursor within a multiline free-text value (moving focus only at
    /// the value's top/bottom line, or on an enum); ←/→ cycle an enum or move the
    /// cursor in text; Home/End jump within the current line; Shift+Enter
    /// inserts a hard newline; plain Enter
    /// opens an enum dropdown or validates+submits; Esc cancels; printable/
    /// Backspace edit at the cursor.
    pub(super) fn def_args_key(&mut self, ev: &crossterm::event::KeyEvent) -> Update {
        use crossterm::event::{KeyCode::*, KeyModifiers};
        let dropdown_open = matches!(&self.mode, Mode::DefArgs { form } if form.dropdown.is_some());
        let shift = ev.modifiers.contains(KeyModifiers::SHIFT);
        let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
        let alt = ev.modifiers.contains(KeyModifiers::ALT);
        let Mode::DefArgs { form } = &mut self.mode else {
            return Update { dirty: false, cmds: vec![] };
        };
        if dropdown_open {
            match ev.code {
                Up => { form.dropdown_move(-1); return Update { dirty: true, cmds: vec![] }; }
                Down => { form.dropdown_move(1); return Update { dirty: true, cmds: vec![] }; }
                Enter => { form.dropdown_pick(); return Update { dirty: true, cmds: vec![] }; }
                Esc => { form.close_dropdown(); return Update { dirty: true, cmds: vec![] }; }
                _ => return Update { dirty: false, cmds: vec![] },
            }
        }
        let enum_focus = form.is_enum(form.focus);
        match ev.code {
            Esc => { self.mode = Mode::List; Update { dirty: true, cmds: vec![] } }
            // Newline chord first — it must win over the plain-Enter run arm.
            // No-op on enum/fixed rows (nothing to insert into). Only shift+enter
            // inserts a newline; alt+enter is no longer special (falls through).
            Enter if shift => { form.insert_newline(); Update { dirty: true, cmds: vec![] } }
            Enter => {
                if enum_focus && !form.is_fixed(form.focus) {
                    form.open_dropdown(form.focus);
                    Update { dirty: true, cmds: vec![] }
                } else {
                    self.submit_def_args()
                }
            }
            Tab if !shift => { form.next_focus(); Update { dirty: true, cmds: vec![] } }
            BackTab => { form.prev_focus(); Update { dirty: true, cmds: vec![] } }
            Tab if shift => { form.prev_focus(); Update { dirty: true, cmds: vec![] } }
            // ↑/↓ walk a multiline value's lines; only at the value edge (or on an
            // enum) do they step focus.
            Up => {
                if form.try_move_up() { Update { dirty: true, cmds: vec![] } }
                else { form.prev_focus(); Update { dirty: true, cmds: vec![] } }
            }
            Down => {
                if form.try_move_down() { Update { dirty: true, cmds: vec![] } }
                else { form.next_focus(); Update { dirty: true, cmds: vec![] } }
            }
            // ←/→ cycle an enum; on a free-text row they move the cursor.
            Left => {
                if enum_focus { let i = form.focus; form.cycle_option(i, -1); }
                else { form.move_left(); }
                Update { dirty: true, cmds: vec![] }
            }
            Right => {
                if enum_focus { let i = form.focus; form.cycle_option(i, 1); }
                else { form.move_right(); }
                Update { dirty: true, cmds: vec![] }
            }
            Home => { form.move_home(); Update { dirty: true, cmds: vec![] } }
            End => { form.move_end(); Update { dirty: true, cmds: vec![] } }
            Backspace => { form.backspace(); Update { dirty: true, cmds: vec![] } }
            Char(c) if !ctrl && !alt => { form.input_char(c); Update { dirty: true, cmds: vec![] } }
            _ => Update { dirty: false, cmds: vec![] },
        }
    }

    /// Validate and dispatch `runDefinition`, or keep the form open on the first
    /// missing field (the row is flagged via `error`).
    fn submit_def_args(&mut self) -> Update {
        let Mode::DefArgs { form } = &mut self.mode else {
            return Update { dirty: false, cmds: vec![] };
        };
        match form.validate() {
            Ok(values) => {
                let cmd = Self::run_definition_cmd(&form.repo, &form.def_name, &values, form.initial_worktree.as_deref());
                self.mode = Mode::List;
                Update { dirty: true, cmds: vec![cmd] }
            }
            Err(_) => Update { dirty: true, cmds: vec![] }, // error flagged on the row
        }
    }

    /// Route a left-click while the args form is open: a `DropdownItem` picks it,
    /// a `FormField` focuses (enum rows open the dropdown), `Button` Confirm
    /// submits and Cancel closes; the `Modal` body is inert.
    pub(super) fn def_args_click(&mut self, target: &HitTarget) -> Update {
        match target {
            HitTarget::DropdownItem(i) => {
                if let Mode::DefArgs { form } = &mut self.mode {
                    form.dropdown = Some(*i);
                    form.dropdown_pick();
                }
                Update { dirty: true, cmds: vec![] }
            }
            HitTarget::FormField(i) => {
                if let Mode::DefArgs { form } = &mut self.mode {
                    form.focus_field(*i);
                    if form.is_enum(*i) && !form.is_fixed(*i) {
                        form.open_dropdown(*i);
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
