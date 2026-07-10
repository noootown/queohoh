use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crossterm::event::{KeyCode, KeyEventKind};
use tui_input::backend::crossterm::EventHandler;

use crate::event::{Cmd, Event, RpcCall};
use crate::hit::{HitMap, HitTarget};
use crate::ipc::types::{ArgSpec, DefinitionSummary, StateSnapshot, TaskDefinition, TaskStatus};
use crate::keymap::AppAction;
use crate::runfiles::RunFiles;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneId {
    Queue,
    Tasks,
    Worktrees,
    Detail,
}

/// What a left-mouse drag is currently manipulating, recorded on `Down` over a
/// draggable target and cleared on `Up`. Generalizes the old scrollbar-only drag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragKind {
    /// Proportional scrollbar drag on a pane (behavior unchanged).
    Scrollbar(PaneId),
    /// Horizontal pane divider: `0` = queue/tasks, `1` = tasks/worktrees.
    DividerH(usize),
    /// Vertical divider between the left pane stack and DETAIL.
    DividerV,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListPane {
    Queue = 0,
    Tasks = 1,
    Worktrees = 2,
}

impl ListPane {
    pub fn idx(self) -> usize {
        self as usize
    }
}

/// Which detail-pane context is showing. Discriminants index `TabUiState.sub_tab`
/// (one remembered sub-tab per kind). See `detail::derive_context`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetailKind {
    Run = 0,
    Definition = 1,
    Worktree = 2,
    Empty = 3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Selection {
    pub cursor: usize,
    pub anchor: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TabUiState {
    /// Invariant: always one of the three list panes (`Queue`/`Tasks`/
    /// `Worktrees`). Detail is display-only and can never be focused — no code
    /// path sets `focus = PaneId::Detail`. `TabUiState` is session-only (never
    /// serialized), so there is no persisted value to coerce; the invariant is
    /// upheld at the mutation sites (`set_focus`, `CyclePane`).
    pub focus: PaneId,
    pub last_list_pane: ListPane,
    pub selections: [Selection; 3],
    pub search: [String; 3],
    pub sub_tab: [usize; 4], // indexed by DetailKind (enum lands in Task 9)
    pub scroll_offset: usize,
}

impl Default for TabUiState {
    fn default() -> Self {
        Self {
            focus: PaneId::Queue,
            last_list_pane: ListPane::Queue,
            selections: [Selection::default(); 3],
            search: [String::new(), String::new(), String::new()],
            sub_tab: [0; 4],
            scroll_offset: 0,
        }
    }
}

/// Fresh-vs-main session choice for a new adhoc task (Task 15 consumes it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionMode {
    Fresh,
    Main,
}

/// Subset of the contract `Mode` — later tasks add ConfirmBulkRemove (16),
/// DefPick (18), DefArgs (19), CreateWorktree (21). Variants are only ever
/// added. `PartialEq` is intentionally not derived: `AddTask`/`WorktreeInput`
/// carry a `tui_input::Input`, which is not `PartialEq`; nothing compares
/// `Mode` by value (tests use `matches!`).
#[derive(Debug, Clone, Default)]
pub enum Mode {
    #[default]
    List,
    /// Filter-typing for one list pane. Printable keys append to
    /// `TabUiState.search[pane]`; the pane title shows `/query█`.
    Search { pane: ListPane },
    /// Full-screen keymap overlay; any key returns to `List`.
    Help,
    /// Single-target action menu over the last-focused list pane's selection.
    /// `index` is the highlighted row; disabled rows are skipped on Enter.
    ActionMenu { title: String, items: Vec<crate::action_menu::ActionItem>, index: usize },
    /// Destructive-confirm for `Remove worktree…`: y removes, n/q/esc cancel.
    ConfirmRemove { repo: String, worktree: String, branch: String },
    /// Destructive-confirm for a bulk `Remove worktrees…`: y removes each,
    /// n/q/esc cancel. `names` are the frozen raw worktree names (Task 16).
    ConfirmBulkRemove { repo: String, names: Vec<String> },
    /// New adhoc-task prompt. Constructed here (Task 14); its key handling and
    /// render land in Task 15.
    AddTask { worktree: Option<String>, session: SessionMode, input: tui_input::Input },
    /// Assign-worktree name input for a needs-input task. Constructed here
    /// (Task 14); its key handling and render land in Task 15.
    WorktreeInput { task_id: String, input: tui_input::Input },
    /// "Run task definition" picker over a targeted worktree (Task 18). `defs`
    /// is the repo's summaries in server (alphabetical) order, `index` the
    /// highlighted row; `worktree`/`branch` are the explicit-target context that
    /// drives the chosen def's args as FIXED values.
    DefPick {
        defs: Vec<DefinitionSummary>,
        index: usize,
        worktree: Option<String>,
        branch: Option<String>,
    },
    /// Per-arg entry form for a chosen def (Task 18 constructs it; its key
    /// handling + render land in Task 19/20).
    DefArgs { form: crate::view::args_form::ArgsForm },
    /// New-worktree branch-name prompt (Task 21). Enter validates via
    /// `worktree_context::validate_branch`; invalid keeps the modal open with
    /// `error` set, valid dispatches `createWorktree` and closes immediately.
    CreateWorktree { input: tui_input::Input, error: Option<String> },
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Update {
    pub dirty: bool,
    pub cmds: Vec<Cmd>,
}

/// Per-call overrides for `App::dispatch_rpc`. Contract addition (M2).
#[derive(Debug, Default, Clone)]
pub struct RpcOpts {
    pub timeout_ms: Option<u64>,
    pub timeout_is_ok: bool,
    pub invalidate_defs_for: Option<String>,
}

#[derive(Clone)]
pub struct App {
    pub snapshot: Option<StateSnapshot>,
    pub connected: bool,
    pub active_tab: usize,
    pub ui_by_tab: HashMap<String, TabUiState>,
    pub mode: Mode,
    pub status_line: Option<String>,
    pub run_files: Option<(String, RunFiles)>,
    pub defs_by_project: HashMap<String, Vec<DefinitionSummary>>,
    /// Repos with a `FetchDefinitions` in flight — the lazy per-tab fetch dedup
    /// set. `reconcile_defs` inserts before emitting; `Event::Definitions`
    /// clears on arrival (Task 18).
    pub defs_inflight: HashSet<String>,
    pub full_defs: HashMap<String, TaskDefinition>, // keyed "repo/name"
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
    /// Active left-mouse drag: `Some(kind)` between a `Down` on a draggable
    /// target (scrollbar thumb/track, a pane divider) and the matching `Up`.
    pub drag: Option<DragKind>,
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
    pub sock_path: PathBuf,
    pub runs_dir: PathBuf,
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
            defs_inflight: HashSet::new(),
            full_defs: HashMap::new(),
            now_epoch_s: now_epoch_s(),
            now_ms: 0,
            prefix_armed: false,
            last_click: None,
            size: (0, 0),
            hit: HitMap::new(),
            detail_max_scroll: std::cell::Cell::new(0),
            detail_wrapped_len: std::cell::Cell::new(0),
            drag: None,
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
            sock_path,
            runs_dir,
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
    // First callers: `execute_menu_action` and the ConfirmRemove handler (Task 14).
    fn dispatch_rpc(&mut self, label: impl Into<String>, method: &str, params: serde_json::Value, opts: RpcOpts) -> Cmd {
        let timeout_ms = opts.timeout_ms.unwrap_or(match method {
            "createWorktree" => 600_000,
            _ => 5_000,
        });
        let timeout_is_ok = opts.timeout_is_ok || method == "runDefinition";
        Cmd::Rpc {
            label: label.into(),
            call: RpcCall { method: method.to_string(), params },
            timeout_ms,
            timeout_is_ok,
            invalidate_defs_for: opts.invalidate_defs_for,
        }
    }

    pub fn update(&mut self, event: Event) -> Update {
        let up = self.update_event(event);
        // After any event, realign the live layout fields to the active project.
        // A tab switch or the first snapshot changes the active project; this is
        // the single place the stash-old / load-new swap happens. The event that
        // moves the active project (snapshot, tab switch) is already `dirty`, so
        // the swap needs no extra redraw signal of its own.
        self.reconcile_active_layout();
        up
    }

