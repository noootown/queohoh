//! Key + mouse handling for the reusable bordered form ([`Mode::Form`]).
//!
//! Mirrors `def_args` in shape: `form_key` drives the keyboard (Tab focus,
//! text edits, the inline dropdown, and the explicit-commit Primary/Cancel
//! buttons) and `form_click` routes a left-click onto the form's hit targets
//! (`FormField`/`DropdownItem`/`Button`). The Primary button validates the
//! [`FormState`] and, on success, fires the frozen [`FormAction`] via
//! `fire_form_action` (the New-session enqueue / Create-worktree create+enqueue
//! flows land in Phase 5).

use super::*;
use crate::ipc::types::CatalogEntry;
use crate::view::form::{DropdownOption, Field, FieldKind, FocusKind, FormState};

/// Hardcoded mirror of `BUILTIN_CATALOG` in packages/core/src/catalog.ts — the
/// model picker's fallback when the cached `settings` payload has no catalog (an
/// old daemon, or settings not fetched yet). **Keep in sync with that file.**
/// codex is omitted deliberately: it ships disabled-by-default, so it never
/// appears in a picker anyway (a disabled provider is filtered out regardless).
pub(super) fn builtin_catalog() -> Vec<CatalogEntry> {
    let e = |provider: &str, id: &str, label: &str| CatalogEntry {
        provider: provider.into(),
        id: id.into(),
        label: label.into(),
        hidden: false,
    };
    vec![
        e("claude", "claude-fable-5", "fable"),
        e("claude", "claude-opus-4-8", "opus"),
        e("claude", "claude-sonnet-5", "sonnet"),
        e("claude", "claude-haiku-4-5", "haiku"),
        e("grok", "grok-4.5", "grok-4.5"),
        e("grok", "grok-composer-2.5-fast", "composer"),
    ]
}

/// The dropdown's head-option display label: `default (<refs joined with " → ">)`,
/// or the bare `default` when there are no refs to show. When launching a
/// specific definition the refs come from the def's own `model:` list and carry
/// a `def: ` marker (`default (def: claude/opus → grok/grok-4.5)`); for an
/// ad-hoc / new-session run they come from the repo's `default_models` and carry
/// no marker (`default (claude/opus)`). The head option's stored VALUE is always
/// the empty string (= leave `model` unset → the daemon resolves the chain).
///
/// `from_def` is exercised by Task 8's def-launch picker; the model forms here
/// pass `false` (the repo-default case).
pub(super) fn default_head_label(refs: &[String], from_def: bool) -> String {
    if refs.is_empty() {
        return "default".into();
    }
    let marker = if from_def { "def: " } else { "" };
    format!("default ({marker}{})", refs.join(" → "))
}

impl App {
    /// The effective model catalog: the cached `settings` payload's `catalog`
    /// when present and non-empty, else the built-in mirror ([`builtin_catalog`]).
    /// Hidden entries and disabled providers are still included here — the picker
    /// ([`Self::visible_model_options`]) filters them. `pub(crate)` so the run-info
    /// detail pane (`view::detail`) can resolve a run's raw model id to its
    /// `label (provider)` display without duplicating the settings/builtin
    /// fallback logic.
    pub(crate) fn model_catalog(&self) -> Vec<CatalogEntry> {
        self.settings
            .as_ref()
            .and_then(|s| s.as_ref())
            .map(|p| p.catalog.clone())
            .filter(|c| !c.is_empty())
            .unwrap_or_else(builtin_catalog)
    }

