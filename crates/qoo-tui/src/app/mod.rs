//! The TUI application model.
//!
//! `mod.rs` holds the [`App`] struct (all session/UI state) and the core glue:
//! construction, layout persistence, self-heal wiring, RPC dispatch, definition
//! reconciliation, and the state accessors shared across the handlers. The
//! behavior is split by concern into submodules — all of which add methods to
//! the same [`App`] via further `impl App` blocks:
//!
//! - [`mode`] — pure state/mode types (`Mode`, `ListPane`, `TabUiState`, …).
//! - [`update`] — the `update`/`update_event` event-dispatch entry point.
//! - [`actions`] — list-mode `AppAction` handling and `Cmd` builders.
//! - [`menus`] — action-menu / bulk-menu open, key, and click handling.
//! - [`def_args`] — def-picker, run-form, and create-worktree input handling.
//! - [`mouse`] — mouse routing, drags, and DETAIL text selection.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crossterm::event::{KeyCode, KeyEventKind};
use ratatui::layout::Position;

use crate::event::{Cmd, Event, RpcCall};
use crate::hit::{HitMap, HitTarget};
use crate::ipc::types::{
    ArgSpec, DefinitionSummary, SettingsPayload, StateSnapshot, TaskDefinition, TaskStatus,
};
use crate::keymap::AppAction;
use crate::runfiles::RunFiles;

mod actions;
mod def_args;
mod form;
mod menus;
mod mode;
mod mouse;
mod update;

pub use mode::*;