    fn update_event(&mut self, event: Event) -> Update {
        match event {
            Event::Snapshot(snapshot) => {
                self.snapshot = Some(snapshot);
                self.connected = true;
                let mut cmds = Vec::new();
                // A fresh snapshot can change (or first-establish) the selected
                // run — debounce a tail read for it.
                self.schedule_run_read(&mut cmds, 120);
                // Daemon self-heal: compare the reported build to disk and act
                // (Defer/RestartNow status + a Cmd::Heal on restart-now).
                cmds.extend(self.heal_on_snapshot());
                Update { dirty: true, cmds }
            }
            Event::RunFiles { task_id, files } => {
                let mut cmds = Vec::new();
                // Stale-read discard: the selection moved while the read was in
                // flight.
                let Some((sel_id, running)) = self.selected_run_task() else {
                    return Update { dirty: false, cmds };
                };
                if task_id != sel_id {
                    return Update { dirty: false, cmds };
                }
                // Poll loop via events: while the selected task runs, each read
                // result arms the next 1s read — no timer state in App.
                if running {
                    cmds.push(Cmd::ReadRunFiles {
                        task_id: sel_id.clone(),
                        tail_lines: self.tail_lines(),
                        delay_ms: 1000,
                    });
                }
                // Identical-content skip: quiet poll → 0 renders.
                let identical = self
                    .run_files
                    .as_ref()
                    .map(|(id, f)| *id == task_id && *f == files)
                    .unwrap_or(false);
                if identical {
                    return Update { dirty: false, cmds };
                }
                self.run_files = Some((task_id, files));
                Update { dirty: true, cmds }
            }
            Event::Disconnected => {
                // The retry loop re-sends this every ~2s while the daemon is
                // down; only the transition repaints (zero idle renders).
                let was_connected = self.connected;
                self.connected = false;
                Update { dirty: was_connected, cmds: vec![] }
            }
            Event::Resize => Update { dirty: true, cmds: vec![] },
            Event::Tick => {
                self.now_epoch_s = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                // Zero idle renders: the elapsed-label repaint only matters while
                // the active project has a running task. The main loop also gates
                // the Tick arm on `wants_tick`; this is the defensive second layer.
                Update { dirty: self.wants_tick(), cmds: vec![] }
            }
            // These overlay modes swallow keys (checked before generic list
            // handling). Guards include the Press filter so key-release events
            // fall through to the generic arm's no-op.
            Event::Key(k)
                if k.kind == KeyEventKind::Press && matches!(self.mode, Mode::ActionMenu { .. }) =>
            {
                use crossterm::event::KeyCode::*;
                match k.code {
                    Esc | Char('q') => {
                        self.mode = Mode::List;
                        Update { dirty: true, cmds: vec![] }
                    }
                    Up | Char('k') => {
                        if let Mode::ActionMenu { items, index, .. } = &mut self.mode {
                            // Circular: k on the first item wraps to the last.
                            *index = index.checked_sub(1).unwrap_or(items.len().saturating_sub(1));
                        }
                        Update { dirty: true, cmds: vec![] }
                    }
                    Down | Char('j') => {
                        if let Mode::ActionMenu { items, index, .. } = &mut self.mode {
                            // Circular: j on the last item wraps to the first.
                            *index = if *index + 1 >= items.len() { 0 } else { *index + 1 };
                        }
                        Update { dirty: true, cmds: vec![] }
                    }
                    Enter => {
                        // Extract the chosen action before dispatch so the
                        // `&self.mode` borrow ends first.
                        let chosen = if let Mode::ActionMenu { items, index, .. } = &self.mode {
                            items.get(*index).cloned()
                        } else {
                            None
                        };
                        match chosen {
                            Some(it) if it.disabled.is_none() => self.execute_menu_action(it.action),
                            _ => Update { dirty: false, cmds: vec![] }, // disabled row is inert
                        }
                    }
                    _ => Update { dirty: false, cmds: vec![] },
                }
            }
            Event::Key(k)
                if k.kind == KeyEventKind::Press && matches!(self.mode, Mode::ConfirmRemove { .. }) =>
            {
                use crossterm::event::KeyCode::*;
                match k.code {
                    Char('y') => {
                        let (repo, worktree) =
                            if let Mode::ConfirmRemove { repo, worktree, .. } = &self.mode {
                                (repo.clone(), worktree.clone())
                            } else {
                                unreachable!()
                            };
                        self.mode = Mode::List;
                        let cmd = self.dispatch_rpc(
                            "remove worktree",
                            "removeWorktree",
                            serde_json::json!({ "repo": repo, "name": worktree }),
                            RpcOpts::default(),
                        );
                        Update { dirty: true, cmds: vec![cmd] }
                    }
                    Char('n') | Char('q') | Esc => {
                        self.mode = Mode::List;
                        Update { dirty: true, cmds: vec![] }
                    }
                    _ => Update { dirty: false, cmds: vec![] },
                }
            }
            Event::Key(k)
                if k.kind == KeyEventKind::Press
                    && matches!(self.mode, Mode::ConfirmBulkRemove { .. }) =>
            {
                use crossterm::event::KeyCode::*;
                match k.code {
                    Char('y') => {
                        let (repo, names) =
                            if let Mode::ConfirmBulkRemove { repo, names } = &self.mode {
                                (repo.clone(), names.clone())
                            } else {
                                unreachable!()
                            };
                        self.clear_range(ListPane::Worktrees);
                        self.mode = Mode::List;
                        Update { dirty: true, cmds: vec![Cmd::RpcSeq {
                            verb: "removed".into(),
                            calls: names
                                .into_iter()
                                .map(|name| RpcCall {
                                    method: "removeWorktree".into(),
                                    params: serde_json::json!({ "repo": repo, "name": name }),
                                })
                                .collect(),
                            invalidate_defs_for: None,
                        }] }
                    }
                    Char('n') | Char('q') | Esc => {
                        self.mode = Mode::List;
                        Update { dirty: true, cmds: vec![] }
                    }
                    _ => Update { dirty: false, cmds: vec![] },
                }
            }
            // Text-input modals. Enter dispatches + closes, Esc cancels, every
            // other Press forwards to `tui_input`. Mouse never reaches here — the
            // `Event::Mouse` arm intercepts all mouse events before these fire.
            Event::Key(k)
                if k.kind == KeyEventKind::Press && matches!(self.mode, Mode::AddTask { .. }) =>
            {
                use crossterm::event::KeyCode::*;
                match k.code {
                    Enter => {
                        let (prompt, sess, worktree) =
                            if let Mode::AddTask { worktree, session, input } = &self.mode {
                                let sess = match session {
                                    SessionMode::Fresh => "fresh",
                                    SessionMode::Main => "main",
                                };
                                (input.value().to_string(), sess, worktree.clone())
                            } else {
                                unreachable!()
                            };
                        let repo = match self.active_repo() {
                            Some(r) => r,
                            None => {
                                self.mode = Mode::List;
                                return Update { dirty: true, cmds: vec![] };
                            }
                        };
                        let mut params =
                            serde_json::json!({ "prompt": prompt, "repo": repo, "session": sess });
                        if let Some(w) = worktree {
                            params["worktree"] = serde_json::Value::String(w);
                        }
                        self.mode = Mode::List;
                        let cmd =
                            self.dispatch_rpc("enqueue task", "enqueue", params, RpcOpts::default());
                        Update { dirty: true, cmds: vec![cmd] }
                    }
                    Esc => {
                        self.mode = Mode::List;
                        Update { dirty: true, cmds: vec![] }
                    }
                    _ => {
                        if let Mode::AddTask { input, .. } = &mut self.mode {
                            input.handle_event(&crossterm::event::Event::Key(k));
                        }
                        Update { dirty: true, cmds: vec![] }
                    }
                }
            }
            Event::Key(k)
                if k.kind == KeyEventKind::Press
                    && matches!(self.mode, Mode::WorktreeInput { .. }) =>
            {
                use crossterm::event::KeyCode::*;
                match k.code {
                    Enter => {
                        let (id, wt) = if let Mode::WorktreeInput { task_id, input } = &self.mode {
                            (task_id.clone(), input.value().to_string())
                        } else {
                            unreachable!()
                        };
                        self.mode = Mode::List;
                        let cmd = self.dispatch_rpc(
                            "assign worktree",
                            "setWorktree",
                            serde_json::json!({ "id": id, "worktree": wt }),
                            RpcOpts::default(),
                        );
                        Update { dirty: true, cmds: vec![cmd] }
                    }
                    Esc => {
                        self.mode = Mode::List;
                        Update { dirty: true, cmds: vec![] }
                    }
                    _ => {
                        if let Mode::WorktreeInput { input, .. } = &mut self.mode {
                            input.handle_event(&crossterm::event::Event::Key(k));
                        }
                        Update { dirty: true, cmds: vec![] }
                    }
                }
            }
            // Def-pick popup owns keys while open (checked before generic list
            // handling); key-release falls through to the generic no-op.
            Event::Key(k)
                if k.kind == KeyEventKind::Press && matches!(self.mode, Mode::DefPick { .. }) =>
            {
                self.def_pick_key(k.code)
            }
            // Args form owns keys while open (checked before generic list
            // handling); key-release falls through to the generic no-op.
            Event::Key(k)
                if k.kind == KeyEventKind::Press && matches!(self.mode, Mode::DefArgs { .. }) =>
            {
                self.def_args_key(&k)
            }
            // Create-worktree modal owns keys while open (checked before generic
            // list handling); key-release falls through to the generic no-op.
            Event::Key(k)
                if k.kind == KeyEventKind::Press
                    && matches!(self.mode, Mode::CreateWorktree { .. }) =>
            {
                self.create_worktree_key(&k)
            }
            Event::Key(key) => {
                if key.kind != KeyEventKind::Press {
                    return Update { dirty: false, cmds: vec![] };
                }
                match &self.mode {
                    Mode::Help => {
                        // Any key closes the help overlay.
                        self.mode = Mode::List;
                        Update { dirty: true, cmds: Vec::new() }
                    }
                    Mode::Search { pane } => {
                        let pane = *pane;
                        let mut dirty = true;
                        // Keystrokes that reset the cursor to 0 (printable,
                        // backspace), Enter-apply, and Esc-clear all change the
                        // effective selection, so they must schedule the debounced
                        // run-file read like every other selection path.
                        let mut cmds = Vec::new();
                        match key.code {
                            KeyCode::Enter => {
                                self.mode = Mode::List; // apply
                                self.schedule_run_read(&mut cmds, 120);
                            }
                            KeyCode::Esc => {
                                self.ui().search[pane as usize].clear();
                                self.ui().selections[pane as usize] =
                                    Selection { cursor: 0, anchor: None };
                                self.mode = Mode::List;
                                self.schedule_run_read(&mut cmds, 120);
                            }
                            KeyCode::Backspace => {
                                self.ui().search[pane as usize].pop();
                                self.ui().selections[pane as usize] =
                                    Selection { cursor: 0, anchor: None };
                                self.schedule_run_read(&mut cmds, 120);
                            }
                            KeyCode::Char(c)
                                if !key.modifiers.contains(
                                    crossterm::event::KeyModifiers::CONTROL,
                                ) =>
                            {
                                self.ui().search[pane as usize].push(c);
                                self.ui().selections[pane as usize] =
                                    Selection { cursor: 0, anchor: None };
                                self.schedule_run_read(&mut cmds, 120);
                            }
                            _ => dirty = false,
                        }
                        Update { dirty, cmds }
                    }
                    Mode::List => {
                        // Status line clears on ANY list-mode keypress (even unbound keys).
                        let had_status = self.status_line.take().is_some();
                        // tmux-style prefix: when armed, this key is consumed —
                        // `n`/`p` cycle project tabs (wrapping), anything else just
                        // disarms and is swallowed. Disarming always repaints (the
                        // footer indicator turns off).
                        if self.prefix_armed {
                            self.prefix_armed = false;
                            let action = match key.code {
                                KeyCode::Char('n') => crate::keymap::AppAction::CycleTab(1),
                                KeyCode::Char('p') => crate::keymap::AppAction::CycleTab(-1),
                                _ => crate::keymap::AppAction::None,
                            };
                            let up = self.apply_action(action);
                            return Update { dirty: true, cmds: up.cmds };
                        }
                        // Arm the prefix on ctrl+s. Consumed here so it never
                        // reaches the keymap; the next key resolves it above.
                        if matches!(key.code, KeyCode::Char('s'))
                            && key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
                        {
                            self.prefix_armed = true;
                            return Update { dirty: true, cmds: Vec::new() };
                        }
                        let action = crate::keymap::list_mode_action(&key, self.ui().focus);
                        let mut up = self.apply_action(action);
                        up.dirty = up.dirty || had_status;
                        up
                    }
                    // AddTask/WorktreeInput input handling lands in Task 15;
                    // ActionMenu/ConfirmRemove only reach here on key-release
                    // (their Press events are handled by the guarded arms above).
                    _ => Update { dirty: false, cmds: vec![] },
                }
            }
            Event::Mouse(m) => self.on_mouse(m),
            Event::ActionResult { status, invalidate_defs_for } => {
                // Success carries status = None → leave the line untouched (never clobber
                // a heal/create message with an empty). Failure carries the message.
                if status.is_some() {
                    self.status_line = status;
                }
                // A self-heal reported its outcome (success emits nothing; failure carries
                // "daemon busy — restart deferred"). Clear the in-flight flag and mark the
                // status heal-owned so the next healthy snapshot clears it.
                if self.healing {
                    self.healing = false;
                    self.heal_status_shown = true;
                }
                let mut cmds = Vec::new();
                if let Some(repo) = invalidate_defs_for {
                    // A run may change dedup state, so drop the cached defs and re-fetch
                    // eagerly (ports App.tsx `invalidateDefs` + the lazy re-fetch effect).
                    // Mark in flight so the event loop's `reconcile_defs` dedups against
                    // this eager re-fetch instead of emitting a duplicate.
                    self.defs_by_project.remove(&repo);
                    self.defs_inflight.insert(repo.clone());
                    cmds.push(Cmd::FetchDefinitions { repo });
                }
                Update { dirty: true, cmds }
            }
            Event::Definitions { repo, defs } => {
                // Cache the repo's summaries and clear its in-flight flag so the
                // next `reconcile_defs` sees it cached (ports the TS effect that
                // stores the fetch result and re-enables re-fetch after invalidation).
                // The daemon's `definitions` call returns entries for EVERY project
                // (a global def appears once per project) — keep only this repo's
                // (ports App.tsx `all.filter((d) => d.repo === activeName)`).
                let defs: Vec<DefinitionSummary> =
                    defs.into_iter().filter(|d| d.repo == repo).collect();
                self.defs_by_project.insert(repo.clone(), defs);
                self.defs_inflight.remove(&repo);
                Update { dirty: true, cmds: vec![] }
            }
            // Remaining ingestion arms (Definition detail) land later.
            _ => Update { dirty: false, cmds: vec![] },
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
            self.schedule_run_read(cmds, 120);
        }
        changed
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
                self.mode = Mode::Help;
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
                            self.schedule_run_read(&mut cmds, 120);
                        }
                        changed
                    }
                }
                None => false,
            },
            A::ScrollEdge(dir) => match self.focused_list() {
                // Lists: g/G jump the cursor to the first/last row.
                Some(pane) => {
                    let len = self.visible_len(pane);
                    let target = if dir < 0 { 0 } else { len.saturating_sub(1) };
                    self.set_cursor(pane, target, &mut cmds)
                }
                None => self.detail_scroll_edge(dir),
            },
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
                match self.open_action_menu() {
                    Some(mode) => self.mode = mode,
                    None => self.status_line = Some("nothing selected".into()),
                }
                true
            }
            A::Create => {
                match self.active_ui().last_list_pane {
                    ListPane::Queue => {
                        self.mode = Mode::AddTask {
                            worktree: None,
                            session: SessionMode::Fresh,
                            input: tui_input::Input::default(),
                        };
                    }
                    ListPane::Worktrees => {
                        self.mode = Mode::CreateWorktree {
                            input: tui_input::Input::default(),
                            error: None,
                        };
                    }
                    ListPane::Tasks => { /* no create on the tasks pane */ }
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
        };
        Update { dirty, cmds }
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

    /// Build the action menu for the last-focused list pane's current selection.
    /// Returns `None` when nothing is selectable (empty pane). Bulk (range > 1)
    /// support is added in Task 16 by prepending a guard here; Task 14 handles
    /// the single-target case.
    fn open_action_menu(&mut self) -> Option<Mode> {
        // Bulk branch: a multi-row range opens the bulk menu with eligibility
        // frozen at open time (Task 16). A single-row selection (anchor cleared
        // or collapsed) falls through to the single-target body below.
        {
            let ui = self.active_ui();
            let pane = ui.last_list_pane;
            let (start, end) = crate::view::selection_range(&ui.selections[pane.idx()]);
            if end > start {
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
                let (title, items) = crate::action_menu::queue_menu(row, task);
                Some(Mode::ActionMenu { title, items, index: 0 })
            }
            ListPane::Tasks => {
                let defs = self.defs_by_project.get(&repo).cloned().unwrap_or_default();
                let vis = crate::selectors::filter_rows(&defs, &ui.search[1], |d| d.name.clone());
                let cursor = ui.selections[1].cursor.min(vis.len().saturating_sub(1));
                let def = vis.get(cursor).and_then(|&i| defs.get(i))?;
                let (title, items) = crate::action_menu::tasks_menu(def);
                Some(Mode::ActionMenu { title, items, index: 0 })
            }
            ListPane::Worktrees => {
                let rows = crate::selectors::worktree_rows(snap, &repo);
                let vis = crate::selectors::filter_rows(&rows, &ui.search[2], |r| r.name.clone());
                let cursor = ui.selections[2].cursor.min(vis.len().saturating_sub(1));
                let row = vis.get(cursor).and_then(|&i| rows.get(i))?;
                let (title, items) = crate::action_menu::worktree_menu(&repo, row, inside_tmux);
                Some(Mode::ActionMenu { title, items, index: 0 })
            }
        }
    }

    /// Clear a list pane's selection anchor on the active tab (collapse a range
    /// to a single cursor). Called before every bulk dispatch, mirroring the
    /// App.tsx `runBulk` clear-then-dispatch order.
    fn clear_range(&mut self, pane: ListPane) {
        if let Some(repo) = self.active_repo() {
            if let Some(ui) = self.ui_by_tab.get_mut(&repo) {
                ui.selections[pane.idx()].anchor = None;
            }
        }
    }

    /// Clamp a frozen `[start, end]` selection span against the current visible
    /// row count. Returns `None` when nothing in the span survives (the visible
    /// set emptied), else `(start, hi, total)` with `start <= hi < vis_len` and
    /// `total` the surviving span width. Guards `vis[start..=hi]` from empty and
    /// inverted-range panics when a daemon snapshot shrinks the rows between the
    /// selection and the menu opening (`total` therefore counts survivors, so
    /// "(N of T)" never overcounts a range that partly scrolled off).
    fn clamp_span(start: usize, end: usize, vis_len: usize) -> Option<(usize, usize, usize)> {
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
    fn open_bulk_menu(&self, pane: ListPane, start: usize, end: usize) -> Option<Mode> {
        use crate::action_menu::{bulk_menu, BulkSelection};
        let snap = self.snapshot.as_ref()?;
        let repo = self.active_repo()?;
        let ui = self.active_ui();
        let (title, items) = match pane {
            ListPane::Queue => {
                let rows = crate::selectors::queue_rows(snap, &repo, self.now_epoch_s);
                let vis = crate::selectors::filter_rows(&rows, &ui.search[0], |r| r.summary.clone());
                let (start, hi, total) = Self::clamp_span(start, end, vis.len())?;
                let slice: Vec<&crate::selectors::QueueRow> =
                    vis[start..=hi].iter().filter_map(|&i| rows.get(i)).collect();
                let status_of = |id: &str| snap.tasks.iter().find(|t| t.id == id).map(|t| t.status);
                let live = || slice.iter().filter(|r| !r.archived);
                let rerun_ids: Vec<String> = live()
                    .filter(|r| matches!(status_of(&r.task_id), Some(TaskStatus::Failed) | Some(TaskStatus::NeedsInput)))
                    .map(|r| r.task_id.clone())
                    .collect();
                let skip_ids: Vec<String> = live()
                    .filter(|r| matches!(status_of(&r.task_id), Some(TaskStatus::Failed) | Some(TaskStatus::NeedsInput) | Some(TaskStatus::Done)))
                    .map(|r| r.task_id.clone())
                    .collect();
                bulk_menu(BulkSelection::Queue { rerun_ids, skip_ids, total })
            }
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
        Some(Mode::ActionMenu { title, items, index: 0 })
    }

    /// Perform a chosen (enabled) menu action: an RPC dispatch, a mode transition
    /// into a follow-up form/confirm, or (M3 stubs) a status line naming the
    /// replacing task. Always closes the menu first (`Mode::List`), then the
    /// form/confirm branches re-open the appropriate mode.
    fn execute_menu_action(&mut self, action: crate::action_menu::MenuAction) -> Update {
        use crate::action_menu::MenuAction as M;
        self.mode = Mode::List;
        match action {
            M::Rerun { id } => {
                let cmd = self.dispatch_rpc("rerun task", "retry", serde_json::json!({ "id": id }), RpcOpts::default());
                Update { dirty: true, cmds: vec![cmd] }
            }
            M::Skip { id } => {
                let cmd = self.dispatch_rpc("skip task", "skip", serde_json::json!({ "id": id }), RpcOpts::default());
                Update { dirty: true, cmds: vec![cmd] }
            }
            M::AssignWorktree { id } => {
                self.mode = Mode::WorktreeInput { task_id: id, input: tui_input::Input::default() };
                Update { dirty: true, cmds: vec![] }
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
            // Tasks pane → the definition is already chosen (ambient run). Zero-arg
            // defs dispatch immediately; otherwise open the args form with a
            // worktree-branch overlay prefilled from the selected worktree row.
            M::RunNamedDef { repo, name } => {
                let Some(def) = self
                    .defs_by_project
                    .get(&repo)
                    .and_then(|defs| defs.iter().find(|d| d.name == name))
                    .cloned()
                else {
                    self.status_line = Some("definition not found".into());
                    return Update { dirty: true, cmds: vec![] };
                };
                if def.args.is_empty() {
                    return Update {
                        dirty: true,
                        cmds: vec![Self::run_definition_cmd(&repo, &name, &[], None)],
                    };
                }
                let rows = self.active_worktree_rows();
                let selected = self.selected_worktree_row();
                let (args, initial) =
                    crate::worktree_context::ambient_run_args(&def.args, &rows, selected.as_ref());
                self.open_def_args(repo, name, args, HashMap::new(), initial, None);
                Update { dirty: true, cmds: vec![] }
            }
            // Worktree menu → the definition is not yet chosen; open the picker
            // for the targeted worktree (its branch drives args as FIXED on Enter).
            M::RunDef { worktree, branch } => {
                let repo = match self.active_repo() {
                    Some(r) => r,
                    None => return Update { dirty: false, cmds: vec![] },
                };
                let defs = self.defs_by_project.get(&repo).cloned().unwrap_or_default();
                if defs.is_empty() {
                    self.status_line = Some("no task definitions found".into());
                    return Update { dirty: true, cmds: vec![] };
                }
                self.mode = Mode::DefPick { defs, index: 0, worktree, branch };
                Update { dirty: true, cmds: vec![] }
            }
            M::CreateWorktree => {
                self.mode = Mode::CreateWorktree {
                    input: tui_input::Input::default(),
                    error: None,
                };
                Update { dirty: true, cmds: vec![] }
            }
            // Squash-merge the selected worktree's branch into a target: look up
            // the repo's `squash-merge` def and open the args form with `source`
            // FIXED to the branch (target stays editable). The def's own
            // `worktree: repo` governs where it runs — no worktree override.
            M::SquashMerge { branch } => {
                let repo = match self.active_repo() {
                    Some(r) => r,
                    None => return Update { dirty: false, cmds: vec![] },
                };
                let def = self
                    .defs_by_project
                    .get(&repo)
                    .and_then(|defs| defs.iter().find(|d| d.name == "squash-merge"))
                    .cloned();
                let Some(def) = def else {
                    self.status_line = Some(
                        "squash-merge definition not found — copy library/tasks/squash-merge to <workspace>/global/tasks/".into(),
                    );
                    return Update { dirty: true, cmds: vec![] };
                };
                let fixed = crate::worktree_context::context_arg_values(&branch);
                self.open_def_args(def.repo, def.name, def.args, fixed, HashMap::new(), None);
                Update { dirty: true, cmds: vec![] }
            }
            // --- Bulk actions (Task 16). Range cleared before dispatch; the
            // frozen ids/names ride inside the action. Verbs are past tense to
            // feed `seq_summary` ("reran 3", "started 1", …). ---
            M::BulkRerun { ids } => {
                self.clear_range(ListPane::Queue);
                Update { dirty: true, cmds: vec![Cmd::RpcSeq {
                    verb: "reran".into(),
                    calls: ids
                        .into_iter()
                        .map(|id| RpcCall { method: "retry".into(), params: serde_json::json!({ "id": id }) })
                        .collect(),
                    invalidate_defs_for: None,
                }] }
            }
            M::BulkSkip { ids } => {
                self.clear_range(ListPane::Queue);
                Update { dirty: true, cmds: vec![Cmd::RpcSeq {
                    verb: "skipped".into(),
                    calls: ids
                        .into_iter()
                        .map(|id| RpcCall { method: "skip".into(), params: serde_json::json!({ "id": id }) })
                        .collect(),
                    invalidate_defs_for: None,
                }] }
            }
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

    /// Build the fire-and-forget `runDefinition` command. Client timeout is
    /// treated as success (discovery can outlive it; the push subscription
    /// re-syncs), and a successful run invalidates the repo's def summaries.
    fn run_definition_cmd(repo: &str, name: &str, values: &[String], worktree: Option<&str>) -> Cmd {
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

    /// Build the fire-and-forget `createWorktree` command. A 10-minute budget
    /// (post-create `wt.toml` hooks routinely run for minutes) and a real
    /// timeout (`timeout_is_ok: false`) so a stall surfaces on the status line
    /// rather than silently "succeeding". No def cache to invalidate.
    fn create_worktree_cmd(repo: &str, name: &str) -> Cmd {
        Cmd::Rpc {
            label: format!("create worktree {name}"),
            call: RpcCall {
                method: "createWorktree".into(),
                params: serde_json::json!({ "repo": repo, "name": name }),
            },
            timeout_ms: 600_000,
            timeout_is_ok: false,
            invalidate_defs_for: None,
        }
    }

    /// `Mode::CreateWorktree` key handling. Enter validates the branch name:
    /// invalid keeps the modal open with the inline error; valid dispatches
    /// `createWorktree` and closes immediately (creation fires async and can
    /// take minutes — progress lives on the status line). Esc cancels; every
    /// other key edits the input and clears any prior error.
    fn create_worktree_key(&mut self, ev: &crossterm::event::KeyEvent) -> Update {
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

    /// Active project's worktree rows (unfiltered), used for ambient overlays.
    fn active_worktree_rows(&self) -> Vec<crate::selectors::WorktreeRow> {
        match (&self.snapshot, self.active_repo()) {
            (Some(snap), Some(repo)) => crate::selectors::worktree_rows(snap, &repo),
            _ => Vec::new(),
        }
    }

    /// Currently-selected worktree row (clamped cursor into the pane's rows).
    fn selected_worktree_row(&self) -> Option<crate::selectors::WorktreeRow> {
        let rows = self.active_worktree_rows();
        let cursor = self
            .active_repo()
            .and_then(|r| self.ui_by_tab.get(&r))
            .map(|ui| ui.selections[ListPane::Worktrees.idx()].cursor)
            .unwrap_or(0);
        rows.into_iter().nth(cursor)
    }

    /// Open the args form. `fixed`/`initial` and `worktree` are caller-decided.
    fn open_def_args(
        &mut self,
        repo: String,
        name: String,
        args: Vec<ArgSpec>,
        fixed: HashMap<String, String>,
        initial: HashMap<String, String>,
        worktree: Option<String>,
    ) {
        self.mode = Mode::DefArgs {
            form: crate::view::args_form::ArgsForm::new(repo, name, args, fixed, initial, worktree),
        };
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

    /// `Mode::DefPick` key handling: j/k move (clamped), q/esc close, Enter picks
    /// the highlighted def (zero-arg dispatch or open the args form with the
    /// targeted worktree's branch as FIXED context).
    fn def_pick_key(&mut self, key: KeyCode) -> Update {
        use crossterm::event::KeyCode::*;
        let Mode::DefPick { defs, index, .. } = &self.mode else {
            return Update { dirty: false, cmds: vec![] };
        };
        let (len, index) = (defs.len(), *index);
        match key {
            Esc | Char('q') => {
                self.mode = Mode::List;
                Update { dirty: true, cmds: vec![] }
            }
            Up | Char('k') => {
                // Circular: k on the first item wraps to the last.
                let i = index.checked_sub(1).unwrap_or(len.saturating_sub(1));
                if let Mode::DefPick { index, .. } = &mut self.mode {
                    *index = i;
                }
                Update { dirty: true, cmds: vec![] }
            }
            Down | Char('j') => {
                // Circular: j on the last item wraps to the first.
                let i = if index + 1 >= len { 0 } else { index + 1 };
                if let Mode::DefPick { index, .. } = &mut self.mode {
                    *index = i;
                }
                Update { dirty: true, cmds: vec![] }
            }
            Enter => self.def_pick_activate(index),
            _ => Update { dirty: false, cmds: vec![] },
        }
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
        self.open_def_args(def.repo, def.name, def.args, fixed, HashMap::new(), worktree);
        Update { dirty: true, cmds: vec![] }
    }

    /// Route a left-click while the def-pick popup is open: a `MenuItem` picks
    /// that row (same path as Enter); the `Modal` body is inert; anything else
    /// closes the popup.
    fn route_def_pick_click(&mut self, target: Option<HitTarget>) -> Update {
        match target {
            Some(HitTarget::MenuItem(i)) => self.def_pick_activate(i),
            Some(HitTarget::Modal) => Update { dirty: false, cmds: vec![] },
            _ => {
                self.mode = Mode::List;
                Update { dirty: true, cmds: vec![] }
            }
        }
    }

    /// `Mode::DefArgs` key handling. Dropdown-open: ↑/↓ move, Enter picks, Esc
    /// closes the dropdown only. Dropdown-closed: Tab/↓ next, Shift-Tab/↑ prev,
    /// ←/→ cycle enum, Enter opens an enum dropdown or validates+submits, Esc
    /// cancels, printable/Backspace edit text.
    fn def_args_key(&mut self, ev: &crossterm::event::KeyEvent) -> Update {
        use crossterm::event::{KeyCode::*, KeyModifiers};
        let dropdown_open = matches!(&self.mode, Mode::DefArgs { form } if form.dropdown.is_some());
        let shift = ev.modifiers.contains(KeyModifiers::SHIFT);
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
        match ev.code {
            Esc => { self.mode = Mode::List; Update { dirty: true, cmds: vec![] } }
            Tab if !shift => { form.next_focus(); Update { dirty: true, cmds: vec![] } }
            Down => { form.next_focus(); Update { dirty: true, cmds: vec![] } }
            BackTab => { form.prev_focus(); Update { dirty: true, cmds: vec![] } }
            Tab if shift => { form.prev_focus(); Update { dirty: true, cmds: vec![] } }
            Up => { form.prev_focus(); Update { dirty: true, cmds: vec![] } }
            Left => { let i = form.focus; form.cycle_option(i, -1); Update { dirty: true, cmds: vec![] } }
            Right => { let i = form.focus; form.cycle_option(i, 1); Update { dirty: true, cmds: vec![] } }
            Enter => {
                let i = form.focus;
                if form.is_enum(i) && !form.is_fixed(i) {
                    form.open_dropdown(i);
                    Update { dirty: true, cmds: vec![] }
                } else {
                    self.submit_def_args()
                }
            }
            Backspace => { form.backspace(); Update { dirty: true, cmds: vec![] } }
            Char(c) if !ev.modifiers.contains(KeyModifiers::CONTROL) && !ev.modifiers.contains(KeyModifiers::ALT) => {
                form.input_char(c);
                Update { dirty: true, cmds: vec![] }
            }
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
    fn def_args_click(&mut self, target: &HitTarget) -> Update {
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
                    form.focus = *i;
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
            HitTarget::Modal => Update { dirty: false, cmds: vec![] }, // body click: inert
            // Any other target is behind the popup (a pane row/body/tab): a click
            // outside the form dismisses it, same as esc (mirrors def-pick/menu).
            _ => {
                self.mode = Mode::List;
                Update { dirty: true, cmds: vec![] }
            }
        }
    }

    /// Route a left-click while an action menu is open: a `MenuItem` selects and
    /// (if enabled) executes that row; the `Modal` body is inert; a click on
    /// anything else (or nothing) closes the menu.
    fn route_menu_click(&mut self, target: Option<HitTarget>) -> Update {
        match target {
            Some(HitTarget::MenuItem(i)) => {
                let chosen = if let Mode::ActionMenu { items, index, .. } = &mut self.mode {
                    *index = i;
                    items.get(i).cloned()
                } else {
                    None
                };
                match chosen {
                    Some(it) if it.disabled.is_none() => self.execute_menu_action(it.action),
                    _ => Update { dirty: true, cmds: vec![] }, // disabled row: highlight only
                }
            }
            Some(HitTarget::Modal) => Update { dirty: false, cmds: vec![] }, // body click: inert
            _ => {
                self.mode = Mode::List; // click outside the popup closes it
                Update { dirty: true, cmds: vec![] }
            }
        }
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

    /// Staged esc: close overlay → clear range → clear filter → noop.
    fn clear_esc(&mut self) -> bool {
        if !matches!(self.mode, Mode::List) {
            self.mode = Mode::List;
            return true;
        }
        let Some(pane) = self.focused_list() else { return false };
        let sel = self.ui().selections[pane as usize];
        if sel.anchor.is_some() {
            self.ui().selections[pane as usize] = Selection { cursor: sel.cursor, anchor: None };
            return true;
        }
        if !self.ui().search[pane as usize].is_empty() {
            self.ui().search[pane as usize].clear();
            self.ui().selections[pane as usize] = Selection { cursor: 0, anchor: None };
            return true;
        }
        false
    }

    /// Current detail kind + sub-tab for the active tab (needed for scroll inversion).
    fn detail_kind_and_subtab(&self) -> (DetailKind, usize) {
        let c = crate::view::compute(self);
        let ctx = match (&self.snapshot, &c.active_name) {
            (Some(snap), Some(name)) => crate::detail::derive_context(
                snap,
                name,
                c.ui.last_list_pane,
                &c.queue,
                &c.worktrees,
                &c.defs,
                &c.ui.selections,
            ),
            _ => crate::detail::DetailContext::Empty,
        };
        let kind = ctx.kind();
        let sub = crate::detail::clamp_sub_tab(c.ui.sub_tab[kind as usize], kind);
        (kind, sub)
    }

    /// `Scroll(delta)` in the detail pane. Bottom-anchored views invert so k =
    /// older. Applies to the active tab's `scroll_offset`.
    pub(crate) fn detail_scroll(&mut self, delta: i32) -> bool {
        let (kind, sub) = self.detail_kind_and_subtab();
        let step = if crate::detail::bottom_anchored(kind, sub) { -delta } else { delta };
        // Clamp BOTH ends: 0 and the render-fed max. Without the upper clamp the
        // stored offset kept growing on over-scroll past the edge, and the user
        // had to scroll back through the phantom distance before the view moved.
        let max = self.detail_max_scroll.get();
        let ui = self.ui();
        let next = ((ui.scroll_offset as i64 + step as i64).max(0) as usize).min(max);
        if next == ui.scroll_offset {
            return false;
        }
        ui.scroll_offset = next;
        true
    }

    /// `ScrollEdge(dir)` in the detail pane. dir < 0 = head/oldest, dir > 0 =
    /// tail/end. Jumps to the render-fed max (not an unclamped sentinel, which
    /// left the stored offset far past the edge — same phantom-scroll bug class
    /// as `detail_scroll`'s missing upper clamp).
    pub(crate) fn detail_scroll_edge(&mut self, dir: i32) -> bool {
        let (kind, sub) = self.detail_kind_and_subtab();
        let bottom = crate::detail::bottom_anchored(kind, sub);
        let to_head = dir < 0;
        let max = self.detail_max_scroll.get();
        // Bottom-anchored: head = max offset, tail = 0. Top-anchored: reverse.
        let offset = if bottom {
            if to_head { max } else { 0 }
        } else if to_head {
            0
        } else {
            max
        };
        let ui = self.ui();
        if ui.scroll_offset == offset {
            return false;
        }
        ui.scroll_offset = offset;
        true
    }

    /// Reset the detail scroll to its anchor. Called on selection / sub-tab /
    /// focus change so a new selection always starts at its default view.
    pub(crate) fn reset_scroll(&mut self) {
        self.ui().scroll_offset = 0;
    }

    /// Detail content height ≈ terminal rows − 6 (header + footer + borders +
    /// chip row), floored at 1. Drives the tail read size, not layout.
    pub(crate) fn detail_height(&self) -> usize {
        (self.size.1 as usize).saturating_sub(6).max(1)
    }

    /// Tail lines to read: 4 windows of scrollback behind the visible region.
    pub(crate) fn tail_lines(&self) -> usize {
        (self.detail_height() * 4).max(1)
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

    /// The pane a hit target belongs to (for wheel/drag routing). Row/PaneBody/
    /// Scrollbar* all map to their owning pane; everything else is `None`.
    fn pane_of_target(t: &HitTarget) -> Option<PaneId> {
        match t {
            HitTarget::Row(pane, _) => Some(match pane {
                ListPane::Queue => PaneId::Queue,
                ListPane::Tasks => PaneId::Tasks,
                ListPane::Worktrees => PaneId::Worktrees,
            }),
            HitTarget::PaneBody(p)
            | HitTarget::ScrollbarThumb(p)
            | HitTarget::ScrollbarTrack(p) => Some(*p),
            _ => None,
        }
    }

    /// Route a mouse event through the previous frame's hit map. Clicks focus/
    /// select/switch, the wheel scrolls the pane under the cursor without
    /// stealing focus, and scrollbar drags map proportionally.
    fn on_mouse(&mut self, m: crossterm::event::MouseEvent) -> Update {
        use crossterm::event::{
            KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind as K,
        };
        // Text-input modals own the mouse: only a left-click routes (Confirm ≡
        // Enter, Cancel ≡ Esc, outside ≡ cancel); every other mouse kind is inert
        // so a move/drag never disturbs the field or closes the popup. Handling
        // mouse here (before the typing arms) is what keeps clicks out of the
        // `tui_input` field entirely.
        if matches!(
            self.mode,
            Mode::AddTask { .. } | Mode::WorktreeInput { .. } | Mode::CreateWorktree { .. }
        ) {
            if let K::Down(MouseButton::Left) = m.kind {
                match self.hit.hit(m.column, m.row).cloned() {
                    // The create-worktree modal registers no buttons; only Modal
                    // (inert) and outside (cancel) apply.
                    Some(HitTarget::Button(crate::hit::ButtonKind::Confirm)) => {
                        return self
                            .update(Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
                    }
                    Some(HitTarget::Button(crate::hit::ButtonKind::Cancel)) => {
                        return self
                            .update(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
                    }
                    Some(HitTarget::Modal) => return Update { dirty: false, cmds: vec![] },
                    _ => {
                        // Click outside the popup cancels (same as esc).
                        self.mode = Mode::List;
                        return Update { dirty: true, cmds: vec![] };
                    }
                }
            }
            return Update { dirty: false, cmds: vec![] };
        }
        let mut cmds = Vec::new();
        let target = self.hit.hit(m.column, m.row).cloned();
        let dirty = match m.kind {
            // An open action menu owns every click: route it via the menu's hit
            // regions (checked before the list-pane routing below).
            K::Down(MouseButton::Left) if matches!(self.mode, Mode::ActionMenu { .. }) => {
                return self.route_menu_click(target);
            }
            // The def-pick popup owns every click while open.
            K::Down(MouseButton::Left) if matches!(self.mode, Mode::DefPick { .. }) => {
                return self.route_def_pick_click(target);
            }
            // The args form owns every click while open: route to its hit
            // targets; a click hitting nothing (outside the popup) cancels.
            K::Down(MouseButton::Left) if matches!(self.mode, Mode::DefArgs { .. }) => {
                return match target {
                    Some(t) => self.def_args_click(&t),
                    None => {
                        self.mode = Mode::List;
                        Update { dirty: true, cmds: vec![] }
                    }
                };
            }
            // Confirm/Help overlays own every click too: a click inside the
            // modal is inert; anything else (including outside → None) dismisses,
            // same as esc — Confirm cancels with no dispatch, Help closes.
            K::Down(MouseButton::Left)
                if matches!(
                    self.mode,
                    Mode::ConfirmRemove { .. } | Mode::ConfirmBulkRemove { .. } | Mode::Help
                ) =>
            {
                return match target {
                    Some(HitTarget::Modal) => Update { dirty: false, cmds: vec![] },
                    _ => {
                        self.mode = Mode::List;
                        Update { dirty: true, cmds: vec![] }
                    }
                };
            }
            K::Down(MouseButton::Left) => match target {
                Some(HitTarget::Modal) => false, // click inside an overlay: inert (menus/forms wire up in M2/M3)
                None => {
                    // Click hitting nothing while an overlay is open closes it
                    // (same staged semantics as esc).
                    if !matches!(self.mode, Mode::List) {
                        self.mode = Mode::List;
                        true
                    } else {
                        false
                    }
                }
                Some(HitTarget::Tab(i)) => {
                    return self.apply_action(crate::keymap::AppAction::SwitchTab(i));
                }
                Some(HitTarget::SubTab(i)) => self.set_sub_tab_clamped(i, &mut cmds),
                Some(HitTarget::PaneBody(p)) => {
                    // Detail is display-only: clicking its body must not steal
                    // focus (wheel scrolling over it still works — that routes by
                    // hover, not focus).
                    if p == PaneId::Detail {
                        false
                    } else {
                        self.set_focus(p);
                        true
                    }
                }
                Some(HitTarget::Row(pane, i)) => {
                    let focus = match pane {
                        ListPane::Queue => PaneId::Queue,
                        ListPane::Tasks => PaneId::Tasks,
                        ListPane::Worktrees => PaneId::Worktrees,
                    };
                    let shift = m.modifiers.contains(crossterm::event::KeyModifiers::SHIFT);
                    self.set_focus(focus);
                    if shift {
                        // Extend: keep (or seed) the anchor, move the cursor to i.
                        // A shift-click is not part of a double-click sequence.
                        self.last_click = None;
                        let sel = self.ui().selections[pane as usize];
                        let anchor = Some(sel.anchor.unwrap_or(sel.cursor));
                        let len = self.visible_len(pane);
                        let cursor = if len == 0 { 0 } else { i.min(len - 1) };
                        self.ui().selections[pane as usize] = Selection {
                            cursor,
                            anchor: if anchor == Some(cursor) { None } else { anchor },
                        };
                        self.ui().scroll_offset = 0;
                        self.schedule_run_read(&mut cmds, 120);
                        true
                    } else {
                        // Single click selects only. A real double-click — a second
                        // click on the SAME ROW IDENTITY within DOUBLE_CLICK_MS —
                        // opens the action menu (same target as Enter/a). Keying on
                        // identity (resolved from the clicked index) not the index
                        // means a resort between clicks can't fire the menu on a
                        // row that merely slid into the clicked slot.
                        let now = self.now_ms;
                        let identity = self.row_identity(pane, i);
                        let double = match (&self.last_click, &identity) {
                            (Some((lp, lid, lt)), Some(id)) => {
                                *lp == pane
                                    && lid == id
                                    && now.saturating_sub(*lt) < DOUBLE_CLICK_MS
                            }
                            _ => false,
                        };
                        self.set_cursor(pane, i, &mut cmds);
                        if double {
                            // Consume the sequence so a third click starts fresh.
                            self.last_click = None;
                            match self.open_action_menu() {
                                Some(mode) => self.mode = mode,
                                None => self.status_line = Some("nothing selected".into()),
                            }
                        } else {
                            // Arm on the clicked row's identity (None → nothing to
                            // match against next click; disarms the sequence).
                            self.last_click = identity.map(|id| (pane, id, now));
                        }
                        true
                    }
                }
                Some(HitTarget::ScrollbarThumb(p)) | Some(HitTarget::ScrollbarTrack(p)) => {
                    self.drag = Some(DragKind::Scrollbar(p));
                    self.drag_to_offset(p, m.row, &mut cmds)
                }
                Some(HitTarget::PaneDividerH(i)) => {
                    self.drag = Some(DragKind::DividerH(i));
                    self.drag_divider_h(i, m.row)
                }
                Some(HitTarget::PaneDividerV) => {
                    self.drag = Some(DragKind::DividerV);
                    self.drag_divider_v(m.column)
                }
                Some(HitTarget::PaneButton(p, btn)) => {
                    // A title-bar button behaves exactly like pressing its hotkey
                    // with that pane focused. `Create`/`Actions` need the focus
                    // (they read `last_list_pane`); `Collapse` is focus-independent
                    // (its outcome — collapsing pane P — matches pressing `x` with
                    // P focused regardless), so it skips the focus/scroll reset.
                    let lp = match p {
                        PaneId::Queue => ListPane::Queue,
                        PaneId::Tasks => ListPane::Tasks,
                        PaneId::Worktrees => ListPane::Worktrees,
                        PaneId::Detail => return Update { dirty: false, cmds }, // no detail buttons
                    };
                    match btn {
                        crate::hit::PaneButton::Collapse => {
                            self.toggle_collapse(lp, &mut cmds);
                            true
                        }
                        crate::hit::PaneButton::Create => {
                            self.set_focus(p);
                            return self.apply_action(crate::keymap::AppAction::Create);
                        }
                        crate::hit::PaneButton::Actions => {
                            self.set_focus(p);
                            return self.apply_action(crate::keymap::AppAction::OpenActionMenu);
                        }
                    }
                }
                Some(_) => false, // MenuItem/FormField/DropdownItem/Button: M2/M3
            },
            K::Drag(MouseButton::Left) => match self.drag {
                Some(DragKind::Scrollbar(p)) => self.drag_to_offset(p, m.row, &mut cmds),
                Some(DragKind::DividerH(i)) => self.drag_divider_h(i, m.row),
                Some(DragKind::DividerV) => self.drag_divider_v(m.column),
                None => false,
            },
            K::Up(MouseButton::Left) => {
                // Drag ends. A divider drag changed the per-project layout → write
                // it through to disk (once, on release — not on every drag frame).
                // Scrollbar drags change no layout, so they don't persist.
                let ended = self.drag.take();
                if matches!(ended, Some(DragKind::DividerH(_)) | Some(DragKind::DividerV)) {
                    cmds.push(self.save_layout_cmd());
                }
                false
            }
            K::ScrollDown | K::ScrollUp => {
                let delta: i32 = if matches!(m.kind, K::ScrollDown) { 1 } else { -1 };
                match target.as_ref().and_then(Self::pane_of_target) {
                    Some(PaneId::Detail) => self.detail_scroll(delta),
                    Some(p) => {
                        // Wheel scrolls the pane UNDER the cursor without focus change.
                        let pane = match p {
                            PaneId::Queue => ListPane::Queue,
                            PaneId::Tasks => ListPane::Tasks,
                            PaneId::Worktrees => ListPane::Worktrees,
                            PaneId::Detail => unreachable!(),
                        };
                        let cur = self.ui().selections[pane as usize].cursor as i64;
                        let next = (cur + delta as i64).max(0) as usize;
                        self.set_cursor(pane, next, &mut cmds)
                    }
                    None => false,
                }
            }
            _ => false,
        };
        Update { dirty, cmds }
    }

    /// Proportional drag: offset = (y − track_top) × scrollable ÷ track_h,
    /// clamped. Lists map to the cursor; detail maps to the window start and
    /// converts per anchor.
    fn drag_to_offset(&mut self, pane: PaneId, y: u16, cmds: &mut Vec<Cmd>) -> bool {
        let track = self
            .hit
            .iter()
            .find(|(_, t)| matches!(t, HitTarget::ScrollbarTrack(p) if *p == pane))
            .map(|(r, _)| *r);
        let Some(track) = track else { return false };
        let track_h = track.height.max(1) as usize;
        let rel = (y.max(track.y) - track.y) as usize;
        match pane {
            PaneId::Detail => {
                let total = crate::view::detail::detail_content_len(self);
                let height = self.detail_height();
                let scrollable = total.saturating_sub(height);
                let start = (rel * scrollable / track_h).min(scrollable);
                let (kind, sub) = self.detail_kind_and_subtab();
                let offset = if crate::detail::bottom_anchored(kind, sub) {
                    scrollable - start
                } else {
                    start
                };
                if self.ui().scroll_offset == offset {
                    return false;
                }
                self.ui().scroll_offset = offset;
                true
            }
            PaneId::Queue | PaneId::Tasks | PaneId::Worktrees => {
                let list = match pane {
                    PaneId::Queue => ListPane::Queue,
                    PaneId::Tasks => ListPane::Tasks,
                    _ => ListPane::Worktrees,
                };
                let len = self.visible_len(list);
                let scrollable = len.saturating_sub(1);
                let cursor = (rel * scrollable / track_h).min(scrollable);
                self.set_cursor(list, cursor, cmds)
            }
        }
    }

    /// The left-pane body rectangle geometry the dividers move within. The header
    /// is a single row, the footer a single row (`view::render`), so the body
    /// starts at row 1 and the left column starts at column 0.
    fn body_height(&self) -> u16 {
        self.size.1.saturating_sub(2)
    }

    /// Drag a horizontal pane divider to absolute mouse row `y`. `which` selects
    /// the boundary (0 = queue/tasks, 1 = tasks/worktrees). The requested height is
    /// the rows between the body top and the drop point; `pane_layout` re-clamps it
    /// so every pane keeps its minimum. Overrides are canonicalized to the realized
    /// (clamped) heights so a drag past the limit can't accumulate stale slack.
    fn drag_divider_h(&mut self, which: usize, y: u16) -> bool {
        // A boundary adjacent to a collapsed pane can't move — the collapsed pane
        // is pinned to COLLAPSED_H. Ignore the drag rather than fight the clamp.
        // `which` 0 = queue/tasks (panes 0,1); 1 = tasks/worktrees (panes 1,2).
        if self.collapsed[which] || self.collapsed[which + 1] {
            return false;
        }
        const BODY_TOP: u16 = 1; // header occupies row 0
        let body_h = self.body_height();
        let rel = y.saturating_sub(BODY_TOP);
        let before = crate::selectors::pane_layout(
            body_h,
            self.queue_h_override,
            self.tasks_h_override,
            self.collapsed,
        );
        let (mut q_ov, mut t_ov) = (self.queue_h_override, self.tasks_h_override);
        match which {
            // queue/tasks boundary → queue height = rows above the boundary.
            0 => q_ov = Some(rel),
            // tasks/worktrees boundary → tasks height = rows between the two
            // boundaries (drop point minus the current queue height).
            _ => t_ov = Some(rel.saturating_sub(before.queue_h)),
        }
        let after = crate::selectors::pane_layout(body_h, q_ov, t_ov, self.collapsed);
        self.queue_h_override = Some(after.queue_h);
        self.tasks_h_override = Some(after.tasks_h);
        after != before
    }

    /// Drag the vertical divider to absolute mouse column `x`: the drop column
    /// becomes the first column of DETAIL, i.e. the left-column width. Clamped so
    /// neither side collapses.
    fn drag_divider_v(&mut self, x: u16) -> bool {
        let clamped = crate::selectors::clamp_left_cols(self.size.0, x);
        if self.left_cols == Some(clamped) {
            return false;
        }
        self.left_cols = Some(clamped);
        true
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
mod tests {
    use super::*;
    use crate::ipc::types::{Project, TaskInstance, TaskTarget};
    use crossterm::event::{KeyEvent, KeyModifiers};

    fn app() -> App {
        App::new(PathBuf::from("/runs"), PathBuf::from("/sock"))
    }

    fn running_task(repo: &str) -> TaskInstance {
        TaskInstance {
            id: "t1".into(),
            status: TaskStatus::Running,
            target: TaskTarget {
                repo: repo.into(),
                git_ref: "temp".into(),
                worktree: Some("wt-a".into()),
            },
            ..Default::default()
        }
    }

    fn snapshot_with(projects: &[&str], tasks: Vec<TaskInstance>) -> StateSnapshot {
        StateSnapshot {
            projects: projects.iter().map(|n| Project { name: n.to_string() }).collect(),
            tasks,
            ..Default::default()
        }
    }

    #[test]
    fn snapshot_event_commits_state_and_dirties() {
        let mut app = app();
        let u = app.update(Event::Snapshot(snapshot_with(&["platform"], vec![])));
        assert!(u.dirty);
        assert!(app.connected);
        assert_eq!(app.snapshot.as_ref().unwrap().projects.len(), 1);
    }

    #[test]
    fn disconnected_dirties_only_on_transition_and_keeps_snapshot() {
        let mut app = app();
        app.update(Event::Snapshot(snapshot_with(&["platform"], vec![])));
        let u = app.update(Event::Disconnected);
        assert!(u.dirty);
        assert!(!app.connected);
        assert!(app.snapshot.is_some()); // last snapshot stays rendered
        // The 2s retry loop re-sends Disconnected while the daemon is down —
        // repeats must not repaint (zero idle renders).
        let again = app.update(Event::Disconnected);
        assert!(!again.dirty);
    }

    #[test]
    fn resize_dirties() {
        let mut app = app();
        assert!(app.update(Event::Resize).dirty);
    }

    #[test]
    fn q_in_list_mode_quits() {
        let mut app = app();
        let key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        let u = app.update(Event::Key(key));
        assert_eq!(u.cmds, vec![Cmd::Quit]);
    }

    #[test]
    fn tick_advances_clock_and_dirties() {
        let mut app = app();
        // Tick repaints (elapsed labels) only while the active project has a
        // running task — otherwise it is a zero-render no-op (see idle test).
        app.update(Event::Snapshot(snapshot_with(&["platform"], vec![running_task("platform")])));
        app.active_tab = 0;
        app.now_epoch_s = 0;
        let u = app.update(Event::Tick);
        assert!(u.dirty);
        assert!(app.now_epoch_s > 0);
    }

    #[test]
    fn wants_tick_requires_running_task_in_active_project() {
        let mut app = app();
        assert!(!app.wants_tick()); // no snapshot yet
        app.update(Event::Snapshot(snapshot_with(
            &["platform", "web"],
            vec![running_task("web")],
        )));
        app.active_tab = 0; // platform — the running task is on web
        assert!(!app.wants_tick());
        app.active_tab = 1; // web
        assert!(app.wants_tick());
    }

    #[test]
    fn detail_scroll_inverts_on_bottom_anchored_transcript() {
        // fixture_app: active detail context is the running task's transcript,
        // which is bottom-anchored (k = older, so a negative delta grows offset).
        let mut app = crate::test_fixtures::fixture_app();
        app.detail_max_scroll.set(10); // as if the last render had 10 lines of slack
        assert!(!app.detail_scroll(1)); // toward newest — already at tail, no-op
        assert!(app.detail_scroll(-1)); // toward older — offset grows
        assert_eq!(app.ui().scroll_offset, 1);
    }

    #[test]
    fn detail_scroll_edge_jumps_head_and_tail() {
        let mut app = crate::test_fixtures::fixture_app();
        app.detail_max_scroll.set(42);
        assert!(app.detail_scroll_edge(-1)); // head/oldest → the render-fed max
        assert_eq!(app.ui().scroll_offset, 42);
        assert!(app.detail_scroll_edge(1)); // tail → 0
        assert_eq!(app.ui().scroll_offset, 0);
        assert!(!app.detail_scroll_edge(1)); // already at tail
    }

    #[test]
    fn detail_scroll_clamps_at_max_so_overscroll_banks_no_phantom_distance() {
        // Regression: over-scrolling past the head kept growing the stored
        // offset, so scrolling back required burning through phantom distance.
        let mut app = crate::test_fixtures::fixture_app();
        app.detail_max_scroll.set(3);
        for _ in 0..10 {
            app.detail_scroll(-1); // way past the head
        }
        assert_eq!(app.ui().scroll_offset, 3, "stored offset stops at the content max");
        // The very next scroll toward the tail must move the view immediately.
        assert!(app.detail_scroll(1));
        assert_eq!(app.ui().scroll_offset, 2);
    }

    #[test]
    fn reset_scroll_returns_to_anchor() {
        let mut app = crate::test_fixtures::fixture_app();
        app.detail_max_scroll.set(10);
        app.detail_scroll(-5);
        assert_eq!(app.ui().scroll_offset, 5);
        app.reset_scroll();
        assert_eq!(app.ui().scroll_offset, 0);
    }

    #[test]
    fn detail_scroll_does_not_invert_on_top_anchored() {
        let mut app = crate::test_fixtures::fixture_app();
        app.defs_by_project.insert(
            "acme".into(),
            vec![DefinitionSummary { repo: "acme".into(), name: "pr-ready".into(), ..Default::default() }],
        );
        let mut ui = TabUiState::default();
        ui.last_list_pane = ListPane::Tasks;
        app.ui_by_tab.insert("acme".into(), ui);
        // Definition context is head-anchored: positive delta grows offset directly.
        app.detail_max_scroll.set(10);
        assert!(app.detail_scroll(1));
        assert_eq!(app.ui().scroll_offset, 1);
        assert!(app.detail_scroll(-1));
        assert_eq!(app.ui().scroll_offset, 0);
    }

    #[test]
    fn wants_tick_false_with_only_queued_tasks() {
        let mut app = app();
        let mut task = running_task("platform");
        task.status = TaskStatus::Queued;
        app.update(Event::Snapshot(snapshot_with(&["platform"], vec![task])));
        app.active_tab = 0;
        assert!(!app.wants_tick());
    }

    // -- Task 10: run-file wiring --------------------------------------------
    use crate::runfiles::RunFiles;

    fn run_files_fixture() -> RunFiles {
        RunFiles {
            transcript_tail: (0..5).map(|i| format!("line {i}")).collect(),
            report: vec!["# ok".to_string()],
        }
    }

    #[test]
    fn snapshot_event_schedules_debounced_read_for_selected_run() {
        let mut app = crate::test_fixtures::fixture_app();
        // last_list_pane defaults to Queue, cursor 0 → task 01RUN (a Run context).
        let up = app.update(Event::Snapshot(crate::test_fixtures::fixture_snapshot()));
        assert!(up.cmds.iter().any(|c| matches!(
            c,
            Cmd::ReadRunFiles { task_id, delay_ms: 120, .. } if task_id == "01RUN"
        )));
    }

    #[test]
    fn stale_run_files_event_is_discarded() {
        let mut app = crate::test_fixtures::fixture_app();
        let up = app.update(Event::RunFiles {
            task_id: "01SOMEONE_ELSE".into(),
            files: run_files_fixture(),
        });
        assert!(!up.dirty);
        assert!(app.run_files.is_none());
    }

    #[test]
    fn identical_run_files_do_not_dirty_but_still_repoll() {
        let mut app = crate::test_fixtures::fixture_app();
        app.run_files = Some(("01RUN".to_string(), run_files_fixture()));
        let up = app.update(Event::RunFiles {
            task_id: "01RUN".into(),
            files: run_files_fixture(),
        });
        assert!(!up.dirty, "content-identical read must not trigger a render");
        // 01RUN is running in the fixture → 1s follow-up poll is scheduled.
        assert!(up.cmds.iter().any(|c| matches!(
            c,
            Cmd::ReadRunFiles { task_id, delay_ms: 1000, .. } if task_id == "01RUN"
        )));
    }

    #[test]
    fn changed_run_files_dirty_and_commit() {
        let mut app = crate::test_fixtures::fixture_app();
        app.run_files = Some(("01RUN".to_string(), RunFiles::default()));
        let up = app.update(Event::RunFiles {
            task_id: "01RUN".into(),
            files: run_files_fixture(),
        });
        assert!(up.dirty);
        assert_eq!(app.run_files.as_ref().unwrap().1, run_files_fixture());
    }

    #[test]
    fn no_repoll_when_selected_task_not_running() {
        let mut app = crate::test_fixtures::fixture_app();
        // Point the queue cursor at 01QUE (index 1, a queued task).
        app.ui().selections[0].cursor = 1;
        let up = app.update(Event::RunFiles {
            task_id: "01QUE".into(),
            files: run_files_fixture(),
        });
        assert!(up.dirty);
        assert!(up.cmds.is_empty(), "non-running task must not start the 1s poll loop");
    }

    // -- Task 11: key dispatch through update() -------------------------------
    use crate::app::{ListPane, PaneId};

    fn press(app: &mut App, code: KeyCode) -> Update {
        app.update(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)))
    }

    #[test]
    fn cycle_pane_wraps_queue_tasks_worktrees() {
        let mut app = crate::test_fixtures::fixture_app();
        // Detail is display-only and never enters the focus cycle.
        let order = [PaneId::Tasks, PaneId::Worktrees, PaneId::Queue];
        for expected in order {
            press(&mut app, KeyCode::Tab);
            assert_eq!(app.ui().focus, expected);
        }
    }

    #[test]
    fn hl_keys_do_not_focus_detail() {
        let mut app = crate::test_fixtures::fixture_app();
        press(&mut app, KeyCode::Tab); // → tasks
        press(&mut app, KeyCode::Char('l')); // no-op: detail is display-only
        assert_eq!(app.ui().focus, PaneId::Tasks);
        press(&mut app, KeyCode::Char('h')); // no-op
        assert_eq!(app.ui().focus, PaneId::Tasks);
    }

    #[test]
    fn move_cursor_wraps_circularly_and_extend_stays_clamped() {
        let mut app = crate::test_fixtures::fixture_app();
        // 4 queue rows (3 live + 1 archived). Navigation is circular: 10 j
        // presses from row 0 land on 10 % 4 = row 2.
        for _ in 0..10 {
            press(&mut app, KeyCode::Char('j'));
        }
        assert_eq!(app.ui().selections[0].cursor, 2);
        press(&mut app, KeyCode::Char('j')); // → 3 (last)
        press(&mut app, KeyCode::Char('j')); // wraps → 0
        assert_eq!(app.ui().selections[0].cursor, 0);
        press(&mut app, KeyCode::Char('k')); // wraps back → 3
        assert_eq!(app.ui().selections[0].cursor, 3);
        // Extend-selection does NOT wrap (a wrapping range would be ambiguous).
        press(&mut app, KeyCode::Char('J')); // can't extend past end → anchor stays None
        assert_eq!(app.ui().selections[0].anchor, None);
        press(&mut app, KeyCode::Char('K'));
        assert_eq!(app.ui().selections[0], Selection { cursor: 2, anchor: Some(3) });
    }

    #[test]
    fn queue_nav_crosses_divider_onto_a_real_finished_row() {
        // Real render so app.hit carries the true divider geometry.
        let mut app = app_rendered(80, 24);
        // The ACTIVE/FINISHED divider is inert: exactly 4 queue rows are
        // clickable (2 active + 2 finished), and none of them is the divider.
        let queue_row_hits = app
            .hit
            .iter()
            .filter(|(_, t)| matches!(t, HitTarget::Row(ListPane::Queue, _)))
            .count();
        assert_eq!(queue_row_hits, 4, "the divider adds no Row hit target");
        // From the last ACTIVE row, j crosses the divider onto the first FINISHED
        // row (index 2) — the cursor never stalls on the divider line.
        press(&mut app, KeyCode::Char('j')); // 0 → 1 (last active)
        press(&mut app, KeyCode::Char('j')); // 1 → 2 (first finished, across divider)
        assert_eq!(app.ui().selections[0].cursor, 2);
        // Opening the menu targets that real finished task — the cursor index maps
        // 1:1 to a real row, so the divider never shifts the row lookup.
        press(&mut app, KeyCode::Char('a'));
        match &app.mode {
            Mode::ActionMenu { title, .. } => assert_eq!(title, "flaky migration"),
            other => panic!("expected ActionMenu on the failed task, got {other:?}"),
        }
    }

    #[test]
    fn queue_range_selection_spans_the_section_divider() {
        let mut app = app_rendered(80, 24);
        press(&mut app, KeyCode::Char('j')); // cursor 0 → 1 (last active row)
        press(&mut app, KeyCode::Char('J')); // extend into row 2 (first finished)
        assert_eq!(app.ui().selections[0], Selection { cursor: 2, anchor: Some(1) });
        // The range covers a real row on EACH side of the divider (active 1 +
        // finished 2) — selections/cursor operate purely in real-row space.
        assert_eq!(crate::view::selection_range(&app.ui().selections[0]), (1, 2));
    }

    #[test]
    fn g_and_shift_g_jump_list_cursor_to_edges() {
        let mut app = crate::test_fixtures::fixture_app();
        app.update(Event::Key(KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT)));
        assert_eq!(app.ui().selections[0].cursor, 3);
        press(&mut app, KeyCode::Char('g'));
        assert_eq!(app.ui().selections[0].cursor, 0);
    }

    #[test]
    fn staged_esc_range_then_filter_then_noop() {
        let mut app = crate::test_fixtures::fixture_app();
        // Build a range and a filter. "cache" matches 01RUN + 01QUE summaries
        // (the fixture has no "line" row, so a filter must keep ≥2 rows to extend).
        app.ui().search[0] = "cache".into();
        press(&mut app, KeyCode::Char('J')); // range 0..1
        assert!(app.ui().selections[0].anchor.is_some());
        press(&mut app, KeyCode::Esc); // 1: clears range
        assert_eq!(app.ui().selections[0].anchor, None);
        assert_eq!(app.ui().search[0], "cache");
        press(&mut app, KeyCode::Esc); // 2: clears filter
        assert_eq!(app.ui().search[0], "");
        let up = press(&mut app, KeyCode::Esc); // 3: noop
        assert!(!up.dirty);
    }

    #[test]
    fn search_mode_types_filters_and_encloses() {
        let mut app = crate::test_fixtures::fixture_app();
        press(&mut app, KeyCode::Char('/'));
        assert!(matches!(app.mode, Mode::Search { pane: ListPane::Queue }));
        press(&mut app, KeyCode::Char('d'));
        press(&mut app, KeyCode::Char('o'));
        assert_eq!(app.ui().search[0], "do");
        // "do" (in "docs") matches only 01QUE's summary → 1 visible row, cursor reset.
        assert_eq!(app.visible_len(ListPane::Queue), 1);
        assert_eq!(app.ui().selections[0], Selection { cursor: 0, anchor: None });
        press(&mut app, KeyCode::Backspace);
        assert_eq!(app.ui().search[0], "d");
        press(&mut app, KeyCode::Enter); // apply: keep filter, back to list
        assert!(matches!(app.mode, Mode::List));
        assert_eq!(app.ui().search[0], "d");
        // esc inside search clears + closes.
        press(&mut app, KeyCode::Char('/'));
        press(&mut app, KeyCode::Char('x'));
        press(&mut app, KeyCode::Esc);
        assert!(matches!(app.mode, Mode::List));
        assert_eq!(app.ui().search[0], "");
    }

    #[test]
    fn search_typing_schedules_run_read() {
        // A search keystroke resets the effective selection to cursor 0 of the
        // filtered list, so it must schedule the debounced run-file read (every
        // other selection-changing path does).
        let mut app = crate::test_fixtures::fixture_app();
        press(&mut app, KeyCode::Char('/')); // search over the queue pane
        // 'c' (in "cache") keeps 01RUN at cursor 0 of the filtered queue.
        let up = press(&mut app, KeyCode::Char('c'));
        assert!(up.cmds.iter().any(|c| matches!(
            c,
            Cmd::ReadRunFiles { task_id, delay_ms: 120, .. } if task_id == "01RUN"
        )), "typing must schedule the 120ms run-file read");
        // Backspace re-scopes the selection → schedules again.
        let up = press(&mut app, KeyCode::Backspace);
        assert!(up.cmds.iter().any(|c| matches!(
            c,
            Cmd::ReadRunFiles { task_id, delay_ms: 120, .. } if task_id == "01RUN"
        )), "backspace must schedule the 120ms run-file read");
    }

    #[test]
    fn search_enter_apply_schedules_run_read() {
        let mut app = crate::test_fixtures::fixture_app();
        press(&mut app, KeyCode::Char('/'));
        press(&mut app, KeyCode::Char('c')); // filter, keeping 01RUN at cursor 0
        let up = press(&mut app, KeyCode::Enter); // apply
        assert!(matches!(app.mode, Mode::List));
        assert!(up.cmds.iter().any(|c| matches!(
            c,
            Cmd::ReadRunFiles { task_id, delay_ms: 120, .. } if task_id == "01RUN"
        )), "Enter-apply must schedule the 120ms run-file read");
    }

    #[test]
    fn search_esc_clear_schedules_run_read() {
        let mut app = crate::test_fixtures::fixture_app();
        press(&mut app, KeyCode::Char('/'));
        press(&mut app, KeyCode::Char('c')); // filter, keeping 01RUN at cursor 0
        let up = press(&mut app, KeyCode::Esc); // clear filter + close
        assert!(matches!(app.mode, Mode::List));
        assert_eq!(app.ui().search[0], "");
        assert!(up.cmds.iter().any(|c| matches!(
            c,
            Cmd::ReadRunFiles { task_id, delay_ms: 120, .. } if task_id == "01RUN"
        )), "Esc-clear must schedule the 120ms run-file read");
    }

    #[test]
    fn help_opens_and_any_key_closes() {
        let mut app = crate::test_fixtures::fixture_app();
        press(&mut app, KeyCode::Char('?'));
        assert!(matches!(app.mode, Mode::Help));
        let up = press(&mut app, KeyCode::Char('z'));
        assert!(matches!(app.mode, Mode::List));
        assert!(up.dirty);
    }

    #[test]
    fn status_line_clears_on_list_mode_keypress() {
        let mut app = crate::test_fixtures::fixture_app();
        app.status_line = Some("boom".into());
        let up = press(&mut app, KeyCode::Char('z')); // even an unbound key
        assert_eq!(app.status_line, None);
        assert!(up.dirty);
    }

    #[test]
    fn cycle_sub_tab_wraps_within_kind() {
        let mut app = crate::test_fixtures::fixture_app();
        // Run context (queue cursor 0 → 01RUN): 3 sub-tabs. ctrl+x = next (global,
        // no detail focus needed), ctrl+z = previous.
        let ctrl = |c| Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
        app.update(ctrl('x'));
        assert_eq!(app.ui().sub_tab[DetailKind::Run as usize], 1);
        app.update(ctrl('x'));
        app.update(ctrl('x'));
        assert_eq!(app.ui().sub_tab[DetailKind::Run as usize], 0, "wraps past the end");
        app.update(ctrl('z'));
        assert_eq!(app.ui().sub_tab[DetailKind::Run as usize], 2, "wraps below zero");
    }

    // -- Task 12: mouse routing ----------------------------------------------------
    use crate::hit::HitTarget;
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    use ratatui::layout::Rect;

    fn mouse(kind: MouseEventKind, col: u16, row: u16) -> Event {
        Event::Mouse(MouseEvent { kind, column: col, row, modifiers: KeyModifiers::NONE })
    }

    /// Fixture app with a synthetic hit map (no render needed): a tab, three queue
    /// rows at y=2..5, the queue body, and a queue scrollbar track x=30 y=2 h=10.
    fn app_with_hits() -> App {
        let mut app = crate::test_fixtures::fixture_app();
        let mut hits = crate::hit::HitMap::new();
        hits.push(Rect { x: 0, y: 0, width: 10, height: 1 }, HitTarget::Tab(0));
        hits.push(Rect { x: 1, y: 2, width: 28, height: 8 }, HitTarget::PaneBody(PaneId::Queue));
        for i in 0..4usize {
            hits.push(
                Rect { x: 1, y: 2 + i as u16, width: 28, height: 1 },
                HitTarget::Row(ListPane::Queue, i),
            );
        }
        hits.push(Rect { x: 30, y: 2, width: 1, height: 10 }, HitTarget::ScrollbarTrack(PaneId::Queue));
        app.hit = hits;
        app
    }

    #[test]
    fn click_row_focuses_and_selects_without_opening_menu() {
        let mut app = app_with_hits();
        app.set_focus(PaneId::Tasks); // start on another list pane
        let up = app.update(mouse(MouseEventKind::Down(MouseButton::Left), 5, 4)); // row 2
        assert!(up.dirty);
        assert_eq!(app.ui().focus, PaneId::Queue);
        assert_eq!(app.ui().selections[0], Selection { cursor: 2, anchor: None });
        assert!(matches!(app.mode, Mode::List), "single click selects only");
    }

    #[test]
    fn double_click_same_row_within_window_opens_menu() {
        let mut app = app_with_hits();
        app.now_ms = 1_000;
        app.update(mouse(MouseEventKind::Down(MouseButton::Left), 5, 3)); // click row 1
        app.status_line = None;
        app.now_ms = 1_200; // 200ms later (< 400ms) → double-click
        app.update(mouse(MouseEventKind::Down(MouseButton::Left), 5, 3));
        match &app.mode {
            // Row 1 is the queued fixture task "write docs for the cache".
            Mode::ActionMenu { title, items, index } => {
                assert_eq!(title, "write docs for the cache");
                assert_eq!(items.len(), 3);
                assert_eq!(*index, 0);
            }
            other => panic!("expected ActionMenu, got {other:?}"),
        }
    }

    #[test]
    fn slow_second_click_only_reselects() {
        let mut app = app_with_hits();
        app.now_ms = 1_000;
        app.update(mouse(MouseEventKind::Down(MouseButton::Left), 5, 3)); // click row 1
        app.now_ms = 1_500; // 500ms later (> 400ms) → NOT a double-click
        app.update(mouse(MouseEventKind::Down(MouseButton::Left), 5, 3));
        assert!(matches!(app.mode, Mode::List), "slow second click must not open the menu");
        assert_eq!(app.ui().selections[0], Selection { cursor: 1, anchor: None });
    }

    #[test]
    fn resort_between_clicks_keys_on_identity_not_index() {
        // Click row 0 (arms on the row's task id), then a new snapshot resorts a
        // DIFFERENT task into index 0. A second click at index 0 within the window
        // must NOT open the menu — the identity changed — it only re-selects.
        let mut app = app_with_hits();
        app.now_ms = 1_000;
        app.update(mouse(MouseEventKind::Down(MouseButton::Left), 5, 2)); // click row 0
        // Snapshot with a single running task whose id differs from the fixture's
        // row-0 task → index 0 now resolves to a different identity.
        app.update(Event::Snapshot(snapshot_with(&["acme"], vec![running_task("acme")])));
        app.now_ms = 1_200; // within 400ms
        app.update(mouse(MouseEventKind::Down(MouseButton::Left), 5, 2)); // click index 0 again
        assert!(
            matches!(app.mode, Mode::List),
            "a resort into the clicked slot must not fire the menu on the wrong row"
        );
        assert_eq!(app.ui().selections[0], Selection { cursor: 0, anchor: None });
    }

    fn ctrl_s(app: &mut App) -> Update {
        app.update(Event::Key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL)))
    }

    #[test]
    fn ctrl_s_prefix_then_n_p_cycles_project_tabs() {
        let mut app = app();
        app.update(Event::Snapshot(snapshot_with(&["a", "b", "c"], vec![])));
        app.active_tab = 0;
        assert!(ctrl_s(&mut app).dirty);
        assert!(app.prefix_armed, "ctrl+s arms the prefix");
        press(&mut app, KeyCode::Char('n')); // next tab, disarms
        assert!(!app.prefix_armed);
        assert_eq!(app.active_tab, 1);
        ctrl_s(&mut app);
        press(&mut app, KeyCode::Char('p')); // previous tab (wraps)
        assert_eq!(app.active_tab, 0);
    }

    #[test]
    fn ctrl_s_prefix_swallows_other_keys_and_disarms() {
        let mut app = app();
        app.update(Event::Snapshot(snapshot_with(&["a", "b"], vec![])));
        app.active_tab = 0;
        ctrl_s(&mut app);
        let before = app.collapsed;
        press(&mut app, KeyCode::Char('z')); // would collapse — swallowed by the prefix
        assert!(!app.prefix_armed, "any other key disarms");
        assert_eq!(app.active_tab, 0, "tab unchanged");
        assert_eq!(app.collapsed, before, "swallowed key had no effect");
    }

    #[test]
    fn pane_button_create_click_focuses_pane_then_acts() {
        let mut app = app_with_hits();
        app.set_focus(PaneId::Queue);
        let mut hits = app.hit.clone();
        hits.push(
            Rect { x: 20, y: 0, width: 4, height: 1 },
            HitTarget::PaneButton(PaneId::Worktrees, crate::hit::PaneButton::Create),
        );
        app.hit = hits;
        app.update(mouse(MouseEventKind::Down(MouseButton::Left), 21, 0));
        // Focus moved to worktrees first, then `c` opened the create-worktree modal.
        assert_eq!(app.active_ui().last_list_pane, ListPane::Worktrees);
        assert!(matches!(app.mode, Mode::CreateWorktree { .. }));
    }

    #[test]
    fn pane_button_collapse_click_toggles_without_moving_focus() {
        let mut app = app_with_hits();
        app.set_focus(PaneId::Queue);
        let before = app.collapsed[ListPane::Tasks.idx()];
        let mut hits = app.hit.clone();
        hits.push(
            Rect { x: 20, y: 0, width: 4, height: 1 },
            HitTarget::PaneButton(PaneId::Tasks, crate::hit::PaneButton::Collapse),
        );
        app.hit = hits;
        app.update(mouse(MouseEventKind::Down(MouseButton::Left), 21, 0));
        assert_ne!(app.collapsed[ListPane::Tasks.idx()], before, "collapse toggled");
        assert_eq!(app.ui().focus, PaneId::Queue, "collapse button leaves focus put");
    }

    /// The right-aligned `PaneButton` rect for `(pane, btn)` from a real render.
    fn pane_button_rect(app: &App, pane: PaneId, btn: crate::hit::PaneButton) -> Rect {
        app.hit
            .iter()
            .find_map(|(r, t)| (*t == HitTarget::PaneButton(pane, btn)).then_some(*r))
            .unwrap_or_else(|| panic!("pane button {pane:?}/{btn:?} registered"))
    }

    #[test]
    fn real_render_worktrees_create_chip_click_opens_modal() {
        // Worktrees' top border is the lower row of divider band 1; the chip must
        // still win the click (PaneButton registered after PaneDividerH).
        let mut app = app_rendered(80, 24);
        app.set_focus(PaneId::Queue);
        let r = pane_button_rect(&app, PaneId::Worktrees, crate::hit::PaneButton::Create);
        app.update(mouse(
            MouseEventKind::Down(MouseButton::Left),
            r.x + r.width / 2,
            r.y,
        ));
        assert_eq!(app.active_ui().last_list_pane, ListPane::Worktrees, "focus moved");
        assert!(matches!(app.mode, Mode::CreateWorktree { .. }), "create modal opened");
    }

    #[test]
    fn real_render_tasks_collapse_chip_click_toggles_over_divider() {
        // Tasks' top border is the lower row of divider band 0; the collapse chip
        // must win over the divider and leave focus unchanged.
        let mut app = app_rendered(80, 24);
        app.set_focus(PaneId::Queue);
        let before = app.collapsed[ListPane::Tasks.idx()];
        let r = pane_button_rect(&app, PaneId::Tasks, crate::hit::PaneButton::Collapse);
        assert_eq!(app.drag, None);
        app.update(mouse(
            MouseEventKind::Down(MouseButton::Left),
            r.x + r.width / 2,
            r.y,
        ));
        assert_ne!(app.collapsed[ListPane::Tasks.idx()], before, "collapse toggled");
        assert_eq!(app.ui().focus, PaneId::Queue, "collapse leaves focus put");
        assert_eq!(app.drag, None, "chip click did not start a divider drag");
    }

    #[test]
    fn detail_body_click_does_not_steal_focus() {
        let mut app = app_with_hits();
        app.set_focus(PaneId::Queue);
        let mut hits = app.hit.clone();
        hits.push(
            Rect { x: 40, y: 2, width: 20, height: 10 },
            HitTarget::PaneBody(PaneId::Detail),
        );
        app.hit = hits;
        let up = app.update(mouse(MouseEventKind::Down(MouseButton::Left), 45, 5));
        assert!(!up.dirty, "detail body click is inert");
        assert_eq!(app.ui().focus, PaneId::Queue);
    }

    #[test]
    fn shift_click_extends_selection() {
        let mut app = app_with_hits();
        // Plain click must land on an UNSELECTED row (the default cursor is 0, so
        // clicking row 0 would open its action menu). Row 1 becomes the anchor.
        app.update(mouse(MouseEventKind::Down(MouseButton::Left), 5, 3)); // row 1
        let ev = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 5,
            row: 5, // row 3
            modifiers: KeyModifiers::SHIFT,
        });
        app.update(ev);
        assert_eq!(app.ui().selections[0], Selection { cursor: 3, anchor: Some(1) });
    }

    #[test]
    fn wheel_moves_pane_under_cursor_without_focus_change() {
        let mut app = app_with_hits();
        app.set_focus(PaneId::Tasks);
        let up = app.update(mouse(MouseEventKind::ScrollDown, 5, 3)); // over queue body
        assert!(up.dirty);
        assert_eq!(app.ui().focus, PaneId::Tasks, "wheel must not steal focus");
        assert_eq!(app.ui().selections[0].cursor, 1);
        app.update(mouse(MouseEventKind::ScrollUp, 5, 3));
        assert_eq!(app.ui().selections[0].cursor, 0);
    }

    #[test]
    fn scrollbar_drag_math_maps_proportionally() {
        let mut app = app_with_hits();
        // Track: y=2, h=10. Queue has 4 rows → scrollable = 3.
        app.update(mouse(MouseEventKind::Down(MouseButton::Left), 30, 2)); // top
        assert!(app.drag == Some(DragKind::Scrollbar(PaneId::Queue)));
        assert_eq!(app.ui().selections[0].cursor, 0); // (2−2)*3/10 = 0
        app.update(mouse(MouseEventKind::Drag(MouseButton::Left), 30, 11)); // near bottom
        assert_eq!(app.ui().selections[0].cursor, 2); // (11−2)*3/10 = 2
        app.update(mouse(MouseEventKind::Drag(MouseButton::Left), 30, 40)); // past end → clamp
        assert_eq!(app.ui().selections[0].cursor, 3);
        app.update(mouse(MouseEventKind::Up(MouseButton::Left), 30, 40));
        assert_eq!(app.drag, None);
    }

    /// Render a real fixture app to a `TestBackend` so `app.hit` carries the true
    /// divider geometry (mirrors the view tests' `render_at`).
    fn app_rendered(w: u16, h: u16) -> App {
        use ratatui::{Terminal, backend::TestBackend};
        let mut app = crate::test_fixtures::fixture_app();
        app.size = (w, h);
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        let mut hits = crate::hit::HitMap::new();
        terminal.draw(|f| hits = crate::view::render(&app, f)).unwrap();
        app.hit = hits;
        app
    }

    fn divider_h_rect(app: &App, which: usize) -> Rect {
        app.hit
            .iter()
            .find_map(|(r, t)| (*t == HitTarget::PaneDividerH(which)).then_some(*r))
            .expect("horizontal divider registered")
    }

    fn divider_v_rect(app: &App) -> Rect {
        app.hit
            .iter()
            .find_map(|(r, t)| (*t == HitTarget::PaneDividerV).then_some(*r))
            .expect("vertical divider registered")
    }

    #[test]
    fn drag_horizontal_divider_resizes_queue_and_up_ends_drag() {
        let mut app = app_rendered(80, 24);
        // Default at 80x24: body_h = 22 → queue 12, tasks 5, worktrees 5.
        assert_eq!(app.queue_h_override, None);
        let r = divider_h_rect(&app, 0); // queue/tasks boundary
        // Down on the divider records the drag kind.
        app.update(mouse(MouseEventKind::Down(MouseButton::Left), r.x + 2, r.y));
        assert_eq!(app.drag, Some(DragKind::DividerH(0)));
        // Drag the boundary down several rows → the queue pane grows.
        let u = app.update(mouse(MouseEventKind::Drag(MouseButton::Left), r.x + 2, r.y + 4));
        assert!(u.dirty);
        let q = app.queue_h_override.expect("queue override set by drag");
        assert!(q > 12, "dragging the boundary down grows the queue (q={q})");
        // Overrides never violate the minimum-height / exact-sum invariant.
        let l = crate::selectors::pane_layout(22, app.queue_h_override, app.tasks_h_override, app.collapsed);
        assert_eq!(l.queue_h + l.tasks_h + l.worktrees_h, 22);
        assert!(l.worktrees_h >= 4 && l.tasks_h >= 4);
        // Up ends the drag; the override persists.
        app.update(mouse(MouseEventKind::Up(MouseButton::Left), r.x + 2, r.y + 4));
        assert_eq!(app.drag, None);
        assert_eq!(app.queue_h_override, Some(q));
    }

    #[test]
    fn drag_tasks_worktrees_divider_resizes_tasks() {
        let mut app = app_rendered(80, 24);
        let r = divider_h_rect(&app, 1); // tasks/worktrees boundary
        app.update(mouse(MouseEventKind::Down(MouseButton::Left), r.x + 2, r.y));
        assert_eq!(app.drag, Some(DragKind::DividerH(1)));
        // Drag the lower boundary down → the tasks pane grows past its default 5.
        app.update(mouse(MouseEventKind::Drag(MouseButton::Left), r.x + 2, r.y + 3));
        let t = app.tasks_h_override.expect("tasks override set");
        assert!(t > 5, "tasks pane grows (t={t})");
        let l = crate::selectors::pane_layout(22, app.queue_h_override, app.tasks_h_override, app.collapsed);
        assert_eq!(l.queue_h + l.tasks_h + l.worktrees_h, 22);
        assert!(l.worktrees_h >= 4);
    }

    #[test]
    fn drag_vertical_divider_resizes_left_column() {
        let mut app = app_rendered(80, 24);
        assert_eq!(app.left_cols, None);
        let r = divider_v_rect(&app);
        // Down a few rows below the top corner (avoid the H-divider overlap).
        app.update(mouse(MouseEventKind::Down(MouseButton::Left), r.x, r.y + 3));
        assert_eq!(app.drag, Some(DragKind::DividerV));
        // Drag right → the left column widens to the clamped drop column.
        let target_col = r.x + 12;
        app.update(mouse(MouseEventKind::Drag(MouseButton::Left), target_col, r.y + 3));
        assert_eq!(app.left_cols, Some(crate::selectors::clamp_left_cols(80, target_col)));
        assert!(app.left_cols.unwrap() > r.x, "left column grew");
        app.update(mouse(MouseEventKind::Up(MouseButton::Left), target_col, r.y + 3));
        assert_eq!(app.drag, None);
    }

    // -- pane collapse + per-project layout persistence ----------------------

    #[test]
    fn key_z_toggles_focused_pane_collapse_and_emits_save() {
        let mut app = crate::test_fixtures::fixture_app();
        // Route the fixture through a snapshot so the active project's layout is
        // reconciled (applied_layout_repo becomes Some("acme")).
        app.update(Event::Snapshot(crate::test_fixtures::fixture_snapshot()));
        assert_eq!(app.collapsed, [false, false, false]);
        // Focus is Queue by default → `z` collapses the queue pane and persists.
        let up = press(&mut app, KeyCode::Char('z'));
        assert_eq!(app.collapsed, [true, false, false]);
        assert!(
            up.cmds.iter().any(|c| matches!(c, Cmd::SaveLayout { .. })),
            "collapse toggle emits a SaveLayout Cmd"
        );
        // Toggling again expands it, again persisting.
        let up = press(&mut app, KeyCode::Char('z'));
        assert_eq!(app.collapsed, [false, false, false]);
        assert!(up.cmds.iter().any(|c| matches!(c, Cmd::SaveLayout { .. })));
    }

    #[test]
    fn key_z_on_detail_focus_is_noop() {
        let mut app = crate::test_fixtures::fixture_app();
        app.set_focus(PaneId::Detail);
        let up = press(&mut app, KeyCode::Char('z'));
        assert_eq!(app.collapsed, [false, false, false]);
        assert!(!up.cmds.iter().any(|c| matches!(c, Cmd::SaveLayout { .. })));
    }

    #[test]
    fn click_on_bare_title_row_does_not_toggle_collapse() {
        // The whole-row collapse toggle was removed: it swallowed divider drags
        // starting on the shared border row. Collapse is the chip or `z` only.
        let mut app = app_rendered(80, 24);
        let r = divider_h_rect(&app, 0); // queue/tasks boundary = TASKS title row
        // Click a border cell away from any chip: starts a divider drag, never
        // a collapse.
        app.update(mouse(MouseEventKind::Down(MouseButton::Left), r.x + 2, r.y + 1));
        assert_eq!(app.collapsed, [false, false, false]);
        assert!(matches!(app.drag, Some(DragKind::DividerH(0))));
        app.update(mouse(MouseEventKind::Up(MouseButton::Left), r.x + 2, r.y + 1));
    }

    #[test]
    fn divider_drag_up_emits_save() {
        let mut app = app_rendered(80, 24);
        let r = divider_h_rect(&app, 0);
        app.update(mouse(MouseEventKind::Down(MouseButton::Left), r.x + 2, r.y));
        app.update(mouse(MouseEventKind::Drag(MouseButton::Left), r.x + 2, r.y + 4));
        // No SaveLayout mid-drag; the write happens only on release.
        let up = app.update(mouse(MouseEventKind::Up(MouseButton::Left), r.x + 2, r.y + 4));
        assert_eq!(app.drag, None);
        assert!(
            up.cmds.iter().any(|c| matches!(c, Cmd::SaveLayout { .. })),
            "divider drag-end emits a SaveLayout Cmd"
        );
    }

    #[test]
    fn scrollbar_drag_up_does_not_emit_save() {
        let mut app = app_with_hits();
        app.update(mouse(MouseEventKind::Down(MouseButton::Left), 30, 2)); // scrollbar track
        let up = app.update(mouse(MouseEventKind::Up(MouseButton::Left), 30, 5));
        assert_eq!(app.drag, None);
        assert!(!up.cmds.iter().any(|c| matches!(c, Cmd::SaveLayout { .. })));
    }

    #[test]
    fn divider_drag_ignored_when_adjacent_pane_collapsed() {
        let mut app = app_rendered(80, 24);
        app.collapsed = [false, true, false]; // tasks collapsed → both H dividers pinned
        let before = app.queue_h_override;
        assert!(!app.drag_divider_h(0, 20), "queue/tasks boundary can't move");
        assert!(!app.drag_divider_h(1, 20), "tasks/worktrees boundary can't move");
        assert_eq!(app.queue_h_override, before);
    }

    #[test]
    fn switching_projects_swaps_and_isolates_layout() {
        let mut app = app();
        app.update(Event::Snapshot(snapshot_with(&["platform", "web"], vec![])));
        // On project 0 (platform): collapse the queue pane.
        assert_eq!(app.active_tab, 0);
        app.collapsed = [true, false, false];
        // Switch to project 1 (web): platform's layout is stashed, web loads
        // defaults (nothing saved yet).
        press(&mut app, KeyCode::Char('2'));
        assert_eq!(app.active_tab, 1);
        assert_eq!(app.collapsed, [false, false, false], "web starts at defaults");
        // Give web a distinct layout, then switch back to platform.
        app.collapsed = [false, false, true];
        press(&mut app, KeyCode::Char('1'));
        assert_eq!(app.active_tab, 0);
        assert_eq!(app.collapsed, [true, false, false], "platform's layout restored");
        // And forward to web again → its own layout.
        press(&mut app, KeyCode::Char('2'));
        assert_eq!(app.collapsed, [false, false, true], "web's layout restored");
    }

    #[test]
    fn loaded_layout_applies_to_active_project_on_first_snapshot() {
        let mut app = app();
        // Simulate a persisted layout for "platform" loaded at startup.
        app.layout_by_project.insert(
            "platform".to_string(),
            crate::layout::ProjectLayout {
                left_cols: Some(50),
                queue_h: None,
                tasks_h: None,
                collapsed: [false, true, false],
            },
        );
        app.update(Event::Snapshot(snapshot_with(&["platform"], vec![])));
        assert_eq!(app.collapsed, [false, true, false]);
        assert_eq!(app.left_cols, Some(50));
    }

    #[test]
    fn click_tab_switches_and_click_nothing_closes_overlay() {
        let mut app = app_with_hits();
        app.mode = Mode::Help;
        // Click hitting no target while an overlay is open → ClearEsc semantics.
        let up = app.update(mouse(MouseEventKind::Down(MouseButton::Left), 70, 20));
        assert!(matches!(app.mode, Mode::List));
        assert!(up.dirty);
        // Tab click (single-project fixture: index 0 → no change, not dirty).
        let up = app.update(mouse(MouseEventKind::Down(MouseButton::Left), 3, 0));
        assert!(!up.dirty);
    }

    #[test]
    fn modal_target_swallows_clicks() {
        let mut app = app_with_hits();
        app.hit.push(Rect { x: 0, y: 0, width: 80, height: 24 }, HitTarget::Modal);
        app.mode = Mode::Help;
        let up = app.update(mouse(MouseEventKind::Down(MouseButton::Left), 5, 4));
        assert!(matches!(app.mode, Mode::Help), "click inside a modal never leaks through");
        assert!(!up.dirty);
    }

    /// Confirm/Help overlays own every click: a click on a live pane widget
    /// behind the popup dismisses it (same as esc, no dispatch); a click inside
    /// the modal body is inert.
    fn assert_overlay_owns_clicks(make_mode: impl Fn() -> Mode) {
        // Click on Row(Queue, 2) at (5, 4), behind the popup → dismiss, no cmd.
        let mut app = app_with_hits();
        app.mode = make_mode();
        let up = app.update(mouse(MouseEventKind::Down(MouseButton::Left), 5, 4));
        assert!(matches!(app.mode, Mode::List), "outside-click must dismiss");
        assert!(up.dirty);
        assert!(up.cmds.is_empty(), "outside-click dismiss dispatches nothing");

        // Click inside the modal body (Modal covers the screen) → inert.
        let mut app = app_with_hits();
        app.hit.push(Rect { x: 0, y: 0, width: 80, height: 24 }, HitTarget::Modal);
        app.mode = make_mode();
        let up = app.update(mouse(MouseEventKind::Down(MouseButton::Left), 5, 4));
        assert!(!matches!(app.mode, Mode::List), "click inside modal stays open");
        assert!(!up.dirty, "click inside modal is inert");
        assert!(up.cmds.is_empty());
    }

    #[test]
    fn confirm_remove_overlay_owns_clicks() {
        assert_overlay_owns_clicks(|| Mode::ConfirmRemove {
            repo: "acme".into(),
            worktree: "acme.feature".into(),
            branch: "feature/x".into(),
        });
    }

    #[test]
    fn confirm_bulk_remove_overlay_owns_clicks() {
        assert_overlay_owns_clicks(|| Mode::ConfirmBulkRemove {
            repo: "acme".into(),
            names: vec!["acme.feature".into(), "acme.hotfix".into()],
        });
    }

    #[test]
    fn help_overlay_owns_clicks() {
        assert_overlay_owns_clicks(|| Mode::Help);
    }

    #[test]
    fn idle_tick_with_nothing_running_is_not_dirty() {
        let mut app = crate::test_fixtures::fixture_app();
        let mut snap = crate::test_fixtures::fixture_snapshot();
        snap.running.clear();
        for t in &mut snap.tasks {
            if matches!(t.status, crate::ipc::types::TaskStatus::Running) {
                t.status = crate::ipc::types::TaskStatus::Done;
            }
        }
        app.update(Event::Snapshot(snap));
        let up = app.update(Event::Tick);
        assert!(!up.dirty, "zero idle renders: Tick with nothing running must not dirty");
    }
}

#[cfg(test)]
mod action_result_tests {
    use super::*;
    use serde_json::json;

    fn app() -> App {
        App::new(std::path::PathBuf::from("/tmp/runs"), std::path::PathBuf::from("/tmp/daemon.sock"))
    }

    #[test]
    fn dispatch_rpc_applies_per_method_defaults() {
        let mut a = app();

        // Plain mutation: 5s timeout, not timeout-ok, no invalidation.
        let cmd = a.dispatch_rpc("rerun task", "retry", json!({ "id": "t1" }), RpcOpts::default());
        match cmd {
            Cmd::Rpc { call, timeout_ms, timeout_is_ok, invalidate_defs_for, .. } => {
                assert_eq!(call.method, "retry");
                assert_eq!(call.params, json!({ "id": "t1" }));
                assert_eq!(timeout_ms, 5_000);
                assert!(!timeout_is_ok);
                assert_eq!(invalidate_defs_for, None);
            }
            other => panic!("expected Cmd::Rpc, got {other:?}"),
        }

        // createWorktree: 10-minute budget for post-create hooks.
        let cmd = a.dispatch_rpc("create worktree", "createWorktree", json!({ "repo": "p", "name": "b" }), RpcOpts::default());
        match cmd {
            Cmd::Rpc { timeout_ms, .. } => assert_eq!(timeout_ms, 600_000),
            other => panic!("expected Cmd::Rpc, got {other:?}"),
        }

        // runDefinition: client timeout is success; invalidation passed by caller.
        let cmd = a.dispatch_rpc(
            "run definition",
            "runDefinition",
            json!({ "repo": "p", "name": "d", "args": [], "source": "tui" }),
            RpcOpts { invalidate_defs_for: Some("p".into()), ..RpcOpts::default() },
        );
        match cmd {
            Cmd::Rpc { timeout_ms, timeout_is_ok, invalidate_defs_for, .. } => {
                assert_eq!(timeout_ms, 5_000);
                assert!(timeout_is_ok);
                assert_eq!(invalidate_defs_for.as_deref(), Some("p"));
            }
            other => panic!("expected Cmd::Rpc, got {other:?}"),
        }
    }

    #[test]
    fn action_result_sets_status_then_next_list_key_clears_it() {
        let mut a = app();
        let u = a.update(Event::ActionResult { status: Some("boom".into()), invalidate_defs_for: None });
        assert!(u.dirty);
        assert_eq!(a.status_line.as_deref(), Some("boom"));
        assert!(u.cmds.is_empty());

        // Any list-mode keypress clears the status line (Task 11 behavior; a
        // no-op key like Char('z') still passes through the clear-at-top path).
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        a.update(Event::Key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE)));
        assert_eq!(a.status_line, None);
    }

    #[test]
    fn action_result_invalidation_drops_cache_and_refetches() {
        let mut a = app();
        a.defs_by_project.insert("platform".into(), vec![]);
        let u = a.update(Event::ActionResult { status: None, invalidate_defs_for: Some("platform".into()) });
        assert!(u.dirty);
        assert!(!a.defs_by_project.contains_key("platform"));
        assert!(
            u.cmds.iter().any(|c| matches!(c, Cmd::FetchDefinitions { repo } if repo == "platform")),
            "expected a FetchDefinitions for the invalidated repo, got {:?}", u.cmds,
        );
        // A None status leaves the status line untouched.
        assert_eq!(a.status_line, None);
    }
}

#[cfg(test)]
mod menu_flow_tests {
    use super::*;
    use crate::action_menu::MenuAction;
    use crate::ipc::types::{Project, StateSnapshot, TaskInstance, TaskStatus, WorktreeInfo};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::collections::HashMap;

    fn key(c: char) -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
    }
    fn enter() -> Event {
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
    }

    fn app_with(snap: StateSnapshot) -> App {
        let mut a = App::new("/tmp/runs".into(), "/tmp/daemon.sock".into());
        a.size = (120, 40);
        a.update(Event::Snapshot(snap));
        a
    }

    fn failed_task_snapshot() -> StateSnapshot {
        let mut t = TaskInstance::default();
        t.id = "t1".into();
        t.status = TaskStatus::Failed;
        t.target.repo = "platform".into();
        StateSnapshot {
            tasks: vec![t],
            projects: vec![Project { name: "platform".into() }],
            ..Default::default()
        }
    }

    fn worktree_snapshot() -> StateSnapshot {
        let mut wts = HashMap::new();
        wts.insert(
            "platform".into(),
            vec![WorktreeInfo { name: "platform.wt-a".into(), path: "/wt/wt-a".into(), branch: "wt-a".into(), ..Default::default() }],
        );
        StateSnapshot {
            projects: vec![Project { name: "platform".into() }],
            worktrees: wts,
            ..Default::default()
        }
    }

    // --- focus helpers (Tab cycles panes; moveFocus order queue→tasks→worktrees) ---
    fn tab(a: &mut App) {
        a.update(Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)));
    }
    fn focus_tasks(a: &mut App) {
        tab(a);
    }
    fn focus_worktrees(a: &mut App) {
        tab(a);
        tab(a);
    }

    #[test]
    fn queue_menu_execute_rerun_emits_retry_and_closes() {
        let mut a = app_with(failed_task_snapshot());
        a.update(enter()); // open menu, index 0 = Rerun (enabled: failed)
        let u = a.update(enter()); // execute Rerun
        assert!(matches!(a.mode, Mode::List));
        assert!(
            u.cmds.iter().any(|c| matches!(c, Cmd::Rpc { call, .. }
                if call.method == "retry" && call.params == serde_json::json!({ "id": "t1" }))),
            "expected retry Cmd, got {:?}",
            u.cmds,
        );
    }

    #[test]
    fn queue_menu_j_moves_highlight() {
        let mut a = app_with(failed_task_snapshot());
        a.update(enter());
        a.update(key('j'));
        match &a.mode {
            Mode::ActionMenu { index, .. } => assert_eq!(*index, 1),
            _ => panic!(),
        }
        a.update(key('k'));
        match &a.mode {
            Mode::ActionMenu { index, .. } => assert_eq!(*index, 0),
            _ => panic!(),
        }
    }

    #[test]
    fn queue_menu_enter_on_disabled_row_is_inert() {
        // A running task: every row disabled; Enter must not dispatch, must not close.
        let mut snap = failed_task_snapshot();
        snap.tasks[0].status = TaskStatus::Running;
        let mut a = app_with(snap);
        a.update(enter());
        let u = a.update(enter()); // on disabled "Rerun"
        assert!(matches!(a.mode, Mode::ActionMenu { .. }));
        assert!(u.cmds.is_empty());
    }

    #[test]
    fn queue_menu_esc_and_q_close() {
        let mut a = app_with(failed_task_snapshot());
        a.update(enter());
        a.update(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
        assert!(matches!(a.mode, Mode::List));
        a.update(enter());
        a.update(key('q'));
        assert!(matches!(a.mode, Mode::List));
    }

    #[test]
    fn assign_worktree_opens_worktree_input() {
        let mut snap = failed_task_snapshot();
        snap.tasks[0].status = TaskStatus::NeedsInput; // enables assign-worktree
        let mut a = app_with(snap);
        a.update(enter());
        a.update(key('j'));
        a.update(key('j')); // -> Assign worktree…
        a.update(enter());
        match &a.mode {
            Mode::WorktreeInput { task_id, .. } => assert_eq!(task_id, "t1"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn worktree_menu_task_fresh_opens_add_task_with_raw_name() {
        let mut a = app_with(worktree_snapshot());
        focus_worktrees(&mut a);
        a.update(enter()); // open worktree menu, index 0 = New task (fresh)…
        a.update(enter());
        match &a.mode {
            Mode::AddTask { worktree, session, .. } => {
                assert_eq!(worktree.as_deref(), Some("platform.wt-a"));
                assert!(matches!(session, SessionMode::Fresh));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn worktree_menu_remove_opens_confirm_remove() {
        let mut a = app_with(worktree_snapshot());
        focus_worktrees(&mut a);
        a.update(enter());
        for _ in 0..5 {
            a.update(key('j'));
        } // -> Remove worktree… (index 5)
        a.update(enter());
        match &a.mode {
            Mode::ConfirmRemove { repo, worktree, branch } => {
                assert_eq!(repo, "platform");
                assert_eq!(worktree, "platform.wt-a"); // raw name for removal
                assert_eq!(branch, "wt-a");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn confirm_remove_y_dispatches_and_n_cancels() {
        let mut a = app_with(worktree_snapshot());
        focus_worktrees(&mut a);
        a.update(enter());
        for _ in 0..5 {
            a.update(key('j'));
        }
        a.update(enter()); // ConfirmRemove
        let u = a.update(key('y'));
        assert!(matches!(a.mode, Mode::List));
        assert!(u.cmds.iter().any(|c| matches!(c, Cmd::Rpc { call, .. }
            if call.method == "removeWorktree"
                && call.params == serde_json::json!({ "repo": "platform", "name": "platform.wt-a" }))));

        // n cancels without a cmd
        a.update(enter());
        for _ in 0..5 {
            a.update(key('j'));
        }
        a.update(enter());
        let u2 = a.update(key('n'));
        assert!(matches!(a.mode, Mode::List));
        assert!(u2.cmds.is_empty());
    }

    #[test]
    fn tasks_pane_run_zero_arg_def_dispatches_and_closes() {
        // tasks-pane Run → RunNamedDef. A zero-arg def dispatches runDefinition
        // immediately (implemented in Task 18; args-form path is Task 19).
        let snap = StateSnapshot {
            projects: vec![Project { name: "platform".into() }],
            ..Default::default()
        };
        let mut a = app_with(snap);
        a.defs_by_project.insert("platform".into(), vec![{
            let mut d = crate::ipc::types::DefinitionSummary::default();
            d.repo = "platform".into();
            d.name = "lint".into();
            d
        }]);
        focus_tasks(&mut a);
        a.update(enter()); // open tasks menu
        let u = a.update(enter()); // execute Run → immediate dispatch (zero-arg)
        assert!(matches!(a.mode, Mode::List));
        assert!(
            u.cmds.iter().any(|c| matches!(c, Cmd::Rpc { call, invalidate_defs_for, .. }
                if call.method == "runDefinition"
                    && call.params["name"] == "lint"
                    && call.params["source"] == "tui"
                    && invalidate_defs_for.as_deref() == Some("platform"))),
            "expected an immediate runDefinition dispatch, got {:?}",
            u.cmds,
        );
        // The menu action carries repo+name for the dispatch.
        let (_t, items) = crate::action_menu::tasks_menu(&a.defs_by_project["platform"][0]);
        assert!(matches!(items[0].action, MenuAction::RunNamedDef { .. }));
    }

    #[test]
    fn click_menu_item_executes() {
        use crate::hit::HitTarget;
        use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
        use ratatui::{Terminal, backend::TestBackend};

        let mut a = app_with(failed_task_snapshot());
        a.update(enter()); // open menu; row 0 = Rerun (enabled: failed task)
        assert!(matches!(a.mode, Mode::ActionMenu { .. }));

        // Render the open menu so the real hit map carries its MenuItem regions.
        let (w, h) = (120u16, 40u16);
        a.size = (w, h);
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        let mut hits = crate::hit::HitMap::new();
        term.draw(|f| hits = crate::view::render(&a, f)).unwrap();
        a.hit = hits;

        // Locate a MenuItem(0) cell and synthesize a left-click on it.
        let mut pos = None;
        'find: for y in 0..h {
            for x in 0..w {
                if let Some(HitTarget::MenuItem(0)) = a.hit.hit(x, y) {
                    pos = Some((x, y));
                    break 'find;
                }
            }
        }
        let (mx, my) = pos.expect("MenuItem(0) region present after render");
        let u = a.update(Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: mx,
            row: my,
            modifiers: KeyModifiers::NONE,
        }));
        assert!(matches!(a.mode, Mode::List), "clicking a row closes the menu");
        assert!(
            u.cmds.iter().any(|c| matches!(c, Cmd::Rpc { call, .. }
                if call.method == "retry" && call.params == serde_json::json!({ "id": "t1" }))),
            "clicking Rerun dispatches retry, got {:?}",
            u.cmds,
        );
    }

    #[test]
    fn click_outside_menu_closes_it() {
        use crate::hit::HitTarget;
        use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
        use ratatui::{Terminal, backend::TestBackend};

        let mut a = app_with(failed_task_snapshot());
        a.update(enter());
        let (w, h) = (120u16, 40u16);
        a.size = (w, h);
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        let mut hits = crate::hit::HitMap::new();
        term.draw(|f| hits = crate::view::render(&a, f)).unwrap();
        a.hit = hits;

        // A corner cell is outside the centered popup (neither Modal nor MenuItem).
        assert!(!matches!(a.hit.hit(0, h - 1), Some(HitTarget::Modal | HitTarget::MenuItem(_))));
        let u = a.update(Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 0,
            row: h - 1,
            modifiers: KeyModifiers::NONE,
        }));
        assert!(matches!(a.mode, Mode::List));
        assert!(u.cmds.is_empty());
    }
}

#[cfg(test)]
mod input_modal_tests {
    use super::*;
    use crate::hit::{ButtonKind, HitTarget};
    use crate::ipc::types::{Project, StateSnapshot, WorktreeInfo};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use std::collections::HashMap;

    fn key(c: char) -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
    }
    fn enter() -> Event {
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
    }
    fn esc() -> Event {
        Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
    }

    fn app() -> App {
        let mut a = App::new("/tmp/runs".into(), "/tmp/s.sock".into());
        a.size = (120, 40);
        let mut wts = HashMap::new();
        wts.insert(
            "platform".into(),
            vec![WorktreeInfo {
                name: "platform.wt-a".into(),
                path: "/wt/wt-a".into(),
                branch: "jus-42".into(),
                ..Default::default()
            }],
        );
        a.update(Event::Snapshot(StateSnapshot {
            projects: vec![Project { name: "platform".into() }],
            worktrees: wts,
            ..Default::default()
        }));
        a
    }

    fn type_str(a: &mut App, s: &str) {
        for c in s.chars() {
            a.update(key(c));
        }
    }

    fn rpc_call(u: &Update) -> &crate::event::RpcCall {
        u.cmds
            .iter()
            .find_map(|c| if let Cmd::Rpc { call, .. } = c { Some(call) } else { None })
            .expect("expected an Rpc cmd")
    }

    #[test]
    fn add_task_worktree_targeted_enqueue_carries_worktree() {
        let mut a = app();
        a.mode = Mode::AddTask {
            worktree: Some("platform.wt-a".into()),
            session: SessionMode::Fresh,
            input: tui_input::Input::default(),
        };
        type_str(&mut a, "do a thing");
        let u = a.update(enter());
        assert!(matches!(a.mode, Mode::List));
        let call = rpc_call(&u);
        assert_eq!(call.method, "enqueue");
        assert_eq!(
            call.params,
            serde_json::json!({
                "prompt": "do a thing", "repo": "platform", "worktree": "platform.wt-a", "session": "fresh"
            })
        );
    }

    #[test]
    fn add_task_adhoc_omits_worktree() {
        let mut a = app();
        a.mode = Mode::AddTask {
            worktree: None,
            session: SessionMode::Fresh,
            input: tui_input::Input::default(),
        };
        type_str(&mut a, "run this now");
        let u = a.update(enter());
        let call = rpc_call(&u);
        assert_eq!(
            call.params,
            serde_json::json!({
                "prompt": "run this now", "repo": "platform", "session": "fresh"
            })
        );
    }

    #[test]
    fn queue_c_opens_adhoc_add_task() {
        let mut a = app(); // queue focused by default
        a.update(key('c'));
        match &a.mode {
            Mode::AddTask { worktree, session, .. } => {
                assert!(worktree.is_none());
                assert!(matches!(session, SessionMode::Fresh));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn add_task_esc_cancels_without_cmd() {
        let mut a = app();
        a.mode = Mode::AddTask {
            worktree: None,
            session: SessionMode::Fresh,
            input: tui_input::Input::default(),
        };
        let u = a.update(esc());
        assert!(matches!(a.mode, Mode::List));
        assert!(u.cmds.is_empty());
    }

    #[test]
    fn typing_q_inserts_literal_and_backspace_edits() {
        let mut a = app();
        a.mode = Mode::AddTask {
            worktree: None,
            session: SessionMode::Fresh,
            input: tui_input::Input::default(),
        };
        a.update(key('q'));
        match &a.mode {
            Mode::AddTask { input, .. } => assert_eq!(input.value(), "q"),
            _ => panic!(),
        }
        a.update(Event::Key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)));
        match &a.mode {
            Mode::AddTask { input, .. } => assert_eq!(input.value(), ""),
            _ => panic!(),
        }
    }

    #[test]
    fn worktree_input_enter_dispatches_set_worktree() {
        let mut a = app();
        a.mode = Mode::WorktreeInput { task_id: "abc123".into(), input: tui_input::Input::default() };
        type_str(&mut a, "wt-x");
        let u = a.update(enter());
        let call = rpc_call(&u);
        assert_eq!(call.method, "setWorktree");
        assert_eq!(call.params, serde_json::json!({ "id": "abc123", "worktree": "wt-x" }));
    }

    #[test]
    fn worktree_input_esc_cancels_without_cmd() {
        let mut a = app();
        a.mode = Mode::WorktreeInput { task_id: "abc123".into(), input: tui_input::Input::default() };
        let u = a.update(esc());
        assert!(matches!(a.mode, Mode::List));
        assert!(u.cmds.is_empty());
    }

    #[test]
    fn mouse_event_never_reaches_the_input() {
        let mut a = app();
        a.mode = Mode::AddTask {
            worktree: None,
            session: SessionMode::Fresh,
            input: tui_input::Input::default(),
        };
        type_str(&mut a, "hi");
        // A drag/motion mouse event over the field must not append glyphs.
        a.update(Event::Mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: 10,
            row: 5,
            modifiers: KeyModifiers::NONE,
        }));
        match &a.mode {
            Mode::AddTask { input, .. } => assert_eq!(input.value(), "hi"),
            _ => panic!(),
        }
    }

    /// Render the current mode into `a.hit` (so mouse routing has real button
    /// geometry), then return the scanned coordinates of a `Button` target.
    fn render_and_find_button(a: &mut App, kind: ButtonKind) -> (u16, u16) {
        use ratatui::{Terminal, backend::TestBackend};
        let (w, h) = a.size;
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        let mut hits = crate::hit::HitMap::new();
        term.draw(|f| hits = crate::view::render(a, f)).unwrap();
        a.hit = hits;
        for y in 0..h {
            for x in 0..w {
                if a.hit.hit(x, y) == Some(&HitTarget::Button(kind)) {
                    return (x, y);
                }
            }
        }
        panic!("Button({kind:?}) region not found after render");
    }

    fn click(a: &mut App, x: u16, y: u16) -> Update {
        a.update(Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        }))
    }

    #[test]
    fn click_confirm_equals_enter_and_cancel_equals_esc() {
        // Click Confirm ≡ Enter: dispatches enqueue and closes to List.
        let mut a = app();
        a.mode = Mode::AddTask {
            worktree: None,
            session: SessionMode::Fresh,
            input: tui_input::Input::default(),
        };
        type_str(&mut a, "run this now");
        let (cx, cy) = render_and_find_button(&mut a, ButtonKind::Confirm);
        let u = click(&mut a, cx, cy);
        assert!(matches!(a.mode, Mode::List));
        let call = rpc_call(&u);
        assert_eq!(call.method, "enqueue");
        assert_eq!(
            call.params,
            serde_json::json!({ "prompt": "run this now", "repo": "platform", "session": "fresh" })
        );

        // Click Cancel ≡ Esc: closes to List with no cmd.
        let mut a = app();
        a.mode = Mode::AddTask {
            worktree: None,
            session: SessionMode::Fresh,
            input: tui_input::Input::default(),
        };
        let (cx, cy) = render_and_find_button(&mut a, ButtonKind::Cancel);
        let u = click(&mut a, cx, cy);
        assert!(matches!(a.mode, Mode::List));
        assert!(u.cmds.is_empty());
    }
}

#[cfg(test)]
mod bulk_flow_tests {
    use super::*;
    use crate::ipc::types::{Project, StateSnapshot, TaskInstance, TaskStatus, WorktreeInfo};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::collections::HashMap;

    fn key(c: char) -> Event { Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)) }
    fn enter() -> Event { Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)) }
    fn shift_down() -> Event { Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT)) }

    fn app_with(snap: StateSnapshot) -> App {
        let mut a = App::new("/tmp/runs".into(), "/tmp/s.sock".into());
        a.size = (120, 40);
        a.update(Event::Snapshot(snap));
        a
    }

    fn two_queue_one_failed() -> StateSnapshot {
        let mut t0 = TaskInstance::default(); t0.id = "t0".into(); t0.status = TaskStatus::Failed; t0.target.repo = "platform".into();
        let mut t1 = TaskInstance::default(); t1.id = "t1".into(); t1.status = TaskStatus::Queued; t1.target.repo = "platform".into();
        StateSnapshot { tasks: vec![t0, t1], projects: vec![Project { name: "platform".into() }], ..Default::default() }
    }

    #[test]
    fn range_over_one_opens_bulk_rerun_with_eligible_count() {
        let mut a = app_with(two_queue_one_failed());
        a.update(shift_down()); // extend queue selection to 2 rows
        a.update(key('a'));     // open bulk menu
        match &a.mode {
            Mode::ActionMenu { title, items, .. } => {
                assert_eq!(title, "2 selected");
                assert_eq!(items[0].label, "Rerun (1 of 2)"); // only t0 is failed
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn bulk_rerun_emits_rpcseq_with_only_eligible_ids_and_clears_range() {
        let mut a = app_with(two_queue_one_failed());
        a.update(shift_down());
        a.update(key('a'));
        let u = a.update(enter()); // execute Rerun
        assert!(matches!(a.mode, Mode::List));
        match u.cmds.iter().find(|c| matches!(c, Cmd::RpcSeq { .. })).unwrap() {
            Cmd::RpcSeq { verb, calls, invalidate_defs_for } => {
                assert_eq!(verb, "reran");
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].method, "retry");
                assert_eq!(calls[0].params, serde_json::json!({ "id": "t0" }));
                assert_eq!(*invalidate_defs_for, None);
            }
            _ => unreachable!(),
        }
        // Range cleared (anchor None) before dispatch.
        assert_eq!(a.active_ui().selections[0].anchor, None);
    }

    #[test]
    fn frozen_targets_survive_snapshot_reshuffle() {
        let mut a = app_with(two_queue_one_failed());
        a.update(shift_down());
        a.update(key('a')); // ids frozen: BulkRerun { ids: ["t0"] }
        // A daemon push reshuffles/replaces rows mid-menu…
        let mut t = TaskInstance::default(); t.id = "zzz".into(); t.status = TaskStatus::Failed; t.target.repo = "platform".into();
        a.update(Event::Snapshot(StateSnapshot { tasks: vec![t], projects: vec![Project { name: "platform".into() }], ..Default::default() }));
        let u = a.update(enter()); // execute the still-open menu
        match u.cmds.iter().find(|c| matches!(c, Cmd::RpcSeq { .. })).unwrap() {
            Cmd::RpcSeq { calls, .. } => assert_eq!(calls[0].params, serde_json::json!({ "id": "t0" })), // NOT zzz
            _ => unreachable!(),
        }
    }

    #[test]
    fn bulk_run_defs_uses_started_verb_and_invalidates() {
        // Parity oracle app.test.tsx:1573 → "started 1"; App.tsx:698 verb "started".
        let mut snap = StateSnapshot { projects: vec![Project { name: "platform".into() }], ..Default::default() };
        snap.tasks = vec![];
        let mut a = app_with(snap);
        a.defs_by_project.insert("platform".into(), vec![
            { let mut d = crate::ipc::types::DefinitionSummary::default(); d.repo = "platform".into(); d.name = "lint".into(); d },
            { let mut d = crate::ipc::types::DefinitionSummary::default(); d.repo = "platform".into(); d.name = "deploy".into(); d.args = vec![crate::ipc::types::ArgSpec { name: "env".into(), ..Default::default() }]; d },
        ]);
        // focus tasks pane, extend to 2, open bulk menu
        a.update(Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)));
        a.update(shift_down());
        a.update(key('a'));
        match &a.mode { Mode::ActionMenu { items, .. } => assert_eq!(items[0].label, "Run (1 of 2)"), _ => panic!() }
        let u = a.update(enter());
        match u.cmds.iter().find(|c| matches!(c, Cmd::RpcSeq { .. })).unwrap() {
            Cmd::RpcSeq { verb, calls, invalidate_defs_for } => {
                assert_eq!(verb, "started");
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].method, "runDefinition");
                assert_eq!(calls[0].params, serde_json::json!({ "repo": "platform", "name": "lint", "args": [], "source": "tui" }));
                assert_eq!(invalidate_defs_for.as_deref(), Some("platform"));
            }
            _ => unreachable!(),
        }
    }

    fn three_worktrees() -> StateSnapshot {
        let mut wts = HashMap::new();
        wts.insert("platform".into(), vec![
            WorktreeInfo { name: "wt-a".into(), path: "/wt/a".into(), branch: "wt-a".into(), ..Default::default() },
            WorktreeInfo { name: "wt-b".into(), path: "/wt/b".into(), branch: "wt-b".into(), ..Default::default() },
            WorktreeInfo { name: "wt-c".into(), path: "/wt/c".into(), branch: "wt-c".into(), ..Default::default() },
        ]);
        StateSnapshot { projects: vec![Project { name: "platform".into() }], worktrees: wts, ..Default::default() }
    }

    #[test]
    fn bulk_remove_confirms_then_rpcseq_removes_each() {
        let mut a = app_with(three_worktrees());
        a.update(Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))); // → tasks
        a.update(Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))); // → worktrees
        a.update(shift_down()); a.update(shift_down()); // 3-row range
        a.update(key('a'));
        a.update(enter()); // Remove worktrees… → ConfirmBulkRemove
        match &a.mode { Mode::ConfirmBulkRemove { names, .. } => assert_eq!(names.len(), 3), other => panic!("{other:?}") }
        let u = a.update(key('y'));
        assert!(matches!(a.mode, Mode::List));
        match u.cmds.iter().find(|c| matches!(c, Cmd::RpcSeq { .. })).unwrap() {
            Cmd::RpcSeq { verb, calls, .. } => {
                assert_eq!(verb, "removed");
                assert_eq!(calls.len(), 3);
                assert_eq!(calls[0].params, serde_json::json!({ "repo": "platform", "name": "wt-a" }));
                assert_eq!(calls[2].params, serde_json::json!({ "repo": "platform", "name": "wt-c" }));
            }
            _ => unreachable!(),
        }
        assert_eq!(a.active_ui().selections[2].anchor, None); // range cleared
    }

    #[test]
    fn bulk_remove_n_cancels_without_cmd() {
        let mut a = app_with(three_worktrees());
        a.update(Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)));
        a.update(Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)));
        a.update(shift_down());
        a.update(key('a'));
        a.update(enter());
        let u = a.update(key('n'));
        assert!(matches!(a.mode, Mode::List));
        assert!(u.cmds.is_empty());
    }

    #[test]
    fn esc_with_active_range_clears_range_before_it_can_open_bulk() {
        // Staged Esc (Task 11): first Esc clears the range, so a subsequent `a`
        // opens a single-target menu, not a bulk one.
        let mut a = app_with(two_queue_one_failed());
        a.update(shift_down());
        assert_ne!(a.active_ui().selections[0].anchor, None);
        a.update(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
        assert_eq!(a.active_ui().selections[0].anchor, None);
        a.update(key('a'));
        match &a.mode { Mode::ActionMenu { title, .. } => assert_ne!(title, "2 selected"), _ => panic!() }
    }

    fn six_queue_failed(ids: &[&str]) -> StateSnapshot {
        let tasks = ids
            .iter()
            .map(|id| {
                let mut t = TaskInstance::default();
                t.id = (*id).into();
                t.status = TaskStatus::Failed;
                t.target.repo = "platform".into();
                t
            })
            .collect();
        StateSnapshot { tasks, projects: vec![Project { name: "platform".into() }], ..Default::default() }
    }

    #[test]
    fn bulk_open_does_not_panic_when_rows_empty_between_select_and_open() {
        // Race: a daemon snapshot empties the visible rows AFTER the range is
        // extended but BEFORE `a` opens the menu. The selection anchor/cursor
        // are untouched by a snapshot, so the range guard still fires; the empty
        // visible set must bail (no `vis[0..=0]` panic on an empty slice).
        let mut a = app_with(two_queue_one_failed());
        a.update(shift_down()); // anchor=0, cursor=1 (range of 2)
        // All queue rows vanish while the range is still active.
        a.update(Event::Snapshot(StateSnapshot { tasks: vec![], projects: vec![Project { name: "platform".into() }], ..Default::default() }));
        a.update(key('a')); // must not panic
        // Nothing survives → open bails, menu never opens (status line set instead).
        assert!(matches!(a.mode, Mode::List));
    }

    #[test]
    fn bulk_open_clamps_when_rows_shrink_below_frozen_start() {
        // Race: the range is anchored high (start=3, cursor=5) then the visible
        // set shrinks to 2 rows before `a`. `hi` becomes 1, below `start=3` — an
        // un-clamped `vis[3..=1]` is an inverted-range panic. The clamp collapses
        // the span to the surviving rows instead.
        let mut a = app_with(six_queue_failed(&["a0", "a1", "a2", "a3", "a4", "a5"]));
        a.update(key('j')); a.update(key('j')); a.update(key('j')); // cursor → 3
        a.update(shift_down()); a.update(shift_down()); // anchor=3, cursor=5
        assert_eq!(a.active_ui().selections[0], Selection { cursor: 5, anchor: Some(3) });
        // Snapshot shrinks the queue to 2 rows while the high range is active.
        a.update(Event::Snapshot(six_queue_failed(&["a0", "a1"])));
        a.update(key('a')); // must not panic
        // Surviving span clamped to a single row.
        match &a.mode { Mode::ActionMenu { title, .. } => assert_eq!(title, "1 selected"), other => panic!("{other:?}") }
    }
}

