//! List-mode action handling for `App`.
//!
//! `apply_action` maps an [`AppAction`] (resolved from a keypress) to state
//! changes and commands, plus the run/bulk helpers and the `Cmd` builders for
//! running a definition and creating a worktree. Split out of `app/mod.rs`
//! verbatim (no behavior change).

use super::*;

/// One resolved QUEUE selection row for the `r`/`x` verbs: `(task_id, status,
/// archived)`. A small alias to keep `queue_selection_rows`' return type legible.
type QueueSelRow = (String, TaskStatus, bool);

/// Kebab-case status name for the queue `r`/`x` no-op status lines
/// ("cannot requeue a queued task").
fn status_kebab(s: TaskStatus) -> &'static str {
    match s {
        TaskStatus::Queued => "queued",
        TaskStatus::NeedsInput => "needs-input",
        TaskStatus::Running => "running",
        TaskStatus::Done => "done",
        TaskStatus::Failed => "failed",
        TaskStatus::Cancelled => "cancelled",
        TaskStatus::Skipped => "skipped",
        TaskStatus::VerifyFailed => "verify-failed",
        TaskStatus::Unknown => "unknown",
    }
}

impl App {
    /// Resolve an `AppAction` from the keymap into state mutations + commands.
    /// Pure per-key logic lives in `keymap::list_mode_action`; per-tab state and
    /// focus-dependent semantics (g/G) resolve here.
    pub(crate) fn apply_action(&mut self, action: AppAction) -> Update {
        use AppAction as A;
        let mut cmds = Vec::new();
        let dirty = match action {
            A::None => false,
            A::Quit => {
                cmds.push(Cmd::Quit);
                false
            }
            A::Help => {
                self.mode = Mode::Help;
                true
            }
            A::Settings => {
                self.mode = Mode::Settings;
                // Fetch once on first open; thereafter the cached value renders
                // instantly. `None` means never fetched (Some(None) is a cached
                // failure that must not re-fetch on every open).
                if self.settings.is_none() {
                    cmds.push(Cmd::FetchSettings);
                }
                true
            }
            A::SwitchTab(i) => {
                let tabs = self
                    .snapshot
                    .as_ref()
                    .map(|s| crate::selectors::build_tabs(s).len())
                    .unwrap_or(0);
                if i < tabs && i != self.active_tab {
                    self.active_tab = i;
                    self.schedule_run_read(&mut cmds, 120);
                    true
                } else {
                    false
                }
            }
            A::CycleTab(d) => {
                let tabs = self
                    .snapshot
                    .as_ref()
                    .map(|s| crate::selectors::build_tabs(s).len())
                    .unwrap_or(0);
                if tabs == 0 {
                    false
                } else {
                    let base = self.active_tab.min(tabs - 1) as i64;
                    self.active_tab = ((base + d as i64).rem_euclid(tabs as i64)) as usize;
                    self.schedule_run_read(&mut cmds, 120);
                    true
                }
            }
            A::CyclePane(d) => {
                // Detail is display-only — the cycle covers only the three list
                // panes, upholding the "focus is always a list pane" invariant.
                const ORDER: [PaneId; 3] = [PaneId::Queue, PaneId::Tasks, PaneId::Worktrees];
                let cur = ORDER.iter().position(|p| *p == self.ui().focus).unwrap_or(0) as i64;
                let next = ORDER[((cur + d as i64).rem_euclid(3)) as usize];
                self.set_focus(next);
                self.schedule_run_read(&mut cmds, 120);
                true
            }
            A::MoveCursor(d) => match self.focused_list() {
                Some(pane) => {
                    let len = self.visible_len(pane);
                    if len == 0 {
                        false
                    } else {
                        // Circular navigation: k on the first row lands on the
                        // last, j on the last wraps to the first. (Extend-
                        // selection stays clamped — a wrapping range would be
                        // ambiguous.)
                        let cur = self.ui().selections[pane as usize].cursor.min(len - 1) as i64;
                        let next = (cur + d as i64).rem_euclid(len as i64) as usize;
                        self.set_cursor(pane, next, &mut cmds)
                    }
                }
                None => false,
            },
            A::ExtendSelection(d) => match self.focused_list() {
                Some(pane) => {
                    let len = self.visible_len(pane);
                    if len == 0 {
                        false
                    } else {
                        let sel = self.ui().selections[pane as usize];
                        let next = ((sel.cursor as i64 + d as i64).max(0) as usize).min(len - 1);
                        // Collapse the anchor when the range shrinks to one row so
                        // Esc falls through to the filter stage (parity).
                        let base = sel.anchor.unwrap_or(sel.cursor);
                        let anchor = if next == base { None } else { Some(base) };
                        let changed = next != sel.cursor || anchor != sel.anchor;
                        self.ui().selections[pane as usize] = Selection { cursor: next, anchor };
                        if changed {
                            self.ui().scroll_offset = 0;
                            // Extending the worktree selection moves the "current"
                            // worktree → reset the detail lane-task row cursor.
                            if pane == ListPane::Worktrees {
                                self.ui().detail_row = 0;
                            }
                            self.schedule_run_read(&mut cmds, 120);
                        }
                        changed
                    }
                }
                None => false,
            },
            // Home/End scroll the detail pane unconditionally — no list branch,
            // so the left-side cursor never moves even though a list pane is
            // focused.
            A::DetailScrollEdge(dir) => self.detail_scroll_edge(dir),
            A::SwitchSubTab(i) => self.set_sub_tab_clamped(i, &mut cmds),
            A::CycleSubTab(d) => {
                let (kind, cur) = self.detail_kind_and_subtab();
                let count = crate::detail::sub_tab_names(kind).len();
                if count == 0 {
                    false
                } else {
                    let next = ((cur as i64 + d as i64).rem_euclid(count as i64)) as usize;
                    self.set_sub_tab_clamped(next, &mut cmds)
                }
            }
            A::OpenSearch => match self.focused_list() {
                Some(pane) => {
                    self.mode = Mode::Search { pane };
                    true
                }
                None => false,
            },
            A::ClearEsc => self.clear_esc(),
            A::OpenActionMenu => {
                let u = self.open_actions_or_run();
                cmds.extend(u.cmds);
                u.dirty
            }
            A::OpenTaskMenu => {
                // `t` is a WORKTREES chip (keymap-gated there), so the def picker
                // always carries the selected worktree row's context. Also
                // prefetches the first highlighted def's prompt for the right pane.
                cmds.extend(self.open_task_menu());
                true
            }
            A::RunSelectedDef => {
                // `r` is a TASKS chip (keymap-gated there): a single-row selection
                // runs the highlighted def, a multi-row range opens the bulk menu
                // (`r` carries the bulk affordance since the tasks pane has no
                // `[a]ctions` chip).
                let u = self.run_or_bulk_selected_task_def();
                cmds.extend(u.cmds);
                u.dirty
            }
            A::Create => {
                // `Create` (`c`) is a QUEUE chip only — it opens the adhoc-task
                // prompt. The worktrees pane's standalone create modal was folded
                // into the launcher (`r` → Create Worktree row), so `c` no longer
                // shows a chip there and this arm is only reached for QUEUE.
                if self.active_ui().last_list_pane == ListPane::Queue {
                    self.mode = Mode::AddTask {
                        worktree: None,
                        resume_session_id: None,
                        resume_label: None,
                        editor: Default::default(),
                    };
                }
                true
            }
            A::ToggleCollapse => match self.focused_list() {
                // Collapse/expand the focused list pane; detail focus is a no-op.
                Some(pane) => {
                    self.toggle_collapse(pane, &mut cmds);
                    true
                }
                None => false,
            },
            // `j`/`k`: move the worktree detail row cursor when the detail shows a
            // selectable lane-task list, else scroll the detail one line.
            A::DetailRowMove(d) => self.detail_row_move(d),
            // `Enter`: jump to the selected lane task's queue detail.
            A::OpenDetailRow => {
                let u = self.open_detail_row();
                cmds.extend(u.cmds);
                u.dirty
            }
            // `r` on QUEUE: re-queue the selected task(s).
            A::RequeueSelected => {
                let u = self.requeue_selected();
                cmds.extend(u.cmds);
                u.dirty
            }
            // `x` on QUEUE: cancel (skip/stop by status) the selected task(s).
            A::CancelSelected => {
                let u = self.cancel_selected();
                cmds.extend(u.cmds);
                u.dirty
            }
            // `r` on WORKTREES: open the session picker for a new task on the worktree.
            A::NewTaskOnWorktree => {
                let u = self.new_task_on_worktree();
                cmds.extend(u.cmds);
                u.dirty
            }
            // `g` on WORKTREES: open the selected worktree/session in tmux.
            A::GotoWorktree => {
                let u = self.goto_worktree();
                cmds.extend(u.cmds);
                u.dirty
            }
            // `x` on WORKTREES: confirm removing the selected worktree.
            A::RemoveSelectedWorktree => {
                let u = self.remove_selected_worktree();
                cmds.extend(u.cmds);
                u.dirty
            }
        };
        Update { dirty, cmds }
    }

