//! Bulk-menu handling for `App`.
//!
//! Opening the WORKTREES bulk-remove confirm and the launcher's SessionPick
//! key/click handling. The single-target queue action menu (its Resume verb
//! is now the direct `[g]oto` action, see `App::goto_queue`) and the Tasks
//! bulk picker (never reachable — `hit::bulk_allowed` refuses it) were retired
//! along with them; `Mode::ActionMenu` no longer exists.

use super::*;

impl App {
    /// Collapse a list pane's bulk selection on the active tab — drop the range
    /// anchor AND the marks. Called before every bulk dispatch (mirroring the
    /// App.tsx `runBulk` clear-then-dispatch order) so a completed action never
    /// leaves a stale selection to widen the next one.
    pub(super) fn clear_range_and_marks(&mut self, pane: ListPane) {
        if let Some(repo) = self.active_repo()
            && let Some(ui) = self.ui_by_tab.get_mut(&repo) {
                ui.selections[pane.idx()].anchor = None;
                ui.marks[pane.idx()].clear();
            }
    }

    /// Build the bulk follow-up for `pane`'s current `selection ∪ marks`,
    /// freezing eligibility (names) at open time — a daemon push reshuffling
    /// rows mid-call can't retarget the dispatch. WORKTREES is the only case
    /// live traffic ever reaches (`hit::bulk_allowed` refuses everything else
    /// in `App::bulk_blocked` before the caller gets here): since it has
    /// exactly one bulk verb (Remove), this opens `Mode::Confirm` directly,
    /// matching the single-row `x` flow. QUEUE and TASKS are a hard "nothing
    /// selected" status line — kept only so `pane: ListPane` stays exhaustive
    /// (see `bulk_flow_tests::tasks_bulk_range_via_r_refuses_not_applicable`).
    /// Always reports dirty so the caller doesn't need its own branch.
    pub(super) fn open_bulk_menu(&mut self, pane: ListPane) -> Update {
        let Some(snap) = self.snapshot.as_ref() else {
            self.status_line = Some("nothing selected".into());
            return Update { dirty: true, cmds: vec![] };
        };
        let Some(repo) = self.active_repo() else {
            self.status_line = Some("nothing selected".into());
            return Update { dirty: true, cmds: vec![] };
        };
        match pane {
            // QUEUE/TASKS have no bulk menu — neither pane's bulk verb ever
            // reaches here (the queue `r`/`x` chips carry QUEUE's bulk verbs
            // instead; TASKS has none).
            ListPane::Queue | ListPane::Tasks => {
                self.status_line = Some("nothing selected".into());
                Update { dirty: true, cmds: vec![] }
            }
            ListPane::Worktrees => {
                let ui = self.active_ui();
                let rows = crate::selectors::worktree_rows(snap, &repo);
                let vis = crate::selectors::filter_rows(&rows, &ui.search[2], |r| r.name.clone());
                let visible: Vec<&crate::selectors::WorktreeRow> =
                    vis.iter().filter_map(|&i| rows.get(i)).collect();
                let sel = ui.selections[2];
                let marks = &ui.marks[2];
                let remove_names: Vec<String> =
                    crate::view::selected_positions(&visible, &sel, marks, |r| r.raw_name.clone())
                        .into_iter()
                        .filter_map(|pos| visible.get(pos).copied())
                        // Eligibility is applied AFTER selection (a session row
                        // or a protected worktree can be marked; it just isn't
                        // removable). Busy is allowed — daemon cancels live
                        // tasks on each worktree before teardown.
                        .filter(|r| !r.is_session && !r.protected)
                        .map(|r| r.raw_name.clone())
                        .collect();
                if remove_names.is_empty() {
                    self.status_line = Some("no eligible rows".into());
                    return Update { dirty: true, cmds: vec![] };
                }
                self.mode = Self::bulk_remove_confirm_mode(repo, remove_names);
                Update { dirty: true, cmds: vec![] }
            }
        }
    }