#[cfg(test)]
mod def_pick_tests {
    use super::*;
    use crate::action_menu::MenuAction;
    use crate::ipc::types::{ArgSpec, DefinitionSummary, Project, StateSnapshot, WorktreeInfo};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::collections::HashMap;

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn arg(name: &str) -> ArgSpec {
        ArgSpec { name: name.into(), default: None, options: None, description: None }
    }

    fn dsum(repo: &str, name: &str, scope: &str, args: Vec<ArgSpec>) -> DefinitionSummary {
        DefinitionSummary { repo: repo.into(), name: name.into(), scope: scope.into(), args, has_discovery: false, cron: None, description: None }
    }

    fn fixture_app_one_project(name: &str) -> App {
        let mut app = App::new("/tmp/runs".into(), "/tmp/daemon.sock".into());
        app.size = (120, 40);
        app.snapshot = Some(StateSnapshot {
            projects: vec![Project { name: name.into() }],
            ..Default::default()
        });
        app.connected = true;
        app
    }

    fn fixture_app_with_defs(repo: &str, defs: Vec<DefinitionSummary>) -> App {
        let mut app = fixture_app_one_project(repo);
        app.defs_by_project.insert(repo.into(), defs);
        app
    }

    fn fixture_app_with_defs_and_worktree(
        repo: &str,
        defs: Vec<DefinitionSummary>,
        (wt_name, branch): (&str, &str),
    ) -> App {
        let mut app = fixture_app_one_project(repo);
        let mut wts = HashMap::new();
        wts.insert(
            repo.to_string(),
            vec![WorktreeInfo {
                name: format!("{repo}.{wt_name}"),
                path: format!("/wt/{wt_name}"),
                branch: branch.into(),
                ..Default::default()
            }],
        );
        app.snapshot = Some(StateSnapshot {
            projects: vec![Project { name: repo.into() }],
            worktrees: wts,
            ..Default::default()
        });
        app.defs_by_project.insert(repo.into(), defs);
        app
    }