    /// The QUEUE selection resolved to `(task_id, status, archived)` rows over the
    /// visible (search-filtered) span, plus whether the selection is a multi-row
    /// range. `None` when there is no snapshot/repo/rows. Shared by the `r`
    /// (re-queue) and `x` (cancel) verbs so both read the exact rows on screen.
    fn queue_selection_rows(&self) -> Option<(Vec<QueueSelRow>, bool)> {
        let snap = self.snapshot.as_ref()?;
        let repo = self.active_repo()?;
        let ui = self.active_ui();
        let rows = crate::selectors::queue_rows(snap, &repo, self.now_epoch_s);
        let vis = crate::selectors::filter_rows(&rows, &ui.search[0], |r| r.summary.clone());
        let (start, end) = crate::view::selection_range(&ui.selections[0]);
        let is_range = end > start;
        let (start, hi, _total) = Self::clamp_span(start, end, vis.len())?;
        let sels = vis[start..=hi]
            .iter()
            .filter_map(|&i| rows.get(i))
            .map(|r| {
                let status = snap
                    .tasks
                    .iter()
                    .chain(snap.archived_recent.iter())
                    .find(|t| t.id == r.task_id)
                    .map(|t| t.status)
                    .unwrap_or(TaskStatus::Unknown);
                (r.task_id.clone(), status, r.archived)
            })
            .collect();
        Some((sels, is_range))
    }