#[derive(Clone)]
pub struct App {
    pub snapshot: Option<StateSnapshot>,
    pub connected: bool,
    pub active_tab: usize,
    pub ui_by_tab: HashMap<String, TabUiState>,
    pub mode: Mode,
    pub status_line: Option<String>,
    pub run_files: Option<(String, Box<RunFiles>)>,
    pub defs_by_project: HashMap<String, Vec<DefinitionSummary>>,
    /// Model-alias settings backing the `s` overlay, lazily fetched on first
    /// open. Three-state: `None` = never fetched (fetch is in flight → overlay
    /// shows "loading"); `Some(None)` = the fetch failed or the daemon predates
    /// the `settings` RPC (overlay shows the "unavailable" line); `Some(Some(p))`
    /// = data. Fetched once and cached for the session.
    pub settings: Option<Option<SettingsPayload>>,
    /// Repos with a `FetchDefinitions` in flight — the lazy per-tab fetch dedup
    /// set. `reconcile_defs` inserts before emitting; `Event::Definitions`
    /// clears on arrival (Task 18).
    pub defs_inflight: HashSet<String>,
    pub full_defs: HashMap<String, TaskDefinition>, // keyed "repo/name"
    /// `"repo/name"` keys with a `FetchDefinition` in flight — the lazy detail
    /// fetch dedup set (mirrors `defs_inflight`), shared by the task-menu
    /// prefetch (`prefetch_full_def`) and the detail-pane preview
    /// (`reconcile_full_def`). A FAILED fetch leaves its key here as a poison
    /// marker so `reconcile_full_def` doesn't refetch-loop; invalidation
    /// (`ActionResult::invalidate_defs_for`) clears the repo's keys.
    pub full_defs_inflight: HashSet<String>,
    pub now_epoch_s: u64,
    /// Monotonic millisecond clock stamped by the event loop before every
    /// `update()` (from an `Instant` taken at program start). Read by
    /// double-click timing; tests set it directly to simulate fast/slow clicks.
    pub now_ms: u64,
    /// tmux-style `ctrl+s` prefix state. When armed, the next key is `n` (next
    /// project tab) / `p` (previous) and anything else disarms and is swallowed.
    /// Only armed/consumed inside `Mode::List`, never in text-input modes.
    pub prefix_armed: bool,
    /// Last plain (non-shift) row click: `(pane, row_identity, now_ms)`. A second
    /// click on the SAME ROW IDENTITY within `DOUBLE_CLICK_MS` opens the action
    /// menu; a single click only selects. Keying on the stable per-row identity
    /// (queue: task id · tasks: `repo/name` · worktrees: raw_name) rather than the
    /// row index means a resort between the two clicks (e.g. a task finishing) can
    /// never open the menu on whatever row slid into the clicked slot.
    pub last_click: Option<(ListPane, String, u64)>,
    pub size: (u16, u16),
    /// Previous frame's hit geometry, stored by the main loop after every draw.
    /// Mouse routing reads it; it always matches the screen because every state
    /// change redraws before the next event is processed.
    pub hit: HitMap,
    /// Render-feedback twin of `hit`: the detail pane's true max scroll offset
    /// (`rendered lines − viewport height`), written by the view each draw via
    /// interior mutability (the view only holds `&App`). `detail_scroll` clamps
    /// against it so the STORED offset can never run past the content — an
    /// unclamped offset kept growing on over-scroll and made the user "scroll
    /// back through" phantom distance. Same freshness argument as `hit`: every
    /// state change redraws, so it always matches what is on screen.
    pub detail_max_scroll: std::cell::Cell<usize>,
    /// Render-feedback twin of `detail_max_scroll`: the detail pane's WRAPPED
    /// display-line count for the last frame (post line-wrapping). Written by the
    /// view each draw; the scrollbar-drag math reads it so its scrollable extent
    /// matches the on-screen wrapped lines instead of recomputing the wrap.
    pub detail_wrapped_len: std::cell::Cell<usize>,
    /// Render-feedback for the picker preview (ActionMenu/DefPick right panel):
    /// max scroll offset (wrapped lines − pane height) written by the view each
    /// draw. The wheel clamps `preview_scroll` against it. Same freshness
    /// argument as `detail_max_scroll`.
    pub menu_preview_max_scroll: std::cell::Cell<usize>,
    /// Active left-mouse drag: `Some(kind)` between a `Down` on a draggable
    /// target (scrollbar thumb/track, a pane divider, a detail text-selection)
    /// and the matching `Up`.
    pub drag: Option<DragKind>,
    /// Current DETAIL-pane text selection (tmux-style copy-on-drag). `Some`
    /// while dragging and briefly after release (a 1s post-copy fade); anchored
    /// to absolute wrapped-line indices so scrolling keeps the same text
    /// highlighted.
    pub detail_selection: Option<DetailSelection>,
    /// Monotonic selection generation. Incremented when a selection starts; the
    /// post-copy fade timer carries the epoch it was armed with, so an expiry
    /// arriving after a NEWER selection began is recognized as stale and ignored.
    pub selection_epoch: u64,
    /// Render-feedback twin of `hit` for the DETAIL content: the geometry +
    /// wrapped lines of the last frame, so a mouse `(col,row)` can be resolved to
    /// a [`DetailPoint`] against exactly what was drawn (interior mutability — the
    /// view holds only `&App`).
    pub detail_geom: std::cell::RefCell<DetailGeom>,
    /// Session-only pane-layout overrides set by dragging the dividers (global,
    /// not per-tab, not persisted to disk). `None` = the default size formula.
    /// Held as requested heights/width; `pane_layout`/`clamp_left_cols` re-clamp
    /// every frame, so a terminal resize can never leave them invalid.
    pub left_cols: Option<u16>,
    pub queue_h_override: Option<u16>,
    pub tasks_h_override: Option<u16>,
    /// Collapsed flag per list pane `[queue, tasks, worktrees]`. A collapsed pane
    /// renders only its title bar; its rows/selection survive the collapse.
    /// Part of the per-project layout (mirrors the active project's saved state).
    pub collapsed: [bool; 3],
    /// Per-project persisted layout (divider overrides + collapsed flags), keyed
    /// by project name like `ui_by_tab`. The live `left_cols`/`*_override`/
    /// `collapsed` fields mirror the active project; this map is the store that is
    /// serialized to disk. Loaded once at startup, written through on collapse
    /// toggle and divider drag-end.
    pub layout_by_project: HashMap<String, crate::layout::ProjectLayout>,
    /// Which project's layout the live fields currently reflect. Drives the
    /// stash-old / load-new swap when the active project changes.
    applied_layout_repo: Option<String>,
    /// Where `layout_by_project` persists (`<state_dir>/tui-layout.json`).
    layout_path: PathBuf,
    pub last_healed_build_id: Option<String>,
    // self-heal effect state (mirror heal.ts App refs: `healing`, `healStatusShown`)
    healing: bool,
    heal_status_shown: bool,
    /// False in attach-only mode (`--no-heal`): the daemon belongs to another
    /// checkout, so a build-id mismatch is expected and must never trigger a
    /// restart (two TUIs from different worktrees would fight over it).
    pub heal_enabled: bool,
    pub sock_path: PathBuf,
    pub runs_dir: PathBuf,
    /// Whether the TUI is running inside a tmux session (`$TMUX` present at
    /// startup). Gates the tmux-only verbs (`g` goto / queue Resume): the daemon
    /// opens new panes/windows via tmux, so they are inert outside it. Read from
    /// the environment once in `new`; tests set it directly.
    pub inside_tmux: bool,
}