    fn fixture_def_pick_defs(defs: Vec<DefinitionSummary>, worktree: Option<String>, branch: Option<String>) -> App {
        let mut app = App::new("/tmp/runs".into(), "/tmp/daemon.sock".into());
        app.size = (120, 40);
        app.mode = Mode::DefPick { defs, index: 0, worktree, branch };
        app
    }

    fn fixture_def_pick(names: Vec<&str>, worktree: Option<&str>, branch: Option<&str>) -> App {
        let defs = names.iter().map(|n| dsum("platform", n, "project", vec![])).collect();
        fixture_def_pick_defs(defs, worktree.map(Into::into), branch.map(Into::into))
    }

    // --- Step 6: lazy fetch + in-flight dedup ---
    #[test]
    fn reconcile_defs_fetches_once_and_dedups() {
        let mut app = fixture_app_one_project("platform");
        let cmd = app.reconcile_defs();
        assert!(matches!(cmd, Some(Cmd::FetchDefinitions { ref repo }) if repo == "platform"));
        assert!(app.defs_inflight.contains("platform"));
        assert!(app.reconcile_defs().is_none());
        app.update(Event::Definitions { repo: "platform".into(), defs: vec![] });
        assert!(app.defs_by_project.contains_key("platform"));
        assert!(!app.defs_inflight.contains("platform"));
        assert!(app.reconcile_defs().is_none());
    }

