//! Action-menu and bulk-menu handling for `App`.
//!
//! Opening the single-target / bulk action pickers, their key navigation and
//! filtering, preview scrolling, click routing, and executing a chosen
//! [`crate::action_menu::MenuAction`]. Split out of `app/mod.rs` verbatim (no
//! behavior change).

use super::*;

impl App {
    /// Build the action menu for the last-focused list pane's current selection.
    /// Returns `None` when nothing is selectable (empty pane). A multi-row range
    /// opens the bulk menu; the single-target case handles queue/worktrees (the
    /// tasks pane has no single-target menu — `open_actions_or_run` runs the
    /// highlighted def before reaching here).
    pub(super) fn open_action_menu(&mut self) -> Option<Mode> {
        // Bulk branch: a multi-row range on TASKS / WORKTREES opens the bulk menu
        // with eligibility frozen at open time. QUEUE is excluded — its bulk verbs
        // are the `r`/`x` chips, so `a` there always targets the cursor row's
        // single Resume menu regardless of range.
        {
            let ui = self.active_ui();
            let pane = ui.last_list_pane;
            let (start, end) = crate::view::selection_range(&ui.selections[pane.idx()]);
            if end > start && pane != ListPane::Queue {
                return self.open_bulk_menu(pane, start, end);
            }
        }
        let snap = self.snapshot.as_ref()?;
        let repo = self.active_repo()?;
        let ui = self.active_ui();
        let inside_tmux = std::env::var_os("TMUX").is_some();
        match ui.last_list_pane {
            ListPane::Queue => {
                let rows = crate::selectors::queue_rows(snap, &repo, self.now_epoch_s);
                let vis = crate::selectors::filter_rows(&rows, &ui.search[0], |r| r.summary.clone());
                let cursor = ui.selections[0].cursor.min(vis.len().saturating_sub(1));
                let row = vis.get(cursor).and_then(|&i| rows.get(i))?;
                let task = snap
                    .tasks
                    .iter()
                    .chain(snap.archived_recent.iter())
                    .find(|t| t.id == row.task_id)?;
                // Resume target: the selected run's recorded Claude session +
                // worktree path (read from its run record), falling back to the
                // task's `resume_session_id` and the snapshot worktree's path.
                let run = self.run_files.as_ref().filter(|(id, _)| id == &row.task_id).map(|(_, f)| f);
                let session_id = run
                    .and_then(|f| f.session_id.clone())
                    .or_else(|| task.resume_session_id.clone());
                let worktree_path = run.and_then(|f| f.worktree_path.clone()).or_else(|| {
                    task.target.worktree.as_deref().and_then(|w| {
                        snap.worktrees
                            .get(&repo)
                            .and_then(|wts| wts.iter().find(|i| i.name == w))
                            .map(|i| i.path.clone())
                    })
                });
                let (title, items) = crate::action_menu::queue_menu(
                    row,
                    session_id.as_deref(),
                    worktree_path.as_deref(),
                    inside_tmux,
                );
                Some(Mode::ActionMenu { title, items, index: 0, query: String::new(), preview_scroll: 0 })
            }
            // Tasks: single-target Enter runs the highlighted def directly
            // (`open_actions_or_run` intercepts before calling this); a bulk range
            // is handled by the guard above. Nothing to show here.
            ListPane::Tasks => None,
            ListPane::Worktrees => {
                let rows = crate::selectors::worktree_rows(snap, &repo);
                let vis = crate::selectors::filter_rows(&rows, &ui.search[2], |r| r.name.clone());
                let cursor = ui.selections[2].cursor.min(vis.len().saturating_sub(1));
                let row = vis.get(cursor).and_then(|&i| rows.get(i))?;
                let (title, items) = crate::action_menu::worktree_menu(&repo, row, inside_tmux);
                Some(Mode::ActionMenu { title, items, index: 0, query: String::new(), preview_scroll: 0 })
            }
        }
    }

    /// Clear a list pane's selection anchor on the active tab (collapse a range
    /// to a single cursor). Called before every bulk dispatch, mirroring the
    /// App.tsx `runBulk` clear-then-dispatch order.
    pub(super) fn clear_range(&mut self, pane: ListPane) {
        if let Some(repo) = self.active_repo()
            && let Some(ui) = self.ui_by_tab.get_mut(&repo) {
                ui.selections[pane.idx()].anchor = None;
            }
    }