    /// Build the `Mode::Confirm` for removing `names` worktrees in `repo`:
    /// a warning line, up to 8 names, then "…and N more" when the range
    /// exceeds 8. Used by the direct bulk-open path in `open_bulk_menu`.
    fn bulk_remove_confirm_mode(repo: String, names: Vec<String>) -> Mode {
        let extra = names.len().saturating_sub(8);
        let mut body = vec![
            "discards uncommitted changes, deletes each local branch,".to_string(),
            "and cancels running/queued tasks on those worktrees".to_string(),
        ];
        body.extend(names.iter().take(8).map(|name| format!("  {name}")));
        if extra > 0 {
            body.push(format!("  …and {extra} more"));
        }
        Mode::Confirm {
            title: format!("Remove {} worktrees", names.len()),
            body,
            confirm_label: "Remove".into(),
            action: ConfirmAction::BulkRemoveWorktrees { repo, names },
            focus: crate::hit::ButtonKind::Confirm,
        }
    }

    /// Current picker preview scroll (0 outside DefPick/DefArgs). Both the run
    /// form (DefArgs) and the DefPick picker carry their scroll on the mode.
    fn menu_preview_scroll_value(&self) -> usize {
        match &self.mode {
            Mode::DefPick { preview_scroll, .. } => *preview_scroll,
            Mode::DefArgs { preview_scroll, .. } => *preview_scroll,
            _ => 0,
        }
    }

    /// Set the picker preview scroll, reporting dirty only on change.
    fn set_menu_preview_scroll(&mut self, next: usize) -> Update {
        match &mut self.mode {
            Mode::DefPick { preview_scroll, .. } => {
                let changed = *preview_scroll != next;
                *preview_scroll = next;
                Update { dirty: changed, cmds: vec![] }
            }
            Mode::DefArgs { preview_scroll, .. } => {
                let changed = *preview_scroll != next;
                *preview_scroll = next;
                Update { dirty: changed, cmds: vec![] }
            }
            _ => Update { dirty: false, cmds: vec![] },
        }
    }

    /// Mouse wheel while the DefPick picker is open: over the preview panel it
    /// scrolls the preview one line (clamped); over the left panel (rows/Modal)
    /// it moves the selection one row (clamped, non-circular — wheel jumps
    /// across the wrap edge would disorient); anywhere else inert.
    pub(super) fn menu_wheel(&mut self, target: Option<HitTarget>, delta: i32) -> Update {
        match target {
            Some(HitTarget::MenuPreview) => {
                // Preview panel scrolls at the SAME step as the DETAIL pane (one
                // wheel tick moves `WHEEL_STEP` wrapped lines); clamp stays.
                let max = self.menu_preview_max_scroll.get();
                let cur = self.menu_preview_scroll_value();
                let next = (cur as i64 + (delta * WHEEL_STEP) as i64).clamp(0, max as i64) as usize;
                self.set_menu_preview_scroll(next)
            }
            Some(HitTarget::MenuItem(_)) | Some(HitTarget::Modal) => {
                let Mode::DefPick { defs, index, query, .. } = &self.mode else {
                    return Update { dirty: false, cmds: vec![] };
                };
                let flen = crate::selectors::filter_rows(defs, query, |d| d.name.clone()).len();
                let cur = *index;
                if flen == 0 {
                    return Update { dirty: false, cmds: vec![] };
                }
                let next = (cur as i64 + delta as i64).clamp(0, flen as i64 - 1) as usize;
                if next == cur {
                    return Update { dirty: false, cmds: vec![] };
                }
                let Mode::DefPick { index, preview_scroll, .. } = &mut self.mode else {
                    unreachable!()
                };
                *index = next;
                *preview_scroll = 0;
                let cmds = self.prefetch_full_def();
                Update { dirty: true, cmds }
            }
            _ => Update { dirty: false, cmds: vec![] },
        }
    }