    #[test]
    fn definitions_event_keeps_only_the_fetched_repos_defs() {
        // The daemon's `definitions` call returns entries for EVERY project (a
        // global def like squash-merge appears once per project). Caching the
        // unfiltered list rendered N duplicate rows in the TASKS pane.
        let mut app = fixture_app_one_project("platform");
        app.update(Event::Definitions {
            repo: "platform".into(),
            defs: vec![
                dsum("platform", "squash-merge", "global", vec![]),
                dsum("web", "squash-merge", "global", vec![]),
                dsum("dotfiles", "squash-merge", "global", vec![]),
                dsum("platform", "pr-review", "project", vec![]),
            ],
        });
        let cached = &app.defs_by_project["platform"];
        assert_eq!(
            cached.iter().map(|d| (d.repo.as_str(), d.name.as_str())).collect::<Vec<_>>(),
            vec![("platform", "squash-merge"), ("platform", "pr-review")]
        );
    }

    #[test]
    fn action_result_invalidation_marks_inflight_so_reconcile_dedups() {
        // The eager re-fetch on invalidation marks the repo in flight; the event
        // loop's follow-up reconcile must not emit a duplicate fetch.
        let mut app = fixture_app_with_defs("platform", vec![dsum("platform", "x", "project", vec![])]);
        let u = app.update(Event::ActionResult { status: None, invalidate_defs_for: Some("platform".into()) });
        assert!(u.cmds.iter().any(|c| matches!(c, Cmd::FetchDefinitions { repo } if repo == "platform")));
        assert!(app.defs_inflight.contains("platform"));
        assert!(app.reconcile_defs().is_none(), "reconcile must dedup against the eager re-fetch");
    }