    /// `r` on QUEUE. Terminal (done/failed/unknown) and needs-input tasks re-queue
    /// via the `retry` RPC; queued/running (and archived) rows are ineligible. A
    /// single row dispatches one call (with a per-status no-op status line when
    /// ineligible); a range fires an `RpcSeq` over every eligible member with the
    /// familiar "requeued N" count feedback.
    pub(super) fn requeue_selected(&mut self) -> Update {
        let requeue_ok = |s: TaskStatus| {
            matches!(
                s,
                TaskStatus::Failed
                    | TaskStatus::VerifyFailed
                    | TaskStatus::NeedsInput
                    | TaskStatus::Done
                    | TaskStatus::Unknown
            )
        };
        let Some((rows, is_range)) = self.queue_selection_rows() else {
            return Update::default();
        };
        if !is_range {
            let Some((id, status, archived)) = rows.into_iter().next() else {
                return Update::default();
            };
            if archived {
                self.status_line = Some("cannot requeue an archived task".into());
                return Update { dirty: true, cmds: vec![] };
            }
            if !requeue_ok(status) {
                self.status_line = Some(format!("cannot requeue a {} task", status_kebab(status)));
                return Update { dirty: true, cmds: vec![] };
            }
            let cmd = self.dispatch_rpc("requeue task", "retry", serde_json::json!({ "id": id }), RpcOpts::default());
            return Update { dirty: true, cmds: vec![cmd] };
        }
        let ids: Vec<String> =
            rows.into_iter().filter(|(_, s, arch)| !arch && requeue_ok(*s)).map(|(id, _, _)| id).collect();
        if ids.is_empty() {
            self.status_line = Some("no re-queueable tasks in selection".into());
            return Update { dirty: true, cmds: vec![] };
        }
        self.clear_range(ListPane::Queue);
        Update {
            dirty: true,
            cmds: vec![Cmd::RpcSeq {
                verb: "requeued".into(),
                calls: ids
                    .into_iter()
                    .map(|id| RpcCall { method: "retry".into(), params: serde_json::json!({ "id": id }) })
                    .collect(),
                invalidate_defs_for: None,
            }],
        }
    }

