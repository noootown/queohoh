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
/// ("cannot rerun a running task").
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

/// Status line for a bulk-blocked verb (see [`App::bulk_blocked`]).
const BULK_NOT_APPLICABLE: &str = "not applicable to bulk selection";

/// Resolution outcome for [`App::goto_queue`]'s target lookup — mirrors the
/// retired queue action-menu's disabled-reason precedence (row existence,
/// then the two data gaps; the tmux check happens before this is even
/// consulted). `Ready` carries the resolved provider name so the resume cmd
/// can pick the right bin (no provider menu on queue).
enum QueueGotoTarget {
    NothingSelected,
    NoSession,
    NoWorktree,
    /// `(session_id, path, provider)` — provider is the name used to look up
    /// `settings.providers[].bin` (falls back to the name itself when unset).
    Ready(String, String, String),
}

/// First segment of a `provider/label` model ref, or `None` when the ref has no
/// `/` (legacy bare alias) or is empty. Used by queue goto to resolve which
/// interactive bin to resume with.
fn model_provider_segment(model: Option<&str>) -> Option<String> {
    let m = model?.trim();
    let slash = m.find('/')?;
    let p = &m[..slash];
    (!p.is_empty()).then(|| p.to_string())
}

/// Resolve the interactive CLI binary for `provider`: `settings.providers[]`
/// entry's `bin` when set and non-empty, else the provider name itself
/// (`claude`, `grok`, …).
fn provider_bin(settings: Option<&SettingsPayload>, provider: &str) -> String {
    settings
        .and_then(|s| s.providers.iter().find(|p| p.name == provider))
        .and_then(|p| p.bin.as_ref())
        .map(|b| b.trim())
        .filter(|b| !b.is_empty())
        .map(|b| b.to_string())
        .unwrap_or_else(|| provider.to_string())
}

impl App {
    /// Refuse `btn` on `pane` with a status line when that pane's OWN
    /// selection is a bulk (multi-row) range and `btn` isn't in the doable set
    /// ([`crate::hit::bulk_allowed`]) — the same rule that dims the chip on
    /// the title bar (`view::panes::button_chip`). `true` (status line set)
    /// means the caller must stop short of its normal cursor-row behavior;
    /// `false` means proceed (either not bulk, or `btn` IS allowed on a
    /// range, e.g. QUEUE's re-queue/stop or WORKTREES' remove).
    pub(super) fn bulk_blocked(&mut self, pane: ListPane, btn: crate::hit::PaneButton) -> bool {
        let ui = self.active_ui();
        let sel = ui.selections[pane.idx()];
        let marks = &ui.marks[pane.idx()];
        if !crate::view::is_bulk_selection(&sel, marks) || crate::hit::bulk_allowed(pane.pane_id(), btn) {
            return false;
        }
        self.status_line = Some(BULK_NOT_APPLICABLE.into());
        true
    }

    /// The operator's currently-active provider name, or `None` when unknown.
    /// Prefers the live snapshot's `active_provider` (the broadcast-reconciled
    /// source the top-bar indicator renders) and falls back to the cached
    /// `settings` payload's copy — so the value survives before the first
    /// snapshot's field lands, and an old daemon that omits it on the snapshot
    /// still shows the fetched settings value. Empty strings count as unknown.
    pub(crate) fn active_provider(&self) -> Option<String> {
        let from_snapshot = self
            .snapshot
            .as_ref()
            .and_then(|s| s.active_provider.clone())
            .filter(|s| !s.is_empty());
        from_snapshot.or_else(|| {
            self.settings
                .as_ref()
                .and_then(|s| s.as_ref())
                .map(|p| p.active_provider.clone())
                .filter(|s| !s.is_empty())
        })
    }

    /// ENABLED provider names from the cached settings payload, in precedence
    /// order. `None` when settings aren't fetched (or the fetch failed) — the
    /// switch dialog has no list to show then. An empty `Some` means every
    /// provider is disabled.
    fn enabled_provider_names(&self) -> Option<Vec<String>> {
        let payload = self.settings.as_ref().and_then(|s| s.as_ref())?;
        Some(
            payload
                .providers
                .iter()
                .filter(|p| p.enabled)
                .map(|p| p.name.clone())
                .collect(),
        )
    }