    /// Clamp a frozen `[start, end]` selection span against the current visible
    /// row count. Returns `None` when nothing in the span survives (the visible
    /// set emptied), else `(start, hi, total)` with `start <= hi < vis_len` and
    /// `total` the surviving span width. Guards `vis[start..=hi]` from empty and
    /// inverted-range panics when a daemon snapshot shrinks the rows between the
    /// selection and the menu opening (`total` therefore counts survivors, so
    /// "(N of T)" never overcounts a range that partly scrolled off).
    pub(super) fn clamp_span(start: usize, end: usize, vis_len: usize) -> Option<(usize, usize, usize)> {
        if vis_len == 0 {
            return None;
        }
        let hi = end.min(vis_len - 1);
        let start = start.min(hi);
        Some((start, hi, hi - start + 1))
    }

    /// Build the bulk menu for a `[start, end]` inclusive range on `pane`,
    /// freezing eligibility (ids/names) into the returned `MenuAction`s at open
    /// time — a daemon push reshuffling rows mid-menu can't retarget the
    /// dispatch. Mirrors App.tsx `openBulkMenu`.
    pub(super) fn open_bulk_menu(&self, pane: ListPane, start: usize, end: usize) -> Option<Mode> {
        use crate::action_menu::{bulk_menu, BulkSelection};
        let snap = self.snapshot.as_ref()?;
        let repo = self.active_repo()?;
        let ui = self.active_ui();
        let (title, items) = match pane {
            // QUEUE has no bulk menu — `open_action_menu` never routes a queue
            // range here (its `r`/`x` chips carry the bulk verbs).
            ListPane::Queue => return None,
            ListPane::Tasks => {
                let defs = self.defs_by_project.get(&repo).cloned().unwrap_or_default();
                let vis = crate::selectors::filter_rows(&defs, &ui.search[1], |d| d.name.clone());
                let (start, hi, total) = Self::clamp_span(start, end, vis.len())?;
                let run_names: Vec<String> = vis[start..=hi]
                    .iter()
                    .filter_map(|&i| defs.get(i))
                    .filter(|d| d.args.is_empty())
                    .map(|d| d.name.clone())
                    .collect();
                bulk_menu(BulkSelection::Tasks { repo: repo.clone(), run_names, total })
            }
            ListPane::Worktrees => {
                let rows = crate::selectors::worktree_rows(snap, &repo);
                let vis = crate::selectors::filter_rows(&rows, &ui.search[2], |r| r.name.clone());
                let (start, hi, total) = Self::clamp_span(start, end, vis.len())?;
                let remove_names: Vec<String> = vis[start..=hi]
                    .iter()
                    .filter_map(|&i| rows.get(i))
                    .filter(|r| !r.is_session && !matches!(r.state, crate::selectors::WtState::Busy))
                    .map(|r| r.raw_name.clone())
                    .collect();
                bulk_menu(BulkSelection::Worktrees { repo: repo.clone(), remove_names, total })
            }
        };
        Some(Mode::ActionMenu { title, items, index: 0, query: String::new(), preview_scroll: 0 })
    }