    /// `x` on QUEUE (and the `[x] cancel` chip). Cancel ALWAYS confirms first: it
    /// freezes the per-task skip/stop RPCs (queued/needs-input → `skip`, running →
    /// `stop`; terminal/archived rows are ineligible) and opens `Mode::Confirm`.
    /// Enter/y in that dialog dispatches (see `update`); a selection with nothing
    /// cancellable never opens the dialog — it sets a status line instead.
    pub(super) fn cancel_selected(&mut self) -> Update {
        // The RPC method for a cancellable status, or None when it can't cancel.
        let cancel_method = |s: TaskStatus| match s {
            TaskStatus::Queued | TaskStatus::NeedsInput => Some("skip"),
            TaskStatus::Running => Some("stop"),
            _ => None,
        };
        let Some((rows, _is_range)) = self.queue_selection_rows() else {
            return Update::default();
        };
        // Eligible rows in row order, keeping status so the summary can describe
        // them; archived rows are never cancellable.
        let eligible: Vec<(String, TaskStatus)> = rows
            .into_iter()
            .filter(|(_, _, arch)| !arch)
            .filter_map(|(id, s, _)| cancel_method(s).map(|_| (id, s)))
            .collect();
        if eligible.is_empty() {
            self.status_line = Some("nothing to cancel in selection".into());
            return Update { dirty: true, cmds: vec![] };
        }
        let stops = eligible.iter().filter(|(_, s)| matches!(s, TaskStatus::Running)).count();
        let n = eligible.len();
        let summary = if n == 1 {
            format!("cancel 1 {} task", status_kebab(eligible[0].1))
        } else if stops > 0 {
            format!("cancel {n} tasks ({stops} running will be stopped)")
        } else {
            format!("cancel {n} tasks")
        };
        let calls: Vec<RpcCall> = eligible
            .into_iter()
            .map(|(id, s)| RpcCall {
                method: cancel_method(s).expect("filtered to cancellable").into(),
                params: serde_json::json!({ "id": id }),
            })
            .collect();
        self.mode = Mode::Confirm {
            title: format!("Cancel {n} task{}", if n == 1 { "" } else { "s" }),
            body: vec![summary],
            confirm_label: "Cancel tasks".into(),
            action: ConfirmAction::CancelTasks { calls },
            focus: crate::hit::ButtonKind::Confirm,
        };
        Update { dirty: true, cmds: vec![] }
    }

    /// `j`/`k` in list mode. When the detail pane shows the worktree lane-task
    /// list, move its row cursor (clamped to the task count); otherwise the vim
    /// keys never go dead — they scroll the detail one line like the wheel.
    pub(super) fn detail_row_move(&mut self, d: i32) -> bool {
        // Only the worktree lane-task list has a row cursor; anything else (or an
        // empty list) scrolls the detail one line so j/k are never dead.
        let len = match self.current_detail_context() {
            crate::detail::DetailContext::Worktree { lane_tasks, .. } if !lane_tasks.is_empty() => {
                lane_tasks.len()
            }
            _ => return self.detail_scroll(d),
        };
        let cur = self.ui().detail_row.min(len - 1);
        let next = (cur as i64 + d as i64).clamp(0, len as i64 - 1) as usize;
        // A stale (out-of-range) stored value re-clamps to `next` too, so compare
        // against the raw stored value to still repaint in that case.
        let changed = next != self.ui().detail_row;
        self.ui().detail_row = next;
        changed
    }