    /// Provider names the payload marks `enabled: false`. Empty when settings are
    /// absent (nothing to filter → the built-in fallback shows all its groups).
    fn disabled_providers(&self) -> std::collections::HashSet<String> {
        self.settings
            .as_ref()
            .and_then(|s| s.as_ref())
            .map(|p| {
                p.providers
                    .iter()
                    .filter(|pr| !pr.enabled)
                    .map(|pr| pr.name.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The catalog entries the model picker offers, in catalog order: hidden
    /// entries and entries whose provider is disabled are filtered out.
    fn visible_model_options(&self) -> Vec<CatalogEntry> {
        let disabled = self.disabled_providers();
        self.model_catalog()
            .into_iter()
            .filter(|e| !e.hidden && !disabled.contains(&e.provider))
            .collect()
    }

    /// The effective `default_models` chain for `repo` (project override, else
    /// global), from the cached `settings` payload. Empty when settings are
    /// absent or the daemon sent no defaults — the head label then reads just
    /// `default`.
    pub(super) fn default_refs_for(&self, repo: &str) -> Vec<String> {
        self.settings
            .as_ref()
            .and_then(|s| s.as_ref())
            .map(|p| p.default_models.refs_for(repo))
            .unwrap_or_default()
    }

    /// The full labeled option list for the model dropdown: the `default (…)`
    /// head (value `""`, label from `repo`'s `default_models`) followed by one
    /// `label (provider)` option per visible catalog entry (value `provider/label`).
    fn model_dropdown_options(&self, repo: &str) -> Vec<DropdownOption> {
        let head = DropdownOption {
            value: String::new(),
            label: default_head_label(&self.default_refs_for(repo), false),
        };
        std::iter::once(head)
            .chain(self.visible_model_options().iter().map(|e| DropdownOption {
                value: e.model_ref(),
                label: e.model_display(),
            }))
            .collect()
    }

    /// The model dropdown field, preselected to its head option (leave `model`
    /// unset → the daemon resolves the chain).
    pub(super) fn model_field(&self, repo: &str) -> Field {
        self.model_field_defaulting(repo, None)
    }

    /// The model dropdown field, preselected to `preferred` when it names a real
    /// catalog option (e.g. the `provider/label` ref a resumed session already
    /// ran on), else the head option (`""` = leave unset). `preferred` is
    /// validated against the visible option VALUES so a stale/foreign ref can't
    /// select a phantom option.
    pub(super) fn model_field_defaulting(&self, repo: &str, preferred: Option<&str>) -> Field {
        let options = self.model_dropdown_options(repo);
        let default = preferred
            .filter(|m| options.iter().any(|o| o.value == *m))
            .unwrap_or("");
        Field::dropdown_labeled("model", options, default)
    }

    /// The adhoc-create session field's display label: `New session` when no
    /// session is pinned, else `↻ <label>` (the session being continued).
    pub(super) fn adhoc_session_label(resume_label: Option<&str>) -> String {
        match resume_label {
            Some(l) => format!("↻ {l}"),
            None => "New session".into(),
        }
    }

    /// Clear a stale adhoc session pin when the target combobox is edited: the
    /// pinned session belongs to a specific worktree, so any change to the target
    /// invalidates it (and resets the session field back to "New session").
    /// No-op unless `action` is an `AdhocTask` currently carrying a pin.
    fn adhoc_reset_pin(action: &mut FormAction, state: &mut FormState) {
        if let FormAction::AdhocTask { resume_session_id, resume_label, resume_worktree, .. } = action
            && resume_session_id.is_some()
        {
            *resume_session_id = None;
            *resume_label = None;
            *resume_worktree = None;
            state.set_field_value(
                crate::app::mode::adhoc_field::SESSION,
                &Self::adhoc_session_label(None),
            );
        }
    }

    /// Open the unified adhoc-create form (`c` / Create), optionally with the
    /// `target` combobox prefilled from the invoking pane's selected entity (an
    /// existing worktree name, or a `pr:N` for a PR-associated row). Fields, in
    /// `adhoc_field` order: `[target combobox, session picker, model dropdown,
    /// prompt textarea]`. The target is optional (empty ⇒ a fresh temp worktree,
    /// the legacy adhoc behavior); the session picker offers continuation when
    /// the target names an existing worktree.
    pub(super) fn open_adhoc_create(&mut self, repo: String, prefill_target: Option<String>) {
        let worktrees = self.active_worktree_names();
        let state = FormState::new(
            &format!("New task · {repo}"),
            "Enqueue",
            vec![
                Field::combobox("worktree / PR / ticket", worktrees, prefill_target.as_deref().unwrap_or("")),
                Field::picker("session", &Self::adhoc_session_label(None)),
                self.model_field(&repo),
                Field::textarea("prompt", "", true),
            ],
        );
        self.mode = Mode::Form { state, action: FormAction::AdhocTask {
            repo,
            resume_session_id: None,
            resume_label: None,
            resume_worktree: None,
        } };
    }

    /// Activate the adhoc-create form's session field: when the current `target`
    /// names an EXISTING worktree, stash the form and open `Mode::SessionPick`
    /// (return variant `Adhoc`) to pick a session to continue; otherwise leave a
    /// status hint (a PR/ticket/temp/new-worktree target has no sessions yet).
    pub(super) fn open_adhoc_session_pick(&mut self) -> Update {
        // Read the current target against the real worktree set BEFORE taking the
        // mode by value.
        let worktrees = self.active_worktree_names();
        let (repo, target) = match &self.mode {
            Mode::Form { state, action: FormAction::AdhocTask { repo, .. } } => (
                repo.clone(),
                state.fields.get(crate::app::mode::adhoc_field::TARGET).map(|f| f.value.trim().to_string()).unwrap_or_default(),
            ),
            _ => return Update { dirty: false, cmds: vec![] },
        };
        if !worktrees.contains(&target) {
            self.status_line = Some("choose an existing worktree to continue a session".into());
            return Update { dirty: true, cmds: vec![] };
        }
        // Take ownership of the form to stash it on the picker for the round-trip.
        let Mode::Form { state, action } = std::mem::replace(&mut self.mode, Mode::List) else {
            return Update { dirty: false, cmds: vec![] };
        };
        self.mode = Mode::SessionPick {
            repo: repo.clone(),
            worktree: target.clone(),
            items: Vec::new(),
            loading: true,
            index: 0,
            query: String::new(),
            focus: crate::hit::ButtonKind::Confirm,
            ret: SessionPickReturn::Adhoc { state: Box::new(state), action: Box::new(action) },
        };
        Update { dirty: true, cmds: vec![Cmd::FetchSessions { repo, worktree: target }] }
    }

    /// `Mode::Form` key handling. Dropdown-open: ↑/↓ move the highlight, Enter
    /// picks, Esc closes the dropdown only. Dropdown-closed: Tab/Shift-Tab are
    /// the ONLY focus movers between fields and the bottom buttons (app-wide
    /// form standard); ↑/↓ open a focused dropdown or move the caret between
    /// lines in a focused textarea (never stepping focus, so multiline stays
    /// navigable); ←/→/Home/End/Backspace/printable edit the focused text
    /// field; Shift+Enter inserts a newline (textarea only). Plain Enter NEVER
    /// submits from a field (explicit-commit rule): it adds a newline in a
    /// textarea, advances focus from a single-line input, or opens a focused
    /// dropdown; only the Primary button submits. Cancel/Esc close.
    pub(super) fn form_key(&mut self, ev: &crossterm::event::KeyEvent) -> Update {
        use crossterm::event::{KeyCode::*, KeyModifiers};
        let shift = ev.modifiers.contains(KeyModifiers::SHIFT);
        let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
        let alt = ev.modifiers.contains(KeyModifiers::ALT);
        // A focused Picker (modal-opening field, e.g. the adhoc session field) is
        // handled before the shared field engine: Enter activates it (opens the
        // sub-picker); Tab/Shift-Tab move focus; Esc cancels; everything else is
        // inert (a Picker has no text/caret/inline list).
        if matches!(&self.mode, Mode::Form { state, .. } if state.is_picker_focused()) {
            return match ev.code {
                Enter => self.open_adhoc_session_pick(),
                Esc => { self.mode = Mode::List; Update { dirty: true, cmds: vec![] } }
                Tab if !shift => {
                    if let Mode::Form { state, .. } = &mut self.mode { state.focus_next(); }
                    Update { dirty: true, cmds: vec![] }
                }
                BackTab | Tab => {
                    if let Mode::Form { state, .. } = &mut self.mode { state.focus_prev(); }
                    Update { dirty: true, cmds: vec![] }
                }
                _ => Update { dirty: false, cmds: vec![] },
            };
        }
        let Mode::Form { state, action } = &mut self.mode else {
            return Update { dirty: false, cmds: vec![] };
        };
        let dropdown_open = state.dropdown_open;
        // A focused Combobox (type-or-pick) has its own handling whether or not
        // its list is open — mirrors `def_args_key` exactly (see its comment):
        // printable/Backspace edit + (re)open, Up/Down open or move the
        // highlight, Enter picks/opens, Esc closes the list only (else cancels),
        // ←/→/Home/End move the caret, Tab/Shift-Tab move focus.
        if state.is_combobox_focused() {
            return match ev.code {
                Esc => {
                    if dropdown_open { state.close_dropdown(); } else { self.mode = Mode::List; }
                    Update { dirty: true, cmds: vec![] }
                }
                Enter => {
                    if dropdown_open {
                        state.dropdown_pick();
                        Self::adhoc_reset_pin(action, state); // picked a new target
                    } else {
                        state.open_dropdown();
                    }
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
                    Self::adhoc_reset_pin(action, state); // target text changed
                    state.open_dropdown(); // re-open + reset the filtered highlight
                    Update { dirty: true, cmds: vec![] }
                }
                Char(c) if !ctrl && !alt => {
                    state.insert_char(c);
                    Self::adhoc_reset_pin(action, state); // target text changed
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
            // Newline chord first — must win over the plain-Enter submit arm; inert
            // on anything but a focused textarea.
            Enter if shift => { state.insert_newline(); Update { dirty: true, cmds: vec![] } }
            // Enter NEVER submits from a text field — only the Primary button
            // does (explicit-commit rule): a focused dropdown opens, a textarea
            // takes a newline, a single-line input advances focus, the buttons
            // fire. This is what stops "type something, hit Enter, everything
            // submits".
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
                FocusKind::Primary => self.submit_form(),
                FocusKind::Cancel => { self.mode = Mode::List; Update { dirty: true, cmds: vec![] } }
            },
            // Tab/Shift-Tab are the ONLY focus movers between fields and the
            // bottom buttons — app-wide form standard. Arrow keys never change
            // focus (they'd hijack a textarea's line navigation).
            Tab if !shift => { state.focus_next(); Update { dirty: true, cmds: vec![] } }
            BackTab => { state.focus_prev(); Update { dirty: true, cmds: vec![] } }
            Tab if shift => { state.focus_prev(); Update { dirty: true, cmds: vec![] } }
            // ↑/↓ open a focused dropdown, move the caret between lines in a
            // focused textarea, and are otherwise inert — they NEVER step focus.
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

    /// Validate the open form and, on success, fire its action; on the first
    /// empty required field keep the form open (the field is flagged via
    /// `error`, focus moved to it by `validate`).
    fn submit_form(&mut self) -> Update {
        let Mode::Form { state, action } = &mut self.mode else {
            return Update { dirty: false, cmds: vec![] };
        };
        let values = match state.validate() {
            Ok(v) => v,
            Err(_) => return Update { dirty: true, cmds: vec![] }, // required field flagged
        };
        // Action-specific secondary validation (e.g. branch-name syntax): keep the
        // form open and flag the offending field on failure.
        if let Some(bad) = Self::action_field_error(action, &values) {
            state.error = Some(bad);
            state.focus_field(bad);
            return Update { dirty: true, cmds: vec![] };
        }
        let action = action.clone();
        self.mode = Mode::List;
        self.fire_form_action(action, values)
    }

    /// Field-level validation beyond required-empty, keyed on the action. For a
    /// Create Worktree the branch/name field must be a valid git branch name.
    /// Returns the failing field index, or `None` when the values pass.
    fn action_field_error(action: &FormAction, values: &[String]) -> Option<usize> {
        match action {
            FormAction::CreateWorktree { .. } => {
                let name = values.first().map(String::as_str).unwrap_or("");
                crate::worktree_context::validate_branch(name).map(|_| 0)
            }
            // The adhoc target combobox accepts a worktree name, a PR/ticket, or
            // empty (temp) — `resolve_target_ref` normalizes all three, so no
            // secondary field validation is needed.
            FormAction::NewSession { .. } | FormAction::AdhocTask { .. } => None,
        }
    }

    /// Route a left-click while the form is open: a `DropdownItem` picks it, a
    /// `FormField` focuses (a dropdown field also opens), `Button` Confirm
    /// submits and Cancel closes; the `Modal`/preview body is inert; anything
    /// else (outside the popup) dismisses.
    pub(super) fn form_click(&mut self, target: &HitTarget) -> Update {
        match target {
            HitTarget::DropdownItem(i) => {
                if let Mode::Form { state, .. } = &mut self.mode {
                    state.dropdown_index = *i;
                    state.dropdown_pick();
                }
                Update { dirty: true, cmds: vec![] }
            }
            HitTarget::FormField(i) => {
                if let Mode::Form { state, .. } = &mut self.mode {
                    state.focus_field(*i);
                    if state.is_dropdown_focused() {
                        state.open_dropdown();
                    }
                }
                // A click on a Picker field (now focused) activates it, the same
                // as Enter — opens the session sub-picker.
                if matches!(&self.mode, Mode::Form { state, .. } if state.is_picker_focused()) {
                    return self.open_adhoc_session_pick();
                }
                Update { dirty: true, cmds: vec![] }
            }
            HitTarget::Button(crate::hit::ButtonKind::Confirm) => self.submit_form(),
            HitTarget::Button(crate::hit::ButtonKind::Cancel) => {
                self.mode = Mode::List;
                Update { dirty: true, cmds: vec![] }
            }
            HitTarget::Modal | HitTarget::MenuPreview => Update { dirty: false, cmds: vec![] },
            _ => {
                self.mode = Mode::List;
                Update { dirty: true, cmds: vec![] }
            }
        }
    }

    /// Fire a validated form's frozen action. `values` are the field values in
    /// declaration order (see the per-`FormAction` field layout). The New-session
    /// enqueue (Task 5.1) and Create-worktree create+enqueue (Task 5.2) wiring
    /// fill these arms.
    fn fire_form_action(&mut self, action: FormAction, values: Vec<String>) -> Update {
        match action {
            // Fields: [model dropdown, prompt textarea]. Enqueue into the frozen
            // worktree, folding in the picked model and (when resuming) the pinned
            // session id.
            FormAction::NewSession { repo, worktree, resume_session_id } => {
                let model = values.first().cloned().unwrap_or_default();
                let prompt = values.get(1).cloned().unwrap_or_default();
                let mut params =
                    serde_json::json!({ "prompt": prompt, "repo": repo, "worktree": worktree });
                if !model.is_empty() {
                    params["model"] = serde_json::Value::String(model);
                }
                if let Some(sid) = resume_session_id {
                    params["resume_session_id"] = serde_json::Value::String(sid);
                }
                let cmd = self.dispatch_rpc("enqueue task", "enqueue", params, RpcOpts::default());
                Update { dirty: true, cmds: vec![cmd] }
            }
            // Fields: [branch/name input, model dropdown, prompt textarea]. The
            // name is validated in `submit_form` before we get here. Create the
            // worktree, then (Option A) the handler enqueues the first task into
            // it using the create reply's path basename.
            FormAction::CreateWorktree { repo } => {
                let name = values.first().cloned().unwrap_or_default();
                let model = values.get(1).cloned().unwrap_or_default();
                let prompt = values.get(2).cloned().unwrap_or_default();
                self.status_line = Some(format!("creating worktree {name}…"));
                let cmd = Self::create_worktree_cmd(
                    &repo,
                    &name,
                    Some(crate::event::EnqueueAfter { prompt, model }),
                );
                Update { dirty: true, cmds: vec![cmd] }
            }
            // Fields: `[target combobox, session picker, model dropdown, prompt
            // textarea]` (see `adhoc_field`). The target resolves to a canonical
            // ref (empty → temp); the pinned session is honored only when the
            // resolved target names the worktree it was picked for.
            FormAction::AdhocTask { repo, resume_session_id, resume_worktree, .. } => {
                use crate::app::mode::adhoc_field;
                let target = values.get(adhoc_field::TARGET).cloned().unwrap_or_default();
                let model = values.get(adhoc_field::MODEL).cloned().unwrap_or_default();
                let prompt = values.get(adhoc_field::PROMPT).cloned().unwrap_or_default();

                let mut params = serde_json::json!({ "prompt": prompt, "repo": repo });
                // A non-empty target → its canonical ref (`worktree:`/`pr:`/
                // `ticket:`); an empty target sends no ref, so the daemon spawns a
                // fresh `temp` worktree (the legacy adhoc behavior). Mirrors
                // `run_definition_cmd`: send `ref`, never `worktree`.
                let resolved = (!target.trim().is_empty())
                    .then(|| super::def_args::resolve_target_ref(target.trim(), &self.active_worktree_names()));
                if let Some(r) = &resolved {
                    params["ref"] = serde_json::Value::String(r.clone());
                }
                if !model.is_empty() {
                    params["model"] = serde_json::Value::String(model);
                }
                // The session pin is only valid on the worktree it was picked for
                // (`resume_worktree`); honor it only when the resolved target
                // still names that worktree.
                if let (Some(sid), Some(wt)) = (resume_session_id, resume_worktree)
                    && resolved.as_deref() == Some(format!("worktree:{wt}").as_str())
                {
                    params["resume_session_id"] = serde_json::Value::String(sid);
                }
                let cmd = self.dispatch_rpc("enqueue task", "enqueue", params, RpcOpts::default());
                Update { dirty: true, cmds: vec![cmd] }
            }
        }
    }
}