    /// `Mode::SessionPick` key handling. The VIEW is
    /// row 0 = synthetic "New session" (always present) followed by the query-
    /// filtered loaded sessions. Esc closes to List; Up/Down (and ctrl+k/j,
    /// ctrl+p/n) move `index` circularly over the view; printable chars extend the
    /// filter and Backspace pops it (both auto-highlighting the first matching
    /// session so Enter resumes it, else falling back to "New session"); Enter
    /// resolves per the picker's `ret`: `Launch` builds a launch `Mode::Form`
    /// (row 0 → fresh, a session row → that session pinned); `Adhoc` returns to
    /// the stashed adhoc-create form with the session pinned/cleared.
    pub(super) fn session_pick_key(&mut self, ev: &crossterm::event::KeyEvent) -> Update {
        use crate::hit::ButtonKind;
        use crossterm::event::{KeyCode::*, KeyModifiers};
        let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
        let alt = ev.modifiers.contains(KeyModifiers::ALT);
        let Mode::SessionPick { items, index, query, repo, worktree, focus, .. } = &self.mode else {
            return Update { dirty: false, cmds: vec![] };
        };
        let filtered = crate::selectors::filter_rows(items, query, |s| s.label.clone());
        // View rows: New session (0), Create Worktree (1), then filtered sessions
        // (2..). `eff` clamps a stale index (e.g. a filter that emptied the
        // matches) back into the view.
        let total = filtered.len() + 2;
        let cur = *index;
        let focus = *focus;
        let repo = repo.clone();
        let worktree = worktree.clone();
        let eff = cur.min(total - 1);
        // Pre-resolve the picked session as OWNED data so the `&self.mode` borrow
        // ends before any arm reassigns `self.mode`.
        let chosen: Option<(String, String, Option<String>)> = if eff >= 2 {
            filtered
                .get(eff - 2)
                .and_then(|&i| items.get(i))
                .map(|s| (s.session_id.clone(), s.label.clone(), s.model.clone()))
        } else {
            None
        };
        // Opened from the adhoc-create form's session field: a confirmed "New
        // session"/resume pick returns to the stashed form; Esc/Cancel restore it
        // unchanged. (Create Worktree stays an escape hatch in both modes.)
        let is_adhoc_return =
            matches!(&self.mode, Mode::SessionPick { ret: SessionPickReturn::Adhoc { .. }, .. });
        match ev.code {
            Esc => {
                if is_adhoc_return {
                    self.cancel_adhoc_session_pick()
                } else {
                    self.note_esc_dismiss();
                    self.mode = Mode::List;
                    Update { dirty: true, cmds: vec![] }
                }
            }
            // Tab toggles the focused button (filter + list stay live: typing
            // filters, ↑/↓ move the selection at any time).
            Tab | BackTab => {
                if let Mode::SessionPick { focus, .. } = &mut self.mode {
                    *focus = match *focus {
                        ButtonKind::Confirm => ButtonKind::Cancel,
                        ButtonKind::Cancel => ButtonKind::Confirm,
                    };
                }
                Update { dirty: true, cmds: vec![] }
            }
            Up => self.session_pick_move(cur, total, -1),
            Down => self.session_pick_move(cur, total, 1),
            Char('k') | Char('p') if ctrl => self.session_pick_move(cur, total, -1),
            Char('j') | Char('n') if ctrl => self.session_pick_move(cur, total, 1),
            // Enter fires the FOCUSED button. Cancel closes; Next (Confirm) acts on
            // the highlighted row: New session / Create Worktree / resume.
            Enter => match focus {
                ButtonKind::Cancel => {
                    if is_adhoc_return {
                        self.cancel_adhoc_session_pick()
                    } else {
                        self.mode = Mode::List;
                        Update { dirty: true, cmds: vec![] }
                    }
                }
                // New session (row 0) / resume (row ≥2) opened from the adhoc form
                // return to it, pinning (or clearing) the session. Create Worktree
                // (row 1) still builds its own form (handled below).
                ButtonKind::Confirm if is_adhoc_return && eff != 1 => {
                    self.finish_adhoc_session_pick(chosen.map(|(sid, label, _)| (sid, label)))
                }
                ButtonKind::Confirm if eff == 1 => {
                    // Create Worktree → the launch form (model + branch/name +
                    // prompt); its Primary creates the worktree then enqueues the
                    // first task into it. Model is field 0 so initial focus lands
                    // on it (the app-wide model-first form standard).
                    let state = crate::view::form::FormState::new(
                        &format!("Create Worktree · {repo}"),
                        "Create",
                        vec![
                            self.model_field(&repo),
                            crate::view::form::Field::input("branch / worktree name", "", true),
                            crate::view::form::Field::textarea("prompt", "", true),
                        ],
                    );
                    self.mode = Mode::Form {
                        state,
                        action: FormAction::CreateWorktree { repo },
                    };
                    Update { dirty: true, cmds: vec![] }
                }
                ButtonKind::Confirm => {
                    // New session (row 0) or a resume row → the launch form
                    // (model dropdown + prompt); its Primary enqueues with the
                    // picked model (and pinned session, when resuming). A resume
                    // defaults the dropdown to the model that session already ran
                    // on (falling back to the project/global default when unknown).
                    let (resume_session_id, title, session_model) = match chosen {
                        Some((sid, label, model)) => {
                            (Some(sid), format!("Resume · {label}"), model)
                        }
                        None => (None, "New session".to_string(), None),
                    };
                    let state = crate::view::form::FormState::new(
                        &title,
                        "Enqueue",
                        vec![
                            self.model_field_defaulting(&repo, session_model.as_deref()),
                            crate::view::form::Field::textarea("prompt", "", true),
                        ],
                    );
                    self.mode = Mode::Form {
                        state,
                        action: FormAction::NewSession { repo, worktree, resume_session_id },
                    };
                    Update { dirty: true, cmds: vec![] }
                }
            },
            Backspace => {
                if let Mode::SessionPick { items, query, index, .. } = &mut self.mode {
                    query.pop();
                    Self::reset_session_index(items, query, index);
                }
                Update { dirty: true, cmds: vec![] }
            }
            Char(c) if !ctrl && !alt => {
                if let Mode::SessionPick { items, query, index, .. } = &mut self.mode {
                    query.push(c);
                    Self::reset_session_index(items, query, index);
                }
                Update { dirty: true, cmds: vec![] }
            }
            _ => Update { dirty: false, cmds: vec![] },
        }
    }