fn now_epoch_s() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A second click on the same row within this many milliseconds is a
/// double-click (opens the action menu). A slower second click only re-selects.
const DOUBLE_CLICK_MS: u64 = 400;

/// Lines scrolled per mouse-wheel tick — the common TUI/terminal step (1 line
/// per tick read as sluggish over long content). Shared by the DETAIL pane and
/// every picker/run-form PREVIEW panel so the two can never drift apart.
pub(crate) const WHEEL_STEP: i32 = 3;

impl App {
    pub fn new(runs_dir: PathBuf, sock_path: PathBuf) -> Self {
        Self {
            snapshot: None,
            connected: false,
            active_tab: 0,
            ui_by_tab: HashMap::new(),
            mode: Mode::List,
            status_line: None,
            run_files: None,
            defs_by_project: HashMap::new(),
            settings: None,
            defs_inflight: HashSet::new(),
            full_defs: HashMap::new(),
            full_defs_inflight: HashSet::new(),
            now_epoch_s: now_epoch_s(),
            now_ms: 0,
            prefix_armed: false,
            last_click: None,
            size: (0, 0),
            hit: HitMap::new(),
            detail_max_scroll: std::cell::Cell::new(0),
            detail_wrapped_len: std::cell::Cell::new(0),
            menu_preview_max_scroll: std::cell::Cell::new(0),
            drag: None,
            detail_selection: None,
            selection_epoch: 0,
            detail_geom: std::cell::RefCell::new(DetailGeom::default()),
            left_cols: None,
            queue_h_override: None,
            tasks_h_override: None,
            collapsed: [false, false, false],
            layout_by_project: HashMap::new(),
            applied_layout_repo: None,
            // Derived here (not read yet) so `App::new` stays disk-free — unit
            // tests never depend on the developer's real state file. `main`
            // calls `load_layout` to populate the map from disk at startup.
            layout_path: crate::paths::layout_path(&crate::paths::state_path()),
            last_healed_build_id: None,
            healing: false,
            heal_status_shown: false,
            heal_enabled: true,
            sock_path,
            runs_dir,
            inside_tmux: std::env::var_os("TMUX").is_some(),
        }
    }

    /// Load persisted per-project layout from disk (best-effort; missing/corrupt
    /// → empty map). Called once at startup from `main`; unit-test constructions
    /// skip it so tests never depend on the developer's real state file.
    pub fn load_layout(&mut self) {
        self.layout_by_project = crate::layout::load(&self.layout_path);
    }

    /// The live layout fields as a `ProjectLayout` (what the active project shows
    /// right now).
    fn live_layout(&self) -> crate::layout::ProjectLayout {
        crate::layout::ProjectLayout {
            left_cols: self.left_cols,
            queue_h: self.queue_h_override,
            tasks_h: self.tasks_h_override,
            collapsed: self.collapsed,
        }
    }

    /// Copy the live layout fields into the map under the active repo.
    fn stash_active_layout(&mut self) {
        if let Some(repo) = self.active_repo() {
            let live = self.live_layout();
            self.layout_by_project.insert(repo, live);
        }
    }

    /// Load a repo's saved layout into the live fields (defaults when absent).
    fn apply_saved_layout(&mut self, repo: Option<&str>) {
        let l = repo
            .and_then(|r| self.layout_by_project.get(r).cloned())
            .unwrap_or_default();
        self.left_cols = l.left_cols;
        self.queue_h_override = l.queue_h;
        self.tasks_h_override = l.tasks_h;
        self.collapsed = l.collapsed;
    }

    /// Keep the live layout fields aligned to the active project. When the active
    /// project changes (tab switch, first snapshot), stash the outgoing project's
    /// live fields into the map and load the incoming project's saved layout.
    /// Returns true when the live fields were swapped (forces a redraw).
    fn reconcile_active_layout(&mut self) -> bool {
        let current = self.active_repo();
        if current == self.applied_layout_repo {
            return false;
        }
        if let Some(prev) = self.applied_layout_repo.take() {
            let live = self.live_layout();
            self.layout_by_project.insert(prev, live);
        }
        self.apply_saved_layout(current.as_deref());
        self.applied_layout_repo = current;
        true
    }