    /// `Enter` over the worktree detail's selected lane task: focus that task in
    /// the QUEUE pane and switch the detail to its Run/transcript view (mirrors
    /// clicking the queue row). Inert on non-worktree detail contexts; a status
    /// line explains when the task is filtered out of the current queue view.
    pub(super) fn open_detail_row(&mut self) -> Update {
        let crate::detail::DetailContext::Worktree { lane_tasks, .. } =
            self.current_detail_context()
        else {
            return Update::default();
        };
        if lane_tasks.is_empty() {
            return Update::default();
        }
        let idx = self.ui().detail_row.min(lane_tasks.len() - 1);
        let task_id = lane_tasks[idx].id.clone();
        // Locate the task in the CURRENT (search-filtered) queue view — the same
        // rows the queue detail derives from.
        let qi = crate::view::compute(self).queue.iter().position(|r| r.task_id == task_id);
        match qi {
            Some(qi) => {
                let mut cmds = Vec::new();
                // Focus + select the queue row (last_list_pane → Queue drives the
                // detail to Run), then pin the transcript sub-tab.
                self.set_focus(PaneId::Queue);
                self.set_cursor(ListPane::Queue, qi, &mut cmds);
                self.ui().sub_tab[DetailKind::Run as usize] = 0;
                self.schedule_run_read(&mut cmds, 120);
                Update { dirty: true, cmds }
            }
            None => {
                self.status_line = Some("task not in queue view".to_string());
                Update { dirty: true, cmds: Vec::new() }
            }
        }
    }

    /// `a` / double-click over a list row (Enter is unbound in list mode). A
    /// single-row selection on the TASKS pane runs the highlighted definition
    /// directly (no menu hop — zero-arg defs dispatch immediately, defs with
    /// args open the run form); a bulk range on any pane and single rows on
    /// queue/worktrees open the action menu.
    pub(super) fn open_actions_or_run(&mut self) -> Update {
        let ui = self.active_ui();
        let pane = ui.last_list_pane;
        let (start, end) = crate::view::selection_range(&ui.selections[pane.idx()]);
        if end == start && pane == ListPane::Tasks {
            return self.run_selected_task_def();
        }
        match self.open_action_menu() {
            Some(mode) => self.mode = mode,
            None => self.status_line = Some("nothing selected".into()),
        }
        Update { dirty: true, cmds: vec![] }
    }

    /// The tasks pane's `r` (key and `[r]un` chip): selection-aware. A single-row
    /// selection runs the highlighted def via [`Self::run_selected_task_def`]; a
    /// multi-row range opens the bulk menu — the same range routing
    /// `open_actions_or_run` gives the tasks pane via [`Self::open_bulk_menu`].
    /// The tasks pane dropped its `[a]ctions` chip, so `r` owns both cases.
    fn run_or_bulk_selected_task_def(&mut self) -> Update {
        let (start, end) =
            crate::view::selection_range(&self.active_ui().selections[ListPane::Tasks.idx()]);
        if end > start {
            match self.open_bulk_menu(ListPane::Tasks, start, end) {
                Some(mode) => self.mode = mode,
                None => self.status_line = Some("nothing selected".into()),
            }
            return Update { dirty: true, cmds: vec![] };
        }
        self.run_selected_task_def()
    }

    /// Run the TASKS pane's highlighted definition directly (single-row Enter /
    /// double-click, and the single-row branch of [`Self::run_or_bulk_selected_task_def`];
    /// a multi-row range goes to the bulk menu there instead). Resolves the def
    /// from the current filtered selection: a zero-arg def dispatches
    /// `runDefinition`; a def with args opens the run form with an ambient
    /// worktree overlay (initial values from the selected worktree row) and
    /// fetches its prompt for the right panel. Mirrors the def-picker `Enter`
    /// path minus the explicit worktree target.
    fn run_selected_task_def(&mut self) -> Update {
        let Some(repo) = self.active_repo() else {
            return Update { dirty: false, cmds: vec![] };
        };
        let ui = self.active_ui();
        let defs = self.defs_by_project.get(&repo).cloned().unwrap_or_default();
        let vis = crate::selectors::filter_rows(&defs, &ui.search[ListPane::Tasks.idx()], |d| d.name.clone());
        let cursor = ui.selections[ListPane::Tasks.idx()].cursor.min(vis.len().saturating_sub(1));
        let Some(def) = vis.get(cursor).and_then(|&i| defs.get(i)).cloned() else {
            self.status_line = Some("nothing selected".into());
            return Update { dirty: true, cmds: vec![] };
        };
        if def.args.is_empty() {
            return Update {
                dirty: true,
                cmds: vec![Self::run_definition_cmd(&def.repo, &def.name, &[], None)],
            };
        }
        let rows = self.active_worktree_rows();
        let selected = self.selected_worktree_row();
        let (args, initial) =
            crate::worktree_context::ambient_run_args(&def.args, &rows, selected.as_ref());
        let cmds = self.open_def_args(def.repo, def.name, args, HashMap::new(), initial, None);
        Update { dirty: true, cmds }
    }