    // --- Step 11: RunNamedDef (tasks pane) dispatch shapes ---
    #[test]
    fn run_named_def_zero_arg_dispatches_immediately() {
        let mut app = fixture_app_with_defs("platform", vec![dsum("platform", "noargs", "project", vec![])]);
        let update = app.execute_menu_action(MenuAction::RunNamedDef { repo: "platform".into(), name: "noargs".into() });
        match &update.cmds[0] {
            Cmd::Rpc { call, invalidate_defs_for, timeout_is_ok, .. } => {
                assert_eq!(call.method, "runDefinition");
                assert_eq!(call.params["repo"], "platform");
                assert_eq!(call.params["name"], "noargs");
                assert_eq!(call.params["args"], serde_json::json!([]));
                assert_eq!(call.params["source"], "tui");
                assert!(call.params.get("worktree").is_none());
                assert_eq!(invalidate_defs_for.as_deref(), Some("platform"));
                assert!(*timeout_is_ok);
            }
            other => panic!("expected runDefinition Rpc, got {other:?}"),
        }
        assert!(matches!(app.mode, Mode::List));
    }

    #[test]
    fn run_named_def_with_args_opens_def_args_with_ambient_overlay() {
        let mut app = fixture_app_with_defs_and_worktree(
            "platform",
            vec![dsum("platform", "deploy", "project", vec![arg("source")])],
            ("wt-a", "jus-9-x"),
        );
        let update = app.execute_menu_action(MenuAction::RunNamedDef { repo: "platform".into(), name: "deploy".into() });
        assert!(update.cmds.is_empty()); // no dispatch yet — form opens
        match &app.mode {
            Mode::DefArgs { form } => {
                assert_eq!(form.args[0].options.as_deref(), Some(&["jus-9-x".to_string()][..]));
                assert_eq!(form.values[0], "jus-9-x");
                assert!(form.fixed.is_empty());
                assert_eq!(form.initial_worktree, None);
            }
            other => panic!("expected DefArgs, got {other:?}"),
        }
    }