    /// Persist the whole per-project map off the UI thread. Stashes the live
    /// fields first so the active project's latest geometry is included.
    fn save_layout_cmd(&mut self) -> Cmd {
        self.stash_active_layout();
        Cmd::SaveLayout {
            path: self.layout_path.clone(),
            json: crate::layout::serialize(&self.layout_by_project),
        }
    }

    /// Flip a list pane's collapsed flag and persist. Focus and selection are
    /// untouched — a collapsed pane just hides its rows.
    fn toggle_collapse(&mut self, pane: ListPane, cmds: &mut Vec<Cmd>) {
        self.collapsed[pane.idx()] = !self.collapsed[pane.idx()];
        cmds.push(self.save_layout_cmd());
    }

    /// Set a status line owned by the self-heal effect (so a later healthy snapshot
    /// may clear it without touching unrelated statuses). Mirrors setHealStatus.
    fn set_heal_status(&mut self, line: &str) {
        self.heal_status_shown = true;
        self.status_line = Some(line.to_string());
    }

    /// Compare the daemon build to disk and act. Called on every Snapshot. Returns
    /// the commands to run (a `Cmd::Heal` only on restart-now). Mirrors heal.ts App effect.
    fn heal_on_snapshot(&mut self) -> Vec<Cmd> {
        if !self.heal_enabled {
            return Vec::new();
        }
        let (build_id, running) = match &self.snapshot {
            Some(s) => (s.build_id.clone(), s.running.len()),
            None => return Vec::new(),
        };
        let disk = crate::heal::disk_build_id(&crate::paths::daemon_dist_dir());
        let decision = crate::heal::decide_heal(
            build_id.as_deref(),
            &disk,
            running,
            self.last_healed_build_id.as_deref(),
        );
        match decision {
            crate::heal::HealDecision::None => {
                if crate::heal::is_stale(build_id.as_deref(), &disk) {
                    // Stale but declined (loop guard). Suppress while a restart is
                    // mid-flight so a lingering old-daemon push can't raise a false alarm.
                    if !self.healing {
                        self.set_heal_status("daemon still outdated — restart it manually");
                    }
                } else {
                    // Healthy: reset guard + clear our own status.
                    self.last_healed_build_id = None;
                    self.healing = false;
                    if self.heal_status_shown {
                        self.heal_status_shown = false;
                        self.status_line = None;
                    }
                }
                Vec::new()
            }
            crate::heal::HealDecision::Defer => {
                self.set_heal_status("daemon outdated — will restart when idle");
                Vec::new()
            }
            crate::heal::HealDecision::RestartNow => {
                // Record the attempt (loop guard) before firing.
                self.last_healed_build_id = Some(disk);
                self.healing = true;
                self.set_heal_status("daemon outdated — restarting…");
                vec![Cmd::Heal]
            }
        }
    }

    /// Build a `Cmd::Rpc` with the same defaults the Ink `createActions` layer
    /// baked in: 5s default timeout, a 10-minute budget for `createWorktree`
    /// (post-create hooks run for minutes), and `runDefinition` treated as
    /// timeout-ok (discovery can outlive the client; the push sub re-syncs).
    // First callers: `execute_menu_action` and the confirm-dialog handler (Task 14).
    fn dispatch_rpc(&mut self, label: impl Into<String>, method: &str, params: serde_json::Value, opts: RpcOpts) -> Cmd {
        // createWorktree no longer routes through here — it has a dedicated Cmd
        // (its 10-minute budget lives in the event handler).
        let timeout_ms = opts.timeout_ms.unwrap_or(5_000);
        let timeout_is_ok = opts.timeout_is_ok || method == "runDefinition";
        Cmd::Rpc {
            label: label.into(),
            call: RpcCall { method: method.to_string(), params },
            timeout_ms,
            timeout_is_ok,
            invalidate_defs_for: opts.invalidate_defs_for,
        }
    }