    /// `Mode::ActionMenu` key handling (lazyvim-style picker). Esc closes;
    /// Up/Ctrl+k/Ctrl+p and Down/Ctrl+j/Ctrl+n move circularly over the FILTERED
    /// rows; Ctrl+d/Ctrl+u scroll the preview by half its height; Enter executes
    /// the highlighted enabled row (disabled rows inert); Backspace/printable
    /// edit the label filter, resetting highlight and preview scroll. `q` is no
    /// longer a close key — it types into the filter.
    pub(super) fn action_menu_key(&mut self, ev: &crossterm::event::KeyEvent) -> Update {
        use crossterm::event::{KeyCode::*, KeyModifiers};
        let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
        let alt = ev.modifiers.contains(KeyModifiers::ALT);
        let Mode::ActionMenu { items, index, query, .. } = &self.mode else {
            return Update { dirty: false, cmds: vec![] };
        };
        let filtered = crate::action_menu::filter_items(items, query);
        let flen = filtered.len();
        let cur = *index;
        // Extract the highlighted action up front (clone) so the immutable borrow
        // of `self.mode` ends before any arm mutates it.
        let chosen = filtered.get(cur).and_then(|&i| items.get(i)).cloned();
        match ev.code {
            Esc => {
                self.mode = Mode::List;
                Update { dirty: true, cmds: vec![] }
            }
            Up => self.action_menu_move(cur, flen, -1),
            Down => self.action_menu_move(cur, flen, 1),
            Char('k') | Char('p') if ctrl => self.action_menu_move(cur, flen, -1),
            Char('j') | Char('n') if ctrl => self.action_menu_move(cur, flen, 1),
            Enter => match chosen {
                Some(it) if it.disabled.is_none() => self.execute_menu_action(it.action),
                _ => Update { dirty: false, cmds: vec![] }, // disabled / no match: inert
            },
            Backspace => {
                if let Mode::ActionMenu { query, index, preview_scroll, .. } = &mut self.mode {
                    query.pop();
                    *index = 0;
                    *preview_scroll = 0;
                }
                Update { dirty: true, cmds: vec![] }
            }
            Char(c) if !ctrl && !alt => {
                if let Mode::ActionMenu { query, index, preview_scroll, .. } = &mut self.mode {
                    query.push(c);
                    *index = 0;
                    *preview_scroll = 0;
                }
                Update { dirty: true, cmds: vec![] }
            }
            _ => Update { dirty: false, cmds: vec![] },
        }
    }

    /// Move the action-menu highlight circularly over the filtered rows (the
    /// preview scroll resets — it belongs to the outgoing row). `cur` is the
    /// current filtered index, `flen` the filtered row count.
    fn action_menu_move(&mut self, cur: usize, flen: usize, dir: i32) -> Update {
        let next = if flen == 0 {
            0
        } else if dir < 0 {
            cur.checked_sub(1).unwrap_or(flen - 1)
        } else if cur + 1 >= flen {
            0
        } else {
            cur + 1
        };
        if let Mode::ActionMenu { index, preview_scroll, .. } = &mut self.mode {
            *index = next;
            *preview_scroll = 0;
        }
        Update { dirty: true, cmds: vec![] }
    }

    /// Current picker preview scroll (0 outside ActionMenu/DefPick/DefArgs). The
    /// run form carries its scroll inside the `ArgsForm`; the pickers on the mode.
    fn menu_preview_scroll_value(&self) -> usize {
        match &self.mode {
            Mode::ActionMenu { preview_scroll, .. } | Mode::DefPick { preview_scroll, .. } => {
                *preview_scroll
            }
            Mode::DefArgs { form } => form.preview_scroll,
            _ => 0,
        }
    }

    /// Set the picker preview scroll, reporting dirty only on change.
    fn set_menu_preview_scroll(&mut self, next: usize) -> Update {
        match &mut self.mode {
            Mode::ActionMenu { preview_scroll, .. } | Mode::DefPick { preview_scroll, .. } => {
                let changed = *preview_scroll != next;
                *preview_scroll = next;
                Update { dirty: changed, cmds: vec![] }
            }
            Mode::DefArgs { form } => {
                let changed = form.preview_scroll != next;
                form.preview_scroll = next;
                Update { dirty: changed, cmds: vec![] }
            }
            _ => Update { dirty: false, cmds: vec![] },
        }
    }