    // --- Step 16: RunDef → Mode::DefPick, ordering, empty guard ---
    #[test]
    fn run_def_opens_def_pick_in_server_order() {
        let mut app = fixture_app_with_defs("platform", vec![
            dsum("platform", "autotest", "project", vec![]),
            dsum("platform", "squash-merge", "global", vec![arg("source")]),
        ]);
        let _ = app.execute_menu_action(MenuAction::RunDef {
            worktree: Some("platform.wt-a".into()),
            branch: Some("jus-4-x".into()),
        });
        match &app.mode {
            Mode::DefPick { defs, index, worktree, branch } => {
                assert_eq!(defs.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), vec!["autotest", "squash-merge"]);
                assert_eq!(*index, 0);
                assert_eq!(worktree.as_deref(), Some("platform.wt-a"));
                assert_eq!(branch.as_deref(), Some("jus-4-x"));
            }
            other => panic!("expected DefPick, got {other:?}"),
        }
    }

    #[test]
    fn run_def_with_no_defs_sets_status_line() {
        let mut app = fixture_app_with_defs("platform", vec![]);
        let update = app.execute_menu_action(MenuAction::RunDef { worktree: Some("wt-a".into()), branch: None });
        assert_eq!(app.status_line.as_deref(), Some("no task definitions found"));
        assert!(update.cmds.is_empty());
        assert!(matches!(app.mode, Mode::List));
    }

    // --- Step 21: Mode::DefPick navigation + Enter ---
    #[test]
    fn def_pick_moves_circularly_and_closes_on_q_esc() {
        let mut app = fixture_def_pick(vec!["a", "b"], Some("platform.wt"), Some("jus-1-x"));
        app.update(key(KeyCode::Char('j'))); // 0 -> 1
        assert!(matches!(app.mode, Mode::DefPick { index: 1, .. }));
        app.update(key(KeyCode::Char('j'))); // wraps -> 0
        assert!(matches!(app.mode, Mode::DefPick { index: 0, .. }));
        app.update(key(KeyCode::Char('k'))); // wraps back -> 1
        assert!(matches!(app.mode, Mode::DefPick { index: 1, .. }));
        app.update(key(KeyCode::Char('q')));
        assert!(matches!(app.mode, Mode::List));
    }

    #[test]
    fn def_pick_enter_zero_arg_dispatches_with_worktree() {
        let mut app = fixture_def_pick_defs(
            vec![dsum("platform", "autotest", "project", vec![])],
            Some("platform.wt-a".into()),
            Some("jus-1-x".into()),
        );
        let update = app.update(key(KeyCode::Enter));
        match &update.cmds[0] {
            Cmd::Rpc { call, invalidate_defs_for, .. } => {
                assert_eq!(call.method, "runDefinition");
                assert_eq!(call.params["worktree"], "platform.wt-a");
                assert_eq!(call.params["args"], serde_json::json!([]));
                assert_eq!(invalidate_defs_for.as_deref(), Some("platform"));
            }
            other => panic!("expected runDefinition, got {other:?}"),
        }
        assert!(matches!(app.mode, Mode::List));
    }

    #[test]
    fn def_pick_enter_with_args_opens_def_args_with_fixed_context() {
        let mut app = fixture_def_pick_defs(
            vec![dsum("platform", "deploy", "project", vec![
                arg("source"),
                ArgSpec { default: Some("main".into()), ..arg("target") },
            ])],
            Some("platform.wt-a".into()),
            Some("jus-9-x".into()),
        );
        app.update(key(KeyCode::Enter));
        match &app.mode {
            Mode::DefArgs { form } => {
                assert_eq!(form.fixed.get("source").map(String::as_str), Some("jus-9-x"));
                assert_eq!(form.fixed.get("ticket").map(String::as_str), Some("JUS-9"));
                assert_eq!(form.values[0], "jus-9-x"); // source row prefilled from fixed
                assert_eq!(form.values[1], "main");     // target from default (editable)
                assert_eq!(form.initial_worktree.as_deref(), Some("platform.wt-a"));
            }
            other => panic!("expected DefArgs, got {other:?}"),
        }
    }

    // --- Task 20: Mode::DefArgs key + mouse handling ---
    fn shift(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::SHIFT))
    }
    fn def_args_app(args: Vec<ArgSpec>, fixed: HashMap<String, String>, worktree: Option<String>) -> App {
        let mut app = fixture_app_one_project("platform");
        app.mode = Mode::DefArgs { form: crate::view::args_form::ArgsForm::new("platform".into(), "pr-ready".into(), args, fixed, HashMap::new(), worktree) };
        app
    }

    #[test]
    fn def_args_fill_text_and_submit_positional_with_fixed_and_worktree() {
        use crossterm::event::KeyCode::*;
        let mut app = def_args_app(
            vec![
                ArgSpec { name: "source".into(), default: None, options: None, description: None },
                ArgSpec { name: "target".into(), default: Some("main".into()), options: None, description: None },
            ],
            HashMap::from([("source".into(), "wt-a".into())]),
            Some("platform.wt-a".into()),
        );
        // Focus starts on target (source fixed). Clear "main", type "dev".
        for _ in 0..4 { app.update(key(Backspace)); }
        for c in "dev".chars() { app.update(key(Char(c))); }
        let update = app.update(key(Enter));
        match &update.cmds[0] {
            Cmd::Rpc { call, invalidate_defs_for, .. } => {
                assert_eq!(call.method, "runDefinition");
                assert_eq!(call.params["args"], serde_json::json!(["wt-a", "dev"]));
                assert_eq!(call.params["worktree"], "platform.wt-a");
                assert_eq!(invalidate_defs_for.as_deref(), Some("platform"));
            }
            other => panic!("expected runDefinition, got {other:?}"),
        }
        assert!(matches!(app.mode, Mode::List));
    }

    #[test]
    fn def_args_required_empty_blocks_submit() {
        use crossterm::event::KeyCode::*;
        let mut app = def_args_app(vec![ArgSpec { name: "pr".into(), default: None, options: None, description: None }], HashMap::new(), None);
        let update = app.update(key(Enter)); // required + empty
        assert!(update.cmds.is_empty());
        assert!(matches!(app.mode, Mode::DefArgs { .. }));
        if let Mode::DefArgs { form } = &app.mode { assert_eq!(form.error, Some(0)); }
    }

    #[test]
    fn def_args_enter_on_enum_opens_dropdown_then_pick() {
        use crossterm::event::KeyCode::*;
        let mut app = def_args_app(
            vec![ArgSpec { name: "mode".into(), default: Some("ready".into()), options: Some(vec!["ready".into(), "create".into()]), description: None }],
            HashMap::new(), None,
        );
        app.update(key(Enter)); // enum focus -> opens dropdown
        if let Mode::DefArgs { form } = &app.mode { assert_eq!(form.dropdown, Some(0)); }
        app.update(key(Down));  // highlight create
        app.update(key(Enter)); // pick
        if let Mode::DefArgs { form } = &app.mode { assert_eq!(form.values[0], "create"); assert_eq!(form.dropdown, None); }
    }

    #[test]
    fn def_args_esc_closes_dropdown_then_cancels() {
        use crossterm::event::KeyCode::*;
        let mut app = def_args_app(
            vec![ArgSpec { name: "mode".into(), default: Some("ready".into()), options: Some(vec!["ready".into(), "create".into()]), description: None }],
            HashMap::new(), None,
        );
        app.update(key(Enter)); // open dropdown
        app.update(key(Esc));   // closes dropdown only
        assert!(matches!(app.mode, Mode::DefArgs { .. }));
        app.update(key(Esc));   // cancels form
        assert!(matches!(app.mode, Mode::List));
    }

    #[test]
    fn def_args_shift_tab_and_arrows_move_and_cycle() {
        use crossterm::event::KeyCode::*;
        let mut app = def_args_app(
            vec![
                ArgSpec { name: "mode".into(), default: Some("ready".into()), options: Some(vec!["ready".into(), "create".into()]), description: None },
                ArgSpec { name: "pr".into(), default: None, options: None, description: None },
            ],
            HashMap::new(), None,
        );
        app.update(key(Right)); // cycle mode -> create
        if let Mode::DefArgs { form } = &app.mode { assert_eq!(form.values[0], "create"); }
        app.update(key(Tab));   // focus pr
        if let Mode::DefArgs { form } = &app.mode { assert_eq!(form.focus, 1); }
        app.update(shift(BackTab)); // shift-tab back to mode
        if let Mode::DefArgs { form } = &app.mode { assert_eq!(form.focus, 0); }
    }

    #[test]
    fn def_args_click_focuses_field_and_run_submits() {
        use crate::hit::{ButtonKind, HitTarget};
        let mut app = def_args_app(vec![ArgSpec { name: "pr".into(), default: None, options: None, description: None }], HashMap::new(), None);
        app.def_args_click(&HitTarget::FormField(0));
        if let Mode::DefArgs { form } = &app.mode { assert_eq!(form.focus, 0); }
        // fill then Run
        app.update(Event::Key(KeyEvent::new(KeyCode::Char('7'), KeyModifiers::NONE)));
        let update = app.def_args_click(&HitTarget::Button(ButtonKind::Confirm));
        assert!(matches!(update.cmds[0], Cmd::Rpc { .. }));
        assert!(matches!(app.mode, Mode::List));
    }

    // A click landing on a pane target behind the popup dismisses the form (same
    // as esc / clicking empty space), matching route_def_pick_click and the menu
    // router. The Modal body stays inert.
    #[test]
    fn def_args_click_on_pane_target_behind_form_cancels() {
        use crate::hit::HitTarget;
        let mut app = def_args_app(vec![ArgSpec { name: "pr".into(), default: None, options: None, description: None }], HashMap::new(), None);
        let update = app.def_args_click(&HitTarget::Row(ListPane::Queue, 0));
        assert!(update.dirty);
        assert!(matches!(app.mode, Mode::List));
    }

    // Drive a click through the REAL rendered hit map (not a hand-built target):
    // render the form, find the [ Run ] button rect, click its center, and assert
    // the geometry routes to a submit.
    #[test]
    fn def_args_run_button_click_through_rendered_hitmap_submits() {
        use crate::hit::{ButtonKind, HitTarget};
        use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
        use ratatui::{Terminal, backend::TestBackend};
        let mut app = def_args_app(vec![ArgSpec { name: "pr".into(), default: None, options: None, description: None }], HashMap::new(), None);
        let (w, h) = app.size;
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| { app.hit = crate::view::render(&app, f); }).unwrap();
        // Fill the required field, then click the real Run button rect.
        app.update(Event::Key(KeyEvent::new(KeyCode::Char('7'), KeyModifiers::NONE)));
        let run = app
            .hit
            .iter()
            .find(|(_, t)| matches!(t, HitTarget::Button(ButtonKind::Confirm)))
            .map(|(r, _)| *r)
            .expect("rendered form registers a Run button");
        let ev = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: run.x + run.width / 2,
            row: run.y,
            modifiers: KeyModifiers::NONE,
        });
        let update = app.update(ev);
        assert!(matches!(update.cmds[0], Cmd::Rpc { .. }));
        assert!(matches!(app.mode, Mode::List));
    }
}