    /// True while the ACTIVE project has a running task — the only time the 1s
    /// Tick (elapsed-label repaint) is armed. Re-evaluated after every event.
    pub fn wants_tick(&self) -> bool {
        let Some(snapshot) = &self.snapshot else {
            return false;
        };
        // Active-project derivation over config projects; Task 8 re-wires this
        // through selectors::build_tabs so synthetic tabs tick too.
        let Some(project) = snapshot.projects.get(self.active_tab) else {
            return false;
        };
        snapshot
            .tasks
            .iter()
            .any(|t| t.status == TaskStatus::Running && t.target.repo == project.name)
    }

    // The detail-scroll cluster below is defined here so scroll semantics land
    // with the detail work; Task 11's key dispatch (`apply_action`) is its first
    // caller.

    /// Mutable UI state for the active tab, creating a default entry on first
    /// touch. Keyed by the active tab name (same derivation as `view::compute`).
    pub(crate) fn ui(&mut self) -> &mut TabUiState {
        let tabs = self
            .snapshot
            .as_ref()
            .map(crate::selectors::build_tabs)
            .unwrap_or_default();
        let active_index = self.active_tab.min(tabs.len().saturating_sub(1));
        let key = tabs.get(active_index).map(|t| t.name.clone()).unwrap_or_default();
        self.ui_by_tab.entry(key).or_default()
    }

    /// Filtered row count of a list pane (what the cursor clamps against).
    pub(crate) fn visible_len(&self, pane: ListPane) -> usize {
        let c = crate::view::compute(self);
        match pane {
            ListPane::Queue => c.queue.len(),
            ListPane::Tasks => c.defs.len(),
            ListPane::Worktrees => c.worktrees.len(),
        }
    }

    /// The stable identity of the row at visible index `i` in `pane`, used to key
    /// the double-click sequence so a resort between clicks can't misfire the menu
    /// on the wrong row. Queue: `task_id`; tasks: `repo/name`; worktrees: `raw_name`.
    /// `None` when the index is out of range (empty/shrunk list).
    pub(crate) fn row_identity(&self, pane: ListPane, i: usize) -> Option<String> {
        let c = crate::view::compute(self);
        match pane {
            ListPane::Queue => c.queue.get(i).map(|r| r.task_id.clone()),
            ListPane::Tasks => c.defs.get(i).map(|d| format!("{}/{}", d.repo, d.name)),
            ListPane::Worktrees => c.worktrees.get(i).map(|r| r.raw_name.clone()),
        }
    }

    fn focused_list(&mut self) -> Option<ListPane> {
        match self.ui().focus {
            PaneId::Queue => Some(ListPane::Queue),
            PaneId::Tasks => Some(ListPane::Tasks),
            PaneId::Worktrees => Some(ListPane::Worktrees),
            PaneId::Detail => None,
        }
    }

    fn set_focus(&mut self, pane: PaneId) {
        let list = match pane {
            PaneId::Queue => Some(ListPane::Queue),
            PaneId::Tasks => Some(ListPane::Tasks),
            PaneId::Worktrees => Some(ListPane::Worktrees),
            PaneId::Detail => None,
        };
        let ui = self.ui();
        ui.focus = pane;
        if let Some(l) = list {
            ui.last_list_pane = l;
            ui.scroll_offset = 0; // leaving detail resets its scroll (parity)
        }
        // A focus change swaps the detail content out from under any selection.
        self.detail_selection = None;
    }

    /// Set a list pane's cursor (clamped), clear the anchor, reset scroll, and
    /// schedule the debounced run-file read. Shared by keys, wheel, and clicks.
    pub(crate) fn set_cursor(&mut self, pane: ListPane, cursor: usize, cmds: &mut Vec<Cmd>) -> bool {
        let len = self.visible_len(pane);
        let next = if len == 0 { 0 } else { cursor.min(len - 1) };
        let sel = &mut self.ui().selections[pane as usize];
        let changed = sel.cursor != next || sel.anchor.is_some();
        sel.cursor = next;
        sel.anchor = None;
        if changed {
            self.ui().scroll_offset = 0;
            // New detail content selected → drop any lingering text selection.
            self.detail_selection = None;
            // A different worktree selection swaps the lane-task list out — reset
            // the detail row cursor so it never points past the new list.
            if pane == ListPane::Worktrees {
                self.ui().detail_row = 0;
            }
            // A different QUEUE selection re-defaults the Run sub-tab: a still-
            // running task has nothing in its report yet, so land on the live
            // transcript instead; otherwise the report is the useful summary.
            // Only a genuinely NEW row resets this — manual ctrl+x/z navigation
            // while viewing the SAME row is never overridden.
            if pane == ListPane::Queue {
                let running = crate::view::compute(self)
                    .queue
                    .get(next)
                    .is_some_and(|r| r.running);
                self.ui().sub_tab[DetailKind::Run as usize] = if running {
                    crate::detail::RUN_TAB_TRANSCRIPT
                } else {
                    crate::detail::RUN_TAB_REPORT
                };
            }
            self.schedule_run_read(cmds, 120);
        }
        changed
    }