    /// Return from an adhoc-return `Mode::SessionPick` to the stashed create
    /// form, pinning the chosen session (`Some`) or clearing the pin (`None` =
    /// "New session"). Restores the target/model/prompt the user already entered
    /// and lands focus back on the session field.
    fn finish_adhoc_session_pick(&mut self, resume: Option<(String, String)>) -> Update {
        let Mode::SessionPick {
            worktree,
            ret: SessionPickReturn::Adhoc { state, action },
            ..
        } = std::mem::replace(&mut self.mode, Mode::List)
        else {
            return Update { dirty: false, cmds: vec![] };
        };
        let mut state = *state;
        let mut action = *action;
        if let FormAction::AdhocTask { resume_session_id, resume_label, resume_worktree, .. } =
            &mut action
        {
            match &resume {
                Some((sid, label)) => {
                    *resume_session_id = Some(sid.clone());
                    *resume_label = Some(label.clone());
                    *resume_worktree = Some(worktree);
                }
                None => {
                    *resume_session_id = None;
                    *resume_label = None;
                    *resume_worktree = None;
                }
            }
        }
        let label = Self::adhoc_session_label(resume.as_ref().map(|(_, l)| l.as_str()));
        state.set_field_value(crate::app::mode::adhoc_field::SESSION, &label);
        state.focus_field(crate::app::mode::adhoc_field::SESSION);
        self.mode = Mode::Form { state, action };
        Update { dirty: true, cmds: vec![] }
    }

    /// Esc / Cancel out of an adhoc-return `Mode::SessionPick`: restore the
    /// stashed create form UNCHANGED (the pin it already carried is preserved).
    fn cancel_adhoc_session_pick(&mut self) -> Update {
        let Mode::SessionPick { ret: SessionPickReturn::Adhoc { state, action }, .. } =
            std::mem::replace(&mut self.mode, Mode::List)
        else {
            return Update { dirty: false, cmds: vec![] };
        };
        self.mode = Mode::Form { state: *state, action: *action };
        Update { dirty: true, cmds: vec![] }
    }