#[cfg(test)]
mod task21_tests {
    use super::*;
    use crate::action_menu::MenuAction;
    use crate::ipc::types::{
        ArgSpec, DefinitionSummary, Project, StateSnapshot, TaskInstance, TaskStatus, WorktreeInfo,
    };
    use crate::keymap::AppAction;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::collections::HashMap;

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn arg(name: &str) -> ArgSpec {
        ArgSpec { name: name.into(), default: None, options: None, description: None }
    }

    fn fixture_app_one_project(name: &str) -> App {
        let mut app = App::new("/tmp/runs".into(), "/tmp/daemon.sock".into());
        app.size = (120, 40);
        app.snapshot = Some(StateSnapshot {
            projects: vec![Project { name: name.into() }],
            ..Default::default()
        });
        app.connected = true;
        app
    }

    fn fixture_app_with_defs(repo: &str, defs: Vec<DefinitionSummary>) -> App {
        let mut app = fixture_app_one_project(repo);
        app.defs_by_project.insert(repo.into(), defs);
        app
    }

    fn fixture_create_worktree(repo: &str) -> App {
        let mut app = fixture_app_one_project(repo);
        app.mode = Mode::CreateWorktree { input: tui_input::Input::default(), error: None };
        app
    }

    /// One project, worktrees pane focused (so `c` opens the create modal).
    fn fixture_app_worktrees_focused(repo: &str) -> App {
        let mut app = fixture_app_one_project(repo);
        app.set_focus(PaneId::Worktrees);
        app
    }

    /// One project with a single worktree that has a running task on its lane
    /// (→ `WtState::Busy`), worktrees pane focused with the busy row selected.
    fn fixture_app_busy_worktree(repo: &str, wt: &str) -> App {
        let mut app = fixture_app_one_project(repo);
        let raw = format!("{repo}.{wt}");
        let mut worktrees = HashMap::new();
        worktrees.insert(
            repo.to_string(),
            vec![WorktreeInfo {
                name: raw.clone(),
                path: format!("/wt/{wt}"),
                branch: "feat-x".into(),
                ..Default::default()
            }],
        );
        let mut running = TaskInstance::default();
        running.id = "01RUN".into();
        running.status = TaskStatus::Running;
        running.target.repo = repo.into();
        running.target.worktree = Some(raw);
        app.snapshot = Some(StateSnapshot {
            projects: vec![Project { name: repo.into() }],
            worktrees,
            tasks: vec![running],
            ..Default::default()
        });
        app.set_focus(PaneId::Worktrees);
        app
    }

    // --- create-worktree flow ---
    #[test]
    fn create_worktree_entered_by_c_in_worktrees_pane() {
        let mut app = fixture_app_worktrees_focused("platform");
        app.update(key(KeyCode::Char('c')));
        assert!(matches!(app.mode, Mode::CreateWorktree { .. }));
    }

    #[test]
    fn create_worktree_invalid_stays_open_with_error() {
        let mut app = fixture_create_worktree("platform");
        for c in "bad name".chars() {
            app.update(key(KeyCode::Char(c)));
        }
        let update = app.update(key(KeyCode::Enter));
        assert!(update.cmds.is_empty());
        match &app.mode {
            Mode::CreateWorktree { error, input } => {
                assert!(error.as_deref().unwrap().contains("whitespace"));
                assert_eq!(input.value(), "bad name"); // input preserved
            }
            other => panic!("expected CreateWorktree, got {other:?}"),
        }
    }

    #[test]
    fn create_worktree_valid_dispatches_and_closes_immediately() {
        let mut app = fixture_create_worktree("platform");
        for c in "feature-x".chars() {
            app.update(key(KeyCode::Char(c)));
        }
        let update = app.update(key(KeyCode::Enter));
        assert!(matches!(app.mode, Mode::List)); // closes immediately (fires async)
        assert_eq!(app.status_line.as_deref(), Some("creating worktree feature-x…"));
        match &update.cmds[0] {
            Cmd::Rpc { call, timeout_ms, timeout_is_ok, invalidate_defs_for, .. } => {
                assert_eq!(call.method, "createWorktree");
                assert_eq!(call.params["repo"], "platform");
                assert_eq!(call.params["name"], "feature-x");
                assert_eq!(*timeout_ms, 600_000);
                assert!(!*timeout_is_ok);
                assert!(invalidate_defs_for.is_none());
            }
            other => panic!("expected createWorktree, got {other:?}"),
        }
    }

    #[test]
    fn create_worktree_esc_cancels() {
        let mut app = fixture_create_worktree("platform");
        let update = app.update(key(KeyCode::Esc));
        assert!(update.cmds.is_empty());
        assert!(matches!(app.mode, Mode::List));
    }

    #[test]
    fn create_worktree_outside_click_cancels_and_keys_stay_out_of_field() {
        use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
        use ratatui::{Terminal, backend::TestBackend};
        let mut app = fixture_create_worktree("platform");
        let (w, h) = app.size;
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| app.hit = crate::view::render(&app, f)).unwrap();
        // Click the top-left corner — outside the centered modal → cancels.
        let ev = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        let update = app.update(ev);
        assert!(update.cmds.is_empty());
        assert!(matches!(app.mode, Mode::List));
    }

    // --- squash-merge flow ---
    #[test]
    fn squash_merge_opens_def_args_with_source_fixed_to_branch() {
        let mut app = fixture_app_with_defs(
            "platform",
            vec![DefinitionSummary {
                repo: "platform".into(),
                name: "squash-merge".into(),
                scope: "global".into(),
                args: vec![
                    arg("source"),
                    ArgSpec { default: Some("main".into()), ..arg("target") },
                ],
                has_discovery: false,
                cron: None,
                description: None,
            }],
        );
        let update = app.execute_menu_action(MenuAction::SquashMerge { branch: "wt-a".into() });
        assert!(update.cmds.is_empty());
        match &app.mode {
            Mode::DefArgs { form } => {
                assert_eq!(form.def_name, "squash-merge");
                assert_eq!(form.fixed.get("source").map(String::as_str), Some("wt-a"));
                assert_eq!(form.values[0], "wt-a"); // source fixed
                assert_eq!(form.values[1], "main"); // target editable default
                assert_eq!(form.initial_worktree, None); // def's `worktree: repo` governs
            }
            other => panic!("expected DefArgs, got {other:?}"),
        }
    }

    #[test]
    fn squash_merge_absent_def_sets_status_line() {
        let mut app = fixture_app_with_defs("platform", vec![]);
        let update = app.execute_menu_action(MenuAction::SquashMerge { branch: "wt-a".into() });
        assert!(update.cmds.is_empty());
        assert!(app.status_line.as_deref().unwrap().contains("squash-merge definition not found"));
        assert!(matches!(app.mode, Mode::List));
    }

    // --- busy worktree menu eligibility (regression) ---
    #[test]
    fn busy_worktree_remove_menu_row_is_disabled() {
        let mut app = fixture_app_busy_worktree("platform", "wt-a");
        app.apply_action(AppAction::OpenActionMenu); // select busy worktree, open menu
        let items = match &app.mode {
            Mode::ActionMenu { items, .. } => items.clone(),
            other => panic!("expected ActionMenu, got {other:?}"),
        };
        let remove = items
            .iter()
            .find(|it| it.label.starts_with("Remove worktree"))
            .expect("remove row present");
        assert_eq!(remove.disabled.as_deref(), Some("a task is running here"));
        // squash-merge is likewise disabled while busy.
        let squash = items.iter().find(|it| it.label.starts_with("Squash merge")).unwrap();
        assert_eq!(squash.disabled.as_deref(), Some("a task is running here"));
    }
}

#[cfg(test)]
mod heal_wiring_tests {
    use super::*;
    use crate::event::{Cmd, Event};
    use crate::ipc::types::StateSnapshot;
    use serial_test::serial;
    use std::path::PathBuf;

    // The env-mutating tests below touch process-global `QUEOHOH_DAEMON_DIST`.
    // `#[serial]` (serial_test's global lock, default group) excludes them not
    // only against each other but against `paths.rs`'s `#[serial]` env tests,
    // which mutate the same var — a local Mutex would not cross that boundary.

    fn dist_with_js() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("cli.js"), "x").unwrap();
        d
    }

    fn app() -> App {
        App::new(std::env::temp_dir(), PathBuf::from("/tmp/qoo-nope.sock"))
    }

    fn snap(build_id: Option<&str>, running: Vec<String>) -> StateSnapshot {
        StateSnapshot {
            build_id: build_id.map(str::to_string),
            running,
            ..Default::default()
        }
    }

    #[test]
    #[serial]
    fn restart_now_sets_status_records_guard_and_emits_heal() {
        let dist = dist_with_js();
        unsafe { std::env::set_var("QUEOHOH_DAEMON_DIST", dist.path()) };
        let disk = crate::heal::disk_build_id(dist.path());

        let mut app = app();
        let upd = app.update(Event::Snapshot(snap(Some("stale-build"), vec![])));

        assert_eq!(app.status_line.as_deref(), Some("daemon outdated — restarting…"));
        assert_eq!(app.last_healed_build_id.as_deref(), Some(disk.as_str()));
        assert!(upd.cmds.iter().any(|c| matches!(c, Cmd::Heal)));
        unsafe { std::env::remove_var("QUEOHOH_DAEMON_DIST") };
    }

    #[test]
    #[serial]
    fn defer_when_task_running_no_heal_cmd() {
        let dist = dist_with_js();
        unsafe { std::env::set_var("QUEOHOH_DAEMON_DIST", dist.path()) };

        let mut app = app();
        let upd = app.update(Event::Snapshot(snap(Some("stale-build"), vec!["t1".into()])));

        assert_eq!(app.status_line.as_deref(), Some("daemon outdated — will restart when idle"));
        assert!(app.last_healed_build_id.is_none());
        assert!(!upd.cmds.iter().any(|c| matches!(c, Cmd::Heal)));
        unsafe { std::env::remove_var("QUEOHOH_DAEMON_DIST") };
    }

    #[test]
    #[serial]
    fn declined_loop_guard_says_restart_manually() {
        let dist = dist_with_js();
        unsafe { std::env::set_var("QUEOHOH_DAEMON_DIST", dist.path()) };
        let disk = crate::heal::disk_build_id(dist.path());

        let mut app = app();
        app.last_healed_build_id = Some(disk); // already attempted this build
        let upd = app.update(Event::Snapshot(snap(Some("stale-build"), vec![])));

        assert_eq!(app.status_line.as_deref(), Some("daemon still outdated — restart it manually"));
        assert!(!upd.cmds.iter().any(|c| matches!(c, Cmd::Heal)));
        unsafe { std::env::remove_var("QUEOHOH_DAEMON_DIST") };
    }

    #[test]
    #[serial]
    fn healthy_snapshot_resets_guard_and_clears_heal_status() {
        let dist = dist_with_js();
        unsafe { std::env::set_var("QUEOHOH_DAEMON_DIST", dist.path()) };
        let disk = crate::heal::disk_build_id(dist.path());

        let mut app = app();
        // Simulate mid-flight state left by a prior restart-now.
        app.last_healed_build_id = Some("old-attempt".into());
        app.healing = true;
        app.heal_status_shown = true;
        app.status_line = Some("daemon outdated — restarting…".into());

        app.update(Event::Snapshot(snap(Some(&disk), vec![])));

        assert!(app.last_healed_build_id.is_none());
        assert!(!app.healing);
        assert!(!app.heal_status_shown);
        assert!(app.status_line.is_none());
        unsafe { std::env::remove_var("QUEOHOH_DAEMON_DIST") };
    }

    #[test]
    fn action_result_during_heal_resets_healing_and_owns_status() {
        let mut app = app();
        app.healing = true; // a Cmd::Heal is in flight
        app.update(Event::ActionResult {
            status: Some("daemon busy — restart deferred".into()),
            invalidate_defs_for: None,
        });
        assert_eq!(app.status_line.as_deref(), Some("daemon busy — restart deferred"));
        assert!(!app.healing);
        assert!(app.heal_status_shown); // heal-owned → cleared by next healthy snapshot
    }
}