    /// Mouse wheel while a picker (ActionMenu/DefPick) is open: over the preview
    /// panel it scrolls the preview one line (clamped); over the left panel
    /// (rows/Modal) it moves the selection one row (clamped, non-circular —
    /// wheel jumps across the wrap edge would disorient); anywhere else inert.
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
                let (flen, cur, is_def_pick) = match &self.mode {
                    Mode::ActionMenu { items, index, query, .. } => {
                        (crate::action_menu::filter_items(items, query).len(), *index, false)
                    }
                    Mode::DefPick { defs, index, query, .. } => (
                        crate::selectors::filter_rows(defs, query, |d| d.name.clone()).len(),
                        *index,
                        true,
                    ),
                    _ => return Update { dirty: false, cmds: vec![] },
                };
                if flen == 0 {
                    return Update { dirty: false, cmds: vec![] };
                }
                let next = (cur as i64 + delta as i64).clamp(0, flen as i64 - 1) as usize;
                if next == cur {
                    return Update { dirty: false, cmds: vec![] };
                }
                match &mut self.mode {
                    Mode::ActionMenu { index, preview_scroll, .. }
                    | Mode::DefPick { index, preview_scroll, .. } => {
                        *index = next;
                        *preview_scroll = 0;
                    }
                    _ => unreachable!(),
                }
                let cmds = if is_def_pick { self.prefetch_full_def() } else { Vec::new() };
                Update { dirty: true, cmds }
            }
            _ => Update { dirty: false, cmds: vec![] },
        }
    }

    /// Perform a chosen (enabled) menu action: an RPC dispatch or a mode
    /// transition into a follow-up form/confirm. Always closes the menu first
    /// (`Mode::List`), then the form/confirm branches re-open the appropriate mode.
    fn execute_menu_action(&mut self, action: crate::action_menu::MenuAction) -> Update {
        use crate::action_menu::MenuAction as M;
        self.mode = Mode::List;
        match action {
            M::Resume { path, session_id } => {
                Update { dirty: true, cmds: vec![Cmd::TmuxResume { path, session_id }] }
            }
            M::TaskFresh { worktree } => {
                self.mode = Mode::AddTask { worktree, session: SessionMode::Fresh, input: tui_input::Input::default() };
                Update { dirty: true, cmds: vec![] }
            }
            M::TaskMain { worktree } => {
                self.mode = Mode::AddTask { worktree, session: SessionMode::Main, input: tui_input::Input::default() };
                Update { dirty: true, cmds: vec![] }
            }
            M::OpenTmux { path } => Update { dirty: true, cmds: vec![Cmd::OpenTmux { path }] },
            M::RemoveWorktree { repo, name, branch } => {
                self.mode = Mode::ConfirmRemove { repo, worktree: name, branch };
                Update { dirty: true, cmds: vec![] }
            }
            // --- Bulk actions. Range cleared before dispatch; the frozen
            // ids/names ride inside the action. Verbs are past tense to feed
            // `seq_summary` ("started 1", …). Queue bulk (rerun/skip) is gone —
            // the queue `r`/`x` chips carry those verbs (see `App::requeue_selected`
            // / `App::cancel_selected`). ---
            M::BulkRunDefs { repo, names } => {
                self.clear_range(ListPane::Tasks);
                // Verb "started" per parity oracle (App.tsx:698 / app.test.tsx:1573).
                Update { dirty: true, cmds: vec![Cmd::RpcSeq {
                    verb: "started".into(),
                    calls: names
                        .into_iter()
                        .map(|name| RpcCall {
                            method: "runDefinition".into(),
                            params: serde_json::json!({ "repo": repo, "name": name, "args": [], "source": "tui" }),
                        })
                        .collect(),
                    invalidate_defs_for: Some(repo),
                }] }
            }
            M::BulkRemove { repo, names } => {
                self.mode = Mode::ConfirmBulkRemove { repo, names };
                Update { dirty: true, cmds: vec![] }
            }
        }
    }

    /// Route a left-click while an action menu is open: a `MenuItem` selects and
    /// (if enabled) executes that row; the `Modal` body is inert; a click on
    /// anything else (or nothing) closes the menu.
    pub(super) fn route_menu_click(&mut self, target: Option<HitTarget>) -> Update {
        match target {
            Some(HitTarget::MenuItem(i)) => {
                // `i` is a FILTERED display index; resolve it to the underlying
                // item through the same filter, set the highlight, and execute if
                // the row is enabled.
                let chosen = if let Mode::ActionMenu { items, index, query, preview_scroll, .. } =
                    &mut self.mode
                {
                    let filtered = crate::action_menu::filter_items(items, query);
                    match filtered.get(i).copied() {
                        Some(actual) => {
                            if *index != i {
                                *index = i;
                                *preview_scroll = 0; // highlight moved → new preview
                            }
                            items.get(actual).cloned()
                        }
                        None => None,
                    }
                } else {
                    None
                };
                match chosen {
                    Some(it) if it.disabled.is_none() => self.execute_menu_action(it.action),
                    _ => Update { dirty: true, cmds: vec![] }, // disabled row: highlight only
                }
            }
            // Panel-body / preview clicks are inert (the wheel scrolls the preview).
            Some(HitTarget::Modal) | Some(HitTarget::MenuPreview) => {
                Update { dirty: false, cmds: vec![] }
            }
            _ => {
                self.mode = Mode::List; // click outside the popup closes it
                Update { dirty: true, cmds: vec![] }
            }
        }
    }
}