    /// Build the fire-and-forget `runDefinition` command. Client timeout is
    /// treated as success (discovery can outlive it; the push subscription
    /// re-syncs), and a successful run invalidates the repo's def summaries.
    pub(super) fn run_definition_cmd(repo: &str, name: &str, values: &[String], worktree: Option<&str>) -> Cmd {
        let mut params = serde_json::json!({
            "repo": repo, "name": name, "args": values, "source": "tui",
        });
        if let Some(wt) = worktree {
            params["worktree"] = serde_json::Value::String(wt.to_string());
        }
        Cmd::Rpc {
            label: "run".into(),
            call: RpcCall { method: "runDefinition".into(), params },
            timeout_ms: 5000,
            timeout_is_ok: true,
            invalidate_defs_for: Some(repo.to_string()),
        }
    }

    /// Build the fire-and-forget create command. The dedicated Cmd (not the
    /// generic Rpc) so its handler can read the reply's `path` and either
    /// auto-open a tmux window (create-only, `enqueue: None`) or enqueue a first
    /// task into the new worktree (`enqueue: Some`); budget/error semantics live
    /// there. Reused by the launcher's Create Worktree form.
    pub(super) fn create_worktree_cmd(
        repo: &str,
        name: &str,
        enqueue: Option<crate::event::EnqueueAfter>,
    ) -> Cmd {
        Cmd::CreateWorktree { repo: repo.to_string(), name: name.to_string(), enqueue }
    }

    /// Active project's worktree rows (unfiltered), used for ambient overlays.
    fn active_worktree_rows(&self) -> Vec<crate::selectors::WorktreeRow> {
        match (&self.snapshot, self.active_repo()) {
            (Some(snap), Some(repo)) => crate::selectors::worktree_rows(snap, &repo),
            _ => Vec::new(),
        }
    }

    /// Currently-selected worktree row (clamped cursor into the pane's rows).
    pub(super) fn selected_worktree_row(&self) -> Option<crate::selectors::WorktreeRow> {
        let rows = self.active_worktree_rows();
        let cursor = self
            .active_repo()
            .and_then(|r| self.ui_by_tab.get(&r))
            .map(|ui| ui.selections[ListPane::Worktrees.idx()].cursor)
            .unwrap_or(0);
        rows.into_iter().nth(cursor)
    }

    /// The WORKTREES row under the cursor in the CURRENT (search-filtered) view —
    /// the exact row the `r`/`g`/`x` verbs act on. Mirrors the resolution the
    /// retired worktree action menu used (`open_action_menu`'s worktrees arm):
    /// cursor is an index into the FILTERED rows, mapped back to the full set.
    fn selected_worktree_row_filtered(&self) -> Option<crate::selectors::WorktreeRow> {
        let snap = self.snapshot.as_ref()?;
        let repo = self.active_repo()?;
        let ui = self.active_ui();
        let rows = crate::selectors::worktree_rows(snap, &repo);
        let vis = crate::selectors::filter_rows(&rows, &ui.search[ListPane::Worktrees.idx()], |r| {
            r.name.clone()
        });
        let cursor = ui.selections[ListPane::Worktrees.idx()].cursor.min(vis.len().saturating_sub(1));
        vis.get(cursor).and_then(|&i| rows.get(i)).cloned()
    }

    /// `r` on WORKTREES (and the `[r]un` chip there): open the session picker
    /// (`Mode::SessionPick`) for the selected worktree and kick off the
    /// `listSessions` fetch. The picker's Enter carries the chosen session (or a
    /// fresh `Mode::AddTask`) into the worktree-targeted new-task flow. Session
    /// rows are interactive sessions, not worktrees, so they can't host a task →
    /// status line, no mode change.
    pub(super) fn new_task_on_worktree(&mut self) -> Update {
        let Some(repo) = self.active_repo() else {
            return Update::default();
        };
        let Some(row) = self.selected_worktree_row_filtered() else {
            self.status_line = Some("no worktree selected".into());
            return Update { dirty: true, cmds: vec![] };
        };
        if row.is_session {
            self.status_line = Some("tasks target worktrees, not sessions".into());
            return Update { dirty: true, cmds: vec![] };
        }
        let worktree = row.raw_name.clone();
        self.mode = Mode::SessionPick {
            repo: repo.clone(),
            worktree: worktree.clone(),
            items: Vec::new(),
            loading: true,
            index: 0,
            query: String::new(),
            focus: crate::hit::ButtonKind::Confirm,
        };
        Update { dirty: true, cmds: vec![Cmd::FetchSessions { repo, worktree }] }
    }