    /// After a filter edit, land the highlight on the first matching session
    /// (view row 2) when the query is non-empty and something matches; otherwise
    /// the "New session" row (0). So typing to find a session auto-selects it
    /// (Enter resumes), while an empty/no-match filter defaults to a fresh task.
    fn reset_session_index(
        items: &[crate::event::SessionChoice],
        query: &str,
        index: &mut usize,
    ) {
        let flen = crate::selectors::filter_rows(items, query, |s| s.label.clone()).len();
        *index = if query.is_empty() || flen == 0 { 0 } else { 2 };
    }

    /// Move the session-picker highlight circularly over the view (`total` =
    /// filtered sessions + 2 for the New session / Create Worktree rows).
    fn session_pick_move(&mut self, cur: usize, total: usize, dir: i32) -> Update {
        let next = if total == 0 {
            0
        } else if dir < 0 {
            cur.checked_sub(1).unwrap_or(total - 1)
        } else if cur + 1 >= total {
            0
        } else {
            cur + 1
        };
        if let Mode::SessionPick { index, .. } = &mut self.mode {
            *index = next;
        }
        Update { dirty: true, cmds: vec![] }
    }

    /// Mouse wheel while the session picker is open: over its body (Modal /
    /// MenuItem) it moves the highlight one row, clamped and non-circular (a
    /// wheel jump across the wrap edge would disorient); anywhere else inert.
    pub(super) fn session_pick_wheel(&mut self, target: Option<HitTarget>, delta: i32) -> Update {
        if !matches!(target, Some(HitTarget::MenuItem(_)) | Some(HitTarget::Modal)) {
            return Update { dirty: false, cmds: vec![] };
        }
        let Mode::SessionPick { items, index, query, .. } = &self.mode else {
            return Update { dirty: false, cmds: vec![] };
        };
        let total = crate::selectors::filter_rows(items, query, |s| s.label.clone()).len() + 2;
        let cur = *index;
        let next = (cur as i64 + delta as i64).clamp(0, total as i64 - 1) as usize;
        if next == cur {
            return Update { dirty: false, cmds: vec![] };
        }
        if let Mode::SessionPick { index, .. } = &mut self.mode {
            *index = next;
        }
        Update { dirty: true, cmds: vec![] }
    }

    /// Route a left-click while the launcher is open: a `MenuItem(i)` (view row
    /// index) highlights that row and fires it as Next (New session / Create
    /// Worktree / resume); the `Next` button fires the highlighted row; `Cancel`
    /// (or an outside click) closes; the `Modal` body is inert.
    pub(super) fn route_session_pick_click(&mut self, target: Option<HitTarget>) -> Update {
        use crate::hit::ButtonKind;
        let fire_next = |app: &mut Self| {
            // Reuse the Enter resolution with focus forced onto Next.
            app.session_pick_key(&crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Enter,
                crossterm::event::KeyModifiers::NONE,
            ))
        };
        match target {
            Some(HitTarget::MenuItem(i)) => {
                if let Mode::SessionPick { index, focus, .. } = &mut self.mode {
                    *index = i;
                    *focus = ButtonKind::Confirm;
                }
                fire_next(self)
            }
            Some(HitTarget::Button(ButtonKind::Confirm)) => {
                if let Mode::SessionPick { focus, .. } = &mut self.mode {
                    *focus = ButtonKind::Confirm;
                }
                fire_next(self)
            }
            Some(HitTarget::Button(ButtonKind::Cancel)) => self.close_or_cancel_session_pick(),
            Some(HitTarget::Modal) => Update { dirty: false, cmds: vec![] },
            _ => self.close_or_cancel_session_pick(),
        }
    }

    /// Close a `Mode::SessionPick` on Cancel / outside-click: an adhoc-return
    /// picker restores its stashed create form; the launcher just closes to List.
    fn close_or_cancel_session_pick(&mut self) -> Update {
        if matches!(&self.mode, Mode::SessionPick { ret: SessionPickReturn::Adhoc { .. }, .. }) {
            self.cancel_adhoc_session_pick()
        } else {
            self.mode = Mode::List;
            Update { dirty: true, cmds: vec![] }
        }
    }
}