    /// Active project name (config tabs + synthetic orphan tabs), clamped to the
    /// current `active_tab`. Mirrors `view::compute`'s active-name derivation.
    pub(crate) fn active_repo(&self) -> Option<String> {
        let snap = self.snapshot.as_ref()?;
        let tabs = crate::selectors::build_tabs(snap);
        if tabs.is_empty() {
            return None;
        }
        let idx = self.active_tab.min(tabs.len() - 1);
        Some(tabs[idx].name.clone())
    }

    /// A clone of the active tab's UI state (default when untouched).
    fn active_ui(&self) -> TabUiState {
        self.active_repo()
            .and_then(|r| self.ui_by_tab.get(&r).cloned())
            .unwrap_or_default()
    }

    /// Emit a `definition` (full/prompt) fetch for `repo/name` when its prompt is
    /// neither cached nor already in flight; marks in-flight before returning so
    /// repeat calls dedup. Shared by the def-picker prefetch and the run-form open
    /// paths (both show the prompt in their right panel).
    fn ensure_full_def(&mut self, repo: &str, name: &str) -> Vec<Cmd> {
        let key = format!("{repo}/{name}");
        if self.full_defs.contains_key(&key) || self.full_defs_inflight.contains(&key) {
            return Vec::new();
        }
        self.full_defs_inflight.insert(key);
        vec![Cmd::FetchDefinition { repo: repo.to_string(), name: name.to_string() }]
    }

    /// Emit a lazy `definitions` fetch for the active repo when its summaries are
    /// neither cached nor already in flight. Marks in-flight before returning so
    /// repeated calls before the reply dedup. The event loop calls this after
    /// every `update` (mirrors the TS effect that re-fires after every render,
    /// including after invalidation drops the cache).
    pub(crate) fn reconcile_defs(&mut self) -> Option<Cmd> {
        let repo = self.active_repo()?;
        if self.defs_by_project.contains_key(&repo) || self.defs_inflight.contains(&repo) {
            return None;
        }
        self.defs_inflight.insert(repo.clone());
        Some(Cmd::FetchDefinitions { repo })
    }

    /// Open the task menu (`t`): the def picker over the active repo. Bails
    /// quietly with no repo; sets a status line and stays in `List` when the repo
    /// has no definitions. When the worktrees pane holds focus and a non-session
    /// worktree row is selected, that row's raw name (and non-empty branch)
    /// become the FIXED arg context; otherwise both are `None`. Returns the
    /// prompt-prefetch commands for the first highlighted def.
    fn open_task_menu(&mut self) -> Vec<Cmd> {
        // `t` is WORKTREES-only; a bulk range there isn't in the doable set
        // (only `Remove` is) — refuse rather than silently targeting just the
        // cursor row's worktree.
        if self.bulk_blocked(ListPane::Worktrees, crate::hit::PaneButton::Tasks) {
            return Vec::new();
        }
        let Some(repo) = self.active_repo() else {
            return Vec::new();
        };
        let defs = self.defs_by_project.get(&repo).cloned().unwrap_or_default();
        if defs.is_empty() {
            self.status_line = Some("no task definitions found".into());
            return Vec::new();
        }
        let (worktree, branch) = if self.active_ui().last_list_pane == ListPane::Worktrees {
            match self.selected_worktree_row() {
                Some(row) if !row.is_session => {
                    let branch = if row.branch.is_empty() { None } else { Some(row.branch.clone()) };
                    (Some(row.raw_name.clone()), branch)
                }
                _ => (None, None),
            }
        } else {
            (None, None)
        };
        self.mode = Mode::DefPick { defs, index: 0, worktree, branch, query: String::new(), preview_scroll: 0 };
        self.prefetch_full_def()
    }