    /// `Space`: toggle the focused pane's cursor row in/out of its marked set.
    /// The mark key is the row's stable identity ([`App::row_identity`]), so it
    /// survives search-filter edits and snapshot reorders. Toggle-in-place — the
    /// cursor and anchor are untouched, which is what makes "jump to a row, mark
    /// it, jump to another" work. Inert (not dirty) when the pane has no row
    /// under the cursor (empty pane / cursor past the end).
    pub(super) fn toggle_mark(&mut self) -> bool {
        let Some(pane) = self.focused_list() else { return false };
        let cursor = self.active_ui().selections[pane.idx()].cursor;
        let Some(id) = self.row_identity(pane, cursor) else { return false };
        let marks = &mut self.ui().marks[pane.idx()];
        if !marks.remove(&id) {
            marks.insert(id);
        }
        true
    }

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
                // Overlay open always clears a sticky ctrl+s prefix — otherwise
                // Esc-dismiss then `n` could still hop projects.
                self.prefix_armed = false;
                self.mode = Mode::Help;
                true
            }
            A::Settings => {
                self.prefix_armed = false;
                self.mode = Mode::Settings;
                // Fetch once on first open; thereafter the cached value renders
                // instantly. `None` means never fetched (Some(None) is a cached
                // failure that must not re-fetch on every open).
                if self.settings.is_none() {
                    cmds.push(Cmd::FetchSettings);
                }
                true
            }
            A::SwitchProvider => {
                // Open a one-field form: dropdown of ENABLED providers in
                // settings-precedence order, defaulting to the current active
                // provider (else the first choice). Replaces the old cycle-to-
                // next confirm (`claude → grok`) — circulating was a bad idea
                // once more than two providers were common; the operator picks
                // explicitly. Options freeze into the field at open so a
                // settings push mid-dialog cannot change the list under the
                // user. Strict no-op (NO form, NO RPC) when settings aren't
                // fetched or no provider is enabled — nothing useful to show.
                match self.enabled_provider_names() {
                    Some(names) if !names.is_empty() => {
                        let default = self
                            .active_provider()
                            .filter(|p| names.iter().any(|n| n == p))
                            .unwrap_or_else(|| names[0].clone());
                        let state = crate::view::form::FormState::new(
                            "Switch provider",
                            "Switch",
                            vec![crate::view::form::Field::dropdown(
                                "provider",
                                names,
                                &default,
                            )],
                        );
                        self.mode = Mode::Form {
                            state,
                            action: FormAction::SwitchProvider,
                        };
                        true
                    }
                    _ => false,
                }
            }
            A::SwitchTab(i) => {
                // Post-Esc digit guard: absorb residual `2`/`3`/… that some
                // terminals deliver after Esc (meta split) so closing a dialog
                // with Esc cannot land the operator on another project tab.
                if self.now_ms < self.suppress_tab_keys_until_ms {
                    return Update { dirty: false, cmds: vec![] };
                }
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
            A::ToggleMark => self.toggle_mark(),
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
            A::DiscoverSelectedDef => {
                // `d` is a TASKS chip (keymap-gated there): explicit discovery
                // fan-out for the highlighted def. Single-row only.
                let u = self.discover_selected_def();
                cmds.extend(u.cmds);
                u.dirty
            }
            A::ToggleCron => {
                // `c` is a TASKS chip (keymap-gated there): pause/resume the
                // highlighted def's cron. Single-row only; a def with no cron
                // refuses with a status line.
                let u = self.toggle_cron();
                cmds.extend(u.cmds);
                u.dirty
            }
            A::Create => {
                // `Create` (`s` on QUEUE, and the `[s]chedule` chip) opens the
                // unified adhoc create form with a free target combobox. Queue
                // only; WORKTREES uses `[r]un` (locked target) instead. Not a
                // bulk verb — a bulk range refuses rather than opening the form.
                self.prefix_armed = false;
                let pane = self.active_ui().last_list_pane;
                if !self.bulk_blocked(pane, crate::hit::PaneButton::Schedule)
                    && let Some(repo) = self.active_repo()
                {
                    let prefill = self.adhoc_prefill_target(pane);
                    cmds.extend(self.open_adhoc_create(repo, prefill, false));
                }
                true
            }
            A::ToggleCollapse => match self.focused_list() {
                // Collapse/expand the focused list pane; detail focus is a no-op.
                // Not in any pane's bulk-doable set — a bulk selection refuses
                // rather than collapsing/expanding out from under it.
                Some(pane) => {
                    if !self.bulk_blocked(pane, crate::hit::PaneButton::Collapse) {
                        self.toggle_collapse(pane, &mut cmds);
                    }
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
            // `a` on QUEUE: archive/unarchive toggle on the selected row.
            A::ArchiveSelected => {
                let u = self.archive_selected();
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
            // `g` on QUEUE: resume the selected task's Claude session in tmux.
            A::GotoQueue => {
                let u = self.goto_queue();
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

    /// The QUEUE rows the `r`/`x` verbs act on, plus whether this is a BULK
    /// selection (a multi-row range OR any mark — see
    /// [`crate::view::is_bulk_selection`]). Rows come back in visible-row order.
    ///
    /// Resolution goes through [`crate::view::selected_positions`], so a marked
    /// row is included even when the cursor sits elsewhere, and — critically —
    /// a bare cursor row is NOT swept in once marks exist.
    fn queue_selection_rows(&self) -> Option<(Vec<QueueSelRow>, bool)> {
        let snap = self.snapshot.as_ref()?;
        let repo = self.active_repo()?;
        let ui = self.active_ui();
        let rows = crate::selectors::queue_rows(snap, &repo, self.now_epoch_s);
        let vis = crate::selectors::filter_rows(&rows, &ui.search[0], |r| r.summary.clone());
        // The VISIBLE rows, in view order — the coordinate space the selection
        // and the marks both live in.
        let visible: Vec<&crate::selectors::QueueRow> =
            vis.iter().filter_map(|&i| rows.get(i)).collect();
        let sel = ui.selections[0];
        let marks = &ui.marks[0];
        // `is_bulk` reads the UNCLAMPED selection (see its docs): a range frozen
        // over rows that have since shrunk must still take the bulk path.
        let is_bulk = crate::view::is_bulk_selection(&sel, marks);
        let sels = crate::view::selected_positions(&visible, &sel, marks, |r| r.task_id.clone())
            .into_iter()
            .filter_map(|pos| visible.get(pos).copied())
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
        Some((sels, is_bulk))
    }

    /// `r` on QUEUE (and the `[r]un`/re-run chip). Re-queue ALWAYS confirms first
    /// (parity with the stop verb and the worktree remove): it freezes the
    /// per-task `retry` RPCs and opens `Mode::Confirm`. EVERY status is
    /// eligible except `running` (its in-flight worker owns the row — stop it
    /// first) and archived rows; a `queued` retry is an idempotent no-op
    /// daemon-side. Enter/y in that dialog dispatches the `RpcSeq` (verb
    /// "reran", see `update`); a single ineligible row explains why with a
    /// per-status no-op line, and a selection with nothing re-queueable never
    /// opens the dialog — it sets a status line instead.
    pub(super) fn requeue_selected(&mut self) -> Update {
        let requeue_ok = |s: TaskStatus| !matches!(s, TaskStatus::Running);
        let Some((rows, is_bulk)) = self.queue_selection_rows() else {
            return Update::default();
        };
        if !is_bulk {
            // Single row: keep the per-status no-op line explaining why the one
            // row can't re-queue; an eligible row opens the confirm dialog.
            let Some((id, status, archived)) = rows.into_iter().next() else {
                return Update::default();
            };
            if archived {
                self.status_line = Some("cannot rerun an archived task".into());
                return Update { dirty: true, cmds: vec![] };
            }
            if !requeue_ok(status) {
                self.status_line = Some(format!("cannot rerun a {} task", status_kebab(status)));
                return Update { dirty: true, cmds: vec![] };
            }
            let calls =
                vec![RpcCall { method: "retry".into(), params: serde_json::json!({ "id": id }) }];
            self.mode = Self::requeue_confirm_mode(1, calls);
            return Update { dirty: true, cmds: vec![] };
        }
        let ids: Vec<String> =
            rows.into_iter().filter(|(_, s, arch)| !arch && requeue_ok(*s)).map(|(id, _, _)| id).collect();
        if ids.is_empty() {
            self.status_line = Some("no rerunnable tasks in selection".into());
            return Update { dirty: true, cmds: vec![] };
        }
        let n = ids.len();
        let calls = ids
            .into_iter()
            .map(|id| RpcCall { method: "retry".into(), params: serde_json::json!({ "id": id }) })
            .collect();
        self.mode = Self::requeue_confirm_mode(n, calls);
        Update { dirty: true, cmds: vec![] }
    }

    /// Build the QUEUE re-queue confirm dialog for `n` tasks. Mirror of the stop
    /// dialog `cancel_selected` builds; `calls` are the frozen `retry` RPCs the
    /// Confirm button fires via [`ConfirmAction::RequeueTasks`]. The range/marks
    /// are cleared on confirm (in `run_confirm_action`), not at open time.
    fn requeue_confirm_mode(n: usize, calls: Vec<RpcCall>) -> Mode {
        let plural = if n == 1 { "" } else { "s" };
        Mode::Confirm {
            title: format!("Rerun {n} task{plural}"),
            // No leading spaces — the modal's interior padding provides the inset.
            body: vec![format!("Rerun {n} task{plural}?")],
            confirm_label: "Rerun".into(),
            action: ConfirmAction::RequeueTasks { calls },
            focus: crate::hit::ButtonKind::Confirm,
        }
    }

    /// `x` on QUEUE (and the `[x]stop` chip). Stop ALWAYS confirms first: it
    /// freezes the per-task skip/stop RPCs (queued/needs-input → `skip`, running →
    /// `stop`; terminal/archived rows are ineligible) and opens `Mode::Confirm`.
    /// Enter/y in that dialog dispatches (see `update`); a selection with nothing
    /// stoppable never opens the dialog — it sets a status line instead.
    pub(super) fn cancel_selected(&mut self) -> Update {
        // The RPC method for a cancellable status, or None when it can't cancel.
        let cancel_method = |s: TaskStatus| match s {
            TaskStatus::Queued | TaskStatus::NeedsInput => Some("skip"),
            TaskStatus::Running => Some("stop"),
            _ => None,
        };
        let Some((rows, _is_bulk)) = self.queue_selection_rows() else {
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
            self.status_line = Some("nothing to stop in selection".into());
            return Update { dirty: true, cmds: vec![] };
        }
        let stops = eligible.iter().filter(|(_, s)| matches!(s, TaskStatus::Running)).count();
        let n = eligible.len();
        let summary = if n == 1 {
            format!("stop 1 {} task", status_kebab(eligible[0].1))
        } else if stops > 0 {
            format!("stop {n} tasks ({stops} running will be stopped)")
        } else {
            format!("stop {n} tasks")
        };
        let calls: Vec<RpcCall> = eligible
            .into_iter()
            .map(|(id, s)| RpcCall {
                method: cancel_method(s).expect("filtered to cancellable").into(),
                params: serde_json::json!({ "id": id }),
            })
            .collect();
        self.mode = Mode::Confirm {
            title: format!("Stop {n} task{}", if n == 1 { "" } else { "s" }),
            body: vec![summary],
            confirm_label: "Stop tasks".into(),
            action: ConfirmAction::CancelTasks { calls },
            focus: crate::hit::ButtonKind::Confirm,
        };
        Update { dirty: true, cmds: vec![] }
    }

    /// `a` on QUEUE (and the `[a]rchive`/`[a]unarchive` chip): archive/unarchive
    /// TOGGLE on the selected row(s). An archived row restores to the live list
    /// (`unarchive`); a terminal live row archives out of it. Only the ACTIVE
    /// rows (queued/running) refuse locally with a status line — hiding live
    /// work is never right — while any other status (including `needs-input`,
    /// which is parked, never started, so it buries nothing) goes to the daemon,
    /// which owns the real eligibility rule (forward-compat: a status this TUI
    /// doesn't know gets the daemon's verdict, not a stale local one). No confirm
    /// dialog: the toggle is its own undo.
    ///
    /// A BULK selection fans the same toggle over every eligible row. Its
    /// DIRECTION is fixed by the FIRST (topmost) selected row — the same row the
    /// title-bar chip's `[a]rchive`/`[a]unarchive` label reflects: an archived
    /// first row means `unarchive` (restore every archived row in the range,
    /// skipping live ones); a live first row means `archive` (archive every
    /// archivable row — terminal and parked `needs-input` — skipping only active
    /// queued/running work and already-archived rows). Rows the direction
    /// doesn't apply to are silently dropped, matching the stop/rerun verbs; a
    /// selection with nothing eligible sets a status line instead.
    pub(super) fn archive_selected(&mut self) -> Update {
        let Some((rows, is_bulk)) = self.queue_selection_rows() else {
            return Update::default();
        };
        if is_bulk {
            return self.archive_selected_bulk(rows);
        }
        let Some((id, status, archived)) = rows.into_iter().next() else {
            return Update::default();
        };
        let method = if archived {
            "unarchive"
        } else {
            if matches!(status, TaskStatus::Queued | TaskStatus::Running) {
                self.status_line =
                    Some(format!("cannot archive a {} task", status_kebab(status)));
                return Update { dirty: true, cmds: vec![] };
            }
            "archive"
        };
        Update {
            dirty: true,
            cmds: vec![Cmd::Rpc {
                label: method.into(),
                call: RpcCall {
                    method: method.into(),
                    params: serde_json::json!({ "id": id }),
                },
                timeout_ms: 5000,
                timeout_is_ok: false,
                invalidate_defs_for: None,
                report_empty_as: None,
            }],
        }
    }

    /// The BULK half of [`Self::archive_selected`]. `rows` are the selected
    /// QUEUE rows in view (topmost-first) order — `rows[0]` is the direction
    /// anchor. Fans one `archive`/`unarchive` RPC per eligible row out through a
    /// range-clearing [`Cmd::RpcSeq`] (verb "archived"/"unarchived"), mirroring
    /// the bulk stop/rerun path but with no confirm — the toggle is its own undo.
    fn archive_selected_bulk(&mut self, rows: Vec<QueueSelRow>) -> Update {
        let Some(&(_, _, first_archived)) = rows.first() else {
            return Update::default();
        };
        // Only queued/running are un-hideable live work; `needs-input` is parked
        // and archivable, so it is NOT counted active here.
        let active = |s: TaskStatus| matches!(s, TaskStatus::Queued | TaskStatus::Running);
        let (method, verb): (&str, &str) =
            if first_archived { ("unarchive", "unarchived") } else { ("archive", "archived") };
        let ids: Vec<String> = rows
            .into_iter()
            .filter(|(_, status, archived)| {
                if first_archived {
                    // Unarchive direction: only the already-archived rows.
                    *archived
                } else {
                    // Archive direction: archivable rows (terminal + parked
                    // needs-input) — skip active queued/running work (can't hide
                    // it) and already-archived rows (opposite half).
                    !*archived && !active(*status)
                }
            })
            .map(|(id, _, _)| id)
            .collect();
        if ids.is_empty() {
            self.status_line = Some(format!("nothing to {method} in selection"));
            return Update { dirty: true, cmds: vec![] };
        }
        self.clear_range_and_marks(ListPane::Queue);
        let calls = ids
            .into_iter()
            .map(|id| RpcCall { method: method.into(), params: serde_json::json!({ "id": id }) })
            .collect();
        Update {
            dirty: true,
            cmds: vec![Cmd::RpcSeq { verb: verb.into(), calls, invalidate_defs_for: None }],
        }
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
                // detail to Run). `set_cursor` preserves the current Run sub-tab,
                // only swapping report → transcript when the target is still
                // running (its report is empty).
                self.set_focus(PaneId::Queue);
                self.set_cursor(ListPane::Queue, qi, &mut cmds);
                self.schedule_run_read(&mut cmds, 120);
                Update { dirty: true, cmds }
            }
            None => {
                self.status_line = Some("task not in queue view".to_string());
                Update { dirty: true, cmds: Vec::new() }
            }
        }
    }

    /// Double-click over a list row (Enter is unbound in list mode). A
    /// single-row selection on the TASKS pane runs the highlighted definition
    /// directly (no menu hop — opens the run form with args + effective-chain
    /// model picker); a single row on QUEUE resumes that task's Claude session
    /// directly (mirrors the `g`/`[g]oto` verb); a single row on WORKTREES has
    /// no direct verb here (its `r`/`g`/`x` hotkeys act on the row instead). A
    /// bulk range refuses on QUEUE (goto isn't bulk-doable) and on TASKS (no
    /// bulk-doable verb there either, mirroring `r`'s own guard); on WORKTREES
    /// it falls through to [`Self::open_bulk_menu`], which fires the confirm
    /// dialog directly.
    pub(super) fn open_actions_or_run(&mut self) -> Update {
        let ui = self.active_ui();
        let pane = ui.last_list_pane;
        let sel = ui.selections[pane.idx()];
        let marks = &ui.marks[pane.idx()];
        let bulk = crate::view::is_bulk_selection(&sel, marks);
        // Single-row TASKS runs the highlighted def directly (no menu hop).
        if !bulk && pane == ListPane::Tasks {
            return self.run_selected_task_def();
        }
        if bulk {
            let btn = match pane {
                ListPane::Queue => crate::hit::PaneButton::Goto,
                ListPane::Tasks => crate::hit::PaneButton::Run,
                ListPane::Worktrees => crate::hit::PaneButton::Remove,
            };
            if self.bulk_blocked(pane, btn) {
                return Update { dirty: true, cmds: vec![] };
            }
            // `open_bulk_menu` resolves rows from `selection ∪ marks` itself —
            // no frozen range needed.
            return self.open_bulk_menu(pane);
        }
        // Single-row QUEUE resumes the task's Claude session directly (no menu
        // hop, mirrors the retired single-target Resume menu). Single-row
        // WORKTREES has no direct verb here — its `r`/`g`/`x` hotkeys act on
        // the row instead — so it just reports nothing to do.
        if pane == ListPane::Queue {
            return self.goto_queue();
        }
        self.status_line = Some("nothing selected".into());
        Update { dirty: true, cmds: vec![] }
    }

    /// The tasks pane's `r` (key and `[r]un` chip): selection-aware. A single-row
    /// selection runs the highlighted def via [`Self::run_selected_task_def`]; a
    /// multi-row range is not in the bulk-doable set (TASKS keeps none — see
    /// [`crate::hit::bulk_allowed`]), so it refuses with a status line instead
    /// of the bulk-run menu the tasks pane's `r` used to open.
    fn run_or_bulk_selected_task_def(&mut self) -> Update {
        let ui = self.active_ui();
        let sel = ui.selections[ListPane::Tasks.idx()];
        let marks = &ui.marks[ListPane::Tasks.idx()];
        if crate::view::is_bulk_selection(&sel, marks) {
            self.status_line = Some(BULK_NOT_APPLICABLE.into());
            return Update { dirty: true, cmds: vec![] };
        }
        self.run_selected_task_def()
    }

    /// Run the TASKS pane's highlighted definition directly (single-row Enter /
    /// double-click, and the single-row branch of [`Self::run_or_bulk_selected_task_def`];
    /// a multi-row range goes to the bulk menu there instead). Resolves the def
    /// from the current filtered selection and opens the run form (args + the
    /// effective-chain model picker) with an ambient worktree overlay (initial
    /// values from the selected worktree row), fetching its prompt for the right
    /// panel. Zero-arg defs open the form too (model picker only) — no immediate
    /// `runDefinition` hop. Mirrors the def-picker `Enter` path minus the
    /// explicit worktree target.
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
        let rows = self.active_worktree_rows();
        let selected = self.selected_worktree_row();
        let worktrees = Self::worktree_names(&rows);
        let (args, initial) =
            crate::worktree_context::ambient_run_args(&def.args, &rows, selected.as_ref());
        let branches = self.active_worktree_branches();
        let cmds = self.open_def_args(
            def.repo,
            def.name,
            args,
            HashMap::new(),
            initial,
            None,
            worktrees,
            branches,
            def.model,
        );
        Update { dirty: true, cmds }
    }

    /// `d` on TASKS (and the `[d]iscover` chip): open a confirm dialog before
    /// running the highlighted def's discovery command. Mirrors
    /// [`Self::run_selected_task_def`]'s selection resolution; a def without a
    /// discovery block refuses with a status line (no dialog/RPC), and a bulk
    /// range refuses like every non-bulk verb. Confirm (via
    /// [`ConfirmAction::DiscoverDef`]) inserts the optimistic spinner and fires
    /// `discoverDefinition`; cancel leaves nothing pending.
    pub(super) fn discover_selected_def(&mut self) -> Update {
        let ui = self.active_ui();
        let sel = ui.selections[ListPane::Tasks.idx()];
        let marks = &ui.marks[ListPane::Tasks.idx()];
        if crate::view::is_bulk_selection(&sel, marks) {
            self.status_line = Some(BULK_NOT_APPLICABLE.into());
            return Update { dirty: true, cmds: vec![] };
        }
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
        if !def.has_discovery {
            self.status_line = Some(format!("{} has no discovery", def.name));
            return Update { dirty: true, cmds: vec![] };
        }
        // Freeze repo/name into the action so confirm runs exactly the def the
        // body names (payload frozen at open, not re-derived on confirm).
        // Spinner + RPC wait until confirm — cancel must leave no in-flight marker.
        let repo = def.repo.clone();
        let name = def.name.clone();
        self.mode = Mode::Confirm {
            title: "Run discovery".into(),
            body: vec![
                format!("Run discovery for {repo}/{name}?"),
                "Fans out one task per discovered item.".into(),
            ],
            confirm_label: "Discover".into(),
            action: ConfirmAction::DiscoverDef { repo, name },
            focus: crate::hit::ButtonKind::Confirm,
        };
        Update { dirty: true, cmds: vec![] }
    }

    /// Build the fire-and-forget `discoverDefinition` command. Same client
    /// contract as [`Self::run_definition_cmd`]: timeout is treated as success
    /// (discovery can outlive it; the push subscription re-syncs) and a
    /// successful call invalidates the repo's def summaries. The timeout is
    /// generous (vs the 5 s default) because the daemon RPC returns only when
    /// the discovery command has run — and the response is what stops the
    /// `⌕`-spinner (`App::discovering`), so a slow `gh`-backed discover should
    /// keep spinning until it actually finishes, not for a token 5 s.
    pub(super) fn discover_definition_cmd(repo: &str, name: &str) -> Cmd {
        Cmd::Rpc {
            label: "discover".into(),
            call: RpcCall {
                method: "discoverDefinition".into(),
                params: serde_json::json!({ "repo": repo, "name": name, "source": "tui" }),
            },
            timeout_ms: 120_000,
            timeout_is_ok: true,
            invalidate_defs_for: Some(repo.to_string()),
            report_empty_as: None,
        }
    }

    /// `c` on TASKS (and the `[c]ron` chip): pause/resume the highlighted def's
    /// cron schedule. Mirrors [`Self::discover_selected_def`]'s selection
    /// resolution. A def with no `cron:` refuses with a status line (no RPC — you
    /// can't toggle a schedule that doesn't exist); a bulk range refuses like
    /// every non-bulk verb. The RPC sends the OPPOSITE of the def's current
    /// `cron_enabled`, and `invalidate_defs_for` refetches the repo's summaries so
    /// the Cron column re-renders dim/bright.
    pub(super) fn toggle_cron(&mut self) -> Update {
        let ui = self.active_ui();
        let sel = ui.selections[ListPane::Tasks.idx()];
        let marks = &ui.marks[ListPane::Tasks.idx()];
        if crate::view::is_bulk_selection(&sel, marks) {
            self.status_line = Some(BULK_NOT_APPLICABLE.into());
            return Update { dirty: true, cmds: vec![] };
        }
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
        if def.cron.is_none() {
            self.status_line = Some(format!("{} has no cron schedule", def.name));
            return Update { dirty: true, cmds: vec![] };
        }
        let enable = !def.cron_enabled;
        self.status_line = Some(format!(
            "cron {} for {}",
            if enable { "resumed" } else { "paused" },
            def.name
        ));
        Update {
            dirty: true,
            cmds: vec![Self::set_cron_enabled_cmd(&def.repo, &def.name, enable)],
        }
    }

    /// Build the fire-and-forget `set_cron_enabled` command: pause (`enabled =
    /// false`) or resume (`true`) the def's cron. A successful call invalidates
    /// the repo's def summaries so the refetched `cron_enabled` re-renders the
    /// Cron column. Short client timeout — it is a cheap SettingsStore write.
    pub(super) fn set_cron_enabled_cmd(repo: &str, name: &str, enabled: bool) -> Cmd {
        Cmd::Rpc {
            label: "cron".into(),
            call: RpcCall {
                method: "set_cron_enabled".into(),
                params: serde_json::json!({ "repo": repo, "name": name, "enabled": enabled }),
            },
            timeout_ms: 5000,
            timeout_is_ok: false,
            invalidate_defs_for: Some(repo.to_string()),
            report_empty_as: None,
        }
    }

    /// Build the fire-and-forget `runDefinition` command. Client timeout is
    /// treated as success (discovery can outlive it; the push subscription
    /// re-syncs), and a successful run invalidates the repo's def summaries.
    ///
    /// `target_ref` (a canonical `pr:N` / `ticket:ID` / `worktree:<name>`)
    /// resolves the worktree-typed arg on submit; when it is `Some` the command
    /// sends `params.ref` and does NOT also send `params.worktree`, so the
    /// daemon honors the ref (create-or-reuse) instead of the legacy worktree
    /// hint. `worktree` (the launch context) is sent only when there is no ref.
    /// `model` is the operator's 1-entry exact pick from the def-run effective
    /// chain (omit / empty → daemon keeps the def's authored chain). The
    /// def-run picker never offers an empty "default" option (see
    /// `def_model_field`), so a present `model` is always sent with
    /// `model_pinned: true` — the daemon runs it exactly, no active-provider
    /// re-head, no fallback.
    ///
    /// Always sends `bypass_dedup: true`: a human filling this form and
    /// pressing Run is explicit "run NOW" intent, so a def's configured
    /// `dedup: skip_seen`/`retry_errored` must not silently collapse the call
    /// to zero created tasks against an item already seen (even a failed one).
    /// Cron/discovery/MCP-driven instantiation is untouched — only this
    /// picker's def-run path sets it.
    ///
    /// Sends `report_empty_as`: with `bypass_dedup` in place a def-run should
    /// never come back empty, but this stays as defensive/honest behavior
    /// (e.g. against an old daemon that doesn't understand the flag yet) — an
    /// empty `runDefinition` reply (zero created tasks) overwrites the
    /// immediate "running {def}…" status instead of silently looking like a
    /// no-op success.
    pub(super) fn run_definition_cmd(
        repo: &str,
        name: &str,
        values: &[String],
        worktree: Option<&str>,
        target_ref: Option<&str>,
        model: Option<&str>,
    ) -> Cmd {
        let mut params = serde_json::json!({
            "repo": repo, "name": name, "args": values, "source": "tui",
            "bypass_dedup": true,
        });
        if let Some(r) = target_ref {
            params["ref"] = serde_json::Value::String(r.to_string());
        } else if let Some(wt) = worktree {
            params["worktree"] = serde_json::Value::String(wt.to_string());
        }
        if let Some(m) = model.filter(|m| !m.is_empty()) {
            params["model_pinned"] = serde_json::Value::Bool(true);
            params["model"] = serde_json::Value::String(m.to_string());
        }
        Cmd::Rpc {
            label: "run".into(),
            call: RpcCall { method: "runDefinition".into(), params },
            timeout_ms: 5000,
            timeout_is_ok: true,
            invalidate_defs_for: Some(repo.to_string()),
            report_empty_as: Some(format!(
                "{name}: nothing ran — deduped (already seen). Use a fresh target or re-run after it clears."
            )),
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
    pub(super) fn active_worktree_rows(&self) -> Vec<crate::selectors::WorktreeRow> {
        match (&self.snapshot, self.active_repo()) {
            (Some(snap), Some(repo)) => crate::selectors::worktree_rows(snap, &repo),
            _ => Vec::new(),
        }
    }

    /// The real worktrees' identifiers (`raw_name`, minus session rows) — the
    /// combobox seed and the exact-match set for the submit ref resolution.
    pub(super) fn worktree_names(rows: &[crate::selectors::WorktreeRow]) -> Vec<String> {
        rows.iter().filter(|r| !r.is_session).map(|r| r.raw_name.clone()).collect()
    }

    /// The active project's worktree identifiers (see [`Self::worktree_names`]).
    pub(super) fn active_worktree_names(&self) -> Vec<String> {
        Self::worktree_names(&self.active_worktree_rows())
    }

    /// The active project's worktree BRANCHES (non-session, non-empty), deduped
    /// and INCLUDING main/master — the seed for a `type: branch` dropdown. This
    /// is deliberately broader than `worktree_context::ambient_run_args`, which
    /// excludes main/master (a `source` to squash from is never main; a `target`
    /// to land on usually is).
    pub(super) fn active_worktree_branches(&self) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        self.active_worktree_rows()
            .into_iter()
            .filter(|r| !r.is_session && !r.branch.is_empty())
            .map(|r| r.branch)
            .filter(|b| seen.insert(b.clone()))
            .collect()
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
    /// the exact row the `r`/`g`/`x` verbs act on: cursor is an index into the
    /// FILTERED rows, mapped back to the full set.
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

    /// The adhoc-create target to prefill from the focused pane's selected row.
    /// Only a WORKTREES real-worktree row prefills (its name — so its sessions
    /// are then offered via the session field); that selection names a concrete
    /// destination. The QUEUE and TASKS panes prefill nothing: a new adhoc task
    /// has nothing to do with which past task / definition happens to be under
    /// the cursor, so those open the form blank.
    fn adhoc_prefill_target(&self, pane: ListPane) -> Option<String> {
        match pane {
            ListPane::Worktrees => {
                self.selected_worktree_row_filtered().filter(|r| !r.is_session).map(|r| r.raw_name)
            }
            ListPane::Queue | ListPane::Tasks => None,
        }
    }

    /// `r` on WORKTREES (and the `[r]un` chip): same adhoc form as QUEUE
    /// `[s]chedule`, with the selected worktree locked as a readonly target.
    /// Prefetches sessions for that worktree. Session rows can't host a task →
    /// status line, no mode change.
    pub(super) fn new_task_on_worktree(&mut self) -> Update {
        // A bulk range isn't in the doable set (only `Remove` is) — refuse
        // rather than silently targeting just the cursor row's worktree.
        if self.bulk_blocked(ListPane::Worktrees, crate::hit::PaneButton::Run) {
            return Update { dirty: true, cmds: vec![] };
        }
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
        let cmds = self.open_adhoc_create(repo, Some(row.raw_name), true);
        Update { dirty: true, cmds }
    }

    /// `g` on WORKTREES (and the `[g]oto` chip): open the Goto-provider form,
    /// then launch a first-class tmux split (left = juice | right = provider bin,
    /// 3:1) at the selected worktree (or session) cwd. Inert with a status line
    /// outside tmux. Session rows resolve to their cwd path.
    pub(super) fn goto_worktree(&mut self) -> Update {
        // A bulk range isn't in the doable set (only `Remove` is) — refuse
        // rather than silently targeting just the cursor row's worktree.
        if self.bulk_blocked(ListPane::Worktrees, crate::hit::PaneButton::Goto) {
            return Update { dirty: true, cmds: vec![] };
        }
        if !self.inside_tmux {
            self.status_line = Some("not inside tmux".into());
            return Update { dirty: true, cmds: vec![] };
        }
        let Some(row) = self.selected_worktree_row_filtered() else {
            self.status_line = Some("no worktree selected".into());
            return Update { dirty: true, cmds: vec![] };
        };
        let path = row.path.clone();
        // Juice Review base: the open PR's base branch when known, else origin/main.
        let juice_base = Self::juice_base_from_pr(row.pr_base.as_deref());
        // Enabled providers + resolved bins, frozen into the form so a
        // settings push mid-dialog cannot retarget the launch.
        let Some(payload) = self.settings.as_ref().and_then(|s| s.as_ref()) else {
            // Settings not cached yet (or a failed fetch). Kick a fetch so the
            // next `g` can open the form; don't invent a default provider.
            let mut cmds = Vec::new();
            if self.settings.is_none() {
                cmds.push(Cmd::FetchSettings);
            }
            self.status_line = Some("settings not loaded — press g again".into());
            return Update { dirty: true, cmds };
        };
        let choices: Vec<(String, String)> = payload
            .providers
            .iter()
            .filter(|p| p.enabled)
            .map(|p| {
                let bin = p
                    .bin
                    .as_ref()
                    .map(|b| b.trim())
                    .filter(|b| !b.is_empty())
                    .map(|b| b.to_string())
                    .unwrap_or_else(|| p.name.clone());
                (p.name.clone(), bin)
            })
            .collect();
        if choices.is_empty() {
            self.status_line = Some("no enabled providers".into());
            return Update { dirty: true, cmds: vec![] };
        }
        // Default the dropdown to the current active provider when it's among
        // the enabled choices, else the first choice.
        let names: Vec<String> = choices.iter().map(|(name, _)| name.clone()).collect();
        let default = self
            .active_provider()
            .filter(|p| names.iter().any(|n| n == p))
            .unwrap_or_else(|| names[0].clone());
        let state = crate::view::form::FormState::new(
            "Goto — provider",
            "Go",
            vec![crate::view::form::Field::dropdown("provider", names, &default)],
        );
        self.mode = Mode::Form {
            state,
            action: FormAction::GotoProvider {
                path,
                choices,
                juice_base,
            },
        };
        Update { dirty: true, cmds: vec![] }
    }

    /// Sticky juice Review base: non-empty PR base when known, else
    /// [`crate::event::DEFAULT_JUICE_BASE`] (`origin/main`).
    pub(super) fn juice_base_from_pr(pr_base: Option<&str>) -> String {
        pr_base
            .map(str::trim)
            .filter(|b| !b.is_empty())
            .unwrap_or(crate::event::DEFAULT_JUICE_BASE)
            .to_string()
    }

    /// `g` on QUEUE (and the `[g]oto` chip): resume the selected task's session
    /// in a first-class tmux split rooted at its worktree — no provider menu;
    /// the provider is resolved from run meta `provider` → run/task model ref
    /// segment (legacy untagged → `claude`). Inert outside tmux, when no
    /// session id is recorded yet, or when no worktree path resolves.
    pub(super) fn goto_queue(&mut self) -> Update {
        // A bulk range isn't in the doable set — refuse rather than silently
        // targeting just the cursor row's task.
        if self.bulk_blocked(ListPane::Queue, crate::hit::PaneButton::Goto) {
            return Update { dirty: true, cmds: vec![] };
        }
        if !self.inside_tmux {
            self.status_line = Some("not inside tmux".into());
            return Update { dirty: true, cmds: vec![] };
        }
        match self.queue_goto_target() {
            QueueGotoTarget::NothingSelected => {
                self.status_line = Some("nothing selected".into());
                Update { dirty: true, cmds: vec![] }
            }
            QueueGotoTarget::NoSession => {
                self.status_line = Some("no session yet (task never ran)".into());
                Update { dirty: true, cmds: vec![] }
            }
            QueueGotoTarget::NoWorktree => {
                self.status_line = Some("no worktree for this task".into());
                Update { dirty: true, cmds: vec![] }
            }
            QueueGotoTarget::Ready(session_id, path, provider) => {
                let settings = self.settings.as_ref().and_then(|s| s.as_ref());
                let bin = provider_bin(settings, &provider);
                let cmd = format!("{bin} --resume {session_id}");
                let juice_base = self.juice_base_for_path(&path);
                Update {
                    dirty: true,
                    cmds: vec![Cmd::Goto {
                        path,
                        cmd,
                        juice_base,
                    }],
                }
            }
        }
    }

    /// Look up a worktree by path in the active project's snapshot and return
    /// its juice Review base (PR base or `origin/main`). Used by queue goto,
    /// which has a path but not a `WorktreeRow` in hand.
    fn juice_base_for_path(&self, path: &str) -> String {
        let pr_base = self.snapshot.as_ref().and_then(|snap| {
            snap.worktrees
                .values()
                .flatten()
                .find(|w| w.path == path)
                .and_then(|w| w.pr_base.as_deref())
        });
        Self::juice_base_from_pr(pr_base)
    }

    /// Resolve the QUEUE cursor row's session id + worktree path + provider for
    /// [`Self::goto_queue`]: the selected run's recorded session + worktree path
    /// (from its run record), falling back to the task's `resume_session_id` and
    /// the snapshot worktree's path. Provider: run meta `provider` → run meta
    /// model segment → task model segment → `"claude"` for legacy untagged.
    fn queue_goto_target(&self) -> QueueGotoTarget {
        let Some(snap) = self.snapshot.as_ref() else { return QueueGotoTarget::NothingSelected };
        let Some(repo) = self.active_repo() else { return QueueGotoTarget::NothingSelected };
        let ui = self.active_ui();
        let rows = crate::selectors::queue_rows(snap, &repo, self.now_epoch_s);
        let vis = crate::selectors::filter_rows(&rows, &ui.search[0], |r| r.summary.clone());
        let cursor = ui.selections[0].cursor.min(vis.len().saturating_sub(1));
        let Some(row) = vis.get(cursor).and_then(|&i| rows.get(i)) else {
            return QueueGotoTarget::NothingSelected;
        };
        let Some(task) =
            snap.tasks.iter().chain(snap.archived_recent.iter()).find(|t| t.id == row.task_id)
        else {
            return QueueGotoTarget::NothingSelected;
        };
        let run = self.run_files.as_ref().filter(|(id, _)| id == &row.task_id).map(|(_, f)| f);
        let session_id =
            run.and_then(|f| f.session_id.clone()).or_else(|| task.resume_session_id.clone());
        let worktree_path = run.and_then(|f| f.worktree_path.clone()).or_else(|| {
            task.target.worktree.as_deref().and_then(|w| {
                snap.worktrees
                    .get(&repo)
                    .and_then(|wts| wts.iter().find(|i| i.name == w))
                    .map(|i| i.path.clone())
            })
        });
        // Provider: run meta.provider (daemon adapter name) → slash-form model
        // segment on the run → task model segment → legacy "claude". Bare model
        // ids like `grok-4.5` have no `/`, so without meta.provider they would
        // otherwise fall through incorrectly.
        let meta = run.and_then(|f| f.meta.as_ref());
        let provider = meta
            .and_then(|m| m.provider.as_deref())
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(|p| p.to_string())
            .or_else(|| meta.and_then(|m| model_provider_segment(m.model.as_deref())))
            .or_else(|| model_provider_segment(task.model.as_deref()))
            .unwrap_or_else(|| "claude".into());
        match (session_id, worktree_path) {
            (None, _) => QueueGotoTarget::NoSession,
            (Some(_), None) => QueueGotoTarget::NoWorktree,
            (Some(session_id), Some(path)) => QueueGotoTarget::Ready(session_id, path, provider),
        }
    }

    /// `x` on WORKTREES (and the `[x]remove` chip): selection-aware, mirroring
    /// the tasks pane's `r`. A bulk selection (multi-row range OR any mark)
    /// opens the bulk remove confirm dialog directly (eligibility frozen at
    /// open time via [`Self::open_bulk_menu`]); a single row confirms removing
    /// just that worktree (opens `Mode::Confirm`; the `y` handler dispatches
    /// the `removeWorktree` RPC). A session row isn't a worktree and a busy
    /// worktree has a task running → status line, no confirm.
    pub(super) fn remove_selected_worktree(&mut self) -> Update {
        let Some(repo) = self.active_repo() else {
            return Update::default();
        };
        // A bulk selection (multi-row range OR any mark) opens the bulk-remove
        // confirm, which resolves the exact rows from `selection ∪ marks`. A
        // single-row (non-bulk) selection removes just the cursor's worktree.
        let ui = self.active_ui();
        let sel = ui.selections[ListPane::Worktrees.idx()];
        let marks = &ui.marks[ListPane::Worktrees.idx()];
        if crate::view::is_bulk_selection(&sel, marks) {
            return self.open_bulk_menu(ListPane::Worktrees);
        }
        let Some(row) = self.selected_worktree_row_filtered() else {
            self.status_line = Some("no worktree selected".into());
            return Update { dirty: true, cmds: vec![] };
        };
        if row.is_session {
            self.status_line = Some("not a worktree".into());
            return Update { dirty: true, cmds: vec![] };
        }
        if row.protected {
            self.status_line = Some("worktree is protected".into());
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