    /// `g` on WORKTREES (and the `[g]oto` chip): open the selected worktree (or
    /// session) in a new tmux window. The daemon drives tmux, so it is inert with
    /// a status line outside tmux. Session rows resolve to their cwd path.
    pub(super) fn goto_worktree(&mut self) -> Update {
        if !self.inside_tmux {
            self.status_line = Some("not inside tmux".into());
            return Update { dirty: true, cmds: vec![] };
        }
        let Some(row) = self.selected_worktree_row_filtered() else {
            self.status_line = Some("no worktree selected".into());
            return Update { dirty: true, cmds: vec![] };
        };
        Update { dirty: true, cmds: vec![Cmd::OpenTmux { path: row.path.clone() }] }
    }

    /// `x` on WORKTREES (and the `[x] remove` chip): selection-aware, mirroring
    /// the tasks pane's `r`. A multi-row range opens the bulk remove menu
    /// (eligibility frozen at open time via [`Self::open_bulk_menu`]); a single
    /// row confirms removing just that worktree (opens `Mode::Confirm`; the
    /// `y` handler dispatches the `removeWorktree` RPC). A session row isn't a
    /// worktree and a busy worktree has a task running → status line, no confirm.
    pub(super) fn remove_selected_worktree(&mut self) -> Update {
        let Some(repo) = self.active_repo() else {
            return Update::default();
        };
        // A multi-row range opens the bulk remove menu (the worktrees pane dropped
        // its `[a]ctions` chip, so `x` carries the bulk affordance now).
        let (start, end) =
            crate::view::selection_range(&self.active_ui().selections[ListPane::Worktrees.idx()]);
        if end > start {
            match self.open_bulk_menu(ListPane::Worktrees, start, end) {
                Some(mode) => self.mode = mode,
                None => self.status_line = Some("nothing selected".into()),
            }
            return Update { dirty: true, cmds: vec![] };
        }
        let Some(row) = self.selected_worktree_row_filtered() else {
            self.status_line = Some("no worktree selected".into());
            return Update { dirty: true, cmds: vec![] };
        };
        if row.is_session {
            self.status_line = Some("not a worktree".into());
            return Update { dirty: true, cmds: vec![] };
        }
        if matches!(row.state, crate::selectors::WtState::Busy) {
            self.status_line = Some("a task is running here".into());
            return Update { dirty: true, cmds: vec![] };
        }
        let worktree = row.raw_name.clone();
        let branch = row.branch.clone();
        let branch_line =
            if branch.is_empty() { String::new() } else { format!(" on branch {branch}") };
        self.mode = Mode::Confirm {
            title: "Remove worktree".into(),
            // No leading spaces — the modal's interior padding provides the inset.
            body: vec![
                format!("Remove {worktree}{branch_line}?"),
                "This discards uncommitted changes and deletes the local branch.".into(),
            ],
            confirm_label: "Remove".into(),
            action: ConfirmAction::RemoveWorktree { repo, worktree },
            focus: crate::hit::ButtonKind::Confirm,
        };
        Update { dirty: true, cmds: vec![] }
    }

    /// `(task_id, is_running)` when the current detail context is a Run.
    pub(crate) fn selected_run_task(&self) -> Option<(String, bool)> {
        let c = crate::view::compute(self);
        let snap = self.snapshot.as_ref()?;
        let name = c.active_name.as_ref()?;
        match crate::detail::derive_context(
            snap,
            name,
            c.ui.last_list_pane,
            &c.queue,
            &c.worktrees,
            &c.defs,
            &c.ui.selections,
        ) {
            crate::detail::DetailContext::Run { task } => {
                Some((task.id.clone(), task.status == TaskStatus::Running))
            }
            _ => None,
        }
    }
}