    /// Emit a lazy `definition` (full/prompt) fetch for the currently highlighted
    /// def-pick row when its prompt is neither cached nor already in flight. Marks
    /// in-flight before returning so repeat calls (rapid navigation) dedup. No-op
    /// when the mode is not `DefPick` or the filter matches nothing.
    fn prefetch_full_def(&mut self) -> Vec<Cmd> {
        let (repo, name) = {
            let Mode::DefPick { defs, index, query, .. } = &self.mode else {
                return Vec::new();
            };
            let filtered = crate::selectors::filter_rows(defs, query, |d| d.name.clone());
            let Some(def) = filtered.get(*index).and_then(|&i| defs.get(i)) else {
                return Vec::new();
            };
            (def.repo.clone(), def.name.clone())
        };
        self.ensure_full_def(&repo, &name)
    }

    /// Emit a lazy full-definition fetch when the detail pane is showing a
    /// Definition context (Tasks pane focused last, cursor on a def) whose full
    /// body is neither cached in `full_defs` nor already in flight. Mirrors the
    /// view's derivation (search-filter then clamp) so the fetched key is exactly
    /// the def the detail pane resolves. Called by the event loop after every
    /// `update`, sibling to [`Self::reconcile_defs`].
    pub(crate) fn reconcile_full_def(&mut self) -> Option<Cmd> {
        let repo = self.active_repo()?;
        let ui = self.ui_by_tab.get(&repo)?;
        if ui.last_list_pane != ListPane::Tasks {
            return None;
        }
        let defs = self.defs_by_project.get(&repo)?;
        let idx = crate::selectors::filter_rows(defs, &ui.search[1], |d| d.name.clone());
        if idx.is_empty() {
            return None;
        }
        let cursor = ui.selections[1].cursor.min(idx.len() - 1);
        let def = &defs[idx[cursor]];
        let key = format!("{}/{}", def.repo, def.name);
        if self.full_defs.contains_key(&key) || self.full_defs_inflight.contains(&key) {
            return None;
        }
        self.full_defs_inflight.insert(key);
        Some(Cmd::FetchDefinition { repo: def.repo.clone(), name: def.name.clone() })
    }

    fn set_sub_tab_clamped(&mut self, idx: usize, cmds: &mut Vec<Cmd>) -> bool {
        let (kind, cur) = self.detail_kind_and_subtab();
        let next = crate::detail::clamp_sub_tab(idx, kind);
        if next == cur {
            return false;
        }
        self.ui().sub_tab[kind as usize] = next;
        self.reset_scroll();
        self.schedule_run_read(cmds, 120);
        true
    }

    /// Staged Esc in `Mode::List`: (1) drop the pane's bulk selection — the
    /// anchored range AND its marks, together, since from the user's side they
    /// are one selection, not two things to peel back separately; (2) else clear
    /// the pane's search filter. Returns whether anything changed (an Esc with
    /// nothing to clear is inert). Any non-List mode is dismissed first.
    fn clear_esc(&mut self) -> bool {
        if !matches!(self.mode, Mode::List) {
            self.mode = Mode::List;
            return true;
        }
        let Some(pane) = self.focused_list() else { return false };
        let sel = self.ui().selections[pane as usize];
        let has_marks = !self.ui().marks[pane as usize].is_empty();
        if sel.anchor.is_some() || has_marks {
            self.ui().selections[pane as usize] = Selection { cursor: sel.cursor, anchor: None };
            self.ui().marks[pane as usize].clear();
            return true;
        }
        if !self.ui().search[pane as usize].is_empty() {
            self.ui().search[pane as usize].clear();
            self.ui().selections[pane as usize] = Selection { cursor: 0, anchor: None };
            return true;
        }
        false
    }

    /// Queue a debounced run-file read for the current selection (no-op for
    /// non-Run contexts). Called after selection, sub-tab, tab, focus, and
    /// snapshot changes.
    pub(crate) fn schedule_run_read(&self, cmds: &mut Vec<Cmd>, delay_ms: u64) {
        if let Some((task_id, _)) = self.selected_run_task() {
            cmds.push(Cmd::ReadRunFiles {
                task_id,
                tail_lines: self.tail_lines(),
                delay_ms,
            });
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod action_result_tests;

#[cfg(test)]
mod menu_flow_tests;

#[cfg(test)]
mod input_modal_tests;

#[cfg(test)]
mod bulk_flow_tests;

#[cfg(test)]
mod mark_flow_tests;

#[cfg(test)]
mod def_pick_tests;

#[cfg(test)]
mod heal_wiring_tests;

#[cfg(test)]
mod form_tests;
