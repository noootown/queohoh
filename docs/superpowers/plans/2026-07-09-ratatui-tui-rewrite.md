# ratatui TUI Rewrite Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rewrite the qoo TUI as a Rust/ratatui binary (`qoo-tui`) with a fully clickable mouse UI, replacing the Ink TUI after parity — per the approved spec at `docs/superpowers/specs/2026-07-09-ratatui-tui-rewrite-design.md`.

**Architecture:** Elm-style single-threaded core: one `Event` enum enters through a tokio mpsc channel (`tokio::select!` over crossterm's `EventStream` + injected events); `App::update(event) -> Update { dirty, cmds }` is a pure state transition; a runtime executor performs `Cmd`s (RPC calls, file reads, process spawns) on tokio tasks and feeds results back as events; `view::render(&app, frame) -> HitMap` draws and returns this frame's hit-test geometry for mouse routing. The daemon, wire protocol, and Ink TUI are untouched.

**Tech Stack:** Rust (edition 2024, stable), ratatui 0.29 + crossterm 0.29, tokio 1.x, serde/serde_json, tui-input, throbber-widgets-tui, insta (dev).

## Global Constraints

- **Do not modify** `packages/tui`, `packages/daemon`, `packages/core`, or the wire protocol. No new RPC methods. The Ink TUI keeps working throughout (parallel until parity).
- Crate lives at `crates/qoo-tui` (package + binary name `qoo-tui`), with a root `Cargo.toml` workspace (`members = ["crates/qoo-tui"]`). Lib + bin split: `src/lib.rs` exposes modules, `src/main.rs` is thin.
- Rust edition 2024, stable toolchain. Dependencies added via `cargo add` (pin whatever resolves); assume ratatui 0.29 / crossterm 0.29 / tui-input 0.14 APIs in plan code — **implementers must check docs.rs for the resolved version and adapt mechanically**. If `tui-popup`/`tui-confirm-dialog` are incompatible with the resolved ratatui, hand-roll (a popup is `Clear` + `Block` + centered `Rect`; the plan's modal code already does this).
- Wire JSON is camelCase → every serde struct uses `#[serde(rename_all = "camelCase", default)]`. **Never crash on old/malformed snapshots** — every field lenient-defaults (spec: `normalizeSnapshot` parity).
- **Zero idle renders:** draw only when `update()` returns `dirty: true`. The 1s `Tick` interval is armed only while the active project has a running task.
- Minimum terminal 60×15; smaller renders only `terminal too small (60x15 minimum)`.
- All colors and status glyphs come from `view/theme.rs` (`Palette` + glyph consts) — no inline color/glyph literals in components.
- Mouse capture always-on; terminal restore (mouse off → alt-screen exit) must run on every exit path: `Drop` guard + panic hook + SIGINT/SIGTERM handler.
- Daemon paths mirror `packages/daemon/src/paths.ts`: state dir = `$QUEOHOH_STATE_DIR` or `~/.local/state/queohoh`; socket `<state>/daemon/daemon.sock`; pidfile `<state>/daemon/daemon.pid`; runs dir `<state>/runs`. Daemon dist dir for self-heal = `$QUEOHOH_DAEMON_DIST` or `<repo>/packages/daemon/dist` derived from `env!("CARGO_MANIFEST_DIR") + "/../../packages/daemon/dist"`.
- TDD per task; conventional commits (`feat(tui-rs): …`, `test(tui-rs): …`); **no Co-Authored-By trailers**.
- Run tests with `cargo test -p qoo-tui`; the workspace must also keep passing `cargo build --release`.
- The TS test suites in `packages/tui/src/__tests__/` are the parity oracle — when a task ports a pure module, mirror the relevant TS test cases (inputs and expected outputs), not just the implementation.

---

## File Structure

```
Cargo.toml                      # workspace root: members = ["crates/qoo-tui"]
crates/qoo-tui/
  Cargo.toml
  src/
    main.rs                     # tokio main: terminal guard, event loop, draw loop
    lib.rs                      # pub mod declarations
    app.rs                      # App state, Mode, TabUiState, update()
    event.rs                    # Event + Cmd enums, event-loop + executor helpers
    keymap.rs                   # KeyEvent -> AppAction (list mode, direct keys)
    hit.rs                      # HitTarget, HitMap (register + reverse-order hit test)
    selectors.rs                # tabs, queue/worktree rows, layout math, windowing, titles, filters
    detail.rs                   # DetailContext derivation, sub-tabs, anchoring, line windowing
    markup.rs                   # markdown-lite line -> styled ratatui Line
    runfiles.rs                 # transcript tail + report reading
    heal.rs                     # decide_heal (pure) + perform_heal (async)
    paths.rs                    # state/socket/pid/runs/daemon-dist path resolution
    worktree_context.rs         # source/branch/ticket auto-fill (M3)
    action_menu.rs              # ActionItem builders per pane + bulk (M2)
    ipc/
      mod.rs
      types.rs                  # StateSnapshot etc. (serde, lenient)
      client.rs                 # NDJSON JSON-RPC: RpcClient (short-lived) + spawn_subscription
    view/
      mod.rs                    # render(&App, &mut Frame) -> HitMap; layout compute
      theme.rs                  # Palette + glyph consts
      tabbar.rs                 # header: tabs + connection + running counter
      panes.rs                  # QUEUE / TASKS / WORKTREES list panes (+ scrollbars)
      detail.rs                 # detail pane render (sub-tab chips, content, scrollbar)
      footer.rs                 # context-sensitive hints / status line
      modal.rs                  # popup geometry, backdrop, text-input modals, confirms
      menu.rs                   # action-menu / def-pick list popups (M2/M3)
      args_form.rs              # args form + dropdown popup (M3)
      help.rs                   # `?` keymap overlay
  tests/                        # integration tests (fake daemon socket, snapshot tests)
```

## Shared Type Contract

Every task uses these exact names and shapes. Tasks define them in the files noted; later tasks consume them verbatim. (Fields may gain private helpers, but public shapes below are fixed.)

```rust
// ipc/types.rs  — wire mirrors (all #[derive(Debug, Clone, PartialEq, Deserialize)] + Default, camelCase, lenient)
pub struct StateSnapshot {
    pub tasks: Vec<TaskInstance>,
    pub archived_recent: Vec<TaskInstance>,
    pub sessions: Vec<SessionEntry>,
    pub running: Vec<String>,
    pub max_concurrent: Option<u32>,
    pub projects: Vec<Project>,                          // Project { name: String }
    pub worktrees: HashMap<String, Vec<WorktreeInfo>>,   // keyed by repo
    pub main_sessions: HashMap<String, String>,          // lane "repo:worktree" -> session id
    pub build_id: Option<String>,                        // None => stale (self-heal)
}
pub enum TaskStatus { Queued, NeedsInput, Running, Done, Failed, #[serde(other)] Unknown }  // kebab-case wire
pub struct TaskInstance { pub id: String, pub status: TaskStatus, pub definition: Option<String>,
    pub item: Option<HashMap<String, String>>, pub item_key: Option<String>, pub target: TaskTarget,
    pub priority: String, pub created: String, pub source: String, pub ephemeral_worktree: bool,
    pub error: Option<String>, pub session: String /* "fresh"|"main" */, pub resume_session_id: Option<String>,
    pub model: Option<String>, pub prompt: String }
pub struct TaskTarget { pub repo: String, #[serde(rename = "ref")] pub git_ref: String, pub worktree: Option<String> }
pub struct SessionEntry { pub kind: String /* "worker"|"interactive" */, pub key: String, pub lane: Option<String>,
    pub cwd: Option<String>, pub pid: Option<u32>, pub started_at: String, pub heartbeat_at: String }
pub struct WorktreeInfo { pub name: String, pub path: String, pub branch: String }
pub struct ArgSpec { pub name: String, pub default: Option<String>, pub options: Option<Vec<String>>, pub description: Option<String> }
pub struct DefinitionSummary { pub repo: String, pub name: String, pub scope: String /* "project"|"global" */,
    pub args: Vec<ArgSpec>, pub has_discovery: bool }
pub struct TaskDefinition { pub name: String, pub repo: String, pub discovery: Option<Discovery>,
    pub args: Vec<ArgSpec>, pub dedup: String, pub worktree: String, pub pre_run: Option<String>,
    pub post_run: Option<String>, pub model: String, pub timeout_ms: u64, pub priority: String, pub prompt: String }
pub struct Discovery { pub command: String, pub item_key: String }

// event.rs
pub enum Event {
    Key(crossterm::event::KeyEvent), Mouse(crossterm::event::MouseEvent), Resize,
    Snapshot(StateSnapshot), Disconnected, Tick,
    RunFiles { task_id: String, files: RunFiles },
    Definitions { repo: String, defs: Vec<DefinitionSummary> },
    Definition { repo: String, name: String, def: Option<TaskDefinition> },
    ActionResult { status: Option<String>, invalidate_defs_for: Option<String> },
}
pub struct RpcCall { pub method: String, pub params: serde_json::Value }
pub enum Cmd {
    Rpc { label: String, call: RpcCall, timeout_ms: u64, timeout_is_ok: bool, invalidate_defs_for: Option<String> },
    RpcSeq { verb: String /* past tense e.g. "reran" */, calls: Vec<RpcCall>, invalidate_defs_for: Option<String> },
    FetchDefinitions { repo: String }, FetchDefinition { repo: String, name: String },
    ReadRunFiles { task_id: String, tail_lines: usize, delay_ms: u64 },
    OpenTmux { path: String }, Heal, Quit,
}

// app.rs
pub enum PaneId { Queue, Tasks, Worktrees, Detail }
pub enum ListPane { Queue = 0, Tasks = 1, Worktrees = 2 }   // ListPane::idx(self) -> usize
pub struct Selection { pub cursor: usize, pub anchor: Option<usize> }
pub enum DetailKind { Run = 0, Definition = 1, Worktree = 2, Empty = 3 }
pub struct TabUiState { pub focus: PaneId, pub last_list_pane: ListPane,
    pub selections: [Selection; 3], pub search: [String; 3], pub sub_tab: [usize; 4], pub scroll_offset: usize }
pub enum SessionMode { Fresh, Main }
pub enum Mode {
    List, Search { pane: ListPane }, Help,
    AddTask { worktree: Option<String>, session: SessionMode, input: tui_input::Input },
    WorktreeInput { task_id: String, input: tui_input::Input },
    CreateWorktree { input: tui_input::Input, error: Option<String> },
    DefPick { defs: Vec<DefinitionSummary>, index: usize, worktree: Option<String>, branch: Option<String> },
    DefArgs { form: crate::view::args_form::ArgsForm },
    ActionMenu { title: String, items: Vec<crate::action_menu::ActionItem>, index: usize },
    ConfirmRemove { repo: String, worktree: String, branch: String },
    ConfirmBulkRemove { repo: String, names: Vec<String> },
}
pub struct App {  // public fields; constructed with App::new(runs_dir, sock_path)
    pub snapshot: Option<StateSnapshot>, pub connected: bool,
    pub active_tab: usize, pub ui_by_tab: HashMap<String, TabUiState>,
    pub mode: Mode, pub status_line: Option<String>,
    pub run_files: Option<(String /* task_id */, RunFiles)>,
    pub defs_by_project: HashMap<String, Vec<DefinitionSummary>>,
    pub full_defs: HashMap<String /* "repo/name" */, TaskDefinition>,
    pub now_epoch_s: u64, pub size: (u16, u16), pub hit: HitMap,
    pub last_healed_build_id: Option<String>,
    pub sock_path: PathBuf, pub runs_dir: PathBuf,
}
pub struct Update { pub dirty: bool, pub cmds: Vec<Cmd> }
impl App { pub fn update(&mut self, event: Event) -> Update; }

// keymap.rs
pub enum AppAction { MoveCursor(i32), ExtendSelection(i32), FocusPane(PaneId), CyclePane(i32),
    SwitchTab(usize), CycleTab(i32), OpenActionMenu, Create, OpenSearch, ClearEsc,
    Scroll(i32), ScrollEdge(i32), SwitchSubTab(usize), Help, Quit, None }
pub fn list_mode_action(key: &crossterm::event::KeyEvent, focus: PaneId) -> AppAction;

// hit.rs
pub enum ButtonKind { Confirm, Cancel }
pub enum HitTarget { Tab(usize), Row(ListPane, usize), PaneBody(PaneId), SubTab(usize),
    MenuItem(usize), FormField(usize), DropdownItem(usize), Button(ButtonKind),
    ScrollbarThumb(PaneId), ScrollbarTrack(PaneId), Modal }
pub struct HitMap { /* Vec<(Rect, HitTarget)> */ }
impl HitMap { pub fn push(&mut self, rect: Rect, target: HitTarget);
    pub fn hit(&self, col: u16, row: u16) -> Option<&HitTarget>; /* reverse registration order = topmost first */ }

// view/mod.rs
pub fn render(app: &App, frame: &mut ratatui::Frame) -> HitMap;

// selectors.rs (pure; mirrors selectors.ts)
pub struct TabInfo { pub name: String, pub synthetic: bool }
pub fn build_tabs(snapshot: &StateSnapshot) -> Vec<TabInfo>;
pub struct QueueRow { pub task_id: String, pub glyph: char, pub running: bool, pub main_session: bool,
    pub lane: String, pub summary: String, pub detail: String, pub archived: bool }
pub fn queue_rows(snapshot: &StateSnapshot, project: &str, now_epoch_s: u64) -> Vec<QueueRow>;
pub enum WtState { Free, Busy, You, Failed }
pub struct WorktreeRow { pub name: String, pub raw_name: String, pub path: String, pub branch: String,
    pub state: WtState, pub has_main_session: bool, pub queued: usize, pub is_session: bool }
pub fn worktree_rows(snapshot: &StateSnapshot, project: &str) -> Vec<WorktreeRow>;
pub struct PaneLayout { pub queue_h: u16, pub tasks_h: u16, pub worktrees_h: u16 }
pub fn pane_layout(body_height: u16) -> PaneLayout;   // list_h = max(4, body/4); queue_h = max(4, body - 2*list_h)
pub fn window_rows(len: usize, cursor: usize, capacity: usize) -> (usize, usize);  // centered window
pub fn pane_title(base: &str, sel: &Selection, filter: &str, searching: bool) -> String;
pub fn filter_rows<'a, T>(rows: &'a [T], filter: &str, text_of: impl Fn(&T) -> String) -> Vec<usize>; // indices of matches
pub fn arg_summary(args: &[ArgSpec]) -> String;       // "pr, mode=ready, review=auto"
pub fn strip_repo_prefix<'a>(worktree: &'a str, repo: &str) -> &'a str;
pub fn lane_key(repo: &str, worktree: &str) -> String; // "repo:worktree"
pub fn prompt_summary(prompt: &str) -> String;         // first non-blank line, ≤60 chars + …
pub fn elapsed_label(created_epoch_s: u64, now_epoch_s: u64) -> String; // "⏱ 5m03s"

// detail.rs
pub enum DetailContext { Run { task: TaskInstance }, Definition { repo: String, name: String },
    Worktree { row: WorktreeRow, lane_tasks: Vec<TaskInstance> }, Empty }
pub fn derive_context(snapshot: &StateSnapshot, project: &str, last: ListPane,
    queue: &[QueueRow], wt: &[WorktreeRow], defs: &[DefinitionSummary], sel: &[Selection; 3]) -> DetailContext;
pub fn sub_tab_names(kind: DetailKind) -> &'static [&'static str];
pub fn clamp_sub_tab(idx: usize, kind: DetailKind) -> usize;
pub fn bottom_anchored(kind: DetailKind, sub_tab: usize) -> bool;   // only Run/transcript
pub fn window_lines(total: usize, height: usize, offset: usize, bottom: bool) -> (usize, usize);

// markup.rs
pub fn style_line(line: &str, p: &Palette) -> ratatui::text::Line<'static>;

// runfiles.rs
pub struct RunFiles { pub transcript_tail: Vec<String>, pub report: Vec<String> }  // PartialEq for dedup
pub async fn read_run_files(runs_dir: &Path, task_id: &str, tail_lines: usize) -> RunFiles;

// heal.rs
pub enum HealDecision { None, Defer, RestartNow }
pub fn decide_heal(snapshot_build_id: Option<&str>, disk_build_id: &str, running: usize,
    last_healed: Option<&str>) -> HealDecision;
pub fn disk_build_id(dist_dir: &Path) -> String;  // max mtime-ms of *.js, "0" on none/error
pub async fn perform_heal(sock: &Path, pid_file: &Path, daemon_cli: &Path) -> Result<(), String>;

// paths.rs
pub fn state_path() -> PathBuf; pub fn socket_path(state: &Path) -> PathBuf;
pub fn pid_path(state: &Path) -> PathBuf; pub fn runs_path(state: &Path) -> PathBuf;
pub fn daemon_dist_dir() -> PathBuf; pub fn daemon_cli_path() -> PathBuf;

// ipc/client.rs
pub struct RpcClient;  // short-lived, one connection
impl RpcClient { pub async fn connect(sock: &Path) -> io::Result<Self>;
    pub async fn call(&mut self, method: &str, params: serde_json::Value, timeout: Duration)
        -> Result<serde_json::Value, String>; }
pub fn spawn_subscription(sock: PathBuf, tx: tokio::sync::mpsc::UnboundedSender<Event>) -> tokio::task::JoinHandle<()>;

// action_menu.rs (M2)
pub struct ActionItem { pub label: String, pub disabled: Option<String>, pub action: MenuAction }
pub enum MenuAction { Rerun { id: String }, Skip { id: String }, AssignWorktree { id: String },
    TaskFresh { worktree: Option<String> }, TaskMain { worktree: Option<String> },
    RunDef { worktree: Option<String>, branch: Option<String> },
    RunNamedDef { repo: String, name: String },
    OpenTmux { path: String }, RemoveWorktree { repo: String, name: String, branch: String },
    CreateWorktree, SquashMerge { branch: String },
    BulkRerun { ids: Vec<String> }, BulkSkip { ids: Vec<String> },
    BulkRunDefs { repo: String, names: Vec<String> }, BulkRemove { repo: String, names: Vec<String> } }

// worktree_context.rs (M3)
pub fn extract_ticket(branch: &str) -> Option<String>;
pub fn context_arg_values(branch: &str) -> HashMap<String, String>;   // keys among {"source","branch","ticket"}
pub fn ambient_run_args(args: &[ArgSpec], worktrees: &[WorktreeRow], selected: Option<&WorktreeRow>)
    -> (Vec<ArgSpec> /* with injected options */, HashMap<String, String> /* initial */);

// view/args_form.rs (M3)
pub struct ArgsForm { pub repo: String, pub def_name: String, pub args: Vec<ArgSpec>,
    pub values: Vec<String>, pub fixed: HashMap<String, String>, pub initial_worktree: Option<String>,
    pub focus: usize, pub error: Option<usize>, pub dropdown: Option<usize /* highlighted option */> }
```

## Task Index

- **M1 — read-only viewer:** 1 scaffold+terminal guard · 2 ipc types · 3 ipc client · 4 event loop + executor · 5 selectors · 6 markup — then — 7 theme+hit · 8 tabbar/panes/footer · 9 detail · 10 runfiles · 11 keymap+search+help · 12 mouse + M1 integration
- **M2 — actions:** 13 RPC executor + status line · 14 action menus (single) · 15 add-task & assign-worktree modals · 16 bulk selection/menus/execution
- **M3 — forms & worktrees:** 17 worktree_context · 18 def-pick + defs cache · 19 args-form logic · 20 args-form view + dropdowns · 21 create/remove worktree + squash-merge + tmux
- **M4 — self-heal & cutover:** 22 heal · 23 end-to-end verification + launcher flip · 24 cutover (delete packages/tui; user-gated)

---
## Milestone 1 — Read-only viewer (Tasks 1–12)

> **Additive-extension convention (whole milestone):** the Shared Type Contract in the plan skeleton is the target shape. Because tasks land in dependency order, an early task declares the *subset* of a struct/enum it can compile, and a later task *adds* fields/variants — never renames or re-types the fields already present. Each additive point is called out at the site (e.g. "Task 7 adds `App::hit`", "Task 14 adds `Mode::ActionMenu`"). Every task still ends green and committed.

---

### Task 1: Workspace scaffold + terminal guard

**Files:**
- Create `Cargo.toml` (workspace root)
- Create `crates/qoo-tui/Cargo.toml`
- Create `crates/qoo-tui/src/lib.rs`
- Create `crates/qoo-tui/src/main.rs`
- Create `crates/qoo-tui/src/paths.rs`
- Modify `.mise.toml` (append two task blocks)
- Test: `crates/qoo-tui/src/paths.rs` (inline `#[cfg(test)] mod tests`)

**Interfaces:**
- Produces `qoo_tui::paths::state_path() -> PathBuf`, `socket_path(&Path) -> PathBuf`, `pid_path(&Path) -> PathBuf`, `runs_path(&Path) -> PathBuf`, `daemon_dist_dir() -> PathBuf`, `daemon_cli_path() -> PathBuf` (exact contract signatures).
- Produces the `qoo-tui` binary (terminal guard + placeholder frame).
- Consumes: nothing (leaf task).

**Steps:**

- [ ] **Step 1: Create the workspace + crate manifests and lib/bin skeleton.** Write root `Cargo.toml`:

```toml
[workspace]
resolver = "3"
members = ["crates/qoo-tui"]
```

Write `crates/qoo-tui/Cargo.toml`:

```toml
[package]
name = "qoo-tui"
version = "0.1.0"
edition = "2024"

[lib]
name = "qoo_tui"
path = "src/lib.rs"

[[bin]]
name = "qoo-tui"
path = "src/main.rs"
```

Write `crates/qoo-tui/src/lib.rs` (grows one `pub mod` line per task):

```rust
pub mod paths;
```

Version note: `resolver = "3"` and `edition = "2024"` require cargo ≥ 1.85; the implementer runs `cargo --version` and, if older, sets `resolver = "2"` (edition 2024 still works, with a workspace-resolver warning).

- [ ] **Step 2: Add dependencies via cargo add.** Run inside `crates/qoo-tui`:

```
cargo add ratatui@0.29
cargo add crossterm@0.29 --features event-stream
cargo add tokio@1 --features rt-multi-thread,net,io-util,sync,time,macros,process,fs
cargo add serde@1 --features derive
cargo add serde_json@1
cargo add tui-input@0.14
cargo add throbber-widgets-tui
cargo add --dev insta
cargo add --dev tempfile
```

Version note: ratatui 0.29 re-exports crossterm; we still add `crossterm` explicitly for the `event-stream` feature (async `EventStream`). If `cargo add crossterm@0.29` resolves a crossterm whose major differs from ratatui's re-exported one, pin crossterm to match ratatui's `Cargo.lock` entry to avoid two crossterm copies (KeyEvent type mismatch).

- [ ] **Step 3 (RED): Write `paths.rs` with tests and unimplemented bodies.** Create `crates/qoo-tui/src/paths.rs`:

```rust
use std::path::{Path, PathBuf};

pub fn state_path() -> PathBuf {
    unimplemented!()
}
pub fn socket_path(_state: &Path) -> PathBuf {
    unimplemented!()
}
pub fn pid_path(_state: &Path) -> PathBuf {
    unimplemented!()
}
pub fn runs_path(_state: &Path) -> PathBuf {
    unimplemented!()
}
pub fn daemon_dist_dir() -> PathBuf {
    unimplemented!()
}
pub fn daemon_cli_path() -> PathBuf {
    unimplemented!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_path_honors_env_override() {
        // set_var is unsafe in edition 2024; env is process-global so we set and
        // restore within this single test.
        unsafe { std::env::set_var("QUEOHOH_STATE_DIR", "/tmp/qoo-state-xyz") };
        assert_eq!(state_path(), PathBuf::from("/tmp/qoo-state-xyz"));
        unsafe { std::env::remove_var("QUEOHOH_STATE_DIR") };
    }

    #[test]
    fn state_path_defaults_under_home_local_state() {
        unsafe { std::env::remove_var("QUEOHOH_STATE_DIR") };
        unsafe { std::env::set_var("HOME", "/home/tester") };
        assert_eq!(
            state_path(),
            PathBuf::from("/home/tester/.local/state/queohoh")
        );
    }

    #[test]
    fn derived_paths_hang_off_state() {
        let state = Path::new("/s");
        assert_eq!(socket_path(state), PathBuf::from("/s/daemon/daemon.sock"));
        assert_eq!(pid_path(state), PathBuf::from("/s/daemon/daemon.pid"));
        assert_eq!(runs_path(state), PathBuf::from("/s/runs"));
    }

    #[test]
    fn daemon_dist_honors_env_override() {
        unsafe { std::env::set_var("QUEOHOH_DAEMON_DIST", "/opt/dist") };
        assert_eq!(daemon_dist_dir(), PathBuf::from("/opt/dist"));
        assert_eq!(daemon_cli_path(), PathBuf::from("/opt/dist/cli.js"));
        unsafe { std::env::remove_var("QUEOHOH_DAEMON_DIST") };
    }
}
```

Run `cargo test -p qoo-tui paths` → **expected FAIL** (each test panics on `unimplemented!()`).

Note: the two env-mutating tests (`state_path_*`) touch process-global `HOME`/`QUEOHOH_STATE_DIR`; run the suite single-threaded for this module if flakes appear (`cargo test -p qoo-tui paths -- --test-threads=1`). They restore/remove what they set.

- [ ] **Step 4 (GREEN): Implement the path functions.** Replace the six bodies in `paths.rs`:

```rust
pub fn state_path() -> PathBuf {
    if let Ok(dir) = std::env::var("QUEOHOH_STATE_DIR") {
        return PathBuf::from(dir);
    }
    let home = std::env::var("HOME").unwrap_or_default();
    Path::new(&home).join(".local/state/queohoh")
}

pub fn socket_path(state: &Path) -> PathBuf {
    state.join("daemon/daemon.sock")
}

pub fn pid_path(state: &Path) -> PathBuf {
    state.join("daemon/daemon.pid")
}

pub fn runs_path(state: &Path) -> PathBuf {
    state.join("runs")
}

pub fn daemon_dist_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("QUEOHOH_DAEMON_DIST") {
        return PathBuf::from(dir);
    }
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/daemon/dist")
}

pub fn daemon_cli_path() -> PathBuf {
    daemon_dist_dir().join("cli.js")
}
```

Run `cargo test -p qoo-tui paths` → **expected PASS** (4 tests).

- [ ] **Step 5: Write the terminal guard + placeholder `main.rs`.** Create `crates/qoo-tui/src/main.rs`:

```rust
use std::io::{self, Stdout};

use crossterm::event::{self, EnableMouseCapture, DisableMouseCapture, Event, KeyCode};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::widgets::Paragraph;
use ratatui::Terminal;

/// Owns the terminal's raw-mode + alt-screen + mouse-capture state and restores
/// all three on Drop, so every exit path (normal return, `?`, panic-after-unwind)
/// leaves the terminal usable and out of mouse-reporting mode.
struct TerminalGuard;

impl TerminalGuard {
    fn new() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut out = io::stdout();
        execute!(out, EnterAlternateScreen, EnableMouseCapture)?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let mut out = io::stdout();
        let _ = execute!(out, DisableMouseCapture, LeaveAlternateScreen);
        let _ = disable_raw_mode();
    }
}

/// Restore the terminal on panic *before* the default hook prints the message,
/// then chain to the previous hook so the backtrace still shows.
fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let mut out = io::stdout();
        let _ = execute!(out, DisableMouseCapture, LeaveAlternateScreen);
        let _ = disable_raw_mode();
        prev(info);
    }));
}

fn main() -> io::Result<()> {
    install_panic_hook();
    let _guard = TerminalGuard::new()?;
    let mut terminal: Terminal<CrosstermBackend<Stdout>> =
        Terminal::new(CrosstermBackend::new(io::stdout()))?;

    // Placeholder synchronous loop — Task 4 replaces this with the async
    // tokio::select! event loop + spawn_subscription.
    loop {
        terminal.draw(|f| {
            f.render_widget(
                Paragraph::new("qoo-tui — hello (press q to quit)"),
                f.area(),
            );
        })?;
        if let Event::Key(k) = event::read()? {
            if k.code == KeyCode::Char('q') {
                break;
            }
        }
    }
    Ok(())
}
```

Version note: ratatui 0.29 uses `Frame::area()` (renamed from `size()` in 0.28). `execute!` needs `use std::io::Write` in scope only when writing to a custom sink; `io::stdout()` satisfies the `Write` bound via crossterm's macro internally.

Manual check: `cargo run -p qoo-tui` shows the hello frame and `q` exits with the terminal restored (no leftover mouse escape codes). This binary has no automated test (terminal side effects); `paths` tests cover the unit-testable surface.

- [ ] **Step 6: Append the mise tasks.** Append to `.mise.toml`:

```toml
# --- rust tui (parallel to the Ink TUI until parity) ---

[tasks."tui:rs"]
description = "Build and launch the Rust ratatui TUI (release)"
run = [
	"cargo build --release -p qoo-tui",
	"./target/release/qoo-tui",
]

[tasks."tui:rs:dev"]
description = "Run the Rust ratatui TUI unoptimized (fast rebuild loop)"
run = "cargo run -p qoo-tui"
```

- [ ] **Step 7: Verify build + commit.** Run `cargo build --release -p qoo-tui` (**expected: compiles**) and `cargo test -p qoo-tui` (**expected: 4 pass**).

```
git add Cargo.toml crates/qoo-tui/Cargo.toml crates/qoo-tui/src/lib.rs crates/qoo-tui/src/main.rs crates/qoo-tui/src/paths.rs .mise.toml Cargo.lock
git commit -m "feat(tui-rs): workspace scaffold, path resolution, terminal guard"
```

---

### Task 2: `ipc/types.rs` — lenient wire mirrors

**Files:**
- Create `crates/qoo-tui/src/ipc/mod.rs`
- Create `crates/qoo-tui/src/ipc/types.rs`
- Modify `crates/qoo-tui/src/lib.rs` (add `pub mod ipc;`)
- Test: `crates/qoo-tui/src/ipc/types.rs` (inline `#[cfg(test)] mod tests`)

**Interfaces:**
- Produces (exact contract shapes): `StateSnapshot`, `TaskInstance`, `TaskStatus`, `TaskTarget`, `SessionEntry`, `WorktreeInfo`, `Project`, `ArgSpec`, `DefinitionSummary`, `TaskDefinition`, `Discovery` — all `#[derive(Debug, Clone, PartialEq, Deserialize, Default)]`, camelCase, lenient.
- Consumes: `serde`.

**Steps:**

- [ ] **Step 1: Register the module.** In `lib.rs` add below `pub mod paths;`:

```rust
pub mod ipc;
```

Create `crates/qoo-tui/src/ipc/mod.rs`:

```rust
pub mod types;
```

- [ ] **Step 2 (RED): Write `types.rs` with the structs + fixtures test.** Create `crates/qoo-tui/src/ipc/types.rs`:

```rust
use std::collections::HashMap;

use serde::{Deserialize, Deserializer};

/// `null` (or a missing field, via container `default`) → `T::default()`. Mirrors
/// `normalizeSnapshot`'s coercion of an old daemon's absent/nullish collections.
/// A *wrong-typed* value (e.g. a string where an array is expected) still errors;
/// the subscription's `unwrap_or_default()` is the crash-safety net for that
/// (the real daemon never sends wrong types — only missing fields on old builds).
fn nullable_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct StateSnapshot {
    #[serde(deserialize_with = "nullable_default")]
    pub tasks: Vec<TaskInstance>,
    #[serde(deserialize_with = "nullable_default")]
    pub archived_recent: Vec<TaskInstance>,
    #[serde(deserialize_with = "nullable_default")]
    pub sessions: Vec<SessionEntry>,
    #[serde(deserialize_with = "nullable_default")]
    pub running: Vec<String>,
    pub max_concurrent: Option<u32>,
    #[serde(deserialize_with = "nullable_default")]
    pub projects: Vec<Project>,
    #[serde(deserialize_with = "nullable_default")]
    pub worktrees: HashMap<String, Vec<WorktreeInfo>>,
    #[serde(deserialize_with = "nullable_default")]
    pub main_sessions: HashMap<String, String>,
    pub build_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Project {
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum TaskStatus {
    Queued,
    NeedsInput,
    Running,
    Done,
    Failed,
    #[default]
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct TaskInstance {
    pub id: String,
    pub status: TaskStatus,
    pub definition: Option<String>,
    pub item: Option<HashMap<String, String>>,
    pub item_key: Option<String>,
    pub target: TaskTarget,
    pub priority: String,
    pub created: String,
    pub source: String,
    pub ephemeral_worktree: bool,
    pub error: Option<String>,
    pub session: String,
    pub resume_session_id: Option<String>,
    pub model: Option<String>,
    pub prompt: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct TaskTarget {
    pub repo: String,
    #[serde(rename = "ref")]
    pub git_ref: String,
    pub worktree: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct SessionEntry {
    pub kind: String,
    pub key: String,
    pub lane: Option<String>,
    pub cwd: Option<String>,
    pub pid: Option<u32>,
    pub started_at: String,
    pub heartbeat_at: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct WorktreeInfo {
    pub name: String,
    pub path: String,
    pub branch: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ArgSpec {
    pub name: String,
    pub default: Option<String>,
    pub options: Option<Vec<String>>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct DefinitionSummary {
    pub repo: String,
    pub name: String,
    pub scope: String,
    pub args: Vec<ArgSpec>,
    pub has_discovery: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct TaskDefinition {
    pub name: String,
    pub repo: String,
    pub discovery: Option<Discovery>,
    pub args: Vec<ArgSpec>,
    pub dedup: String,
    pub worktree: String,
    pub pre_run: Option<String>,
    pub post_run: Option<String>,
    pub model: String,
    pub timeout_ms: u64,
    pub priority: String,
    pub prompt: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Discovery {
    pub command: String,
    pub item_key: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A full modern snapshot: every field present, one task with every field.
    fn modern_json() -> &'static str {
        r#"{
          "tasks": [{
            "id": "01TASKAAA000000000000000000",
            "status": "running",
            "definition": "pr-ready",
            "item": {"pr": "257"},
            "itemKey": "pr:257",
            "target": {"repo": "platform", "ref": "worktree:platform.feat-a", "worktree": "platform.feat-a"},
            "priority": "normal",
            "created": "2026-07-08T10:00:00.000Z",
            "source": "tui",
            "ephemeralWorktree": false,
            "error": null,
            "session": "main",
            "resumeSessionId": "sess-1",
            "model": "opus",
            "prompt": "do the thing\n"
          }],
          "archivedRecent": [],
          "sessions": [{
            "kind": "interactive", "key": "s1", "lane": "platform:platform.feat-a",
            "cwd": "/wt/platform.feat-a", "pid": 4242,
            "startedAt": "2026-07-08T09:00:00.000Z", "heartbeatAt": "2026-07-08T10:00:00.000Z"
          }],
          "running": ["01TASKAAA000000000000000000"],
          "maxConcurrent": 3,
          "projects": [{"name": "platform"}, {"name": "web"}],
          "worktrees": {"platform": [{"name": "platform.feat-a", "path": "/wt/platform.feat-a", "branch": "feat-a"}]},
          "mainSessions": {"platform:platform.feat-a": "sess-main"},
          "buildId": "1751970000000"
        }"#
    }

    #[test]
    fn deserializes_a_full_modern_snapshot() {
        let s: StateSnapshot = serde_json::from_str(modern_json()).unwrap();
        assert_eq!(s.tasks.len(), 1);
        let t = &s.tasks[0];
        assert_eq!(t.id, "01TASKAAA000000000000000000");
        assert_eq!(t.status, TaskStatus::Running);
        assert_eq!(t.definition.as_deref(), Some("pr-ready"));
        assert_eq!(t.item.as_ref().unwrap().get("pr").map(String::as_str), Some("257"));
        assert_eq!(t.item_key.as_deref(), Some("pr:257"));
        assert_eq!(t.target.repo, "platform");
        assert_eq!(t.target.git_ref, "worktree:platform.feat-a");
        assert_eq!(t.target.worktree.as_deref(), Some("platform.feat-a"));
        assert!(!t.ephemeral_worktree);
        assert_eq!(t.session, "main");
        assert_eq!(t.resume_session_id.as_deref(), Some("sess-1"));
        assert_eq!(t.model.as_deref(), Some("opus"));
        assert_eq!(t.prompt, "do the thing\n");
        assert_eq!(s.sessions[0].kind, "interactive");
        assert_eq!(s.sessions[0].pid, Some(4242));
        assert_eq!(s.max_concurrent, Some(3));
        assert_eq!(s.projects, vec![Project { name: "platform".into() }, Project { name: "web".into() }]);
        assert_eq!(s.worktrees["platform"][0].branch, "feat-a");
        assert_eq!(s.main_sessions["platform:platform.feat-a"], "sess-main");
        assert_eq!(s.build_id.as_deref(), Some("1751970000000"));
    }

    #[test]
    fn old_daemon_snapshot_missing_new_fields_defaults_without_error() {
        // Predates projects/worktrees/maxConcurrent/buildId (mirrors
        // use-daemon.test's OLD-daemon case): only the original four fields.
        let old = r#"{"tasks": [{"id": "t1", "target": {"repo": "platform", "ref": "temp"}}],
                      "archivedRecent": [], "sessions": [], "running": []}"#;
        let s: StateSnapshot = serde_json::from_str(old).unwrap();
        assert_eq!(s.tasks.len(), 1);
        assert_eq!(s.tasks[0].id, "t1");
        // status absent → Unknown (default); target.worktree absent → None.
        assert_eq!(s.tasks[0].status, TaskStatus::Unknown);
        assert_eq!(s.tasks[0].target.worktree, None);
        assert_eq!(s.projects, vec![]);
        assert!(s.worktrees.is_empty());
        assert!(s.main_sessions.is_empty());
        assert_eq!(s.max_concurrent, None);
        // buildId absent → None means "stale" for self-heal — must NOT default to "".
        assert_eq!(s.build_id, None);
    }

    #[test]
    fn null_valued_collections_coerce_to_empty() {
        // The nullable_default shim: `null` where an array/object is expected → default.
        let s: StateSnapshot = serde_json::from_str(
            r#"{"tasks": null, "running": null, "worktrees": null, "projects": null}"#,
        )
        .unwrap();
        assert_eq!(s.tasks, vec![]);
        assert_eq!(s.running, vec![] as Vec<String>);
        assert!(s.worktrees.is_empty());
        assert_eq!(s.projects, vec![]);
    }

    #[test]
    fn unknown_status_maps_to_unknown_variant() {
        let t: TaskInstance =
            serde_json::from_str(r#"{"id": "x", "status": "paused-by-alien"}"#).unwrap();
        assert_eq!(t.status, TaskStatus::Unknown);
    }

    #[test]
    fn kebab_status_needs_input_round_trips() {
        let t: TaskInstance =
            serde_json::from_str(r#"{"id": "x", "status": "needs-input"}"#).unwrap();
        assert_eq!(t.status, TaskStatus::NeedsInput);
    }
}
```

Run `cargo test -p qoo-tui types` → **expected FAIL to compile first** only if a struct name/field is mistyped; once the file is as above it **PASSES** (5 tests). (This task has no separate stub phase — the structs and their tests are written together; the "RED" is a mis-decode caught by running.)

- [ ] **Step 3: Verify + commit.** Run `cargo test -p qoo-tui types` → **expected PASS (5)**.

```
git add crates/qoo-tui/src/lib.rs crates/qoo-tui/src/ipc/mod.rs crates/qoo-tui/src/ipc/types.rs
git commit -m "feat(tui-rs): lenient serde wire mirrors for the daemon snapshot"
```

---

### Task 3: `ipc/client.rs` — NDJSON JSON-RPC client + push subscription

**Files:**
- Create `crates/qoo-tui/src/ipc/client.rs`
- Create `crates/qoo-tui/src/event.rs` (Event subset — Task 4 extends it)
- Modify `crates/qoo-tui/src/ipc/mod.rs` (add `pub mod client;`)
- Modify `crates/qoo-tui/src/lib.rs` (add `pub mod event;`)
- Test: `crates/qoo-tui/src/ipc/client.rs` (inline `#[cfg(test)] mod tests`, fake NDJSON server on a tempdir `UnixListener`)

**Interfaces:**
- Produces `RpcClient::connect(sock: &Path) -> io::Result<Self>` and `RpcClient::call(&mut self, method: &str, params: serde_json::Value, timeout: Duration) -> Result<serde_json::Value, String>` (exact contract signatures).
- Produces `spawn_subscription(sock: PathBuf, tx: tokio::sync::mpsc::UnboundedSender<Event>) -> tokio::task::JoinHandle<()>`.
- Produces (subset) `event::Event { Snapshot(StateSnapshot), Disconnected }`.
- Consumes `ipc::types::StateSnapshot` (Task 2).

**Steps:**

- [ ] **Step 1: Register modules + the Event subset.** In `lib.rs` add:

```rust
pub mod event;
```

In `ipc/mod.rs` add:

```rust
pub mod client;
```

Create `crates/qoo-tui/src/event.rs`:

```rust
use crate::ipc::types::StateSnapshot;

/// Subset of the contract `Event` enum — Task 4 extends it verbatim with
/// Key/Mouse/Resize/Tick/RunFiles/Definitions/Definition/ActionResult.
/// Variants are only ever added, never renamed.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Snapshot(StateSnapshot),
    Disconnected,
}
```

- [ ] **Step 2 (RED): Write `client.rs` — signatures, stub bodies, full tests.** Create `crates/qoo-tui/src/ipc/client.rs`:

```rust
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::UnixStream;
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;

use crate::event::Event;
use crate::ipc::types::StateSnapshot;

/// Short-lived NDJSON JSON-RPC client: one Unix-socket connection, sequential
/// calls (mirror of `ApiClient` in packages/daemon/src/client.ts as used by
/// actions.ts's `withClient` — connect, call, drop).
pub struct RpcClient {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
    next_id: u64,
}

impl RpcClient {
    pub async fn connect(sock: &Path) -> io::Result<Self> {
        let _ = sock;
        unimplemented!()
    }

    pub async fn call(
        &mut self,
        method: &str,
        params: serde_json::Value,
        timeout: Duration,
    ) -> Result<serde_json::Value, String> {
        let _ = (method, params, timeout);
        unimplemented!()
    }
}

pub fn spawn_subscription(sock: PathBuf, tx: UnboundedSender<Event>) -> JoinHandle<()> {
    let _ = (sock, tx);
    unimplemented!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::net::UnixListener;
    use tokio::sync::mpsc;
    use tokio::time::timeout;

    const SNAP_A: &str = r#"{"tasks":[],"archivedRecent":[],"sessions":[],"running":[]}"#;
    const SNAP_B: &str = r#"{"tasks":[],"archivedRecent":[],"sessions":[],"running":["t1"]}"#;

    fn sock_dir() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("d.sock");
        (dir, sock)
    }

    /// Read one NDJSON request line; return (raw id value, method).
    async fn read_req(reader: &mut BufReader<OwnedReadHalf>) -> (serde_json::Value, String) {
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        let method = v["method"].as_str().unwrap().to_string();
        (v["id"].clone(), method)
    }

    /// One scripted subscription session: reply to `subscribe` then `state`
    /// (serving `state_json` as the result), write each push frame, then either
    /// close the connection or hold it open past the test's horizon.
    async fn serve_session(
        listener: &UnixListener,
        state_json: &str,
        pushes: &[&str],
        close_after: bool,
    ) {
        let (stream, _) = listener.accept().await.unwrap();
        let (r, mut w) = stream.into_split();
        let mut reader = BufReader::new(r);
        let (id, method) = read_req(&mut reader).await;
        assert_eq!(method, "subscribe");
        w.write_all(format!("{}\n", json!({"id": id, "result": null})).as_bytes())
            .await
            .unwrap();
        let (id, method) = read_req(&mut reader).await;
        assert_eq!(method, "state");
        let state: serde_json::Value = serde_json::from_str(state_json).unwrap();
        w.write_all(format!("{}\n", json!({"id": id, "result": state})).as_bytes())
            .await
            .unwrap();
        for push in pushes {
            let data: serde_json::Value = serde_json::from_str(push).unwrap();
            w.write_all(format!("{}\n", json!({"event": "state", "data": data})).as_bytes())
                .await
                .unwrap();
        }
        if !close_after {
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
        // returning drops both halves → connection closes
    }

    #[tokio::test]
    async fn call_returns_result_and_maps_error_frames() {
        let (_dir, sock) = sock_dir();
        let listener = UnixListener::bind(&sock).unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (r, mut w) = stream.into_split();
            let mut reader = BufReader::new(r);
            let (id, method) = read_req(&mut reader).await;
            assert_eq!(method, "ping");
            w.write_all(format!("{}\n", json!({"id": id, "result": "pong"})).as_bytes())
                .await
                .unwrap();
            let (id, _) = read_req(&mut reader).await;
            w.write_all(format!("{}\n", json!({"id": id, "error": "boom"})).as_bytes())
                .await
                .unwrap();
        });

        let mut client = RpcClient::connect(&sock).await.unwrap();
        let ok = client
            .call("ping", serde_json::Value::Null, Duration::from_secs(1))
            .await;
        assert_eq!(ok.unwrap(), json!("pong"));
        let err = client
            .call("retry", json!({"id": "x"}), Duration::from_secs(1))
            .await;
        assert_eq!(err.unwrap_err(), "boom");
    }

    #[tokio::test]
    async fn call_times_out_with_method_in_message() {
        let (_dir, sock) = sock_dir();
        let listener = UnixListener::bind(&sock).unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (r, _w) = stream.into_split();
            let mut reader = BufReader::new(r);
            let mut line = String::new();
            let _ = reader.read_line(&mut line).await; // read, never reply
            tokio::time::sleep(Duration::from_secs(5)).await; // hold open
        });
        let mut client = RpcClient::connect(&sock).await.unwrap();
        let err = client
            .call("ping", serde_json::Value::Null, Duration::from_millis(100))
            .await;
        assert_eq!(err.unwrap_err(), "call timed out: ping");
    }

    #[tokio::test]
    async fn subscription_delivers_state_then_pushes_and_dedups_identical_payloads() {
        let (_dir, sock) = sock_dir();
        let listener = UnixListener::bind(&sock).unwrap();
        tokio::spawn(async move {
            // initial state A; pushes: A (dup — skipped), A (dup — skipped), B
            serve_session(&listener, SNAP_A, &[SNAP_A, SNAP_A, SNAP_B], false).await;
        });
        let (tx, mut rx) = mpsc::unbounded_channel();
        let _handle = spawn_subscription(sock, tx);

        let first = timeout(Duration::from_secs(2), rx.recv()).await.unwrap().unwrap();
        let Event::Snapshot(s) = first else {
            panic!("expected snapshot, got {first:?}")
        };
        assert!(s.running.is_empty());
        let second = timeout(Duration::from_secs(2), rx.recv()).await.unwrap().unwrap();
        let Event::Snapshot(s) = second else {
            panic!("expected snapshot, got {second:?}")
        };
        assert_eq!(s.running, vec!["t1".to_string()]);
        // Nothing else arrives — the two byte-identical pushes were skipped.
        assert!(timeout(Duration::from_millis(300), rx.recv()).await.is_err());
    }

    #[tokio::test]
    async fn reconnect_sends_disconnected_and_recommits_identical_snapshot() {
        let (_dir, sock) = sock_dir();
        let listener = UnixListener::bind(&sock).unwrap();
        tokio::spawn(async move {
            // Session 1 serves A then closes; session 2 serves the SAME A.
            serve_session(&listener, SNAP_A, &[], true).await;
            serve_session(&listener, SNAP_A, &[], false).await;
        });
        let (tx, mut rx) = mpsc::unbounded_channel();
        let _handle = spawn_subscription(sock, tx);

        let first = timeout(Duration::from_secs(2), rx.recv()).await.unwrap().unwrap();
        assert!(matches!(first, Event::Snapshot(_)));
        let second = timeout(Duration::from_secs(2), rx.recv()).await.unwrap().unwrap();
        assert_eq!(second, Event::Disconnected);
        // After the 2s retry, the byte-identical snapshot IS delivered again —
        // the first snapshot after a (re)connect always commits.
        let third = timeout(Duration::from_secs(5), rx.recv()).await.unwrap().unwrap();
        assert!(matches!(third, Event::Snapshot(_)));
    }
}
```

Run `cargo test -p qoo-tui client` → **expected FAIL** (all 4 tests panic on `unimplemented!()`).

- [ ] **Step 3 (GREEN): Implement `RpcClient` and `spawn_subscription`.** Replace the three stub bodies in `client.rs`:

```rust
impl RpcClient {
    pub async fn connect(sock: &Path) -> io::Result<Self> {
        let stream = UnixStream::connect(sock).await?;
        let (r, w) = stream.into_split();
        Ok(Self {
            reader: BufReader::new(r),
            writer: w,
            next_id: 1,
        })
    }

    pub async fn call(
        &mut self,
        method: &str,
        params: serde_json::Value,
        timeout: Duration,
    ) -> Result<serde_json::Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        // The daemon reads `req.params ?? {}` (api.ts), so `null` params are safe.
        let frame = format!(
            "{}\n",
            serde_json::json!({ "id": id, "method": method, "params": params })
        );
        let fut = async {
            self.writer
                .write_all(frame.as_bytes())
                .await
                .map_err(|e| e.to_string())?;
            let mut line = String::new();
            loop {
                line.clear();
                let n = self
                    .reader
                    .read_line(&mut line)
                    .await
                    .map_err(|e| e.to_string())?;
                if n == 0 {
                    return Err("connection closed".to_string());
                }
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) else {
                    continue; // mirror handleFrame: unparseable lines are dropped
                };
                // Push frames and replies to other ids are not ours — skip
                // (correlate by id, sequential single-caller client).
                if v.get("event").is_some() {
                    continue;
                }
                if v.get("id").and_then(serde_json::Value::as_u64) != Some(id) {
                    continue;
                }
                // TS checks `frame.error !== undefined` — key presence is an error.
                if let Some(err) = v.get("error") {
                    return Err(err
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| err.to_string()));
                }
                return Ok(v.get("result").cloned().unwrap_or(serde_json::Value::Null));
            }
        };
        match tokio::time::timeout(timeout, fut).await {
            Ok(res) => res,
            Err(_) => Err(format!("call timed out: {method}")),
        }
    }
}

/// Persistent push-subscription task: connect → `subscribe` → `state` → forward
/// every snapshot; on any error/EOF send `Disconnected`, sleep 2s, reconnect —
/// forever (mirror of use-daemon.ts's attempt/scheduleRetry loop).
pub fn spawn_subscription(sock: PathBuf, tx: UnboundedSender<Event>) -> JoinHandle<()> {
    tokio::spawn(async move {
        // Persisted across reconnects: dedup key of the last committed snapshot
        // (mirror of lastPushedJson). The per-session `first` flag inside
        // subscription_session mirrors connectedRef: the first snapshot after a
        // (re)connect always commits, even when byte-identical.
        let mut last_committed: Option<String> = None;
        loop {
            let _ = subscription_session(&sock, &tx, &mut last_committed).await;
            if tx.send(Event::Disconnected).is_err() {
                return; // receiver dropped — the app exited
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    })
}

async fn subscription_session(
    sock: &Path,
    tx: &UnboundedSender<Event>,
    last_committed: &mut Option<String>,
) -> Result<(), String> {
    let stream = UnixStream::connect(sock).await.map_err(|e| e.to_string())?;
    let (r, mut w) = stream.into_split();
    let mut reader = BufReader::new(r);
    w.write_all(b"{\"id\":1,\"method\":\"subscribe\"}\n")
        .await
        .map_err(|e| e.to_string())?;
    w.write_all(b"{\"id\":2,\"method\":\"state\"}\n")
        .await
        .map_err(|e| e.to_string())?;

    let mut first = true;
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).await.map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("connection closed".to_string());
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };
        // Snapshot ingress has two envelopes — the `state` reply ({id:2,result})
        // and push frames ({event:"state",data}). One choke point, mirroring
        // use-daemon's applySnapshot.
        let data = if v.get("event").and_then(serde_json::Value::as_str) == Some("state") {
            v.get("data").cloned()
        } else if v.get("id").and_then(serde_json::Value::as_u64) == Some(2) {
            v.get("result").cloned()
        } else {
            None // subscribe ack (id 1) or anything else
        };
        let Some(data) = data else { continue };
        // Dedup on the serialized snapshot payload (envelope-independent, like
        // JSON.stringify(pushed)) — except the first snapshot of this session.
        let raw = data.to_string();
        if !first && last_committed.as_deref() == Some(raw.as_str()) {
            continue;
        }
        first = false;
        *last_committed = Some(raw);
        // Lenient decode: a malformed payload becomes the empty default rather
        // than killing the subscription task (types.rs handles missing fields).
        let snapshot: StateSnapshot = serde_json::from_value(data).unwrap_or_default();
        if tx.send(Event::Snapshot(snapshot)).is_err() {
            return Ok(());
        }
    }
}
```

Run `cargo test -p qoo-tui client` → **expected PASS (4)**. The reconnect test takes ~2s (real retry sleep) — acceptable; do not shorten the production delay for tests.

- [ ] **Step 4: Commit.**

```
git add crates/qoo-tui/src/lib.rs crates/qoo-tui/src/event.rs crates/qoo-tui/src/ipc/mod.rs crates/qoo-tui/src/ipc/client.rs
git commit -m "feat(tui-rs): NDJSON JSON-RPC client + push subscription with dedup and reconnect"
```

---

### Task 4: `event.rs` full enums + event loop + executor, `app.rs` skeleton

**Files:**
- Modify `crates/qoo-tui/src/event.rs` (full `Event` + `Cmd` + loop + executor)
- Create `crates/qoo-tui/src/app.rs`
- Create `crates/qoo-tui/src/runfiles.rs` (`RunFiles` struct + stub reader; Task 10 supplies the real body)
- Modify `crates/qoo-tui/src/main.rs` (async event loop replaces the Task 1 sync loop)
- Modify `crates/qoo-tui/src/lib.rs` (add `pub mod app;` and `pub mod runfiles;`)
- Modify `crates/qoo-tui/Cargo.toml` (via `cargo add futures`)
- Test: inline `#[cfg(test)]` in `app.rs` (update/wants_tick) and `event.rs` (seq_summary)

**Interfaces:**
- Produces `Event` and `Cmd` enums verbatim from the contract; `RpcCall { method, params }`.
- Produces `run_event_loop<B: Backend>(terminal: &mut Terminal<B>, app: &mut App) -> io::Result<()>`.
- Produces `execute(cmd: Cmd, tx: UnboundedSender<Event>, sock: PathBuf, runs_dir: PathBuf)` and `seq_summary(verb: &str, ok: usize, errs: &[String]) -> String`.
- Produces `App` (contract fields minus `hit` — Task 7 adds it), `App::new(runs_dir, sock_path)`, `App::update(&mut self, Event) -> Update`, `App::wants_tick(&self) -> bool`, `Update { dirty, cmds }`, `PaneId`, `ListPane` (+`idx()`), `Selection`, `TabUiState`, `Mode` (subset: `List`).
- Produces `runfiles::RunFiles { transcript_tail: Vec<String>, report: Vec<String> }` + `read_run_files` (stub).
- Consumes `ipc::client::{RpcClient, spawn_subscription}` (Task 3), `ipc::types` (Task 2).

**Steps:**

- [ ] **Step 1: Add the stream adapter dep.** crossterm's `EventStream` implements `futures_core::Stream`; polling it inside `tokio::select!` needs `StreamExt::next`. Run inside `crates/qoo-tui`:

```
cargo add futures@0.3
```

(One dep beyond the original list — required by the `event-stream` feature's async API.)

- [ ] **Step 2: Create the `runfiles.rs` stub module.** Create `crates/qoo-tui/src/runfiles.rs`:

```rust
use std::path::Path;

/// Contents of a run's on-disk files. `PartialEq` so content-identical re-reads
/// can be dropped before becoming events (Task 10's poll loop).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RunFiles {
    pub transcript_tail: Vec<String>,
    pub report: Vec<String>,
}

/// Contract-shaped stub so the Task 4 executor wiring is final. Task 10 replaces
/// this body with the real reader (report.md in full; transcript.md tail-only via
/// a seek window from EOF). Until then run files render empty in the detail pane.
pub async fn read_run_files(_runs_dir: &Path, _task_id: &str, _tail_lines: usize) -> RunFiles {
    RunFiles::default()
}
```

In `lib.rs` add:

```rust
pub mod app;
pub mod runfiles;
```

- [ ] **Step 3 (RED): Write `app.rs` and the full `event.rs` with stub pure functions + tests.** Create `crates/qoo-tui/src/app.rs`:

```rust
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crossterm::event::{KeyCode, KeyEventKind};

use crate::event::{Cmd, Event};
use crate::ipc::types::{DefinitionSummary, StateSnapshot, TaskDefinition, TaskStatus};
use crate::runfiles::RunFiles;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneId {
    Queue,
    Tasks,
    Worktrees,
    Detail,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Selection {
    pub cursor: usize,
    pub anchor: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TabUiState {
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

/// Subset of the contract `Mode` — later tasks add Search + Help (11),
/// ActionMenu + ConfirmBulkRemove (14/16), AddTask + WorktreeInput (15),
/// DefPick (18), DefArgs (19), CreateWorktree + ConfirmRemove (21).
/// Variants are only ever added.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Mode {
    #[default]
    List,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Update {
    pub dirty: bool,
    pub cmds: Vec<Cmd>,
}

pub struct App {
    pub snapshot: Option<StateSnapshot>,
    pub connected: bool,
    pub active_tab: usize,
    pub ui_by_tab: HashMap<String, TabUiState>,
    pub mode: Mode,
    pub status_line: Option<String>,
    pub run_files: Option<(String, RunFiles)>,
    pub defs_by_project: HashMap<String, Vec<DefinitionSummary>>,
    pub full_defs: HashMap<String, TaskDefinition>, // keyed "repo/name"
    pub now_epoch_s: u64,
    pub size: (u16, u16),
    // Task 7 adds `pub hit: HitMap` (contract field; needs hit.rs).
    pub last_healed_build_id: Option<String>,
    pub sock_path: PathBuf,
    pub runs_dir: PathBuf,
}

fn now_epoch_s() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

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
            full_defs: HashMap::new(),
            now_epoch_s: now_epoch_s(),
            size: (0, 0),
            last_healed_build_id: None,
            sock_path,
            runs_dir,
        }
    }

    pub fn update(&mut self, event: Event) -> Update {
        let _ = event;
        unimplemented!()
    }

    /// True while the ACTIVE project has a running task — the only time the 1s
    /// Tick (elapsed-label repaint) is armed. Re-evaluated after every event.
    pub fn wants_tick(&self) -> bool {
        unimplemented!()
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
    fn wants_tick_false_with_only_queued_tasks() {
        let mut app = app();
        let mut task = running_task("platform");
        task.status = TaskStatus::Queued;
        app.update(Event::Snapshot(snapshot_with(&["platform"], vec![task])));
        app.active_tab = 0;
        assert!(!app.wants_tick());
    }
}
```

Replace `crates/qoo-tui/src/event.rs` entirely:

```rust
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures::StreamExt;
use tokio::sync::mpsc::{self, UnboundedSender};

use crate::app::{App, Update};
use crate::ipc::client::{spawn_subscription, RpcClient};
use crate::ipc::types::{DefinitionSummary, StateSnapshot, TaskDefinition};
use crate::runfiles::RunFiles;

/// Everything enters the app through this one enum (contract-verbatim).
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Key(crossterm::event::KeyEvent),
    Mouse(crossterm::event::MouseEvent),
    Resize,
    Snapshot(StateSnapshot),
    Disconnected,
    Tick,
    RunFiles { task_id: String, files: RunFiles },
    Definitions { repo: String, defs: Vec<DefinitionSummary> },
    Definition { repo: String, name: String, def: Option<TaskDefinition> },
    ActionResult { status: Option<String>, invalidate_defs_for: Option<String> },
}

#[derive(Debug, Clone, PartialEq)]
pub struct RpcCall {
    pub method: String,
    pub params: serde_json::Value,
}

/// Side effects `App::update` requests; performed by `execute` on tokio tasks
/// (contract-verbatim).
#[derive(Debug, Clone, PartialEq)]
pub enum Cmd {
    Rpc {
        label: String,
        call: RpcCall,
        timeout_ms: u64,
        timeout_is_ok: bool,
        invalidate_defs_for: Option<String>,
    },
    RpcSeq {
        verb: String, // past tense, e.g. "reran"
        calls: Vec<RpcCall>,
        invalidate_defs_for: Option<String>,
    },
    FetchDefinitions { repo: String },
    FetchDefinition { repo: String, name: String },
    ReadRunFiles { task_id: String, tail_lines: usize, delay_ms: u64 },
    OpenTmux { path: String },
    Heal,
    Quit,
}

fn map_terminal_event(ev: crossterm::event::Event) -> Option<Event> {
    use crossterm::event::Event as Ct;
    match ev {
        Ct::Key(k) => Some(Event::Key(k)),
        Ct::Mouse(m) => Some(Event::Mouse(m)),
        Ct::Resize(_, _) => Some(Event::Resize),
        _ => None, // focus/paste events unused
    }
}

/// Interim frame until `view::render` lands in Tasks 7–8.
fn draw_placeholder(app: &App, f: &mut ratatui::Frame) {
    let status = if app.connected {
        "connected"
    } else {
        "daemon unreachable — retrying…"
    };
    let tasks = app.snapshot.as_ref().map(|s| s.tasks.len()).unwrap_or(0);
    let text = format!("qoo-tui — {status} · {tasks} tasks · q quits");
    f.render_widget(ratatui::widgets::Paragraph::new(text), f.area());
}

/// Elm-style single-threaded core: one select! over the terminal's EventStream,
/// the injected-event channel, and (only while armed) the 1s Tick. Draw only
/// when `update()` reports dirty — 1 render per keypress, zero idle renders.
pub async fn run_event_loop<B: ratatui::backend::Backend>(
    terminal: &mut ratatui::Terminal<B>,
    app: &mut App,
) -> std::io::Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel::<Event>();
    let _subscription = spawn_subscription(app.sock_path.clone(), tx.clone());
    let mut term_events = crossterm::event::EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    // A long-disarmed tick must not burst-fire on re-arm.
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    terminal.draw(|f| draw_placeholder(app, f))?; // first paint pre-snapshot

    loop {
        // wants_tick re-evaluated after EVERY event: the Tick arm exists only
        // while the active project has a running task.
        let event = if app.wants_tick() {
            tokio::select! {
                maybe = term_events.next() => {
                    match maybe.and_then(|r| r.ok()).and_then(map_terminal_event) {
                        Some(ev) => ev,
                        None => continue,
                    }
                }
                received = rx.recv() => match received {
                    Some(ev) => ev,
                    None => return Ok(()),
                },
                _ = tick.tick() => Event::Tick,
            }
        } else {
            tokio::select! {
                maybe = term_events.next() => {
                    match maybe.and_then(|r| r.ok()).and_then(map_terminal_event) {
                        Some(ev) => ev,
                        None => continue,
                    }
                }
                received = rx.recv() => match received {
                    Some(ev) => ev,
                    None => return Ok(()),
                },
            }
        };

        // The contract's Resize carries no dims; refresh App::size from the
        // terminal before update() so selectors see current geometry.
        if matches!(event, Event::Resize) {
            let size = terminal.size()?;
            app.size = (size.width, size.height);
        }

        let Update { dirty, cmds } = app.update(event);
        let mut quit = false;
        for cmd in cmds {
            if matches!(cmd, Cmd::Quit) {
                quit = true; // handled here, never dispatched to the executor
                continue;
            }
            execute(cmd, tx.clone(), app.sock_path.clone(), app.runs_dir.clone());
        }
        if quit {
            return Ok(());
        }
        if dirty {
            terminal.draw(|f| draw_placeholder(app, f))?;
        }
    }
}

/// Compose the RpcSeq status line: "reran 3" or "reran 2, 1 failed: <first error>".
pub fn seq_summary(verb: &str, ok: usize, errs: &[String]) -> String {
    let _ = (verb, ok, errs);
    unimplemented!()
}

async fn rpc_once(sock: &Path, call: &RpcCall, timeout_ms: u64) -> Result<serde_json::Value, String> {
    let mut client = RpcClient::connect(sock).await.map_err(|e| e.to_string())?;
    client
        .call(&call.method, call.params.clone(), Duration::from_millis(timeout_ms))
        .await
}

/// Perform one Cmd on a detached tokio task; results come back as Events.
/// The UI thread never blocks (mutations are fire-and-forget).
pub fn execute(cmd: Cmd, tx: UnboundedSender<Event>, sock: PathBuf, runs_dir: PathBuf) {
    match cmd {
        Cmd::Rpc { label, call, timeout_ms, timeout_is_ok, invalidate_defs_for } => {
            tokio::spawn(async move {
                let result = rpc_once(&sock, &call, timeout_ms).await;
                let status = match result {
                    Ok(_) => None,
                    // runDefinition parity: discovery can outlive the client
                    // timeout — the tasks may still land and the subscription
                    // re-syncs, so timeout reports as success (actions.ts).
                    Err(e) if timeout_is_ok && e.starts_with("call timed out") => None,
                    Err(e) => Some(format!("{label}: {e}")),
                };
                let _ = tx.send(Event::ActionResult { status, invalidate_defs_for });
            });
        }
        Cmd::RpcSeq { verb, calls, invalidate_defs_for } => {
            tokio::spawn(async move {
                // Sequential on purpose: per-item error capture with frozen order
                // (bulk execution parity). Default 5s per-call budget.
                let mut ok = 0usize;
                let mut errs: Vec<String> = Vec::new();
                for call in &calls {
                    match rpc_once(&sock, call, 5_000).await {
                        Ok(_) => ok += 1,
                        Err(e) => errs.push(e),
                    }
                }
                let _ = tx.send(Event::ActionResult {
                    status: Some(seq_summary(&verb, ok, &errs)),
                    invalidate_defs_for,
                });
            });
        }
        Cmd::FetchDefinitions { repo } => {
            tokio::spawn(async move {
                let call = RpcCall { method: "definitions".into(), params: serde_json::Value::Null };
                // Errors → empty vec (actions.ts `definitions` catch → []).
                let defs = match rpc_once(&sock, &call, 5_000).await {
                    Ok(v) => serde_json::from_value::<Vec<DefinitionSummary>>(v).unwrap_or_default(),
                    Err(_) => Vec::new(),
                };
                let _ = tx.send(Event::Definitions { repo, defs });
            });
        }
        Cmd::FetchDefinition { repo, name } => {
            tokio::spawn(async move {
                let call = RpcCall {
                    method: "definition".into(),
                    params: serde_json::json!({ "repo": &repo, "name": &name }),
                };
                // Errors → None (actions.ts `definition` catch → null).
                let def = match rpc_once(&sock, &call, 5_000).await {
                    Ok(v) => serde_json::from_value::<TaskDefinition>(v).ok(),
                    Err(_) => None,
                };
                let _ = tx.send(Event::Definition { repo, name, def });
            });
        }
        Cmd::ReadRunFiles { task_id, tail_lines, delay_ms } => {
            tokio::spawn(async move {
                // Selection-settle debounce lives here (caller just issues the Cmd
                // with delay_ms=120). runfiles::read_run_files is a stub until
                // Task 10 lands the real tail/report reader.
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                let files = crate::runfiles::read_run_files(&runs_dir, &task_id, tail_lines).await;
                let _ = tx.send(Event::RunFiles { task_id, files });
            });
        }
        Cmd::OpenTmux { path } => {
            tokio::spawn(async move {
                let result = tokio::process::Command::new("tmux")
                    .args(["new-window", "-c", &path])
                    .output()
                    .await;
                let status = match result {
                    Ok(out) if out.status.success() => None,
                    Ok(out) => Some(format!(
                        "tmux: {}",
                        String::from_utf8_lossy(&out.stderr).trim()
                    )),
                    Err(e) => Some(format!("tmux: {e}")),
                };
                if status.is_some() {
                    let _ = tx.send(Event::ActionResult { status, invalidate_defs_for: None });
                }
            });
        }
        Cmd::Heal => {
            // Self-heal lands in Task 22; until then no update() emits Heal and
            // the executor deliberately does nothing for it.
        }
        Cmd::Quit => {
            // Handled by run_event_loop before dispatch; never reaches here.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seq_summary_all_ok() {
        assert_eq!(seq_summary("reran", 3, &[]), "reran 3");
        assert_eq!(seq_summary("skipped", 1, &[]), "skipped 1");
    }

    #[test]
    fn seq_summary_with_failures_reports_count_and_first_error() {
        assert_eq!(
            seq_summary("reran", 2, &["boom".to_string()]),
            "reran 2, 1 failed: boom"
        );
        assert_eq!(
            seq_summary("skipped", 0, &["first".to_string(), "second".to_string()]),
            "skipped 0, 2 failed: first"
        );
    }
}
```

Run `cargo test -p qoo-tui app event` — filters don't compose in one invocation; run `cargo test -p qoo-tui` → **expected FAIL** (app tests + seq_summary tests panic on `unimplemented!()`; client/types/paths tests still pass).

- [ ] **Step 4 (GREEN): Implement `update`, `wants_tick`, `seq_summary`.** In `app.rs` replace the two stub bodies:

```rust
    pub fn update(&mut self, event: Event) -> Update {
        match event {
            Event::Snapshot(snapshot) => {
                self.snapshot = Some(snapshot);
                self.connected = true;
                Update { dirty: true, cmds: vec![] }
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
                Update { dirty: true, cmds: vec![] }
            }
            Event::Key(key) => {
                if key.kind != KeyEventKind::Press {
                    return Update { dirty: false, cmds: vec![] };
                }
                match self.mode {
                    Mode::List => {
                        if key.code == KeyCode::Char('q') {
                            return Update { dirty: false, cmds: vec![Cmd::Quit] };
                        }
                        // Full list-mode keymap lands in Task 11.
                        Update { dirty: false, cmds: vec![] }
                    }
                }
            }
            // Mouse routing (Task 12), RunFiles/Definitions ingestion (Tasks
            // 9/10/18), ActionResult → status line (Task 13) land later.
            _ => Update { dirty: false, cmds: vec![] },
        }
    }

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
```

In `event.rs` replace the `seq_summary` body:

```rust
pub fn seq_summary(verb: &str, ok: usize, errs: &[String]) -> String {
    if errs.is_empty() {
        return format!("{verb} {ok}");
    }
    format!("{verb} {ok}, {} failed: {}", errs.len(), errs[0])
}
```

Run `cargo test -p qoo-tui` → **expected PASS** (paths 4, types 5, client 4, app 8, event 2).

- [ ] **Step 5: Rewrite `main.rs` onto the async loop.** Replace the placeholder loop (keep `TerminalGuard` and `install_panic_hook` exactly as written in Task 1):

```rust
use std::io::{self, Stdout};

use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

struct TerminalGuard;

impl TerminalGuard {
    fn new() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut out = io::stdout();
        execute!(out, EnterAlternateScreen, EnableMouseCapture)?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let mut out = io::stdout();
        let _ = execute!(out, DisableMouseCapture, LeaveAlternateScreen);
        let _ = disable_raw_mode();
    }
}

fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let mut out = io::stdout();
        let _ = execute!(out, DisableMouseCapture, LeaveAlternateScreen);
        let _ = disable_raw_mode();
        prev(info);
    }));
}

#[tokio::main]
async fn main() -> io::Result<()> {
    install_panic_hook();
    let state = qoo_tui::paths::state_path();
    let sock = qoo_tui::paths::socket_path(&state);
    let runs = qoo_tui::paths::runs_path(&state);

    let _guard = TerminalGuard::new()?;
    let mut terminal: Terminal<CrosstermBackend<Stdout>> =
        Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut app = qoo_tui::app::App::new(runs, sock);
    let size = terminal.size()?;
    app.size = (size.width, size.height);

    qoo_tui::event::run_event_loop(&mut terminal, &mut app).await
}
```

Manual check: with the daemon running, `cargo run -p qoo-tui` shows `connected · N tasks`; stop the daemon → the line flips to `daemon unreachable — retrying…` within ~2s; restart it → flips back; `q` exits cleanly.

- [ ] **Step 6: Verify + commit.** `cargo test -p qoo-tui` → **expected PASS (23 tests)**; `cargo build --release -p qoo-tui` → **expected: compiles**.

```
git add crates/qoo-tui/Cargo.toml Cargo.lock crates/qoo-tui/src/lib.rs crates/qoo-tui/src/event.rs crates/qoo-tui/src/app.rs crates/qoo-tui/src/runfiles.rs crates/qoo-tui/src/main.rs
git commit -m "feat(tui-rs): elm-style event loop, Cmd executor, App update skeleton"
```

---

### Task 5: `selectors.rs` — port of selectors.ts + format.ts

**Files:**
- Create `crates/qoo-tui/src/selectors.rs`
- Modify `crates/qoo-tui/src/lib.rs` (add `pub mod selectors;`)
- Test: `crates/qoo-tui/src/selectors.rs` (inline `#[cfg(test)] mod tests` — mirrors `packages/tui/src/__tests__/selectors.test.ts` + `format.test.ts` cases)

**Interfaces:**
- Produces (exact contract signatures): `TabInfo`, `build_tabs`; `QueueRow`, `queue_rows(snapshot, project, now_epoch_s)`; `WtState`, `WorktreeRow`, `worktree_rows`; `PaneLayout { queue_h, tasks_h, worktrees_h }`, `pane_layout`; `window_rows(len, cursor, capacity) -> (usize, usize)`; `pane_title(base, sel, filter, searching)`; `filter_rows`; `arg_summary`; `strip_repo_prefix`; `lane_key`; `prompt_summary`; `elapsed_label`.
- Consumes `ipc::types::{ArgSpec, StateSnapshot, TaskInstance, TaskStatus}` (Task 2), `app::Selection` (Task 4).
- Contract deltas vs TS (all decided by the plan skeleton): `prompt_summary` uses a fixed 60-char budget (TS took `width`); `window_rows` returns the half-open `(start, end)` slice indices (TS returned `{rows, offset}` — `start` IS the offset); `pane_title` derives the selection count from `Selection` (TS took a count); `WorktreeRow.branch` is `String` with `""` for sessions (TS `string | null`); `elapsed_label` includes the `⏱ ` prefix (queue-row detail parity).

**Steps:**

- [ ] **Step 1: Register the module.** In `lib.rs` add:

```rust
pub mod selectors;
```

- [ ] **Step 2 (RED): Write `selectors.rs` — types, stub bodies, full mirrored tests.** Create `crates/qoo-tui/src/selectors.rs`:

```rust
use std::collections::{HashMap, HashSet};

use crate::app::Selection;
use crate::ipc::types::{ArgSpec, StateSnapshot, TaskInstance, TaskStatus};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabInfo {
    pub name: String,
    /// repo seen in tasks/archivedRecent but absent from config projects
    pub synthetic: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueRow {
    pub task_id: String,
    pub glyph: char,
    /// drives the animated throbber in place of the static ▶
    pub running: bool,
    /// ⛓ marker: task resumes the lane's main session
    pub main_session: bool,
    pub lane: String,
    pub summary: String,
    pub detail: String,
    pub archived: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WtState {
    Free,
    Busy,
    You,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeRow {
    /// display name (`<repo>.` prefix stripped) — never an identifier
    pub name: String,
    /// untouched worktree identifier used for every daemon action
    pub raw_name: String,
    pub path: String,
    /// "" for session rows (no real worktree)
    pub branch: String,
    pub state: WtState,
    pub has_main_session: bool,
    pub queued: usize,
    pub is_session: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneLayout {
    pub queue_h: u16,
    pub tasks_h: u16,
    pub worktrees_h: u16,
}

pub fn build_tabs(snapshot: &StateSnapshot) -> Vec<TabInfo> {
    let _ = snapshot;
    unimplemented!()
}

pub fn queue_rows(snapshot: &StateSnapshot, project: &str, now_epoch_s: u64) -> Vec<QueueRow> {
    let _ = (snapshot, project, now_epoch_s);
    unimplemented!()
}

pub fn worktree_rows(snapshot: &StateSnapshot, project: &str) -> Vec<WorktreeRow> {
    let _ = (snapshot, project);
    unimplemented!()
}

pub fn pane_layout(body_height: u16) -> PaneLayout {
    let _ = body_height;
    unimplemented!()
}

/// Cursor-centered scroll window: half-open `(start, end)` slice indices of the
/// visible rows (`start` is the TS `offset`).
pub fn window_rows(len: usize, cursor: usize, capacity: usize) -> (usize, usize) {
    let _ = (len, cursor, capacity);
    unimplemented!()
}

pub fn pane_title(base: &str, sel: &Selection, filter: &str, searching: bool) -> String {
    let _ = (base, sel, filter, searching);
    unimplemented!()
}

/// Indices of rows whose text matches the filter (case-insensitive substring;
/// empty filter matches everything).
pub fn filter_rows<'a, T>(rows: &'a [T], filter: &str, text_of: impl Fn(&T) -> String) -> Vec<usize> {
    let _ = (rows, filter);
    let _ = text_of;
    unimplemented!()
}

/// "pr, mode=ready, review=auto" — `name` for required args, `name=default` otherwise.
pub fn arg_summary(args: &[ArgSpec]) -> String {
    let _ = args;
    unimplemented!()
}

pub fn strip_repo_prefix<'a>(worktree: &'a str, repo: &str) -> &'a str {
    let _ = (worktree, repo);
    unimplemented!()
}

pub fn lane_key(repo: &str, worktree: &str) -> String {
    let _ = (repo, worktree);
    unimplemented!()
}

/// First non-blank line of the prompt, trimmed, clipped to ≤60 chars with `…`.
pub fn prompt_summary(prompt: &str) -> String {
    let _ = prompt;
    unimplemented!()
}

/// "⏱ 47s" / "⏱ 5m03s" (zero-padded seconds) / "⏱ 1h02m" (zero-padded minutes).
pub fn elapsed_label(created_epoch_s: u64, now_epoch_s: u64) -> String {
    let _ = (created_epoch_s, now_epoch_s);
    unimplemented!()
}

/// Parse a daemon ISO-8601 UTC timestamp ("YYYY-MM-DDTHH:MM:SS[.mmm]Z") into
/// epoch seconds. No date crate: Howard Hinnant's days-from-civil algorithm.
fn parse_iso_epoch_s(iso: &str) -> u64 {
    let _ = iso;
    unimplemented!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::types::{Project, SessionEntry, TaskTarget, WorktreeInfo};

    // ---- fixtures (mirror __tests__/helpers.ts makeTask/makeSnapshot/makeSession) ----

    fn make_task(status: TaskStatus) -> TaskInstance {
        TaskInstance {
            id: "01TUI000000000000000000001".into(),
            status,
            definition: None,
            item: None,
            item_key: None,
            target: TaskTarget {
                repo: "platform".into(),
                git_ref: "temp".into(),
                worktree: Some("wt-a".into()),
            },
            priority: "normal".into(),
            created: "2026-07-08T10:00:00.000Z".into(),
            source: "tui".into(),
            ephemeral_worktree: false,
            error: None,
            session: "fresh".into(),
            resume_session_id: None,
            model: None,
            prompt: "fix the flaky test\nmore context\n".into(),
        }
    }

    fn task_on(status: TaskStatus, id: &str, repo: &str, worktree: Option<&str>) -> TaskInstance {
        let mut t = make_task(status);
        t.id = id.into();
        t.target.repo = repo.into();
        t.target.worktree = worktree.map(str::to_string);
        t
    }

    fn make_session(cwd: &str, kind: &str) -> SessionEntry {
        SessionEntry {
            kind: kind.into(),
            key: format!("sess-{cwd}"),
            lane: None,
            cwd: Some(cwd.into()),
            pid: Some(4242),
            started_at: "2026-07-08T09:00:00.000Z".into(),
            heartbeat_at: "2026-07-08T10:00:00.000Z".into(),
        }
    }

    fn wt(name: &str, path: &str, branch: &str) -> WorktreeInfo {
        WorktreeInfo { name: name.into(), path: path.into(), branch: branch.into() }
    }

    fn platform_worktrees() -> HashMap<String, Vec<WorktreeInfo>> {
        HashMap::from([(
            "platform".to_string(),
            vec![
                wt("wt-a", "/wt/wt-a", "feat/a"),
                wt("wt-b", "/wt/wt-b", "feat/b"),
                wt("wt-c", "/wt/wt-c", "feat/c"),
            ],
        )])
    }

    fn snap(tasks: Vec<TaskInstance>, archived: Vec<TaskInstance>) -> StateSnapshot {
        StateSnapshot { tasks, archived_recent: archived, ..Default::default() }
    }

    fn projects(names: &[&str]) -> Vec<Project> {
        names.iter().map(|n| Project { name: n.to_string() }).collect()
    }

    /// NOW from the TS suites: Date.parse("2026-07-08T10:03:12.000Z")
    fn now() -> u64 {
        parse_iso_epoch_s("2026-07-08T10:03:12.000Z")
    }

    // ---- parse_iso_epoch_s ----

    #[test]
    fn parse_iso_epoch_anchors_and_deltas() {
        assert_eq!(parse_iso_epoch_s("1970-01-01T00:00:00.000Z"), 0);
        assert_eq!(parse_iso_epoch_s("1970-01-02T00:00:00.000Z"), 86_400);
        let a = parse_iso_epoch_s("2026-07-08T10:00:00.000Z");
        let b = parse_iso_epoch_s("2026-07-08T10:03:12.000Z");
        assert_eq!(b - a, 192);
        // leap-year sanity: 2024-02-29 is exactly one day before 2024-03-01
        assert_eq!(
            parse_iso_epoch_s("2024-03-01T00:00:00.000Z")
                - parse_iso_epoch_s("2024-02-29T00:00:00.000Z"),
            86_400
        );
    }

    // ---- build_tabs (mirrors buildProjectTabs) ----

    #[test]
    fn tabs_list_config_projects_in_order_without_synthetic() {
        let mut s = snap(vec![task_on(TaskStatus::Running, "t1", "platform", Some("wt-a"))], vec![]);
        s.projects = projects(&["platform", "web"]);
        assert_eq!(
            build_tabs(&s),
            vec![
                TabInfo { name: "platform".into(), synthetic: false },
                TabInfo { name: "web".into(), synthetic: false },
            ]
        );
    }

    #[test]
    fn tabs_append_synthetic_orphan_repos_sorted_alphabetically() {
        let mut s = snap(
            vec![
                task_on(TaskStatus::Running, "t1", "zeta", Some("wt-a")),
                task_on(TaskStatus::Queued, "t2", "platform", Some("wt-a")),
            ],
            vec![task_on(TaskStatus::Done, "t3", "alpha", Some("wt-a"))],
        );
        s.projects = projects(&["platform"]);
        assert_eq!(
            build_tabs(&s),
            vec![
                TabInfo { name: "platform".into(), synthetic: false },
                TabInfo { name: "alpha".into(), synthetic: true },
                TabInfo { name: "zeta".into(), synthetic: true },
            ]
        );
    }

    #[test]
    fn tabs_keep_config_projects_with_no_tasks() {
        let mut s = snap(vec![], vec![]);
        s.projects = projects(&["platform"]);
        assert_eq!(build_tabs(&s), vec![TabInfo { name: "platform".into(), synthetic: false }]);
    }

    // ---- queue_rows (mirrors queueRowsForProject + buildQueueRows) ----

    #[test]
    fn queue_rows_exclude_other_projects_live_and_archived() {
        let s = snap(
            vec![
                task_on(TaskStatus::Running, "01TASKAAA000000000000000000", "platform", Some("wt-a")),
                task_on(TaskStatus::Running, "01TASKBBB000000000000000000", "web", Some("wt-b")),
            ],
            vec![
                task_on(TaskStatus::Done, "01TASKCCC000000000000000000", "platform", Some("wt-a")),
                task_on(TaskStatus::Done, "01TASKDDD000000000000000000", "web", Some("wt-b")),
            ],
        );
        let rows = queue_rows(&s, "platform", now());
        assert_eq!(
            rows.iter().map(|r| r.task_id.as_str()).collect::<Vec<_>>(),
            vec!["01TASKAAA000000000000000000", "01TASKCCC000000000000000000"]
        );
        assert_eq!(rows.iter().map(|r| r.archived).collect::<Vec<_>>(), vec![false, true]);
    }

    #[test]
    fn queue_rows_detail_running_elapsed_queued_position_failed_error() {
        let running = task_on(TaskStatus::Running, "t1", "platform", Some("wt-a"));
        let q1 = task_on(TaskStatus::Queued, "t2", "platform", Some("wt-a"));
        let q2 = task_on(TaskStatus::Queued, "t3", "platform", Some("wt-a"));
        let mut failed = task_on(TaskStatus::Failed, "t4", "platform", Some("wt-a"));
        failed.error = Some("tree left dirty".into());
        let done = task_on(TaskStatus::Done, "t5", "platform", Some("wt-a"));
        let rows = queue_rows(&snap(vec![running, q1, q2, failed, done], vec![]), "platform", now());
        assert_eq!(rows[0].detail, "⏱ 3m12s");
        assert_eq!(rows[1].detail, "#1 in lane");
        assert_eq!(rows[2].detail, "#2 in lane");
        assert_eq!(rows[3].detail, "tree left dirty");
        assert_eq!(rows[4].detail, "done");
        assert_eq!(rows[0].lane, "platform:wt-a");
        assert!(rows[0].running && !rows[1].running);
        assert_eq!(
            rows.iter().map(|r| r.glyph).collect::<Vec<_>>(),
            vec!['▶', '○', '○', '✗', '✓']
        );
    }

    #[test]
    fn queue_rows_needs_input_without_error_falls_back_to_status_word() {
        let ni = task_on(TaskStatus::NeedsInput, "t1", "platform", Some("wt-a"));
        let rows = queue_rows(&snap(vec![ni], vec![]), "platform", now());
        assert_eq!(rows[0].detail, "needs-input");
        assert_eq!(rows[0].glyph, '?');
    }

    #[test]
    fn queue_rows_use_ref_as_lane_when_worktree_unresolved_and_append_archived() {
        let mut pending = task_on(TaskStatus::Queued, "t1", "platform", None);
        pending.target.git_ref = "pr:257".into();
        let old = task_on(TaskStatus::Done, "t0", "platform", Some("wt-a"));
        let rows = queue_rows(&snap(vec![pending], vec![old]), "platform", now());
        assert_eq!(rows[0].lane, "platform:pr:257");
        assert!(rows[1].archived);
        assert_eq!(rows[1].detail, "archived");
    }

    #[test]
    fn queue_rows_cap_archived_at_last_10() {
        let archived: Vec<TaskInstance> = (0..15)
            .map(|i| task_on(TaskStatus::Done, &format!("t{i:02}"), "platform", Some("wt-a")))
            .collect();
        let rows = queue_rows(&snap(vec![], archived), "platform", now());
        assert_eq!(rows.len(), 10);
        assert_eq!(rows[0].task_id, "t05"); // last 10 → t05..t14
    }

    #[test]
    fn queue_rows_mark_main_session_tasks_live_and_archived() {
        let mut main_task = task_on(TaskStatus::Running, "t1", "platform", Some("wt-a"));
        main_task.session = "main".into();
        let fresh = task_on(TaskStatus::Queued, "t2", "platform", Some("wt-a"));
        let mut archived_main = task_on(TaskStatus::Done, "t3", "platform", Some("wt-a"));
        archived_main.session = "main".into();
        let rows = queue_rows(&snap(vec![main_task, fresh], vec![archived_main]), "platform", now());
        assert!(rows[0].main_session);
        assert!(!rows[1].main_session);
        assert!(rows[2].main_session);
    }

    #[test]
    fn queue_rows_strip_repo_prefix_in_lane() {
        let running = task_on(
            TaskStatus::Running,
            "t1",
            "platform",
            Some("platform.dedup-dependabot-run"),
        );
        let rows = queue_rows(&snap(vec![running], vec![]), "platform", now());
        assert_eq!(rows[0].lane, "platform:dedup-dependabot-run");
    }

    // ---- elapsed_label / prompt_summary / strip_repo_prefix / lane_key / arg_summary ----

    #[test]
    fn elapsed_label_formats_seconds_minutes_hours() {
        assert_eq!(elapsed_label(0, 47), "⏱ 47s");
        assert_eq!(elapsed_label(0, 192), "⏱ 3m12s");
        assert_eq!(elapsed_label(0, 303), "⏱ 5m03s"); // zero-padded seconds
        assert_eq!(elapsed_label(0, 3840), "⏱ 1h04m"); // zero-padded minutes
        assert_eq!(elapsed_label(100, 50), "⏱ 0s"); // clock skew clamps to 0
    }

    #[test]
    fn prompt_summary_first_non_blank_line_clipped_at_60() {
        assert_eq!(prompt_summary("\n\nfix the thing\nrest"), "fix the thing");
        assert_eq!(prompt_summary(""), "");
        let long = "a".repeat(70);
        let expected = format!("{}…", "a".repeat(59));
        assert_eq!(prompt_summary(&long), expected);
        assert_eq!(prompt_summary(&"a".repeat(60)), "a".repeat(60)); // exactly 60 fits
    }

    #[test]
    fn strip_repo_prefix_cases() {
        assert_eq!(strip_repo_prefix("platform.dedup-dependabot-run", "platform"), "dedup-dependabot-run");
        assert_eq!(strip_repo_prefix("platform", "platform"), "platform"); // bare repo kept
        assert_eq!(strip_repo_prefix("wt-a", "platform"), "wt-a"); // unprefixed kept
    }

    #[test]
    fn lane_key_joins_repo_and_worktree() {
        assert_eq!(lane_key("platform", "wt-a"), "platform:wt-a");
    }

    #[test]
    fn arg_summary_names_and_defaults() {
        let args = vec![
            ArgSpec { name: "pr".into(), default: None, options: None, description: None },
            ArgSpec { name: "mode".into(), default: Some("ready".into()), options: None, description: None },
            ArgSpec { name: "review".into(), default: Some("auto".into()), options: None, description: None },
        ];
        assert_eq!(arg_summary(&args), "pr, mode=ready, review=auto");
        assert_eq!(arg_summary(&[]), "");
    }

    // ---- worktree_rows (mirrors buildWorktreeRows) ----

    #[test]
    fn worktree_busy_when_running_task_shares_lane() {
        let mut s = snap(vec![task_on(TaskStatus::Running, "t1", "platform", Some("wt-a"))], vec![]);
        s.worktrees = platform_worktrees();
        let rows = worktree_rows(&s, "platform");
        assert_eq!(rows.iter().find(|r| r.name == "wt-a").unwrap().state, WtState::Busy);
    }

    #[test]
    fn worktree_failed_when_newest_lane_task_failed() {
        let mut s = snap(
            vec![
                task_on(TaskStatus::Done, "01TASKB00000000000000000001", "platform", Some("wt-b")),
                task_on(TaskStatus::Failed, "01TASKB00000000000000000002", "platform", Some("wt-b")),
            ],
            vec![],
        );
        s.worktrees = platform_worktrees();
        let rows = worktree_rows(&s, "platform");
        assert_eq!(rows.iter().find(|r| r.name == "wt-b").unwrap().state, WtState::Failed);
    }

    #[test]
    fn worktree_free_when_newest_lane_task_not_failed() {
        let mut s = snap(
            vec![
                task_on(TaskStatus::Failed, "01TASKB00000000000000000001", "platform", Some("wt-c")),
                task_on(TaskStatus::Done, "01TASKB00000000000000000002", "platform", Some("wt-c")),
            ],
            vec![],
        );
        s.worktrees = platform_worktrees();
        let rows = worktree_rows(&s, "platform");
        assert_eq!(rows.iter().find(|r| r.name == "wt-c").unwrap().state, WtState::Free);
    }

    #[test]
    fn worktree_running_beats_newer_failed_task() {
        let mut s = snap(
            vec![
                task_on(TaskStatus::Running, "01TASKB00000000000000000001", "platform", Some("wt-a")),
                task_on(TaskStatus::Failed, "01TASKB00000000000000000009", "platform", Some("wt-a")),
            ],
            vec![],
        );
        s.worktrees = platform_worktrees();
        let rows = worktree_rows(&s, "platform");
        assert_eq!(rows.iter().find(|r| r.name == "wt-a").unwrap().state, WtState::Busy);
    }

    #[test]
    fn worktree_rows_emitted_in_order_with_full_fields() {
        let mut s = snap(vec![], vec![]);
        s.worktrees = platform_worktrees();
        let rows = worktree_rows(&s, "platform");
        assert_eq!(
            rows,
            vec![
                WorktreeRow {
                    name: "wt-a".into(), raw_name: "wt-a".into(), path: "/wt/wt-a".into(),
                    branch: "feat/a".into(), state: WtState::Free, has_main_session: false,
                    queued: 0, is_session: false,
                },
                WorktreeRow {
                    name: "wt-b".into(), raw_name: "wt-b".into(), path: "/wt/wt-b".into(),
                    branch: "feat/b".into(), state: WtState::Free, has_main_session: false,
                    queued: 0, is_session: false,
                },
                WorktreeRow {
                    name: "wt-c".into(), raw_name: "wt-c".into(), path: "/wt/wt-c".into(),
                    branch: "feat/c".into(), state: WtState::Free, has_main_session: false,
                    queued: 0, is_session: false,
                },
            ]
        );
    }

    #[test]
    fn worktree_flags_main_session_lanes() {
        let mut s = snap(vec![], vec![]);
        s.worktrees = platform_worktrees();
        s.main_sessions = HashMap::from([("platform:wt-b".to_string(), "sess-main".to_string())]);
        let rows = worktree_rows(&s, "platform");
        assert!(!rows.iter().find(|r| r.name == "wt-a").unwrap().has_main_session);
        assert!(rows.iter().find(|r| r.name == "wt-b").unwrap().has_main_session);
    }

    #[test]
    fn session_row_appended_for_interactive_cwd_inside_worktree() {
        let mut s = snap(vec![], vec![]);
        s.worktrees = platform_worktrees();
        s.sessions = vec![
            make_session("/wt/wt-b/packages/tui", "interactive"),
            make_session("/elsewhere/repo", "interactive"),
            make_session("/wt/wt-a", "worker"),
        ];
        let rows = worktree_rows(&s, "platform");
        let sessions: Vec<&WorktreeRow> = rows.iter().filter(|r| r.is_session).collect();
        assert_eq!(sessions.len(), 1);
        let row = sessions[0];
        assert_eq!(row.name, "tui");
        assert_eq!(row.raw_name, "tui");
        assert_eq!(row.path, "/wt/wt-b/packages/tui");
        assert_eq!(row.branch, "");
        assert_eq!(row.state, WtState::You);
        assert_eq!(row.queued, 0);
    }

    #[test]
    fn session_row_matches_exact_cwd_but_not_sibling_prefix() {
        let mut s = snap(vec![], vec![]);
        s.worktrees = platform_worktrees();
        s.sessions = vec![make_session("/wt/wt-a", "interactive")];
        let rows = worktree_rows(&s, "platform");
        assert_eq!(rows.iter().filter(|r| r.is_session).count(), 1);

        s.sessions = vec![make_session("/wt/wt-a-sibling", "interactive")];
        let rows = worktree_rows(&s, "platform");
        assert_eq!(rows.iter().filter(|r| r.is_session).count(), 0);
    }

    #[test]
    fn no_rows_for_project_without_worktrees() {
        let mut s = snap(vec![], vec![]);
        s.worktrees = platform_worktrees();
        assert_eq!(worktree_rows(&s, "web"), vec![]);
    }

    #[test]
    fn worktree_rows_strip_repo_prefix_but_keep_raw_name() {
        let mut s = snap(vec![], vec![]);
        s.worktrees = HashMap::from([(
            "platform".to_string(),
            vec![
                wt("platform", "/wt/platform", "main"),
                wt("platform.dedup-dependabot-run", "/wt/platform.dedup-dependabot-run", "dedup-dependabot-run"),
            ],
        )]);
        let rows = worktree_rows(&s, "platform");
        assert_eq!(
            rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            vec!["platform", "dedup-dependabot-run"]
        );
        assert_eq!(
            rows.iter().map(|r| r.raw_name.as_str()).collect::<Vec<_>>(),
            vec!["platform", "platform.dedup-dependabot-run"]
        );
        assert_eq!(rows[1].path, "/wt/platform.dedup-dependabot-run");
    }

    #[test]
    fn session_row_display_name_strips_repo_prefix() {
        let mut s = snap(vec![], vec![]);
        s.worktrees = HashMap::from([(
            "platform".to_string(),
            vec![wt("platform.feat-x", "/wt/platform.feat-x", "feat-x")],
        )]);
        s.sessions = vec![make_session("/wt/platform.feat-x", "interactive")];
        let rows = worktree_rows(&s, "platform");
        let session = rows.iter().find(|r| r.is_session).unwrap();
        assert_eq!(session.name, "feat-x");
        assert_eq!(session.raw_name, "feat-x"); // mirrors display name — never dispatched
    }

    #[test]
    fn worktree_counts_queued_tasks_per_lane() {
        let mut s = snap(
            vec![
                task_on(TaskStatus::Queued, "01TASKQ00000000000000000001", "platform", Some("wt-a")),
                task_on(TaskStatus::Queued, "01TASKQ00000000000000000002", "platform", Some("wt-a")),
                task_on(TaskStatus::Running, "01TASKQ00000000000000000003", "platform", Some("wt-b")),
            ],
            vec![],
        );
        s.worktrees = platform_worktrees();
        let rows = worktree_rows(&s, "platform");
        assert_eq!(rows.iter().find(|r| r.name == "wt-a").unwrap().queued, 2);
        assert_eq!(rows.iter().find(|r| r.name == "wt-b").unwrap().queued, 0);
        assert_eq!(rows.iter().find(|r| r.name == "wt-c").unwrap().queued, 0);
    }

    // ---- pane_layout (mirrors computePaneLayout) ----

    #[test]
    fn pane_layout_sums_exactly_to_body_height() {
        for body in [13u16, 20, 38, 50, 77] {
            let l = pane_layout(body);
            assert_eq!(l.queue_h + l.tasks_h + l.worktrees_h, body, "body={body}");
        }
    }

    #[test]
    fn pane_layout_gives_queue_half_and_lists_quarter_each() {
        let l = pane_layout(38);
        assert_eq!(l.tasks_h, 9);
        assert_eq!(l.worktrees_h, 9);
        assert_eq!(l.queue_h, 20);
    }

    #[test]
    fn pane_layout_keeps_minimums_for_tiny_body() {
        let l = pane_layout(1);
        assert!(l.tasks_h >= 4);
        assert!(l.worktrees_h >= 4);
        assert!(l.queue_h >= 4);
    }

    // ---- window_rows (mirrors windowRows) ----

    #[test]
    fn window_rows_edges_and_centering() {
        assert_eq!(window_rows(10, 3, 20), (0, 10)); // all rows fit
        assert_eq!(window_rows(10, 0, 4), (0, 4)); // top edge
        assert_eq!(window_rows(10, 5, 4), (3, 7)); // centered
        assert_eq!(window_rows(10, 9, 4), (6, 10)); // bottom edge
        assert_eq!(window_rows(10, 3, 0), (0, 0)); // non-positive capacity
        assert_eq!(window_rows(0, 0, 4), (0, 0)); // empty list
        assert_eq!(window_rows(10, 99, 4), (6, 10)); // out-of-range cursor clamps
    }

    // ---- pane_title (mirrors paneTitle incl. selection count) ----

    #[test]
    fn pane_title_variants() {
        let single = Selection { cursor: 0, anchor: None };
        assert_eq!(pane_title("QUEUE", &single, "", false), "QUEUE");
        assert_eq!(pane_title("QUEUE", &single, "foo", false), "QUEUE /foo");
        assert_eq!(pane_title("QUEUE", &single, "fo", true), "QUEUE /fo█");
        assert_eq!(pane_title("QUEUE", &single, "", true), "QUEUE /█");
    }

    #[test]
    fn pane_title_selection_count() {
        let three = Selection { cursor: 4, anchor: Some(2) }; // rows 2..=4
        assert_eq!(pane_title("WORKTREES", &three, "", false), "WORKTREES · 3 selected");
        let two = Selection { cursor: 1, anchor: Some(2) };
        assert_eq!(pane_title("WORKTREES", &two, "tmp", false), "WORKTREES · 2 selected /tmp");
        let anchored_single = Selection { cursor: 3, anchor: Some(3) };
        assert_eq!(pane_title("QUEUE", &anchored_single, "", false), "QUEUE");
    }

    // ---- filter_rows (mirrors matchesFilter) ----

    #[test]
    fn filter_rows_empty_query_matches_everything_else_ci_substring() {
        let rows = vec!["Fix-TUI-Bug".to_string(), "other".to_string(), "fix-tui-bug".to_string()];
        assert_eq!(filter_rows(&rows, "", |r| r.clone()), vec![0, 1, 2]);
        assert_eq!(filter_rows(&rows, "tui", |r| r.clone()), vec![0, 2]);
        assert_eq!(filter_rows(&rows, "TUI", |r| r.clone()), vec![0, 2]);
        assert_eq!(filter_rows(&rows, "xyz", |r| r.clone()), Vec::<usize>::new());
    }
}
```

Run `cargo test -p qoo-tui selectors` → **expected FAIL** (every test panics on `unimplemented!()`).

- [ ] **Step 3 (GREEN): Implement every selector.** Replace the stub bodies in `selectors.rs`:

```rust
pub fn build_tabs(snapshot: &StateSnapshot) -> Vec<TabInfo> {
    let configured: HashSet<&str> =
        snapshot.projects.iter().map(|p| p.name.as_str()).collect();
    let mut tabs: Vec<TabInfo> = snapshot
        .projects
        .iter()
        .map(|p| TabInfo { name: p.name.clone(), synthetic: false })
        .collect();
    // Repos seen in tasks/archived but absent from config → synthetic tabs,
    // sorted alphabetically after the configured ones.
    let mut orphans: Vec<String> = Vec::new();
    for task in snapshot.tasks.iter().chain(snapshot.archived_recent.iter()) {
        let repo = &task.target.repo;
        if !configured.contains(repo.as_str()) && !orphans.contains(repo) {
            orphans.push(repo.clone());
        }
    }
    orphans.sort();
    for name in orphans {
        tabs.push(TabInfo { name, synthetic: true });
    }
    tabs
}

fn status_glyph(status: TaskStatus) -> char {
    match status {
        TaskStatus::Running => '▶',
        TaskStatus::Queued => '○',
        TaskStatus::NeedsInput => '?',
        TaskStatus::Done => '✓',
        TaskStatus::Failed => '✗',
        TaskStatus::Unknown => '·', // no TS counterpart (old-daemon statuses only)
    }
}

fn status_str(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Queued => "queued",
        TaskStatus::NeedsInput => "needs-input",
        TaskStatus::Running => "running",
        TaskStatus::Done => "done",
        TaskStatus::Failed => "failed",
        TaskStatus::Unknown => "unknown",
    }
}

/// `repo:worktree-or-ref` with the redundant `<repo>.` display prefix stripped.
fn lane_label(task: &TaskInstance) -> String {
    let lane = task.target.worktree.as_deref().unwrap_or(&task.target.git_ref);
    format!("{}:{}", task.target.repo, strip_repo_prefix(lane, &task.target.repo))
}

pub fn queue_rows(snapshot: &StateSnapshot, project: &str, now_epoch_s: u64) -> Vec<QueueRow> {
    // Live rows in snapshot order (the daemon stores by creation), then the
    // last 10 archived rows, dimmed by the view via `archived: true`.
    let mut queued_position: HashMap<String, usize> = HashMap::new();
    let mut rows: Vec<QueueRow> = Vec::new();
    for task in snapshot.tasks.iter().filter(|t| t.target.repo == project) {
        let detail = match task.status {
            TaskStatus::Running => {
                elapsed_label(parse_iso_epoch_s(&task.created), now_epoch_s)
            }
            TaskStatus::Queued => {
                let lane = lane_label(task);
                let position = queued_position.get(&lane).copied().unwrap_or(0) + 1;
                queued_position.insert(lane, position);
                format!("#{position} in lane")
            }
            TaskStatus::NeedsInput | TaskStatus::Failed => task
                .error
                .clone()
                .unwrap_or_else(|| status_str(task.status).to_string()),
            TaskStatus::Done => "done".to_string(),
            TaskStatus::Unknown => status_str(task.status).to_string(),
        };
        rows.push(QueueRow {
            task_id: task.id.clone(),
            glyph: status_glyph(task.status),
            running: task.status == TaskStatus::Running,
            main_session: task.session == "main",
            lane: lane_label(task),
            summary: prompt_summary(&task.prompt),
            detail,
            archived: false,
        });
    }
    let archived: Vec<&TaskInstance> = snapshot
        .archived_recent
        .iter()
        .filter(|t| t.target.repo == project)
        .collect();
    let start = archived.len().saturating_sub(10);
    for task in &archived[start..] {
        rows.push(QueueRow {
            task_id: task.id.clone(),
            glyph: status_glyph(task.status),
            running: false,
            main_session: task.session == "main",
            lane: lane_label(task),
            summary: prompt_summary(&task.prompt),
            detail: "archived".to_string(),
            archived: true,
        });
    }
    rows
}

/// `repo:worktree` from a task's target; None while the worktree is unresolved
/// (mirror of core's laneKey — raw identifiers, no display stripping).
fn task_lane(task: &TaskInstance) -> Option<String> {
    task.target
        .worktree
        .as_ref()
        .map(|wt| format!("{}:{}", task.target.repo, wt))
}

fn worktree_state(snapshot: &StateSnapshot, lane: &str) -> WtState {
    let on_lane: Vec<&TaskInstance> = snapshot
        .tasks
        .iter()
        .filter(|t| task_lane(t).as_deref() == Some(lane))
        .collect();
    if on_lane.iter().any(|t| t.status == TaskStatus::Running) {
        return WtState::Busy;
    }
    // newest by id — ULIDs sort chronologically
    match on_lane.iter().max_by(|a, b| a.id.cmp(&b.id)) {
        Some(t) if t.status == TaskStatus::Failed => WtState::Failed,
        _ => WtState::Free,
    }
}

fn queued_on_lane(snapshot: &StateSnapshot, lane: &str) -> usize {
    snapshot
        .tasks
        .iter()
        .filter(|t| task_lane(t).as_deref() == Some(lane) && t.status == TaskStatus::Queued)
        .count()
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

pub fn worktree_rows(snapshot: &StateSnapshot, project: &str) -> Vec<WorktreeRow> {
    let empty: Vec<crate::ipc::types::WorktreeInfo> = Vec::new();
    let worktrees = snapshot.worktrees.get(project).unwrap_or(&empty);
    let mut rows: Vec<WorktreeRow> = worktrees
        .iter()
        .map(|wt| {
            let lane = lane_key(project, &wt.name);
            WorktreeRow {
                name: strip_repo_prefix(&wt.name, project).to_string(),
                raw_name: wt.name.clone(),
                path: wt.path.clone(),
                branch: wt.branch.clone(),
                state: worktree_state(snapshot, &lane),
                has_main_session: snapshot.main_sessions.contains_key(&lane),
                queued: queued_on_lane(snapshot, &lane),
                is_session: false,
            }
        })
        .collect();

    // One "You" row per interactive session whose cwd is inside a project
    // worktree (exact path or path + "/" prefix — never a sibling).
    for session in &snapshot.sessions {
        if session.kind != "interactive" {
            continue;
        }
        let Some(cwd) = session.cwd.as_deref() else { continue };
        let inside = worktrees
            .iter()
            .any(|wt| cwd == wt.path || cwd.starts_with(&format!("{}/", wt.path)));
        if !inside {
            continue;
        }
        // A session is not a real worktree: rawName mirrors the display name and
        // is never dispatched to the daemon as a worktree identifier.
        let display = strip_repo_prefix(basename(cwd), project).to_string();
        rows.push(WorktreeRow {
            name: display.clone(),
            raw_name: display,
            path: cwd.to_string(),
            branch: String::new(),
            state: WtState::You,
            has_main_session: false,
            queued: 0,
            is_session: true,
        });
    }
    rows
}

pub fn pane_layout(body_height: u16) -> PaneLayout {
    // queue : tasks : worktrees ≈ 2:1:1, explicit heights (no flex-grow) so a
    // pane never balloons past its capped content. Row capacity per pane is
    // height − 3 (border + title chrome), computed by the view.
    let list_h = std::cmp::max(4, body_height / 4);
    let queue_h = std::cmp::max(4, body_height.saturating_sub(2 * list_h));
    PaneLayout { queue_h, tasks_h: list_h, worktrees_h: list_h }
}

pub fn window_rows(len: usize, cursor: usize, capacity: usize) -> (usize, usize) {
    if capacity == 0 || len == 0 {
        return (0, 0);
    }
    if len <= capacity {
        return (0, len);
    }
    let clamped = cursor.min(len - 1);
    let start = clamped.saturating_sub(capacity / 2).min(len - capacity);
    (start, start + capacity)
}

pub fn pane_title(base: &str, sel: &Selection, filter: &str, searching: bool) -> String {
    let selected = match sel.anchor {
        Some(anchor) => anchor.abs_diff(sel.cursor) + 1,
        None => 1,
    };
    let title = if selected > 1 {
        format!("{base} · {selected} selected")
    } else {
        base.to_string()
    };
    if !searching && filter.is_empty() {
        return title;
    }
    let cursor = if searching { "█" } else { "" };
    format!("{title} /{filter}{cursor}")
}

pub fn filter_rows<'a, T>(rows: &'a [T], filter: &str, text_of: impl Fn(&T) -> String) -> Vec<usize> {
    if filter.is_empty() {
        return (0..rows.len()).collect();
    }
    let needle = filter.to_lowercase();
    rows.iter()
        .enumerate()
        .filter(|(_, row)| text_of(row).to_lowercase().contains(&needle))
        .map(|(i, _)| i)
        .collect()
}

pub fn arg_summary(args: &[ArgSpec]) -> String {
    args.iter()
        .map(|a| match &a.default {
            Some(d) => format!("{}={}", a.name, d),
            None => a.name.clone(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn strip_repo_prefix<'a>(worktree: &'a str, repo: &str) -> &'a str {
    match worktree.strip_prefix(repo) {
        Some(rest) => match rest.strip_prefix('.') {
            Some(stripped) => stripped,
            None => worktree, // bare repo name or shared prefix without the dot
        },
        None => worktree,
    }
}

pub fn lane_key(repo: &str, worktree: &str) -> String {
    format!("{repo}:{worktree}")
}

pub fn prompt_summary(prompt: &str) -> String {
    let line = prompt
        .lines()
        .find(|l| !l.trim().is_empty())
        .map(str::trim)
        .unwrap_or("");
    let chars: Vec<char> = line.chars().collect();
    if chars.len() <= 60 {
        return line.to_string();
    }
    let mut out: String = chars[..59].iter().collect();
    out.push('…');
    out
}

pub fn elapsed_label(created_epoch_s: u64, now_epoch_s: u64) -> String {
    let total = now_epoch_s.saturating_sub(created_epoch_s);
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;
    if hours > 0 {
        format!("⏱ {hours}h{minutes:02}m")
    } else if minutes > 0 {
        format!("⏱ {minutes}m{seconds:02}s")
    } else {
        format!("⏱ {seconds}s")
    }
}

fn parse_iso_epoch_s(iso: &str) -> u64 {
    if iso.len() < 19 {
        return 0;
    }
    let num = |s: &str| s.parse::<i64>().unwrap_or(0);
    let (y, m, d) = (num(&iso[0..4]), num(&iso[5..7]), num(&iso[8..10]));
    let (hh, mm, ss) = (num(&iso[11..13]), num(&iso[14..16]), num(&iso[17..19]));
    let secs = days_from_civil(y, m, d) * 86_400 + hh * 3600 + mm * 60 + ss;
    if secs < 0 { 0 } else { secs as u64 }
}

/// Days since 1970-01-01 for a proleptic-Gregorian civil date
/// (Howard Hinnant's `days_from_civil`).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}
```

Run `cargo test -p qoo-tui selectors` → **expected PASS (26 tests)**.

- [ ] **Step 4: Full-suite verify + commit.** `cargo test -p qoo-tui` → **expected PASS**; `cargo build --release -p qoo-tui` → **expected: compiles**.

```
git add crates/qoo-tui/src/lib.rs crates/qoo-tui/src/selectors.rs
git commit -m "feat(tui-rs): port selectors + format (tabs, queue/worktree rows, layout math)"
```

---

### Task 6: `markup.rs` — markdown-lite line styler

**Files:**
- Create `crates/qoo-tui/src/markup.rs`
- Create `crates/qoo-tui/src/view/mod.rs`
- Create `crates/qoo-tui/src/view/theme.rs` (Palette subset — Task 7 extends it)
- Modify `crates/qoo-tui/src/lib.rs` (add `pub mod markup;` and `pub mod view;`)
- Test: `crates/qoo-tui/src/markup.rs` (inline `#[cfg(test)] mod tests` — mirrors `packages/tui/src/__tests__/markup.test.ts`)

**Interfaces:**
- Produces `pub fn style_line(line: &str, p: &Palette) -> ratatui::text::Line<'static>` (exact contract signature).
- Produces (subset) `view::theme::Palette { code: Color, link: Color }` with `Default` (cyan/blue) — Task 7 adds the full color set + glyph consts; fields only ever added.
- Consumes: ratatui `Line`/`Span`/`Style`.
- Styling contract (mirror of markup.ts): whole-line rules win — `#`/`##`/`###` + whitespace → whole line bold with markers stripped (4+ hashes are plain), `---+` (3+ dashes only) → dim; otherwise inline precedence at each position is `**bold**` → `` `code` `` (cyan) → bare `http(s)://…` URL (blue); everything else plain. Always at least one span. One `Line` per input line — no wrapping (scroll math depends on it).

**Steps:**

- [ ] **Step 1: Register modules + Palette subset.** In `lib.rs` add:

```rust
pub mod markup;
pub mod view;
```

Create `crates/qoo-tui/src/view/mod.rs`:

```rust
pub mod theme;
```

Create `crates/qoo-tui/src/view/theme.rs`:

```rust
use ratatui::style::Color;

/// Central palette — every color the UI uses lives here (no inline literals in
/// components). Task 7 extends it with the full pane/status color set and the
/// glyph consts; fields are only ever added, never renamed.
#[derive(Debug, Clone)]
pub struct Palette {
    /// inline `code` spans in the detail pane
    pub code: Color,
    /// bare http(s) URLs
    pub link: Color,
}

impl Default for Palette {
    fn default() -> Self {
        Self { code: Color::Cyan, link: Color::Blue }
    }
}
```

- [ ] **Step 2 (RED): Write `markup.rs` with stub body + full mirrored tests.** Create `crates/qoo-tui/src/markup.rs`:

```rust
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::view::theme::Palette;

/// Style one detail-pane line (port of markup.ts styleLine). Whole-line rules
/// (headings, horizontal rules) win; otherwise the line is tokenized into
/// **bold** / `code` / URL spans with surrounding text plain. Returns an owned
/// Line — always at least one span.
pub fn style_line(line: &str, p: &Palette) -> Line<'static> {
    let _ = (line, p);
    unimplemented!()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts(line: &Line) -> Vec<(String, Style)> {
        line.spans
            .iter()
            .map(|s| (s.content.to_string(), s.style))
            .collect()
    }

    fn bold() -> Style {
        Style::default().add_modifier(Modifier::BOLD)
    }
    fn dim() -> Style {
        Style::default().add_modifier(Modifier::DIM)
    }
    fn plain() -> Style {
        Style::default()
    }
    fn code(p: &Palette) -> Style {
        Style::default().fg(p.code)
    }
    fn link(p: &Palette) -> Style {
        Style::default().fg(p.link)
    }

    #[test]
    fn bolds_headings_and_strips_markers() {
        let p = Palette::default();
        assert_eq!(parts(&style_line("## Findings", &p)), vec![("Findings".into(), bold())]);
        assert_eq!(parts(&style_line("# Title", &p)), vec![("Title".into(), bold())]);
        assert_eq!(parts(&style_line("### Deep", &p)), vec![("Deep".into(), bold())]);
    }

    #[test]
    fn four_hashes_or_no_space_are_not_headings() {
        let p = Palette::default();
        assert_eq!(parts(&style_line("#### Four", &p)), vec![("#### Four".into(), plain())]);
        assert_eq!(parts(&style_line("#hash", &p)), vec![("#hash".into(), plain())]);
    }

    #[test]
    fn dims_a_horizontal_rule_of_three_or_more_dashes() {
        let p = Palette::default();
        assert_eq!(parts(&style_line("---", &p)), vec![("---".into(), dim())]);
        assert_eq!(parts(&style_line("-----", &p)), vec![("-----".into(), dim())]);
        assert_eq!(parts(&style_line("--", &p)), vec![("--".into(), plain())]);
    }

    #[test]
    fn plain_text_is_a_single_plain_segment() {
        let p = Palette::default();
        assert_eq!(
            parts(&style_line("just some text", &p)),
            vec![("just some text".into(), plain())]
        );
    }

    #[test]
    fn bolds_double_star_spans_and_strips_markers() {
        let p = Palette::default();
        assert_eq!(
            parts(&style_line("see **Full report:** here", &p)),
            vec![
                ("see ".into(), plain()),
                ("Full report:".into(), bold()),
                (" here".into(), plain()),
            ]
        );
    }

    #[test]
    fn colors_inline_code_cyan_and_strips_backticks() {
        let p = Palette::default();
        assert_eq!(
            parts(&style_line("call `foo.py:275` now", &p)),
            vec![
                ("call ".into(), plain()),
                ("foo.py:275".into(), code(&p)),
                (" now".into(), plain()),
            ]
        );
    }

    #[test]
    fn colors_urls_blue() {
        let p = Palette::default();
        assert_eq!(
            parts(&style_line("link https://example.com/x done", &p)),
            vec![
                ("link ".into(), plain()),
                ("https://example.com/x".into(), link(&p)),
                (" done".into(), plain()),
            ]
        );
    }

    #[test]
    fn styles_multiple_spans_in_one_line() {
        let p = Palette::default();
        assert_eq!(
            parts(&style_line("**Full report:** `pr.md` at https://x.io", &p)),
            vec![
                ("Full report:".into(), bold()),
                (" ".into(), plain()),
                ("pr.md".into(), code(&p)),
                (" at ".into(), plain()),
                ("https://x.io".into(), link(&p)),
            ]
        );
    }

    #[test]
    fn unclosed_bold_stays_plain() {
        let p = Palette::default();
        assert_eq!(
            parts(&style_line("a **b never closes", &p)),
            vec![("a **b never closes".into(), plain())]
        );
    }

    #[test]
    fn returns_one_segment_for_an_empty_line() {
        let p = Palette::default();
        assert_eq!(parts(&style_line("", &p)), vec![("".into(), plain())]);
    }
}
```

Run `cargo test -p qoo-tui markup` → **expected FAIL** (all 10 tests panic on `unimplemented!()`).

- [ ] **Step 3 (GREEN): Implement the styler.** Replace the `style_line` body and add the private helpers in `markup.rs`:

```rust
pub fn style_line(line: &str, p: &Palette) -> Line<'static> {
    if let Some(text) = heading_text(line) {
        return Line::from(Span::styled(
            text.to_string(),
            Style::default().add_modifier(Modifier::BOLD),
        ));
    }
    if is_rule(line) {
        return Line::from(Span::styled(
            line.to_string(),
            Style::default().add_modifier(Modifier::DIM),
        ));
    }

    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut last = 0usize;
    let mut i = 0usize;
    while i < line.len() {
        if !line.is_char_boundary(i) {
            i += 1;
            continue;
        }
        if let Some((end, span)) = match_token(line, i, p) {
            if i > last {
                spans.push(Span::raw(line[last..i].to_string()));
            }
            spans.push(span);
            last = end;
            i = end;
        } else {
            i += 1;
        }
    }
    if last < line.len() {
        spans.push(Span::raw(line[last..].to_string()));
    }
    if spans.is_empty() {
        spans.push(Span::raw(line.to_string()));
    }
    Line::from(spans)
}

/// `^#{1,3}\s+(.*)$` — 1–3 hashes followed by ≥1 whitespace; returns the text
/// after the whitespace run. 4+ hashes or no whitespace → not a heading.
fn heading_text(line: &str) -> Option<&str> {
    let hashes = line.bytes().take_while(|&b| b == b'#').count();
    if !(1..=3).contains(&hashes) {
        return None;
    }
    let rest = &line[hashes..];
    let trimmed = rest.trim_start();
    if trimmed.len() == rest.len() {
        return None; // no whitespace after the markers
    }
    Some(trimmed)
}

/// `^---+$` — three or more dashes, nothing else.
fn is_rule(line: &str) -> bool {
    line.len() >= 3 && line.bytes().all(|b| b == b'-')
}

/// Try to match an inline token starting exactly at byte `i`. Precedence order
/// mirrors the TS alternation: **bold**, then `code`, then URL.
fn match_token(line: &str, i: usize, p: &Palette) -> Option<(usize, Span<'static>)> {
    let rest = &line[i..];
    // \*\*[^*]+\*\* — star-free, non-empty content between double stars
    if let Some(inner) = rest.strip_prefix("**") {
        if let Some(close) = inner.find("**") {
            let content = &inner[..close];
            if !content.is_empty() && !content.contains('*') {
                let end = i + 2 + close + 2;
                return Some((
                    end,
                    Span::styled(
                        content.to_string(),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                ));
            }
        }
    }
    // `[^`]+` — non-empty content between backticks
    if let Some(inner) = rest.strip_prefix('`') {
        if let Some(close) = inner.find('`') {
            if close > 0 {
                let content = &inner[..close];
                let end = i + 1 + close + 1;
                return Some((end, Span::styled(content.to_string(), Style::default().fg(p.code))));
            }
        }
    }
    // https?://[^\s)>\]"']+
    if rest.starts_with("http://") || rest.starts_with("https://") {
        let stop = rest
            .find(|c: char| c.is_whitespace() || matches!(c, ')' | '>' | ']' | '"' | '\''))
            .unwrap_or(rest.len());
        return Some((i + stop, Span::styled(rest[..stop].to_string(), Style::default().fg(p.link))));
    }
    None
}
```

Run `cargo test -p qoo-tui markup` → **expected PASS (10 tests)**.

Version note: in ratatui 0.29, `Line::spans` is a public `Vec<Span>` field and `Span::content` is a `Cow<str>` — the test helpers above rely on both; if a future ratatui privatizes them, switch the helper to `line.iter()`.

- [ ] **Step 4: Full-suite verify + commit.** `cargo test -p qoo-tui` → **expected PASS (all modules)**; `cargo build --release -p qoo-tui` → **expected: compiles**.

```
git add crates/qoo-tui/src/lib.rs crates/qoo-tui/src/markup.rs crates/qoo-tui/src/view/mod.rs crates/qoo-tui/src/view/theme.rs
git commit -m "feat(tui-rs): markdown-lite line styler for the detail pane"
```
### Task 7: view/theme.rs + hit.rs

**Files:**
- Create: `crates/qoo-tui/src/view/theme.rs`
- Create: `crates/qoo-tui/src/hit.rs`
- Test: `crates/qoo-tui/src/hit.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Produces (`view/theme.rs`): `pub struct Palette { pub accent, border, border_focused, dim, error, ok, warn, info, fg, selection_fg, selection_bg: ratatui::style::Color }`; `impl Default for Palette`; `impl Palette { pub fn selection(&self) -> Style; pub fn dim_style(&self) -> Style; pub fn border_style(&self, focused: bool) -> Style }`; glyph consts `GLYPH_QUEUED '○'`, `GLYPH_NEEDS_INPUT '?'`, `GLYPH_DONE '✓'`, `GLYPH_FAILED '✗'`, `GLYPH_RUNNING '▶'`, `GLYPH_MAIN_SESSION '⛓'`, `GLYPH_MAIN_WT '◆'`, `GLYPH_DISCOVERY '⏰'`.
- Produces (`hit.rs`, per contract): `pub enum ButtonKind { Confirm, Cancel }`; `pub enum HitTarget { Tab(usize), Row(ListPane, usize), PaneBody(PaneId), SubTab(usize), MenuItem(usize), FormField(usize), DropdownItem(usize), Button(ButtonKind), ScrollbarThumb(PaneId), ScrollbarTrack(PaneId), Modal }`; `pub struct HitMap`; `impl HitMap { pub fn new(); pub fn push(&mut self, Rect, HitTarget); pub fn hit(&self, u16, u16) -> Option<&HitTarget>; pub fn len(&self) -> usize; pub fn is_empty(&self) -> bool; pub fn iter() }`.
- Consumes: `crate::app::{ListPane, PaneId}` — **both must derive `Debug, Clone, Copy, PartialEq, Eq`** (add derives in app.rs if the skeleton lacks them; `HitTarget: PartialEq` requires it and Task 12's mouse routing pattern-matches them).
- **Contract note:** `GLYPH_RUNNING` is added beyond the Task-7 glyph list — the running static glyph used by detail lane rows (list panes use the animated throbber instead). Keeping it in `theme.rs` honours the "no inline glyph literals" global constraint.
- **Ordering note:** the plan places `markup.rs` (Task 6) before `theme.rs`, yet `markup::style_line` consumes `&Palette`. If Task 6 introduced a minimal `Palette` to compile, this task **replaces** it with the full struct below (fields are a superset: markup uses `info` for `` `code` ``, `accent` for URLs, `dim` for rules, plus `Modifier::BOLD`). Do not define `Palette` twice.

- [ ] **Step 1: Write failing hit-test unit tests.** Create `hit.rs` with the type/`impl` skeleton compiling (`hit()` returns `None` for now) plus this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{ListPane, PaneId};
    use ratatui::layout::Rect;

    fn r(x: u16, y: u16, w: u16, h: u16) -> Rect { Rect { x, y, width: w, height: h } }

    #[test]
    fn empty_map_hits_nothing() {
        let m = HitMap::new();
        assert_eq!(m.hit(0, 0), None);
        assert!(m.is_empty());
    }

    #[test]
    fn single_rect_inside_and_outside() {
        let mut m = HitMap::new();
        m.push(r(2, 3, 5, 4), HitTarget::Tab(1));
        assert_eq!(m.hit(2, 3), Some(&HitTarget::Tab(1))); // top-left corner inside
        assert_eq!(m.hit(6, 6), Some(&HitTarget::Tab(1))); // bottom-right inside (x<7,y<7)
        assert_eq!(m.hit(7, 3), None);                     // x == right edge is outside
        assert_eq!(m.hit(2, 7), None);                     // y == bottom edge is outside
        assert_eq!(m.hit(1, 3), None);                     // left of rect
    }

    #[test]
    fn overlap_resolves_to_last_registered() {
        let mut m = HitMap::new();
        m.push(r(0, 0, 10, 10), HitTarget::PaneBody(PaneId::Queue)); // background
        m.push(r(2, 2, 4, 4), HitTarget::Row(ListPane::Queue, 3));   // foreground row
        m.push(r(0, 0, 10, 10), HitTarget::Modal);                   // modal registered LAST
        // Modal covers everything and wins because hit() scans in reverse.
        assert_eq!(m.hit(3, 3), Some(&HitTarget::Modal));
        assert_eq!(m.hit(8, 8), Some(&HitTarget::Modal));
    }

    #[test]
    fn foreground_wins_over_background_without_modal() {
        let mut m = HitMap::new();
        m.push(r(0, 0, 10, 10), HitTarget::PaneBody(PaneId::Queue));
        m.push(r(2, 2, 4, 4), HitTarget::Row(ListPane::Queue, 3));
        assert_eq!(m.hit(3, 3), Some(&HitTarget::Row(ListPane::Queue, 3)));
        assert_eq!(m.hit(9, 9), Some(&HitTarget::PaneBody(PaneId::Queue)));
    }

    #[test]
    fn zero_sized_rect_never_hits() {
        let mut m = HitMap::new();
        m.push(r(5, 5, 0, 3), HitTarget::Button(ButtonKind::Confirm));
        m.push(r(5, 5, 3, 0), HitTarget::Button(ButtonKind::Cancel));
        assert_eq!(m.hit(5, 5), None);
    }
}
```

- [ ] **Step 2: Run — expect FAIL.** `cargo test -p qoo-tui hit::tests` (overlap/edge tests fail: `hit()` stubbed to `None`).

- [ ] **Step 3: Implement `hit.rs`.** Full file:

```rust
use ratatui::layout::{Position, Rect};

use crate::app::{ListPane, PaneId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    Confirm,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HitTarget {
    Tab(usize),
    Row(ListPane, usize),
    PaneBody(PaneId),
    SubTab(usize),
    MenuItem(usize),
    FormField(usize),
    DropdownItem(usize),
    Button(ButtonKind),
    ScrollbarThumb(PaneId),
    ScrollbarTrack(PaneId),
    Modal,
}

/// Ordered registry of `(Rect, HitTarget)`. Elements are registered painter's-
/// order (background first, modals last); `hit` scans in reverse so the topmost
/// (last-registered) element under a point wins — clicks never leak through a
/// modal into the body beneath it.
#[derive(Debug, Default, Clone)]
pub struct HitMap {
    entries: Vec<(Rect, HitTarget)>,
}

impl HitMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, rect: Rect, target: HitTarget) {
        self.entries.push((rect, target));
    }

    /// Topmost target containing `(col, row)`, or `None`. Uses `Rect::contains`
    /// (ratatui 0.29): a point is inside iff `x ∈ [x, x+width)` and
    /// `y ∈ [y, y+height)` — the right/bottom edges are exclusive, zero-sized
    /// rects contain nothing.
    pub fn hit(&self, col: u16, row: u16) -> Option<&HitTarget> {
        let p = Position { x: col, y: row };
        self.entries
            .iter()
            .rev()
            .find(|(rect, _)| rect.contains(p))
            .map(|(_, target)| target)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &(Rect, HitTarget)> {
        self.entries.iter()
    }
}
```

- [ ] **Step 4: Run — expect PASS.** `cargo test -p qoo-tui hit::tests`.

- [ ] **Step 5: Implement `view/theme.rs`.** Full file:

```rust
use ratatui::style::{Color, Modifier, Style};

// Status + marker glyphs. All glyph literals live here (global constraint: no
// inline glyphs in components). Running list rows use an animated throbber
// instead of a static glyph; GLYPH_RUNNING is the static fallback used by the
// detail pane's lane-task rows.
pub const GLYPH_QUEUED: char = '○';
pub const GLYPH_NEEDS_INPUT: char = '?';
pub const GLYPH_DONE: char = '✓';
pub const GLYPH_FAILED: char = '✗';
pub const GLYPH_RUNNING: char = '▶';
pub const GLYPH_MAIN_SESSION: char = '⛓';
pub const GLYPH_MAIN_WT: char = '◆';
pub const GLYPH_DISCOVERY: char = '⏰';

/// Central color palette (Catppuccin Mocha-inspired dark theme). The one place
/// colors are defined; components take `&Palette` and never name raw colors.
#[derive(Debug, Clone)]
pub struct Palette {
    pub accent: Color,
    pub border: Color,
    pub border_focused: Color,
    pub dim: Color,
    pub error: Color,
    pub ok: Color,
    pub warn: Color,
    pub info: Color,
    pub fg: Color,
    pub selection_fg: Color,
    pub selection_bg: Color,
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            accent: Color::Rgb(137, 180, 250),       // blue
            border: Color::Rgb(69, 71, 90),          // surface1
            border_focused: Color::Rgb(137, 180, 250),
            dim: Color::Rgb(127, 132, 156),          // overlay1
            error: Color::Rgb(243, 139, 168),        // red
            ok: Color::Rgb(166, 227, 161),           // green
            warn: Color::Rgb(249, 226, 175),         // yellow
            info: Color::Rgb(148, 226, 213),         // teal (`code` markup)
            fg: Color::Rgb(205, 214, 244),           // text
            selection_fg: Color::Rgb(30, 30, 46),    // base
            selection_bg: Color::Rgb(137, 180, 250), // blue
        }
    }
}

impl Palette {
    /// Inverse-style highlight for the selected/active row.
    pub fn selection(&self) -> Style {
        Style::default().fg(self.selection_fg).bg(self.selection_bg)
    }

    /// Dimmed style for archived rows, hints, disabled items.
    pub fn dim_style(&self) -> Style {
        Style::default().fg(self.dim).add_modifier(Modifier::DIM)
    }

    /// Pane border color by focus state.
    pub fn border_style(&self, focused: bool) -> Style {
        Style::default().fg(if focused {
            self.border_focused
        } else {
            self.border
        })
    }
}
```

- [ ] **Step 6: Wire modules.** In `lib.rs` add `pub mod hit;` and ensure `pub mod view;` with `view/mod.rs` containing `pub mod theme;` (create a stub `view/mod.rs` if Task 6 has not: `pub mod theme;`). Build check.

- [ ] **Step 7: Run + commit.** `cargo test -p qoo-tui` (green), `cargo build -p qoo-tui`. Commit:
  `git add crates/qoo-tui/src/hit.rs crates/qoo-tui/src/view/theme.rs crates/qoo-tui/src/lib.rs crates/qoo-tui/src/view/mod.rs`
  `git commit -m "feat(tui-rs): palette theme + reverse-order hit map"`

---

### Task 8: tabbar / panes / footer render + view/mod.rs compute pass

**Files:**
- Create: `crates/qoo-tui/src/view/tabbar.rs`
- Create: `crates/qoo-tui/src/view/panes.rs`
- Create: `crates/qoo-tui/src/view/footer.rs`
- Modify: `crates/qoo-tui/src/view/mod.rs` (the `render` orchestrator + compute helpers)
- Create: `crates/qoo-tui/src/test_fixtures.rs` (`#[cfg(test)] pub` fixtures)
- Test: `crates/qoo-tui/src/view/mod.rs` (`#[cfg(test)] mod tests`, insta snapshots + HitMap assertions)

**Interfaces:**
- Produces (contract): `pub fn render(app: &App, frame: &mut ratatui::Frame) -> HitMap`.
- Produces (module-internal): `tabbar::render(app, frame, area, &mut HitMap, &Palette)`, `panes::render(app, frame, area, &mut HitMap, &Palette)`, `footer::render(app, frame, area, &Palette)`, plus `view::Computed<'a>` (the compute-pass struct) and `view::compute<'a>(app: &'a App) -> Computed<'a>`.
- Consumes: `selectors::{build_tabs, queue_rows, worktree_rows, pane_layout, window_rows, pane_title, filter_rows, QueueRow, WorktreeRow, WtState, TabInfo, PaneLayout}`, `ipc::types::DefinitionSummary`, `selectors::arg_summary`, `crate::app::{App, TabUiState, Selection, ListPane, PaneId, Mode}`, `view::theme::*`, `hit::{HitMap, HitTarget}`.
- **Assumption on `window_rows`:** contract signature is `window_rows(len, cursor, capacity) -> (usize, usize)`. This task treats the **first** returned value as the window `start`/`offset` (the index of the first visible row); the visible count is recomputed locally as `capacity.min(len - offset)`. This is robust whether Task 5 returns `(start, end_exclusive)` or `(offset, count)` — only `.0` is used. State this in a code comment.
- **Assumption on `TabUiState`/`Selection`:** derive `Default` + `Clone`; `Selection { cursor: 0, anchor: None }` default. Read via `app.ui_by_tab.get(name).cloned().unwrap_or_default()`.
- **Version note (throbber):** `throbber-widgets-tui` 0.x exposes `Throbber` (a `StatefulWidget`) + `ThrobberState { calc_next() }`. This task renders one throbber cell per running row via `render_stateful_widget`, seeding a fresh `ThrobberState` advanced `app.now_epoch_s % 8` steps so the spinner animates on the 1 s tick without App holding throbber state.

- [ ] **Step 1: Build the shared fixture.** Create `test_fixtures.rs` (declare `#[cfg(test)] mod test_fixtures;` — or `#[cfg(any(test, feature = "..."))]`; simplest: `#[cfg(test)] pub mod test_fixtures;` in `lib.rs`). Full file:

```rust
//! Shared test fixtures: a representative `StateSnapshot` + a ready `App` for
//! render/snapshot tests across the view and app modules.
#![cfg(test)]

use std::collections::HashMap;

use crate::app::App;
use crate::ipc::types::{
    Project, SessionEntry, StateSnapshot, TaskInstance, TaskStatus, TaskTarget, WorktreeInfo,
};

fn task(
    id: &str,
    status: TaskStatus,
    repo: &str,
    worktree: Option<&str>,
    prompt: &str,
    session: &str,
    created: &str,
) -> TaskInstance {
    TaskInstance {
        id: id.to_string(),
        status,
        definition: None,
        item: None,
        item_key: None,
        target: TaskTarget {
            repo: repo.to_string(),
            git_ref: worktree
                .map(|w| format!("worktree:{w}"))
                .unwrap_or_else(|| "main".to_string()),
            worktree: worktree.map(str::to_string),
        },
        priority: "normal".to_string(),
        created: created.to_string(),
        source: "tui".to_string(),
        ephemeral_worktree: false,
        error: None,
        session: session.to_string(),
        resume_session_id: None,
        model: None,
        prompt: prompt.to_string(),
    }
}

/// A snapshot with one project, four queue tasks (running/queued/failed live +
/// one archived), two worktrees, and a main session. `created` timestamps are
/// fixed ISO strings so elapsed labels are deterministic against `now_epoch_s`.
pub fn fixture_snapshot() -> StateSnapshot {
    let tasks = vec![
        task(
            "01RUN",
            TaskStatus::Running,
            "acme",
            Some("acme.feature"),
            "implement the widget cache",
            "main",
            "2026-07-09T12:00:00.000Z",
        ),
        task(
            "01QUE",
            TaskStatus::Queued,
            "acme",
            Some("acme.feature"),
            "write docs for the cache",
            "fresh",
            "2026-07-09T12:04:00.000Z",
        ),
        {
            let mut t = task(
                "01FAIL",
                TaskStatus::Failed,
                "acme",
                None,
                "flaky migration",
                "fresh",
                "2026-07-09T11:50:00.000Z",
            );
            t.error = Some("exit code 1".to_string());
            t
        },
    ];
    let archived = vec![task(
        "01OLD",
        TaskStatus::Done,
        "acme",
        None,
        "earlier cleanup task",
        "fresh",
        "2026-07-09T10:00:00.000Z",
    )];
    let mut worktrees: HashMap<String, Vec<WorktreeInfo>> = HashMap::new();
    worktrees.insert(
        "acme".to_string(),
        vec![
            WorktreeInfo {
                name: "acme.feature".to_string(),
                path: "/repos/acme.feature".to_string(),
                branch: "feature/JB-1200-cache".to_string(),
            },
            WorktreeInfo {
                name: "acme.hotfix".to_string(),
                path: "/repos/acme.hotfix".to_string(),
                branch: "hotfix/login".to_string(),
            },
        ],
    );
    let mut main_sessions: HashMap<String, String> = HashMap::new();
    main_sessions.insert("acme:acme.feature".to_string(), "sess-abc".to_string());

    StateSnapshot {
        tasks,
        archived_recent: archived,
        sessions: vec![SessionEntry {
            kind: "interactive".to_string(),
            key: "acme:acme.feature".to_string(),
            lane: Some("acme:acme.feature".to_string()),
            cwd: Some("/repos/acme.feature".to_string()),
            pid: Some(4242),
            started_at: "2026-07-09T11:59:00.000Z".to_string(),
            heartbeat_at: "2026-07-09T12:05:00.000Z".to_string(),
        }],
        running: vec!["01RUN".to_string()],
        max_concurrent: Some(3),
        projects: vec![Project {
            name: "acme".to_string(),
        }],
        worktrees,
        main_sessions,
        build_id: Some("build-1".to_string()),
    }
}

/// App seeded with the fixture snapshot, connected, at a fixed `now_epoch_s`
/// (2026-07-09T12:05:03Z → 5m03s elapsed for the running task).
pub fn fixture_app() -> App {
    let mut app = App::new(
        std::path::PathBuf::from("/tmp/qoo-runs"),
        std::path::PathBuf::from("/tmp/qoo.sock"),
    );
    app.snapshot = Some(fixture_snapshot());
    app.connected = true;
    app.now_epoch_s = 1_752_062_703; // 2026-07-09T12:05:03Z
    app.size = (80, 24);
    app
}
```

- [ ] **Step 2: Write failing render tests.** In `view/mod.rs` add:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::PaneId;
    use crate::hit::HitTarget;
    use crate::test_fixtures::fixture_app;
    use ratatui::{Terminal, backend::TestBackend};

    fn render_at(app: &App, w: u16, h: u16) -> (Terminal<TestBackend>, HitMap) {
        let mut app = app.clone();
        app.size = (w, h);
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        let mut hits = HitMap::new();
        terminal
            .draw(|frame| {
                hits = render(&app, frame);
            })
            .unwrap();
        (terminal, hits)
    }

    #[test]
    fn snapshot_default_80x24() {
        let (terminal, _hits) = render_at(&fixture_app(), 80, 24);
        insta::assert_snapshot!("view_default_80x24", terminal.backend());
    }

    #[test]
    fn snapshot_too_small() {
        let (terminal, hits) = render_at(&fixture_app(), 40, 10);
        insta::assert_snapshot!("view_too_small", terminal.backend());
        assert!(hits.is_empty(), "too-small guard registers no hit targets");
    }

    #[test]
    fn snapshot_disconnected() {
        let mut app = fixture_app();
        app.connected = false;
        let (terminal, _hits) = render_at(&app, 80, 24);
        insta::assert_snapshot!("view_disconnected", terminal.backend());
    }

    #[test]
    fn hitmap_has_one_tab_target() {
        let (_t, hits) = render_at(&fixture_app(), 80, 24);
        let tabs = hits
            .iter()
            .filter(|(_, t)| matches!(t, HitTarget::Tab(_)))
            .count();
        assert_eq!(tabs, 1, "fixture has one project → one clickable tab");
    }

    #[test]
    fn hitmap_has_queue_rows_and_bodies() {
        let (_t, hits) = render_at(&fixture_app(), 80, 24);
        let rows = hits
            .iter()
            .filter(|(_, t)| matches!(t, HitTarget::Row(crate::app::ListPane::Queue, _)))
            .count();
        assert!(rows >= 3, "3 live + 1 archived queue rows visible");
        assert!(
            hits.iter().any(|(_, t)| *t == HitTarget::PaneBody(PaneId::Queue)),
            "queue pane body registered for empty-area clicks"
        );
    }
}
```

- [ ] **Step 3: Run — expect FAIL.** `cargo test -p qoo-tui view::tests` (snapshots missing + `render` stub returns empty HitMap).

- [ ] **Step 4: Implement `view/mod.rs` orchestration + compute pass.** Full file (replacing the Task-7 stub):

```rust
pub mod detail;
pub mod footer;
pub mod panes;
pub mod tabbar;
pub mod theme;

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::Text;
use ratatui::widgets::Paragraph;

use crate::app::{App, ListPane, Selection, TabUiState};
use crate::hit::HitMap;
use crate::ipc::types::DefinitionSummary;
use crate::selectors::{
    QueueRow, WorktreeRow, build_tabs, filter_rows, pane_layout, queue_rows, worktree_rows,
};
use theme::Palette;

/// Everything a frame needs, computed once so hit-testing and drawing use the
/// same geometry and the same filtered/selected view-model.
pub struct Computed<'a> {
    pub palette: Palette,
    pub active_name: Option<String>,
    pub tab_names: Vec<String>,
    pub active_index: usize,
    pub ui: TabUiState,
    pub queue: Vec<QueueRow>,
    pub defs: Vec<DefinitionSummary>,
    pub worktrees: Vec<WorktreeRow>,
    pub queue_sel: Selection,
    pub tasks_sel: Selection,
    pub wt_sel: Selection,
    pub _marker: std::marker::PhantomData<&'a ()>,
}

fn clamp_sel(sel: &Selection, len: usize) -> Selection {
    if len == 0 {
        return Selection { cursor: 0, anchor: None };
    }
    let cursor = sel.cursor.min(len - 1);
    let anchor = sel.anchor.map(|a| a.min(len - 1));
    Selection { cursor, anchor }
}

/// The compute pass. Derives the active project, its filtered rows, and clamped
/// selections. Pure — no drawing.
pub fn compute(app: &App) -> Computed<'_> {
    let palette = Palette::default();
    let tabs = app
        .snapshot
        .as_ref()
        .map(build_tabs)
        .unwrap_or_default();
    let active_index = app.active_tab.min(tabs.len().saturating_sub(1));
    let active_name = tabs.get(active_index).map(|t| t.name.clone());
    let ui = active_name
        .as_ref()
        .and_then(|n| app.ui_by_tab.get(n).cloned())
        .unwrap_or_default();

    let (queue, defs, worktrees) = match (&app.snapshot, &active_name) {
        (Some(snap), Some(name)) => {
            let q = queue_rows(snap, name, app.now_epoch_s);
            let d = app.defs_by_project.get(name).cloned().unwrap_or_default();
            let w = worktree_rows(snap, name);
            (q, d, w)
        }
        _ => (Vec::new(), Vec::new(), Vec::new()),
    };

    // Filter each pane by its search string (indices → owned rows).
    let q_idx = filter_rows(&queue, &ui.search[0], |r| r.summary.clone());
    let d_idx = filter_rows(&defs, &ui.search[1], |d| d.name.clone());
    let w_idx = filter_rows(&worktrees, &ui.search[2], |r| r.name.clone());
    let queue: Vec<QueueRow> = q_idx.into_iter().map(|i| queue[i].clone()).collect();
    let defs: Vec<DefinitionSummary> = d_idx.into_iter().map(|i| defs[i].clone()).collect();
    let worktrees: Vec<WorktreeRow> = w_idx.into_iter().map(|i| worktrees[i].clone()).collect();

    let queue_sel = clamp_sel(&ui.selections[0], queue.len());
    let tasks_sel = clamp_sel(&ui.selections[1], defs.len());
    let wt_sel = clamp_sel(&ui.selections[2], worktrees.len());

    Computed {
        palette,
        active_name,
        tab_names: tabs.iter().map(|t| t.name.clone()).collect(),
        active_index,
        ui,
        queue,
        defs,
        worktrees,
        queue_sel,
        tasks_sel,
        wt_sel,
        _marker: std::marker::PhantomData,
    }
}

/// Draw the whole frame, returning the hit map for mouse routing.
pub fn render(app: &App, frame: &mut ratatui::Frame) -> HitMap {
    let mut hits = HitMap::new();
    let area = frame.area();
    let p = Palette::default();

    if area.width < 60 || area.height < 15 {
        let msg = Paragraph::new(Text::from("terminal too small (60x15 minimum)"))
            .style(Style::default().fg(p.fg));
        frame.render_widget(msg, area);
        return hits; // no clickable targets while too small
    }

    let c = compute(app);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // header
            Constraint::Min(1),    // body
            Constraint::Length(1), // footer
        ])
        .split(area);
    let (header, body, foot) = (rows[0], rows[1], rows[2]);

    tabbar::render(app, &c, frame, header, &mut hits);

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(34), Constraint::Min(1)])
        .split(body);
    let (left, right) = (cols[0], cols[1]);

    panes::render(app, &c, frame, left, &mut hits);
    detail::render(app, &c, frame, right, &mut hits);
    footer::render(app, &c, frame, foot);

    hits
}

/// Inclusive `(start, end)` selection range from a `Selection`.
pub(crate) fn selection_range(sel: &Selection) -> (usize, usize) {
    match sel.anchor {
        Some(a) => (a.min(sel.cursor), a.max(sel.cursor)),
        None => (sel.cursor, sel.cursor),
    }
}

/// Window `start` for a cursor-centered slice of `len` rows into `capacity`
/// rows. Uses only `window_rows(...).0` (see task assumption note).
pub(crate) fn window_start(len: usize, cursor: usize, capacity: usize) -> usize {
    crate::selectors::window_rows(len, cursor, capacity).0
}
```

- [ ] **Step 5: Implement `view/tabbar.rs`.** Full file:

```rust
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::hit::{HitMap, HitTarget};
use crate::view::Computed;

pub fn render(app: &App, c: &Computed, frame: &mut ratatui::Frame, area: Rect, hits: &mut HitMap) {
    let p = &c.palette;
    // Left: tab chips. Track x as we lay them out so each chip gets a hit rect.
    let mut spans: Vec<Span> = Vec::new();
    let mut x = area.x;
    for (i, name) in c.tab_names.iter().enumerate() {
        let label = format!(" {}:{} ", i + 1, name);
        let w = label.chars().count() as u16;
        let style = if i == c.active_index {
            Style::default()
                .fg(p.selection_fg)
                .bg(p.selection_bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(p.fg)
        };
        // Clamp the hit rect to the header width.
        if x < area.right() {
            let clamped_w = w.min(area.right() - x);
            hits.push(
                Rect { x, y: area.y, width: clamped_w, height: 1 },
                HitTarget::Tab(i),
            );
        }
        spans.push(Span::styled(label, style));
        x = x.saturating_add(w);
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);

    // Right: connection indicator + running counter, right-aligned.
    let running = app.snapshot.as_ref().map(|s| s.running.len()).unwrap_or(0);
    let max = app.snapshot.as_ref().and_then(|s| s.max_concurrent);
    let run_label = match max {
        Some(m) => format!(" running {}/{}", running, m),
        None => format!(" running {}", running),
    };
    let conn: Span = if app.connected {
        Span::styled("●", Style::default().fg(p.ok))
    } else {
        Span::styled("daemon unreachable — retrying…", Style::default().fg(p.warn))
    };
    let right = Line::from(vec![conn, Span::styled(run_label, Style::default().fg(p.fg))]);
    let width = area.width; // right-align via Paragraph alignment
    frame.render_widget(
        Paragraph::new(right).alignment(ratatui::layout::Alignment::Right),
        Rect { x: area.x, y: area.y, width, height: 1 },
    );
}
```

- [ ] **Step 6: Implement `view/panes.rs`.** Full file (the three stacked list panes, throbber, scrollbar, hit registration):

```rust
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Scrollbar, ScrollbarOrientation,
    ScrollbarState};
use throbber_widgets_tui::{Throbber, ThrobberState};

use crate::app::{App, ListPane, PaneId};
use crate::hit::{HitMap, HitTarget};
use crate::ipc::types::DefinitionSummary;
use crate::selectors::{QueueRow, WorktreeRow, WtState, arg_summary, pane_layout, pane_title};
use crate::view::theme::{GLYPH_MAIN_SESSION, GLYPH_MAIN_WT, GLYPH_DISCOVERY, Palette};
use crate::view::{Computed, selection_range, window_start};

/// Render one pane's chrome (rounded border, focused accent, bold title). Returns
/// the inner content `Rect` (below the title line).
fn pane_chrome(
    frame: &mut ratatui::Frame,
    area: Rect,
    title: &str,
    focused: bool,
    p: &Palette,
) -> Rect {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(p.border_style(focused));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    // Title line (bold) at the top of the inner area.
    if inner.height > 0 {
        let title_line = Line::from(Span::styled(
            title.to_string(),
            Style::default().fg(p.fg).add_modifier(Modifier::BOLD),
        ));
        frame.render_widget(
            Paragraph::new(title_line),
            Rect { x: inner.x, y: inner.y, width: inner.width, height: 1 },
        );
    }
    // Content region starts one row below the title.
    Rect {
        x: inner.x,
        y: inner.y.saturating_add(1),
        width: inner.width,
        height: inner.height.saturating_sub(1),
    }
}

fn queue_line<'a>(row: &'a QueueRow, p: &Palette) -> Line<'a> {
    let mut spans: Vec<Span> = Vec::new();
    // Glyph column: running rows get a placeholder space (throbber painted over).
    if row.running {
        spans.push(Span::raw(" "));
    } else {
        spans.push(Span::raw(row.glyph.to_string()));
    }
    spans.push(Span::raw(" "));
    if row.main_session {
        spans.push(Span::styled(format!("{GLYPH_MAIN_SESSION} "), Style::default().fg(p.info)));
    }
    spans.push(Span::raw(format!("{} {} {}", row.lane, row.summary, row.detail)));
    Line::from(spans)
}

fn worktree_line<'a>(row: &'a WorktreeRow, p: &Palette) -> Line<'a> {
    let dot = match row.state {
        WtState::Free => p.ok,
        WtState::Busy | WtState::You => p.warn,
        WtState::Failed => p.error,
    };
    let mut spans = vec![
        Span::styled("●", Style::default().fg(dot)),
        Span::raw(format!(" {}", row.name)),
    ];
    if row.has_main_session {
        spans.push(Span::styled(format!(" {GLYPH_MAIN_WT}"), Style::default().fg(p.info)));
    }
    if row.queued > 0 {
        spans.push(Span::styled(format!(" [{}]", row.queued), p.dim_style()));
    }
    Line::from(spans)
}

fn def_line(def: &DefinitionSummary) -> Line<'static> {
    let mut s = def.name.clone();
    if !def.args.is_empty() {
        s.push_str(&format!(" ({})", arg_summary(&def.args)));
    }
    if def.has_discovery {
        s.push(' ');
        s.push(GLYPH_DISCOVERY);
    }
    Line::from(s)
}

/// Register a vertical scrollbar hit region (track + proportional thumb) and draw
/// the built-in Scrollbar. `total` rows, `offset` first-visible, `visible` rows.
fn render_scrollbar(
    frame: &mut ratatui::Frame,
    area: Rect,
    total: usize,
    offset: usize,
    visible: usize,
    pane: PaneId,
    hits: &mut HitMap,
) {
    if total <= visible || area.height == 0 {
        return;
    }
    let mut state = ScrollbarState::new(total.saturating_sub(visible)).position(offset);
    let track = Rect { x: area.right().saturating_sub(1), y: area.y, width: 1, height: area.height };
    hits.push(track, HitTarget::ScrollbarTrack(pane));
    // Proportional thumb: height ≈ visible/total of the track, top ≈ offset/total.
    let h = area.height as usize;
    let thumb_h = ((visible * h) / total).max(1).min(h) as u16;
    let max_off = total - visible;
    let thumb_top = if max_off == 0 {
        area.y
    } else {
        area.y + (((offset * (h.saturating_sub(thumb_h as usize))) / max_off) as u16)
    };
    hits.push(
        Rect { x: track.x, y: thumb_top, width: 1, height: thumb_h },
        HitTarget::ScrollbarThumb(pane),
    );
    frame.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight),
        area,
        &mut state,
    );
}

pub fn render(app: &App, c: &Computed, frame: &mut ratatui::Frame, area: Rect, hits: &mut HitMap) {
    let p = &c.palette;
    let layout = pane_layout(area.height);
    let regions = ratatui::layout::Layout::default()
        .direction(ratatui::layout::Direction::Vertical)
        .constraints([
            ratatui::layout::Constraint::Length(layout.queue_h),
            ratatui::layout::Constraint::Length(layout.tasks_h),
            ratatui::layout::Constraint::Length(layout.worktrees_h),
        ])
        .split(area);

    render_queue(app, c, frame, regions[0], hits, p);
    render_tasks(c, frame, regions[1], hits, p);
    render_worktrees(c, frame, regions[2], hits, p);
}

fn render_queue(
    app: &App,
    c: &Computed,
    frame: &mut ratatui::Frame,
    area: Rect,
    hits: &mut HitMap,
    p: &Palette,
) {
    let (start_i, end_i) = selection_range(&c.queue_sel);
    let count = if c.queue.is_empty() { 0 } else { end_i - start_i + 1 };
    let focused = matches!(c.ui.focus, PaneId::Queue);
    let searching = matches!(&app.mode, crate::app::Mode::Search { pane } if *pane == ListPane::Queue);
    let title = pane_title("QUEUE", &c.queue_sel, &c.ui.search[0], searching);
    let _ = count; // pane_title already encodes selection count
    let inner = pane_chrome(frame, area, &title, focused, p);
    hits.push(inner, HitTarget::PaneBody(PaneId::Queue));

    if c.queue.is_empty() {
        frame.render_widget(
            Paragraph::new("queue empty — [a] on a worktree to add a task").style(p.dim_style()),
            inner,
        );
        return;
    }
    let cap = inner.height as usize;
    let offset = window_start(c.queue.len(), c.queue_sel.cursor, cap);
    let visible = cap.min(c.queue.len() - offset);
    let mut lines: Vec<Line> = Vec::with_capacity(visible);
    for vi in 0..visible {
        let idx = offset + vi;
        let row = &c.queue[idx];
        let mut line = queue_line(row, p);
        let selected = focused && idx >= start_i && idx <= end_i;
        if selected {
            line = line.style(p.selection());
        } else if row.archived {
            line = line.style(p.dim_style());
        }
        lines.push(line);
        hits.push(
            Rect { x: inner.x, y: inner.y + vi as u16, width: inner.width, height: 1 },
            HitTarget::Row(ListPane::Queue, idx),
        );
    }
    frame.render_widget(Paragraph::new(Text::from(lines)), inner);
    // Throbbers over running rows.
    let mut tstate = ThrobberState::default();
    for _ in 0..(app.now_epoch_s % 8) {
        tstate.calc_next();
    }
    for vi in 0..visible {
        let idx = offset + vi;
        if c.queue[idx].running {
            let mut st = tstate.clone();
            frame.render_stateful_widget(
                Throbber::default(),
                Rect { x: inner.x, y: inner.y + vi as u16, width: 1, height: 1 },
                &mut st,
            );
        }
    }
    render_scrollbar(frame, inner, c.queue.len(), offset, visible, PaneId::Queue, hits);
}

fn render_tasks(
    c: &Computed,
    frame: &mut ratatui::Frame,
    area: Rect,
    hits: &mut HitMap,
    p: &Palette,
) {
    let focused = matches!(c.ui.focus, PaneId::Tasks);
    let searching = false; // filled by App.mode below
    let title = pane_title("TASKS", &c.tasks_sel, &c.ui.search[1], searching);
    let inner = pane_chrome(frame, area, &title, focused, p);
    hits.push(inner, HitTarget::PaneBody(PaneId::Tasks));
    if c.defs.is_empty() {
        frame.render_widget(Paragraph::new("no task definitions").style(p.dim_style()), inner);
        return;
    }
    let (start_i, end_i) = selection_range(&c.tasks_sel);
    let cap = inner.height as usize;
    let offset = window_start(c.defs.len(), c.tasks_sel.cursor, cap);
    let visible = cap.min(c.defs.len() - offset);
    let mut lines: Vec<Line> = Vec::with_capacity(visible);
    for vi in 0..visible {
        let idx = offset + vi;
        let mut line = def_line(&c.defs[idx]);
        if focused && idx >= start_i && idx <= end_i {
            line = line.style(p.selection());
        }
        lines.push(line);
        hits.push(
            Rect { x: inner.x, y: inner.y + vi as u16, width: inner.width, height: 1 },
            HitTarget::Row(ListPane::Tasks, idx),
        );
    }
    frame.render_widget(Paragraph::new(Text::from(lines)), inner);
    render_scrollbar(frame, inner, c.defs.len(), offset, visible, PaneId::Tasks, hits);
}

fn render_worktrees(
    c: &Computed,
    frame: &mut ratatui::Frame,
    area: Rect,
    hits: &mut HitMap,
    p: &Palette,
) {
    let focused = matches!(c.ui.focus, PaneId::Worktrees);
    let title = pane_title("WORKTREES", &c.wt_sel, &c.ui.search[2], false);
    let inner = pane_chrome(frame, area, &title, focused, p);
    hits.push(inner, HitTarget::PaneBody(PaneId::Worktrees));
    if c.worktrees.is_empty() {
        frame.render_widget(Paragraph::new("no worktrees").style(p.dim_style()), inner);
        return;
    }
    let (start_i, end_i) = selection_range(&c.wt_sel);
    let cap = inner.height as usize;
    let offset = window_start(c.worktrees.len(), c.wt_sel.cursor, cap);
    let visible = cap.min(c.worktrees.len() - offset);
    let mut lines: Vec<Line> = Vec::with_capacity(visible);
    for vi in 0..visible {
        let idx = offset + vi;
        let mut line = worktree_line(&c.worktrees[idx], p);
        if focused && idx >= start_i && idx <= end_i {
            line = line.style(p.selection());
        }
        lines.push(line);
        hits.push(
            Rect { x: inner.x, y: inner.y + vi as u16, width: inner.width, height: 1 },
            HitTarget::Row(ListPane::Worktrees, idx),
        );
    }
    frame.render_widget(Paragraph::new(Text::from(lines)), inner);
    render_scrollbar(frame, inner, c.worktrees.len(), offset, visible, PaneId::Worktrees, hits);
}
```

> **Note (searching flag):** `pane_title(..., searching)` should be `true` only for the pane currently in `Mode::Search { pane }`. Thread `app.mode` into `render_tasks`/`render_worktrees` the same way `render_queue` derives `searching` (small mechanical edit — pass `app` down or precompute a `[bool; 3]` in `Computed`). Simplest: add `pub searching: [bool; 3]` to `Computed` in `compute()` from `app.mode`, and read `c.searching[n]` in each pane. Do this in Step 4 when finalizing `Computed`.

- [ ] **Step 7: Implement `view/footer.rs`.** Reproduce `Footer.tsx` priority order **minus the retired `[C-s] prefix`**, plus the searching hint. Full file:

```rust
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Text;
use ratatui::widgets::Paragraph;

use crate::app::{App, Mode, PaneId};
use crate::view::{Computed, selection_range};
use crate::view::theme::Palette;

const LIST_HINT: &str =
    "[a] actions · [enter] detail · [↑↓] move · [/] filter · [?] help · [q]uit";

fn hint_for(focus: PaneId) -> String {
    match focus {
        PaneId::Queue => format!("[c] new run · {LIST_HINT}"),
        PaneId::Tasks => LIST_HINT.to_string(),
        PaneId::Worktrees => format!("[c] new worktree · {LIST_HINT}"),
        PaneId::Detail => {
            "[↑↓/jk] scroll · [g/G] top/bottom · [{ }] sub-tab · [a] actions · [?] help · [q]uit"
                .to_string()
        }
    }
}

pub fn render(app: &App, c: &Computed, frame: &mut ratatui::Frame, area: Rect) {
    let p: &Palette = &c.palette;
    // Priority: searching > status line > selection-count > per-pane hints.
    let searching = matches!(app.mode, Mode::Search { .. });
    if searching {
        frame.render_widget(
            Paragraph::new("type to filter · [enter] apply · [esc] clear").style(p.dim_style()),
            area,
        );
        return;
    }
    if let Some(status) = &app.status_line {
        frame.render_widget(
            Paragraph::new(Text::from(status.clone())).style(Style::default().fg(p.error)),
            area,
        );
        return;
    }
    // Selection count of the focused list pane.
    let sel = match c.ui.focus {
        PaneId::Queue => Some((&c.queue_sel, c.queue.len())),
        PaneId::Tasks => Some((&c.tasks_sel, c.defs.len())),
        PaneId::Worktrees => Some((&c.wt_sel, c.worktrees.len())),
        PaneId::Detail => None,
    };
    let count = sel
        .filter(|(_, len)| *len > 0)
        .map(|(s, _)| {
            let (a, b) = selection_range(s);
            b - a + 1
        })
        .unwrap_or(0);
    if count > 1 {
        frame.render_widget(
            Paragraph::new(format!(
                "{count} selected · [a] bulk actions · [shift+↑↓] extend · [esc] clear"
            ))
            .style(p.dim_style()),
            area,
        );
        return;
    }
    frame.render_widget(Paragraph::new(hint_for(c.ui.focus)).style(p.dim_style()), area);
}
```

- [ ] **Step 8: Add a `detail::render` stub so `view/mod.rs` compiles.** Create `view/detail.rs` with a minimal `pub fn render(app, c, frame, area, hits)` that draws an empty bordered "DETAIL" pane (fully implemented in Task 9). Enough to build + snapshot the left column now.

- [ ] **Step 9: Run + accept snapshots + PASS.** `cargo test -p qoo-tui view::tests` (fails: pending snapshots) → `cargo insta accept` → re-run PASS. Verify the `.snap` files render the tab bar, three panes, footer, too-small guard text, disconnected banner.

- [ ] **Step 10: Commit.**
  `git add crates/qoo-tui/src/view/mod.rs crates/qoo-tui/src/view/tabbar.rs crates/qoo-tui/src/view/panes.rs crates/qoo-tui/src/view/footer.rs crates/qoo-tui/src/view/detail.rs crates/qoo-tui/src/test_fixtures.rs crates/qoo-tui/src/lib.rs crates/qoo-tui/src/snapshots/`
  `git commit -m "feat(tui-rs): tab bar, list panes, footer + compute/hit pass"`

---

### Task 9: detail.rs + view/detail.rs

**Files:**
- Create: `crates/qoo-tui/src/detail.rs`
- Modify: `crates/qoo-tui/src/view/detail.rs` (replace Task-8 stub)
- Modify: `crates/qoo-tui/src/app.rs` (scroll handling in `update`)
- Test: `crates/qoo-tui/src/detail.rs` (`#[cfg(test)] mod tests`) + `view/detail.rs` snapshot test

**Interfaces:**
- Produces (`detail.rs`, contract): `pub enum DetailContext { Run { task: TaskInstance }, Definition { repo: String, name: String }, Worktree { row: WorktreeRow, lane_tasks: Vec<TaskInstance> }, Empty }`; `pub fn derive_context(snapshot, project, last: ListPane, queue: &[QueueRow], wt: &[WorktreeRow], defs: &[DefinitionSummary], sel: &[Selection; 3]) -> DetailContext`; `pub fn sub_tab_names(kind: DetailKind) -> &'static [&'static str]`; `pub fn clamp_sub_tab(idx: usize, kind: DetailKind) -> usize`; `pub fn bottom_anchored(kind: DetailKind, sub_tab: usize) -> bool`; `pub fn window_lines(total, height, offset, bottom) -> (usize, usize)` returning `(start, end_exclusive)` into the line list.
- Consumes: `crate::app::{DetailKind, ListPane, Selection}`, `selectors::{QueueRow, WorktreeRow, WtState, lane_key, prompt_summary, arg_summary}`, `ipc::types::{StateSnapshot, TaskInstance, TaskStatus, TaskDefinition, DefinitionSummary}`, `markup::style_line`, `view::theme::*`.
- **Contract note (`window_lines` return shape):** the contract lists `window_lines(total, height, offset, bottom) -> (usize, usize)`. This task fixes the meaning as `(start, end_exclusive)`; the visible slice is `lines[start..end]`. Mirrors the TS `windowLines` behavior (offset from the anchor, clamped).
- **Contract note (`DetailKind`):** `bottom_anchored`/`clamp_sub_tab`/`sub_tab_names` key on `DetailKind` (from app.rs). A `DetailContext` maps to a `DetailKind` via `context.kind()` (add `impl DetailContext { pub fn kind(&self) -> DetailKind }`).

- [ ] **Step 1: Failing detail.rs unit tests** (mirror `detail.test.ts` — `subTabsFor`, `clampSubTab`, `anchorFor`, `windowLines`):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::DetailKind;

    #[test]
    fn sub_tab_names_per_kind() {
        assert_eq!(sub_tab_names(DetailKind::Run), &["transcript", "report", "prompt"]);
        assert_eq!(sub_tab_names(DetailKind::Definition), &["prompt", "config"]);
        assert_eq!(sub_tab_names(DetailKind::Worktree), &["info"]);
        assert_eq!(sub_tab_names(DetailKind::Empty), &[] as &[&str]);
    }

    #[test]
    fn clamp_sub_tab_into_range() {
        assert_eq!(clamp_sub_tab(0, DetailKind::Run), 0);
        assert_eq!(clamp_sub_tab(2, DetailKind::Run), 2);
        assert_eq!(clamp_sub_tab(5, DetailKind::Run), 2);
        assert_eq!(clamp_sub_tab(3, DetailKind::Definition), 1);
        assert_eq!(clamp_sub_tab(1, DetailKind::Worktree), 0);
        assert_eq!(clamp_sub_tab(0, DetailKind::Empty), 0);
        assert_eq!(clamp_sub_tab(4, DetailKind::Empty), 0);
    }

    #[test]
    fn bottom_anchored_only_run_transcript() {
        assert!(bottom_anchored(DetailKind::Run, 0));
        assert!(!bottom_anchored(DetailKind::Run, 1));
        assert!(!bottom_anchored(DetailKind::Run, 2));
        assert!(!bottom_anchored(DetailKind::Definition, 0));
        assert!(!bottom_anchored(DetailKind::Worktree, 0));
        assert!(!bottom_anchored(DetailKind::Empty, 0));
    }

    // window_lines returns (start, end_exclusive). 5 lines "a".."e".
    #[test]
    fn window_all_when_fits() {
        assert_eq!(window_lines(5, 10, 0, false), (0, 5));
        assert_eq!(window_lines(5, 10, 3, true), (0, 5));
    }
    #[test]
    fn window_zero_height() {
        assert_eq!(window_lines(5, 0, 0, false), (0, 0));
    }
    #[test]
    fn window_top_default_and_offset() {
        assert_eq!(window_lines(5, 2, 0, false), (0, 2)); // a,b
        assert_eq!(window_lines(5, 2, 2, false), (2, 4)); // c,d
    }
    #[test]
    fn window_bottom_default_and_offset() {
        assert_eq!(window_lines(5, 2, 0, true), (3, 5)); // d,e
        assert_eq!(window_lines(5, 2, 1, true), (2, 4)); // c,d
    }
    #[test]
    fn window_clamps_offset() {
        assert_eq!(window_lines(5, 2, 99, false), (3, 5)); // d,e
        assert_eq!(window_lines(5, 2, 99, true), (0, 2));  // a,b
    }
}
```

- [ ] **Step 2: Run — expect FAIL.** `cargo test -p qoo-tui detail::tests`.

- [ ] **Step 3: Implement `detail.rs`.** Full file:

```rust
use crate::app::{DetailKind, ListPane, Selection};
use crate::ipc::types::{DefinitionSummary, StateSnapshot, TaskInstance};
use crate::selectors::{QueueRow, WorktreeRow, lane_key};

#[derive(Debug, Clone, PartialEq)]
pub enum DetailContext {
    Run { task: TaskInstance },
    Definition { repo: String, name: String },
    Worktree { row: WorktreeRow, lane_tasks: Vec<TaskInstance> },
    Empty,
}

impl DetailContext {
    pub fn kind(&self) -> DetailKind {
        match self {
            DetailContext::Run { .. } => DetailKind::Run,
            DetailContext::Definition { .. } => DetailKind::Definition,
            DetailContext::Worktree { .. } => DetailKind::Worktree,
            DetailContext::Empty => DetailKind::Empty,
        }
    }
}

const RUN_TABS: &[&str] = &["transcript", "report", "prompt"];
const DEF_TABS: &[&str] = &["prompt", "config"];
const WT_TABS: &[&str] = &["info"];
const NO_TABS: &[&str] = &[];

pub fn sub_tab_names(kind: DetailKind) -> &'static [&'static str] {
    match kind {
        DetailKind::Run => RUN_TABS,
        DetailKind::Definition => DEF_TABS,
        DetailKind::Worktree => WT_TABS,
        DetailKind::Empty => NO_TABS,
    }
}

pub fn clamp_sub_tab(idx: usize, kind: DetailKind) -> usize {
    let count = sub_tab_names(kind).len();
    if count == 0 {
        return 0;
    }
    idx.min(count - 1)
}

pub fn bottom_anchored(kind: DetailKind, sub_tab: usize) -> bool {
    matches!(kind, DetailKind::Run) && sub_tab == 0
}

/// `(start, end_exclusive)` slice into `total` lines for a `height`-tall window
/// shifted `offset` from its anchor (`bottom` = tail-anchored, else head).
pub fn window_lines(total: usize, height: usize, offset: usize, bottom: bool) -> (usize, usize) {
    if height == 0 {
        return (0, 0);
    }
    if total <= height {
        return (0, total);
    }
    let max_offset = total - height;
    let offset = offset.min(max_offset);
    if bottom {
        let end = total - offset;
        (end - height, end)
    } else {
        (offset, offset + height)
    }
}

/// Derive the detail context from the last-focused list pane and its selection.
pub fn derive_context(
    snapshot: &StateSnapshot,
    project: &str,
    last: ListPane,
    queue: &[QueueRow],
    wt: &[WorktreeRow],
    defs: &[DefinitionSummary],
    sel: &[Selection; 3],
) -> DetailContext {
    match last {
        ListPane::Queue => {
            let Some(row) = queue.get(sel[0].cursor) else {
                return DetailContext::Empty;
            };
            let task = snapshot
                .tasks
                .iter()
                .chain(snapshot.archived_recent.iter())
                .find(|t| t.id == row.task_id)
                .cloned();
            match task {
                Some(task) => DetailContext::Run { task },
                None => DetailContext::Empty,
            }
        }
        ListPane::Tasks => match defs.get(sel[1].cursor) {
            Some(def) => DetailContext::Definition { repo: def.repo.clone(), name: def.name.clone() },
            None => DetailContext::Empty,
        },
        ListPane::Worktrees => {
            let Some(row) = wt.get(sel[2].cursor) else {
                return DetailContext::Empty;
            };
            let lane = lane_key(project, &row.raw_name);
            let lane_tasks: Vec<TaskInstance> = snapshot
                .tasks
                .iter()
                .chain(snapshot.archived_recent.iter())
                .filter(|t| {
                    t.target
                        .worktree
                        .as_deref()
                        .map(|w| lane_key(&t.target.repo, w) == lane)
                        .unwrap_or(false)
                })
                .cloned()
                .collect();
            DetailContext::Worktree { row: row.clone(), lane_tasks }
        }
    }
}
```

- [ ] **Step 4: Run — expect PASS.** `cargo test -p qoo-tui detail::tests`.

- [ ] **Step 5: Implement `view/detail.rs` render** (sub-tab chips + content per context/sub-tab, scrollbar, hit targets). Full file:

```rust
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Scrollbar, ScrollbarOrientation,
    ScrollbarState};

use crate::app::{App, DetailKind, ListPane, PaneId, Selection};
use crate::detail::{DetailContext, bottom_anchored, clamp_sub_tab, derive_context, sub_tab_names,
    window_lines};
use crate::hit::{HitMap, HitTarget};
use crate::ipc::types::{TaskDefinition, TaskStatus};
use crate::markup::style_line;
use crate::selectors::{WtState, arg_summary, prompt_summary};
use crate::view::Computed;
use crate::view::theme::{GLYPH_DONE, GLYPH_FAILED, GLYPH_NEEDS_INPUT, GLYPH_QUEUED, GLYPH_RUNNING,
    Palette};

fn status_glyph(s: &TaskStatus) -> char {
    match s {
        TaskStatus::Running => GLYPH_RUNNING,
        TaskStatus::Queued => GLYPH_QUEUED,
        TaskStatus::NeedsInput => GLYPH_NEEDS_INPUT,
        TaskStatus::Done => GLYPH_DONE,
        TaskStatus::Failed | TaskStatus::Unknown => GLYPH_FAILED,
    }
}

fn config_lines(def: &TaskDefinition) -> Vec<String> {
    vec![
        format!("args: {}", if def.args.is_empty() { "—".to_string() } else { arg_summary(&def.args) }),
        format!("worktree: {}", def.worktree),
        format!("dedup: {}", def.dedup),
        format!("model: {}", def.model),
        format!("timeout: {}ms", def.timeout_ms),
        format!("priority: {}", def.priority),
        format!("discovery: {}", def.discovery.as_ref().map(|d| d.command.clone()).unwrap_or_else(|| "—".to_string())),
    ]
}

/// Content lines + placeholder for the given context/sub-tab. `def` is the
/// resolved full definition (None while loading), `run_files` the current run's
/// (report, transcript_tail).
fn content_for(
    ctx: &DetailContext,
    sub_tab: usize,
    def: Option<&TaskDefinition>,
    run_files: Option<&crate::runfiles::RunFiles>,
) -> (Vec<String>, &'static str) {
    match ctx {
        DetailContext::Run { task } => match sub_tab {
            1 => (
                run_files.map(|f| f.report.clone()).unwrap_or_default(),
                "(no report yet)",
            ),
            2 => (task.prompt.split('\n').map(str::to_string).collect(), "(no prompt)"),
            _ => (
                run_files.map(|f| f.transcript_tail.clone()).unwrap_or_default(),
                "(no transcript yet)",
            ),
        },
        DetailContext::Definition { .. } => match def {
            None => (Vec::new(), "(loading definition…)"),
            Some(d) if sub_tab == 1 => (config_lines(d), ""),
            Some(d) => (d.prompt.split('\n').map(str::to_string).collect(), "(no prompt)"),
        },
        DetailContext::Worktree { row, lane_tasks } => {
            let mut lines = vec![
                format!("path: {}", row.path),
                format!("branch: {}", if row.branch.is_empty() { "—".to_string() } else { row.branch.clone() }),
                format!("state: {}", match row.state {
                    WtState::Free => "free",
                    WtState::Busy => "busy",
                    WtState::You => "you",
                    WtState::Failed => "failed",
                }),
                String::new(),
                "tasks on this lane:".to_string(),
            ];
            if lane_tasks.is_empty() {
                lines.push("(none)".to_string());
            } else {
                for t in lane_tasks {
                    lines.push(format!("{} {}", status_glyph(&t.status), prompt_summary(&t.prompt)));
                }
            }
            (lines, "")
        }
        DetailContext::Empty => (Vec::new(), "(nothing selected)"),
    }
}

pub fn render(app: &App, c: &Computed, frame: &mut ratatui::Frame, area: Rect, hits: &mut HitMap) {
    let p: &Palette = &c.palette;
    let focused = matches!(c.ui.focus, PaneId::Detail);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(p.border_style(focused))
        .title(Span::styled("DETAIL", Style::default().fg(p.fg).add_modifier(Modifier::BOLD)));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    hits.push(inner, HitTarget::PaneBody(PaneId::Detail));
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    // Resolve context from the last-focused list pane.
    let ctx = match (&app.snapshot, &c.active_name) {
        (Some(snap), Some(name)) => derive_context(
            snap,
            name,
            c.ui.last_list_pane,
            &c.queue,
            &c.worktrees,
            &c.defs,
            &c.ui.selections,
        ),
        _ => DetailContext::Empty,
    };
    let kind = ctx.kind();
    let sub_tab = clamp_sub_tab(c.ui.sub_tab[kind as usize], kind);

    // Sub-tab chip row.
    let tabs = sub_tab_names(kind);
    let mut content_top = inner.y;
    if !tabs.is_empty() {
        let mut x = inner.x;
        let mut spans: Vec<Span> = Vec::new();
        for (i, label) in tabs.iter().enumerate() {
            let chip = format!(" {}:{} ", i + 1, label);
            let w = chip.chars().count() as u16;
            let style = if i == sub_tab {
                Style::default().fg(p.selection_fg).bg(p.accent).add_modifier(Modifier::BOLD)
            } else {
                p.dim_style()
            };
            if x < inner.right() {
                hits.push(
                    Rect { x, y: inner.y, width: w.min(inner.right() - x), height: 1 },
                    HitTarget::SubTab(i),
                );
            }
            spans.push(Span::styled(chip, style));
            x = x.saturating_add(w);
        }
        frame.render_widget(Paragraph::new(Line::from(spans)),
            Rect { x: inner.x, y: inner.y, width: inner.width, height: 1 });
        content_top = inner.y + 1;
    }
    let content_area = Rect {
        x: inner.x,
        y: content_top,
        width: inner.width,
        height: inner.bottom().saturating_sub(content_top),
    };
    if content_area.height == 0 {
        return;
    }

    // Resolve full definition + run files for the current selection.
    let def = if let DetailContext::Definition { repo, name } = &ctx {
        app.full_defs.get(&format!("{repo}/{name}")).cloned()
    } else {
        None
    };
    let run_files = match &ctx {
        DetailContext::Run { task } => app
            .run_files
            .as_ref()
            .filter(|(id, _)| id == &task.id)
            .map(|(_, f)| f),
        _ => None,
    };

    let (lines, placeholder) = content_for(&ctx, sub_tab, def.as_ref(), run_files);
    if lines.is_empty() {
        frame.render_widget(Paragraph::new(placeholder).style(p.dim_style()), content_area);
        return;
    }
    let bottom = bottom_anchored(kind, sub_tab);
    let height = content_area.height as usize;
    let (start, end) = window_lines(lines.len(), height, app_scroll_offset(app, c), bottom);
    let styled: Vec<Line> = lines[start..end]
        .iter()
        .map(|l| if l.is_empty() { Line::from(" ") } else { style_line(l, p) })
        .collect();
    frame.render_widget(Paragraph::new(Text::from(styled)), content_area);

    // Scrollbar over the content region.
    if lines.len() > height {
        let mut state = ScrollbarState::new(lines.len() - height).position(start);
        hits.push(
            Rect { x: content_area.right().saturating_sub(1), y: content_area.y, width: 1, height: content_area.height },
            HitTarget::ScrollbarTrack(PaneId::Detail),
        );
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight),
            content_area,
            &mut state,
        );
    }
}

fn app_scroll_offset(app: &App, c: &Computed) -> usize {
    let _ = app;
    c.ui.scroll_offset
}
```

- [ ] **Step 6: Add scroll handling to `app.rs` `update`.** Add these action handlers (invoked from Task 11's key dispatch; defined now so scroll semantics land with the detail work). Complete methods:

```rust
impl App {
    /// Current detail kind + sub-tab for the active tab (needed for scroll inversion).
    fn detail_kind_and_subtab(&self) -> (crate::app::DetailKind, usize) {
        let c = crate::view::compute(self);
        let ctx = match (&self.snapshot, &c.active_name) {
            (Some(snap), Some(name)) => crate::detail::derive_context(
                snap, name, c.ui.last_list_pane, &c.queue, &c.worktrees, &c.defs, &c.ui.selections,
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
        let ui = self.ui();
        let next = (ui.scroll_offset as i64 + step as i64).max(0) as usize;
        if next == ui.scroll_offset {
            return false;
        }
        ui.scroll_offset = next;
        true
    }

    /// `ScrollEdge(dir)` in the detail pane. dir < 0 = head/oldest, dir > 0 =
    /// tail/end. Uses a large sentinel that `window_lines` clamps to the real max.
    pub(crate) fn detail_scroll_edge(&mut self, dir: i32) -> bool {
        let (kind, sub) = self.detail_kind_and_subtab();
        let bottom = crate::detail::bottom_anchored(kind, sub);
        let to_head = dir < 0;
        // Bottom-anchored: head = large offset, tail = 0. Top-anchored: reverse.
        let offset = if bottom {
            if to_head { 1_000_000 } else { 0 }
        } else if to_head {
            0
        } else {
            1_000_000
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
}
```

- [ ] **Step 7: Detail snapshot test.** In `view/detail.rs` `#[cfg(test)] mod tests`: build `fixture_app()`, set `run_files = Some(("01RUN", RunFiles { transcript_tail: (0..40).map(|i| format!("line {i}")).collect(), report: vec![] }))`, focus detail on the queue selection (`last_list_pane = Queue`, `focus = Detail`), render at 80×24, `insta::assert_snapshot!("detail_transcript", terminal.backend())`. Assert a `SubTab(0)` hit target exists. Also a clamp test: set `sub_tab[Run as usize] = 9`, render, assert no panic and content = transcript (sub-tab clamped to 0..2). Run → `cargo insta accept` → PASS.

- [ ] **Step 8: Run full + commit.** `cargo test -p qoo-tui` (green).
  `git add crates/qoo-tui/src/detail.rs crates/qoo-tui/src/view/detail.rs crates/qoo-tui/src/app.rs crates/qoo-tui/src/lib.rs crates/qoo-tui/src/snapshots/`
  `git commit -m "feat(tui-rs): detail context, sub-tabs, anchored line windowing"`

---

### Task 10: runfiles.rs + wiring

**Files:**
- Create: `crates/qoo-tui/src/runfiles.rs`
- Modify: `crates/qoo-tui/src/app.rs` (run-file scheduling + `Event::RunFiles` handling in `update`)
- Modify: `crates/qoo-tui/src/event.rs` (executor arm for `Cmd::ReadRunFiles`, if Task 4 stubbed it)
- Modify: `crates/qoo-tui/Cargo.toml` (`cargo add --dev tempfile`)
- Test: `crates/qoo-tui/src/runfiles.rs` (`#[cfg(test)] mod tests`, tokio tests) + `crates/qoo-tui/src/app.rs` wiring tests

**Interfaces:**
- Produces (contract): `pub struct RunFiles { pub transcript_tail: Vec<String>, pub report: Vec<String> }` deriving `Debug, Clone, PartialEq, Default` (PartialEq drives the identical-content skip); `pub async fn read_run_files(runs_dir: &Path, task_id: &str, tail_lines: usize) -> RunFiles`.
- Produces (app.rs helpers): `pub(crate) fn detail_height(&self) -> usize`, `pub(crate) fn tail_lines(&self) -> usize`, `pub(crate) fn selected_run_task(&self) -> Option<(String, bool /* running */)>`, `pub(crate) fn schedule_run_read(&self, cmds: &mut Vec<Cmd>, delay_ms: u64)`.
- Consumes: `event::{Event, Cmd}` (contract: `Cmd::ReadRunFiles { task_id, tail_lines, delay_ms }`, `Event::RunFiles { task_id, files }`), `detail::{derive_context, DetailContext}`, `view::compute`.
- **Parity deviation (deliberate, per plan spec):** TS `readTranscriptTail` uses a window of `min(262144, max(65536, tailLines*512))` and keeps whatever partial first line the seek produced. This port uses a **fixed 256 KiB window and drops the first (partial) line when the read started mid-file** — strictly more correct (never renders a torn line); the tail-lines contract (`last N lines`) is identical for every file the TS tests cover. Report becomes `Vec<String>` lines (TS kept one string) because the detail renderer consumes lines.
- **Debounce semantics:** the TS 120 ms selection-settle debounce (clear-pending-timer per keypress) becomes *delay + stale-discard*: every selection change emits `Cmd::ReadRunFiles { delay_ms: 120 }`; the executor sleeps then reads; `update()` discards any result whose `task_id` no longer matches the selection and skips `dirty` when content is identical. Rapid cursor movement causes a few extra cheap reads instead of zero — same rendered behavior, no timers in App.

- [ ] **Step 1: Failing runfiles tests.** Create `runfiles.rs` with the struct + an `unimplemented!()`-free stub (`read_run_files` returning `RunFiles::default()`) and this test module (mirrors `run-files.test.ts` cases):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn setup(task_id: &str, transcript: Option<&str>, report: Option<&str>) -> PathBuf {
        let runs = tempfile::tempdir().unwrap().keep();
        let dir = runs.join(task_id);
        std::fs::create_dir_all(&dir).unwrap();
        if let Some(t) = transcript {
            std::fs::write(dir.join("transcript.md"), t).unwrap();
        }
        if let Some(r) = report {
            std::fs::write(dir.join("report.md"), r).unwrap();
        }
        runs
    }

    #[tokio::test]
    async fn reads_report_and_last_25_lines() {
        let lines: Vec<String> = (0..40).map(|i| format!("line {i}")).collect();
        let runs = setup("01TASK", Some(&lines.join("\n")), Some("# Result\nok\n"));
        let out = read_run_files(&runs, "01TASK", 25).await;
        assert_eq!(out.report[0], "# Result");
        assert_eq!(out.transcript_tail.len(), 25);
        assert_eq!(out.transcript_tail[24], "line 39");
    }

    #[tokio::test]
    async fn honors_tail_lines() {
        let lines: Vec<String> = (0..200).map(|i| format!("line {i}")).collect();
        let runs = setup("01TAIL", Some(&lines.join("\n")), None);
        let out = read_run_files(&runs, "01TAIL", 100).await;
        assert_eq!(out.transcript_tail.len(), 100);
        assert_eq!(out.transcript_tail[99], "line 199");
        assert!(out.report.is_empty());
    }

    #[tokio::test]
    async fn clamps_tail_lines_below_1() {
        let lines: Vec<String> = (0..10).map(|i| format!("line {i}")).collect();
        let runs = setup("01ZERO", Some(&lines.join("\n")), None);
        let out = read_run_files(&runs, "01ZERO", 0).await;
        assert_eq!(out.transcript_tail, vec!["line 9".to_string()]);
    }

    #[tokio::test]
    async fn missing_dir_yields_empty() {
        let runs = tempfile::tempdir().unwrap().keep();
        let out = read_run_files(&runs, "01NOPE", 25).await;
        assert!(out.report.is_empty());
        assert!(out.transcript_tail.is_empty());
    }

    #[tokio::test]
    async fn empty_transcript_yields_empty() {
        let runs = setup("01EMPTY", Some(""), None);
        let out = read_run_files(&runs, "01EMPTY", 25).await;
        assert!(out.transcript_tail.is_empty());
    }

    #[tokio::test]
    async fn large_transcript_tail_correct_and_partial_line_dropped() {
        // Push well past the 256 KiB window, then 25 known tail lines. The seek
        // lands mid-line inside the padding; the torn first line must be dropped.
        let padding: Vec<String> = (0..8000)
            .map(|i| format!("padding line {i} {}", "x".repeat(32)))
            .collect();
        let tail: Vec<String> = (0..25).map(|i| format!("tail {i}")).collect();
        let content = [padding.clone(), tail.clone()].concat().join("\n");
        assert!(content.len() > 262_144);
        let runs = setup("01BIG", Some(&content), None);
        let out = read_run_files(&runs, "01BIG", 25).await;
        assert_eq!(out.transcript_tail, tail);
        // Torn-line check: nothing in the tail starts mid-word garbage.
        assert!(out.transcript_tail.iter().all(|l| l.starts_with("tail ")));
    }
}
```

- [ ] **Step 2: Run — expect FAIL.** `cargo add --dev tempfile -p qoo-tui`, then `cargo test -p qoo-tui runfiles::tests`.

- [ ] **Step 3: Implement `runfiles.rs`.** Full file:

```rust
use std::io::SeekFrom;
use std::path::Path;

use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

/// Fixed tail window: read at most this many bytes from the end of transcript.md.
const TAIL_WINDOW: u64 = 262_144;

/// Files backing a single run's detail view. `PartialEq` powers the
/// identical-content dedup in `App::update` (a quiet 1s poll must not dirty).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RunFiles {
    pub transcript_tail: Vec<String>,
    pub report: Vec<String>,
}

/// Read `<runs_dir>/<task_id>/{report.md, transcript.md}`. Report is read fully
/// and split into lines; transcript is tail-read: stat the length, seek to
/// `max(0, len − 256KiB)`, read to end, drop the first (partial) line when the
/// seek started mid-file, keep the last `tail_lines` lines. Missing or
/// unreadable files yield empty vecs — never an error (parity: run dirs appear
/// lazily as the worker writes).
pub async fn read_run_files(runs_dir: &Path, task_id: &str, tail_lines: usize) -> RunFiles {
    let dir = runs_dir.join(task_id);
    let report = match fs::read_to_string(dir.join("report.md")).await {
        Ok(s) => s.split('\n').map(str::to_string).collect(),
        Err(_) => Vec::new(),
    };
    let transcript_tail = read_tail(&dir.join("transcript.md"), tail_lines)
        .await
        .unwrap_or_default();
    RunFiles { transcript_tail, report }
}

async fn read_tail(path: &Path, tail_lines: usize) -> std::io::Result<Vec<String>> {
    // Clamp at the source (parity with the TS slice(-0) guard): 0 would keep
    // everything instead of one line.
    let tail_lines = tail_lines.max(1);
    let mut file = fs::File::open(path).await?;
    let len = file.metadata().await?.len();
    if len == 0 {
        return Ok(Vec::new());
    }
    let start = len.saturating_sub(TAIL_WINDOW);
    if start > 0 {
        file.seek(SeekFrom::Start(start)).await?;
    }
    let mut buf = Vec::with_capacity((len - start) as usize);
    file.read_to_end(&mut buf).await?;
    let text = String::from_utf8_lossy(&buf);
    let mut lines: Vec<&str> = text.split('\n').collect();
    // A mid-file seek almost certainly landed inside a line: the first split
    // element is a torn fragment — drop it (keep at least one line).
    if start > 0 && lines.len() > 1 {
        lines.remove(0);
    }
    let keep_from = lines.len().saturating_sub(tail_lines);
    Ok(lines[keep_from..].iter().map(|s| s.to_string()).collect())
}
```

- [ ] **Step 4: Run — expect PASS.** `cargo test -p qoo-tui runfiles::tests`.

- [ ] **Step 5: Executor arm for `Cmd::ReadRunFiles`.** In `event.rs`'s executor (Task 4's `execute` — the function that receives a `Cmd`, the `UnboundedSender<Event>`, and the runs/sock paths), add or verify this arm (complete code; if Task 4 already implemented it identically, skip):

```rust
Cmd::ReadRunFiles { task_id, tail_lines, delay_ms } => {
    let runs = runs_dir.clone();
    let tx = tx.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
        let files = crate::runfiles::read_run_files(&runs, &task_id, tail_lines).await;
        // Receiver dropped ⇒ app is exiting; ignore.
        let _ = tx.send(Event::RunFiles { task_id, files });
    });
}
```

- [ ] **Step 6: Failing app-wiring tests.** In `app.rs` tests (extend the existing `#[cfg(test)] mod tests`):

```rust
// -- Task 10: run-file wiring ------------------------------------------------
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
    let up = app.update(Event::RunFiles { task_id: "01SOMEONE_ELSE".into(), files: run_files_fixture() });
    assert!(!up.dirty);
    assert!(app.run_files.is_none());
}

#[test]
fn identical_run_files_do_not_dirty_but_still_repoll() {
    let mut app = crate::test_fixtures::fixture_app();
    app.run_files = Some(("01RUN".to_string(), run_files_fixture()));
    let up = app.update(Event::RunFiles { task_id: "01RUN".into(), files: run_files_fixture() });
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
    let up = app.update(Event::RunFiles { task_id: "01RUN".into(), files: run_files_fixture() });
    assert!(up.dirty);
    assert_eq!(app.run_files.as_ref().unwrap().1, run_files_fixture());
}

#[test]
fn no_repoll_when_selected_task_not_running() {
    let mut app = crate::test_fixtures::fixture_app();
    // Point the queue cursor at 01QUE (index 1, a queued task).
    app.ui().selections[0].cursor = 1;
    let up = app.update(Event::RunFiles { task_id: "01QUE".into(), files: run_files_fixture() });
    assert!(up.dirty);
    assert!(up.cmds.is_empty(), "non-running task must not start the 1s poll loop");
}
```

- [ ] **Step 7: Run — expect FAIL.** `cargo test -p qoo-tui app::tests` (the wiring is missing).

- [ ] **Step 8: Implement the app.rs wiring.** Add helpers + event handling:

```rust
impl App {
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
            snap, name, c.ui.last_list_pane, &c.queue, &c.worktrees, &c.defs, &c.ui.selections,
        ) {
            crate::detail::DetailContext::Run { task } => Some((
                task.id.clone(),
                matches!(task.status, crate::ipc::types::TaskStatus::Running),
            )),
            _ => None,
        }
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
```

In `update()`:
1. In the `Event::Snapshot(s)` arm, after committing the snapshot (existing Task-5 skeleton code), append `self.schedule_run_read(&mut cmds, 120);`.
2. Add the `Event::RunFiles` arm:

```rust
Event::RunFiles { task_id, files } => {
    let mut cmds = Vec::new();
    // Stale-read discard: the selection moved while the read was in flight.
    let Some((sel_id, running)) = self.selected_run_task() else {
        return Update { dirty: false, cmds };
    };
    if task_id != sel_id {
        return Update { dirty: false, cmds };
    }
    // Poll loop via events: while the selected task runs, each read result
    // arms the next 1s read — no timer state in App.
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
```

(Selection/sub-tab-change scheduling from key/mouse actions lands with those actions in Tasks 11–12 — each mutation helper that moves the cursor, switches tab/sub-tab, or changes `last_list_pane` calls `schedule_run_read(&mut cmds, 120)` and resets `run_files` staleness naturally via the id gate.)

- [ ] **Step 9: Run + commit.** `cargo test -p qoo-tui` (green).
  `git add crates/qoo-tui/src/runfiles.rs crates/qoo-tui/src/app.rs crates/qoo-tui/src/event.rs crates/qoo-tui/src/lib.rs crates/qoo-tui/Cargo.toml Cargo.lock`
  `git commit -m "feat(tui-rs): transcript tail reads with debounce, stale-discard, quiet-poll dedup"`

---

### Task 11: keymap.rs + search + help overlay

**Files:**
- Create: `crates/qoo-tui/src/keymap.rs`
- Create: `crates/qoo-tui/src/view/help.rs`
- Modify: `crates/qoo-tui/src/app.rs` (`apply_action`, `ui()` helper, search-mode input, help mode)
- Modify: `crates/qoo-tui/src/view/mod.rs` (render help overlay when `Mode::Help`)
- Test: `crates/qoo-tui/src/keymap.rs` + `crates/qoo-tui/src/app.rs` test modules

**Interfaces:**
- Produces (contract): `pub enum AppAction { MoveCursor(i32), ExtendSelection(i32), FocusPane(PaneId), CyclePane(i32), SwitchTab(usize), CycleTab(i32), OpenActionMenu, Create, OpenSearch, ClearEsc, Scroll(i32), ScrollEdge(i32), SwitchSubTab(usize), Help, Quit, None }` deriving `Debug, Clone, Copy, PartialEq, Eq`; `pub fn list_mode_action(key: &crossterm::event::KeyEvent, focus: PaneId) -> AppAction`.
- **Contract addition — `AppAction::CycleSubTab(i32)`:** digit keys are project-tab switches globally (spec keymap: `1–9` = switch project tab; the Ink TUI's unprefixed-digit-→-sub-tab binding is retired with the prefix). Sub-tab switching keeps two paths: clickable chips (→ `SwitchSubTab(usize)`, produced by mouse in Task 12) and the `{` / `}` keys → `CycleSubTab(-1/1)`, applied in app.rs as `clamp_sub_tab(current ± 1, kind)`.
- **Contract addition — `AppAction::FocusBack`:** `h`/`←` from the detail pane must return to `last_list_pane`, which the pure keymap cannot see; `FocusBack` is resolved by `apply_action` to `FocusPane(ui.last_list_pane)`.
- Produces (app.rs): `pub(crate) fn ui(&mut self) -> &mut TabUiState` (entry keyed by active project name, `or_default`), `pub(crate) fn apply_action(&mut self, action: AppAction) -> Update`, plus `pub(crate) fn visible_len(&self, pane: ListPane) -> usize`.
- Consumes: `app::{PaneId, ListPane, Mode, TabUiState}`, `detail::{clamp_sub_tab, sub_tab_names}`, Task 9's `detail_scroll`/`detail_scroll_edge`/`reset_scroll`, Task 10's `schedule_run_read`, `selectors::pane_title` (search title comes for free — panes already pass `searching`).
- **g/G semantics (defined here):** keymap always returns `ScrollEdge(-1)` for `g` and `ScrollEdge(1)` for `G`. `apply_action` interprets by focus — detail: `detail_scroll_edge(dir)` (Task 9, sentinel-clamped); list pane: cursor jump to `0` / `len−1` with anchor cleared and scroll reset.
- **Enter semantics:** lists → `OpenActionMenu` (spec: universal "act on this"); detail → `AppAction::None`.

- [ ] **Step 1: Failing keymap decision tests.** Create `keymap.rs` with the enum + a stub `list_mode_action` returning `AppAction::None`, and this test module (translates `keymap.test.ts` decisions to the new bindings):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::PaneId;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn k(code: KeyCode) -> KeyEvent { KeyEvent::new(code, KeyModifiers::NONE) }
    fn sk(code: KeyCode) -> KeyEvent { KeyEvent::new(code, KeyModifiers::SHIFT) }
    const LISTS: [PaneId; 3] = [PaneId::Queue, PaneId::Tasks, PaneId::Worktrees];
    const ALL: [PaneId; 4] = [PaneId::Queue, PaneId::Tasks, PaneId::Worktrees, PaneId::Detail];

    #[test]
    fn q_quits_from_every_pane() {
        for f in ALL {
            assert_eq!(list_mode_action(&k(KeyCode::Char('q')), f), AppAction::Quit);
        }
    }

    #[test]
    fn digits_switch_project_tabs_globally() {
        for f in ALL {
            for n in 1..=9u32 {
                let c = char::from_digit(n, 10).unwrap();
                assert_eq!(
                    list_mode_action(&k(KeyCode::Char(c)), f),
                    AppAction::SwitchTab((n - 1) as usize)
                );
            }
        }
    }

    #[test]
    fn brackets_cycle_project_tabs_and_braces_cycle_sub_tabs() {
        assert_eq!(list_mode_action(&k(KeyCode::Char('[')), PaneId::Queue), AppAction::CycleTab(-1));
        assert_eq!(list_mode_action(&k(KeyCode::Char(']')), PaneId::Queue), AppAction::CycleTab(1));
        assert_eq!(list_mode_action(&k(KeyCode::Char('{')), PaneId::Detail), AppAction::CycleSubTab(-1));
        assert_eq!(list_mode_action(&k(KeyCode::Char('}')), PaneId::Detail), AppAction::CycleSubTab(1));
    }

    #[test]
    fn tab_cycles_panes() {
        assert_eq!(list_mode_action(&k(KeyCode::Tab), PaneId::Queue), AppAction::CyclePane(1));
        assert_eq!(list_mode_action(&k(KeyCode::BackTab), PaneId::Detail), AppAction::CyclePane(-1));
    }

    #[test]
    fn jk_arrows_move_cursor_in_lists() {
        for f in LISTS {
            assert_eq!(list_mode_action(&k(KeyCode::Char('j')), f), AppAction::MoveCursor(1));
            assert_eq!(list_mode_action(&k(KeyCode::Down), f), AppAction::MoveCursor(1));
            assert_eq!(list_mode_action(&k(KeyCode::Char('k')), f), AppAction::MoveCursor(-1));
            assert_eq!(list_mode_action(&k(KeyCode::Up), f), AppAction::MoveCursor(-1));
        }
    }

    #[test]
    fn jk_arrows_scroll_in_detail() {
        assert_eq!(list_mode_action(&k(KeyCode::Char('j')), PaneId::Detail), AppAction::Scroll(1));
        assert_eq!(list_mode_action(&k(KeyCode::Down), PaneId::Detail), AppAction::Scroll(1));
        assert_eq!(list_mode_action(&k(KeyCode::Char('k')), PaneId::Detail), AppAction::Scroll(-1));
        assert_eq!(list_mode_action(&k(KeyCode::Up), PaneId::Detail), AppAction::Scroll(-1));
        // shift+arrow in detail keeps scrolling (no extend) — parity with TS.
        assert_eq!(list_mode_action(&sk(KeyCode::Down), PaneId::Detail), AppAction::Scroll(1));
    }

    #[test]
    fn extend_selection_bindings() {
        for f in LISTS {
            assert_eq!(list_mode_action(&sk(KeyCode::Down), f), AppAction::ExtendSelection(1));
            assert_eq!(list_mode_action(&sk(KeyCode::Up), f), AppAction::ExtendSelection(-1));
            assert_eq!(list_mode_action(&sk(KeyCode::Char('J')), f), AppAction::ExtendSelection(1));
            assert_eq!(list_mode_action(&sk(KeyCode::Char('K')), f), AppAction::ExtendSelection(-1));
        }
    }

    #[test]
    fn horizontal_focus_moves() {
        for f in LISTS {
            assert_eq!(list_mode_action(&k(KeyCode::Char('l')), f), AppAction::FocusPane(PaneId::Detail));
            assert_eq!(list_mode_action(&k(KeyCode::Right), f), AppAction::FocusPane(PaneId::Detail));
            // h/← on a list pane stays put.
            assert_eq!(list_mode_action(&k(KeyCode::Char('h')), f), AppAction::None);
            assert_eq!(list_mode_action(&k(KeyCode::Left), f), AppAction::None);
        }
        assert_eq!(list_mode_action(&k(KeyCode::Char('h')), PaneId::Detail), AppAction::FocusBack);
        assert_eq!(list_mode_action(&k(KeyCode::Left), PaneId::Detail), AppAction::FocusBack);
        assert_eq!(list_mode_action(&k(KeyCode::Char('l')), PaneId::Detail), AppAction::None);
    }

    #[test]
    fn enter_opens_action_menu_on_lists_only() {
        for f in LISTS {
            assert_eq!(list_mode_action(&k(KeyCode::Enter), f), AppAction::OpenActionMenu);
        }
        assert_eq!(list_mode_action(&k(KeyCode::Enter), PaneId::Detail), AppAction::None);
    }

    #[test]
    fn a_c_slash_esc_help() {
        for f in ALL {
            assert_eq!(list_mode_action(&k(KeyCode::Char('a')), f), AppAction::OpenActionMenu);
            assert_eq!(list_mode_action(&k(KeyCode::Char('c')), f), AppAction::Create);
            assert_eq!(list_mode_action(&k(KeyCode::Char('?')), f), AppAction::Help);
            assert_eq!(list_mode_action(&k(KeyCode::Esc), f), AppAction::ClearEsc);
        }
        for f in LISTS {
            assert_eq!(list_mode_action(&k(KeyCode::Char('/')), f), AppAction::OpenSearch);
        }
        assert_eq!(list_mode_action(&k(KeyCode::Char('/')), PaneId::Detail), AppAction::None);
    }

    #[test]
    fn g_edges_everywhere() {
        for f in ALL {
            assert_eq!(list_mode_action(&k(KeyCode::Char('g')), f), AppAction::ScrollEdge(-1));
            assert_eq!(list_mode_action(&sk(KeyCode::Char('G')), f), AppAction::ScrollEdge(1));
        }
    }

    #[test]
    fn unbound_keys_are_none() {
        // r/s/w/f/m/t moved to the action menu (parity with the Ink keymap tests).
        for c in ['r', 's', 'w', 'f', 'm', 't', 'z', '0'] {
            assert_eq!(list_mode_action(&k(KeyCode::Char(c)), PaneId::Queue), AppAction::None);
        }
    }
}
```

- [ ] **Step 2: Run — expect FAIL.** `cargo test -p qoo-tui keymap::tests`.

- [ ] **Step 3: Implement `keymap.rs`.** Full file:

```rust
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::PaneId;

/// Contract enum + two additions (see plan): `CycleSubTab(i32)` — `{`/`}` cycle
/// the detail sub-tab (digits are project tabs globally); `FocusBack` — detail
/// h/← returns to `last_list_pane`, resolved in `apply_action`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppAction {
    MoveCursor(i32),
    ExtendSelection(i32),
    FocusPane(PaneId),
    FocusBack,
    CyclePane(i32),
    SwitchTab(usize),
    CycleTab(i32),
    CycleSubTab(i32),
    OpenActionMenu,
    Create,
    OpenSearch,
    ClearEsc,
    Scroll(i32),
    ScrollEdge(i32),
    SwitchSubTab(usize),
    Help,
    Quit,
    None,
}

/// KeyEvent → AppAction in `Mode::List`. Pure; per-pane semantics resolved here
/// (lists vs detail), per-tab state resolved by `App::apply_action`.
/// Version note: crossterm 0.29 delivers shifted letters as uppercase
/// `Char('J')` with `SHIFT` set; we match on the char and treat the modifier as
/// advisory.
pub fn list_mode_action(key: &KeyEvent, focus: PaneId) -> AppAction {
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let detail = matches!(focus, PaneId::Detail);
    match key.code {
        KeyCode::Tab => AppAction::CyclePane(1),
        KeyCode::BackTab => AppAction::CyclePane(-1),
        KeyCode::Char(c @ '1'..='9') => AppAction::SwitchTab(c as usize - '1' as usize),
        KeyCode::Char('[') => AppAction::CycleTab(-1),
        KeyCode::Char(']') => AppAction::CycleTab(1),
        KeyCode::Char('{') => AppAction::CycleSubTab(-1),
        KeyCode::Char('}') => AppAction::CycleSubTab(1),
        KeyCode::Char('q') => AppAction::Quit,
        KeyCode::Char('?') => AppAction::Help,
        KeyCode::Char('a') => AppAction::OpenActionMenu,
        KeyCode::Char('c') => AppAction::Create,
        KeyCode::Char('g') => AppAction::ScrollEdge(-1),
        KeyCode::Char('G') => AppAction::ScrollEdge(1),
        KeyCode::Esc => AppAction::ClearEsc,
        KeyCode::Char('/') if !detail => AppAction::OpenSearch,
        KeyCode::Enter if !detail => AppAction::OpenActionMenu,
        KeyCode::Char('J') if !detail => AppAction::ExtendSelection(1),
        KeyCode::Char('K') if !detail => AppAction::ExtendSelection(-1),
        KeyCode::Down | KeyCode::Char('j') => {
            if detail {
                AppAction::Scroll(1)
            } else if shift {
                AppAction::ExtendSelection(1)
            } else {
                AppAction::MoveCursor(1)
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if detail {
                AppAction::Scroll(-1)
            } else if shift {
                AppAction::ExtendSelection(-1)
            } else {
                AppAction::MoveCursor(-1)
            }
        }
        KeyCode::Right | KeyCode::Char('l') => {
            if detail { AppAction::None } else { AppAction::FocusPane(PaneId::Detail) }
        }
        KeyCode::Left | KeyCode::Char('h') => {
            if detail { AppAction::FocusBack } else { AppAction::None }
        }
        _ => AppAction::None,
    }
}
```

- [ ] **Step 4: Run — expect PASS.** `cargo test -p qoo-tui keymap::tests`.

- [ ] **Step 5: Failing app-integration tests** (staged esc, focus wrap, search through `update()`, help, status-line clear, g/G list jump):

```rust
// -- Task 11: key dispatch through update() -----------------------------------
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn press(app: &mut App, code: KeyCode) -> Update {
    app.update(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)))
}

#[test]
fn cycle_pane_wraps_queue_tasks_worktrees_detail() {
    let mut app = crate::test_fixtures::fixture_app();
    let order = [PaneId::Tasks, PaneId::Worktrees, PaneId::Detail, PaneId::Queue];
    for expected in order {
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.ui().focus, expected);
    }
}

#[test]
fn focus_back_returns_to_last_list_pane() {
    let mut app = crate::test_fixtures::fixture_app();
    press(&mut app, KeyCode::Tab); // → tasks
    press(&mut app, KeyCode::Char('l')); // → detail, last_list_pane = tasks
    assert_eq!(app.ui().focus, PaneId::Detail);
    press(&mut app, KeyCode::Char('h'));
    assert_eq!(app.ui().focus, PaneId::Tasks);
}

#[test]
fn move_cursor_clamps_and_clears_anchor() {
    let mut app = crate::test_fixtures::fixture_app();
    // 4 queue rows (3 live + 1 archived); hammer j past the end.
    for _ in 0..10 {
        press(&mut app, KeyCode::Char('j'));
    }
    assert_eq!(app.ui().selections[0].cursor, 3);
    press(&mut app, KeyCode::Char('J')); // can't extend past end → anchor stays None
    assert_eq!(app.ui().selections[0].anchor, None);
    press(&mut app, KeyCode::Char('K'));
    assert_eq!(app.ui().selections[0], Selection { cursor: 2, anchor: Some(3) });
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
    // Build a range and a filter.
    app.ui().search[0] = "line".into();
    press(&mut app, KeyCode::Char('J')); // range 0..1
    assert!(app.ui().selections[0].anchor.is_some());
    press(&mut app, KeyCode::Esc); // 1: clears range
    assert_eq!(app.ui().selections[0].anchor, None);
    assert_eq!(app.ui().search[0], "line");
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
    // "docs" prompt matches only 01QUE's summary → 1 visible row, cursor reset.
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
    // Run context (queue cursor 0 → 01RUN): 3 sub-tabs.
    press(&mut app, KeyCode::Char('}'));
    assert_eq!(app.ui().sub_tab[DetailKind::Run as usize], 1);
    press(&mut app, KeyCode::Char('}'));
    press(&mut app, KeyCode::Char('}'));
    assert_eq!(app.ui().sub_tab[DetailKind::Run as usize], 0, "wraps past the end");
    press(&mut app, KeyCode::Char('{'));
    assert_eq!(app.ui().sub_tab[DetailKind::Run as usize], 2, "wraps below zero");
}
```

- [ ] **Step 6: Run — expect FAIL.** `cargo test -p qoo-tui app::tests`.

- [ ] **Step 7: Implement app.rs dispatch.** Add the `ui()`/`visible_len` helpers and `apply_action`; route `Event::Key` per mode. Complete code:

```rust
impl App {
    /// Mutable per-tab UI state for the active project (created on demand).
    pub(crate) fn ui(&mut self) -> &mut TabUiState {
        let name = self.active_project_name().unwrap_or_default();
        self.ui_by_tab.entry(name).or_default()
    }

    fn active_project_name(&self) -> Option<String> {
        let snap = self.snapshot.as_ref()?;
        let tabs = crate::selectors::build_tabs(snap);
        let idx = self.active_tab.min(tabs.len().checked_sub(1)?);
        tabs.get(idx).map(|t| t.name.clone())
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
                let tabs = self.snapshot.as_ref().map(|s| crate::selectors::build_tabs(s).len()).unwrap_or(0);
                if i < tabs && i != self.active_tab {
                    self.active_tab = i;
                    self.schedule_run_read(&mut cmds, 120);
                    true
                } else {
                    false
                }
            }
            A::CycleTab(d) => {
                let tabs = self.snapshot.as_ref().map(|s| crate::selectors::build_tabs(s).len()).unwrap_or(0);
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
                const ORDER: [PaneId; 4] = [PaneId::Queue, PaneId::Tasks, PaneId::Worktrees, PaneId::Detail];
                let cur = ORDER.iter().position(|p| *p == self.ui().focus).unwrap_or(0) as i64;
                let next = ORDER[((cur + d as i64).rem_euclid(4)) as usize];
                self.set_focus(next);
                self.schedule_run_read(&mut cmds, 120);
                true
            }
            A::FocusPane(p) => {
                self.set_focus(p);
                true
            }
            A::FocusBack => {
                let back = self.ui().last_list_pane;
                self.set_focus(match back {
                    ListPane::Queue => PaneId::Queue,
                    ListPane::Tasks => PaneId::Tasks,
                    ListPane::Worktrees => PaneId::Worktrees,
                });
                true
            }
            A::MoveCursor(d) => match self.focused_list() {
                Some(pane) => {
                    let cur = self.ui().selections[pane as usize].cursor as i64;
                    let next = (cur + d as i64).max(0) as usize;
                    self.set_cursor(pane, next, &mut cmds)
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
            A::Scroll(d) => self.detail_scroll(d),
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
            // M2 stubs — replaced in Task 14 (action menus) / Task 15 (create modals).
            A::OpenActionMenu => {
                self.status_line = Some("actions arrive in M2".into());
                true
            }
            A::Create => {
                self.status_line = Some("create arrives in M2".into());
                true
            }
        };
        Update { dirty, cmds }
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
}
```

Route `Event::Key(key)` in `update()` (replacing the Task-5 skeleton's bare `q` handler):

```rust
Event::Key(key) => match &self.mode {
    Mode::Help => {
        // Any key closes the help overlay.
        self.mode = Mode::List;
        Update { dirty: true, cmds: Vec::new() }
    }
    Mode::Search { pane } => {
        let pane = *pane;
        let mut dirty = true;
        match key.code {
            crossterm::event::KeyCode::Enter => self.mode = Mode::List, // apply
            crossterm::event::KeyCode::Esc => {
                self.ui().search[pane as usize].clear();
                self.ui().selections[pane as usize] = Selection { cursor: 0, anchor: None };
                self.mode = Mode::List;
            }
            crossterm::event::KeyCode::Backspace => {
                self.ui().search[pane as usize].pop();
                self.ui().selections[pane as usize] = Selection { cursor: 0, anchor: None };
            }
            crossterm::event::KeyCode::Char(c)
                if !key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) =>
            {
                self.ui().search[pane as usize].push(c);
                self.ui().selections[pane as usize] = Selection { cursor: 0, anchor: None };
            }
            _ => dirty = false,
        }
        Update { dirty, cmds: Vec::new() }
    }
    Mode::List => {
        // Status line clears on ANY list-mode keypress (even unbound keys).
        let had_status = self.status_line.take().is_some();
        let action = crate::keymap::list_mode_action(&key, self.ui().focus);
        let mut up = self.apply_action(action);
        up.dirty = up.dirty || had_status;
        up
    }
    _ => Update { dirty: false, cmds: Vec::new() }, // M2/M3 modal modes: later tasks
},
```

(`TabUiState` must `derive(Default)` with `focus: PaneId::Queue`, `last_list_pane: ListPane::Queue` — implement `Default` manually if the derive can't express it.)

- [ ] **Step 8: Implement `view/help.rs`.** Full file (centered popup listing the spec keymap; registered as `Modal` so Task 12's click routing treats it as an overlay):

```rust
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

use crate::hit::{HitMap, HitTarget};
use crate::view::theme::Palette;

const HELP_ROWS: [(&str, &str); 13] = [
    ("Tab / Shift+Tab", "cycle focus between panes (incl. detail)"),
    ("1–9", "switch project tab"),
    ("[ / ]", "previous / next project tab"),
    ("{ / }", "previous / next detail sub-tab"),
    ("j/k · arrows", "move cursor (detail: scroll)"),
    ("J/K · shift+↑↓", "extend selection"),
    ("Enter / a", "action menu for selection"),
    ("c", "create (queue: adhoc task · worktrees: worktree)"),
    ("/", "filter focused pane"),
    ("esc", "clear range → clear filter → close overlay"),
    ("g / G", "top / bottom (detail scroll or list jump)"),
    ("?", "this help"),
    ("q", "quit"),
];

pub fn render(frame: &mut ratatui::Frame, area: Rect, hits: &mut HitMap, p: &Palette) {
    let width = (area.width.saturating_sub(8)).clamp(20, 64);
    let height = (HELP_ROWS.len() as u16 + 3).min(area.height);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let popup = Rect { x, y, width, height };
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(p.accent))
        .title(Span::styled(" keymap ", Style::default().fg(p.fg).add_modifier(Modifier::BOLD)));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let mut lines: Vec<Line> = HELP_ROWS
        .iter()
        .map(|(keys, what)| {
            Line::from(vec![
                Span::styled(format!(" {keys:<16}"), Style::default().fg(p.accent)),
                Span::styled((*what).to_string(), Style::default().fg(p.fg)),
            ])
        })
        .collect();
    lines.push(Line::from(Span::styled(" any key to close", p.dim_style())));
    frame.render_widget(Paragraph::new(Text::from(lines)), inner);
    // Registered last → topmost: clicks inside never leak to the body.
    hits.push(popup, HitTarget::Modal);
}
```

In `view/mod.rs::render`, after the footer (so the popup registers last in the hit map):

```rust
if matches!(app.mode, crate::app::Mode::Help) {
    help::render(frame, area, &mut hits, &p);
}
```

and add `pub mod help;` to the view module list.

- [ ] **Step 9: Run — expect PASS.** `cargo test -p qoo-tui` (keymap + app tests green; existing view snapshots unchanged — help only renders in `Mode::Help`). Add one insta snapshot: render `fixture_app()` with `mode = Mode::Help` at 80×24 → `insta::assert_snapshot!("view_help_overlay", ...)`; `cargo insta accept`.

- [ ] **Step 10: Commit.**
  `git add crates/qoo-tui/src/keymap.rs crates/qoo-tui/src/view/help.rs crates/qoo-tui/src/view/mod.rs crates/qoo-tui/src/app.rs crates/qoo-tui/src/lib.rs crates/qoo-tui/src/snapshots/`
  `git commit -m "feat(tui-rs): direct-key keymap, staged esc, search mode, help overlay"`

---

### Task 12: mouse + M1 integration

**Files:**
- Modify: `crates/qoo-tui/src/app.rs` (`Event::Mouse` routing, `drag` field, wheel/click/drag helpers)
- Modify: `crates/qoo-tui/src/view/detail.rs` (`content_for` → `pub(crate)`, add `detail_content_len`)
- Modify: `crates/qoo-tui/src/main.rs` (final assembly: guard → channel → subscription → loop → restore; store render's HitMap into `app.hit`)
- Test: `crates/qoo-tui/src/app.rs` mouse tests + idle-render test

**Interfaces:**
- **Contract addition — `App::drag: pub Option<PaneId>`:** scrollbar drag state; `Some(pane)` between a Down on `ScrollbarThumb/Track` and the matching Up. Initialize `None` in `App::new`.
- Produces (app.rs): `fn on_mouse(&mut self, m: crossterm::event::MouseEvent) -> Update`; `fn drag_to_offset(&mut self, pane: PaneId, y: u16, cmds: &mut Vec<Cmd>) -> bool`; `fn wheel_pane_under(&self, col: u16, row: u16) -> Option<PaneId>`.
- Produces (view/detail.rs): `pub(crate) fn detail_content_len(app: &App) -> usize` — total content lines of the current detail view (drag math needs the scrollable extent).
- Consumes: `hit::{HitMap, HitTarget}` (`app.hit` is the **previous frame's** map, stored by the main loop after each draw — geometry is stable between draws because every state change redraws), Task 11's `apply_action`/`set_cursor`/`set_focus` helpers, Task 10's `schedule_run_read`.
- **Decision (recorded):** the wheel scrolls the pane **under the cursor** and does **not** change focus — lists move that pane's cursor (anchor cleared, run-read scheduled), detail scrolls content with bottom-anchor inversion. Row/PaneBody/ScrollbarThumb/ScrollbarTrack targets all map to their pane for wheel routing.
- **Decision (recorded):** click on the already-selected single row opens its action menu; until Task 14 lands this sets `status_line = "actions arrive in M2"` (explicitly marked: replace the stub call with `apply_action(AppAction::OpenActionMenu)`'s real menu in Task 14).
- **Drag math (defined):** `offset = (y − track_top) × scrollable ÷ track_h`, clamped to `[0, scrollable]`. For list panes `scrollable = visible_len − 1` and the result is the **cursor index**; for detail `scrollable = detail_content_len − content_height` and the result is the window **start**, converted to `scroll_offset` (`bottom_anchored ⇒ offset = scrollable − start`, else `offset = start`).
- **Assumption on Task 4's executor entry point:** this task's `main.rs` calls `crate::event::execute(cmd, tx.clone(), &runs_dir, &sock_path)` for every non-`Quit` cmd. If Task 4 named the function differently, adapt the one call site mechanically — the contract-level requirement is only "each Cmd is performed on a tokio task and results re-enter as Events".

- [ ] **Step 1: Failing mouse tests.** In `app.rs` tests:

```rust
// -- Task 12: mouse routing ----------------------------------------------------
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
fn click_row_focuses_and_selects() {
    let mut app = app_with_hits();
    app.set_focus(PaneId::Detail); // start elsewhere
    let up = app.update(mouse(MouseEventKind::Down(MouseButton::Left), 5, 4)); // row 2
    assert!(up.dirty);
    assert_eq!(app.ui().focus, PaneId::Queue);
    assert_eq!(app.ui().selections[0], Selection { cursor: 2, anchor: None });
}

#[test]
fn click_selected_row_again_opens_menu_stub() {
    let mut app = app_with_hits();
    app.update(mouse(MouseEventKind::Down(MouseButton::Left), 5, 3)); // select row 1
    app.status_line = None;
    app.update(mouse(MouseEventKind::Down(MouseButton::Left), 5, 3)); // same row
    assert_eq!(app.status_line.as_deref(), Some("actions arrive in M2"));
}

#[test]
fn shift_click_extends_selection() {
    let mut app = app_with_hits();
    app.update(mouse(MouseEventKind::Down(MouseButton::Left), 5, 2)); // row 0
    let ev = Event::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 5,
        row: 5, // row 3
        modifiers: KeyModifiers::SHIFT,
    });
    app.update(ev);
    assert_eq!(app.ui().selections[0], Selection { cursor: 3, anchor: Some(0) });
}

#[test]
fn wheel_moves_pane_under_cursor_without_focus_change() {
    let mut app = app_with_hits();
    app.set_focus(PaneId::Detail);
    let up = app.update(mouse(MouseEventKind::ScrollDown, 5, 3)); // over queue body
    assert!(up.dirty);
    assert_eq!(app.ui().focus, PaneId::Detail, "wheel must not steal focus");
    assert_eq!(app.ui().selections[0].cursor, 1);
    app.update(mouse(MouseEventKind::ScrollUp, 5, 3));
    assert_eq!(app.ui().selections[0].cursor, 0);
}

#[test]
fn scrollbar_drag_math_maps_proportionally() {
    let mut app = app_with_hits();
    // Track: y=2, h=10. Queue has 4 rows → scrollable = 3.
    app.update(mouse(MouseEventKind::Down(MouseButton::Left), 30, 2)); // top
    assert!(app.drag == Some(PaneId::Queue));
    assert_eq!(app.ui().selections[0].cursor, 0); // (2−2)*3/10 = 0
    app.update(mouse(MouseEventKind::Drag(MouseButton::Left), 30, 11)); // near bottom
    assert_eq!(app.ui().selections[0].cursor, 2); // (11−2)*3/10 = 2
    app.update(mouse(MouseEventKind::Drag(MouseButton::Left), 30, 40)); // past end → clamp
    assert_eq!(app.ui().selections[0].cursor, 3);
    app.update(mouse(MouseEventKind::Up(MouseButton::Left), 30, 40));
    assert_eq!(app.drag, None);
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
```

- [ ] **Step 2: Run — expect FAIL.** `cargo test -p qoo-tui app::tests` (no `Event::Mouse` handling, no `drag` field).

- [ ] **Step 3: Expose content length from `view/detail.rs`.** Change `fn content_for` to `pub(crate) fn content_for` and add:

```rust
/// Total content lines of the current detail view — the drag math's scrollable
/// extent. Recomputes the same context/content the renderer uses.
pub(crate) fn detail_content_len(app: &crate::app::App) -> usize {
    let c = crate::view::compute(app);
    let ctx = match (&app.snapshot, &c.active_name) {
        (Some(snap), Some(name)) => derive_context(
            snap, name, c.ui.last_list_pane, &c.queue, &c.worktrees, &c.defs, &c.ui.selections,
        ),
        _ => DetailContext::Empty,
    };
    let kind = ctx.kind();
    let sub = clamp_sub_tab(c.ui.sub_tab[kind as usize], kind);
    let def = if let DetailContext::Definition { repo, name } = &ctx {
        app.full_defs.get(&format!("{repo}/{name}")).cloned()
    } else {
        None
    };
    let run_files = match &ctx {
        DetailContext::Run { task } => app
            .run_files
            .as_ref()
            .filter(|(id, _)| id == &task.id)
            .map(|(_, f)| f),
        _ => None,
    };
    content_for(&ctx, sub, def.as_ref(), run_files).0.len()
}
```

- [ ] **Step 4: Implement `Event::Mouse` in app.rs.** Add `pub drag: Option<PaneId>` to `App` (+ `drag: None` in `App::new`). Add the arm `Event::Mouse(m) => self.on_mouse(m),` and:

```rust
impl App {
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

    fn on_mouse(&mut self, m: crossterm::event::MouseEvent) -> Update {
        use crossterm::event::{MouseButton, MouseEventKind as K};
        let mut cmds = Vec::new();
        let target = self.hit.hit(m.column, m.row).cloned();
        let dirty = match m.kind {
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
                    self.set_focus(p);
                    true
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
                        let sel = self.ui().selections[pane as usize];
                        let already = sel.cursor == i && sel.anchor.is_none();
                        self.set_cursor(pane, i, &mut cmds);
                        if already {
                            // M2 STUB — Task 14 replaces this with
                            // apply_action(AppAction::OpenActionMenu)'s real menu.
                            self.status_line = Some("actions arrive in M2".into());
                        }
                        true
                    }
                }
                Some(HitTarget::ScrollbarThumb(p)) | Some(HitTarget::ScrollbarTrack(p)) => {
                    self.drag = Some(p);
                    self.drag_to_offset(p, m.row, &mut cmds)
                }
                Some(_) => false, // MenuItem/FormField/DropdownItem/Button: M2/M3
            },
            K::Drag(MouseButton::Left) => match self.drag {
                Some(p) => self.drag_to_offset(p, m.row, &mut cmds),
                None => false,
            },
            K::Up(MouseButton::Left) => {
                self.drag = None;
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
}
```

- [ ] **Step 5: Run — expect PASS.** `cargo test -p qoo-tui app::tests`.

- [ ] **Step 6: Assemble the final `main.rs`.** Complete file (thin, per the lib/bin split; consumes Task 1's terminal guard and Task 4's loop pieces — the **new line this task guarantees is `app.hit = hits`** after every draw):

```rust
use std::io;
use std::time::Duration;

use crossterm::event::{Event as CtEvent, EventStream};
use futures::StreamExt;
use qoo_tui::app::App;
use qoo_tui::event::{Cmd, Event};
use qoo_tui::hit::HitMap;

fn draw(
    terminal: &mut ratatui::Terminal<impl ratatui::backend::Backend>,
    app: &mut App,
) -> io::Result<()> {
    let mut hits = HitMap::new();
    terminal.draw(|frame| {
        hits = qoo_tui::view::render(app, frame);
    })?;
    // The loop stores each frame's hit geometry for mouse routing. Stale-free:
    // every state change redraws, so `app.hit` always matches the screen.
    app.hit = hits;
    Ok(())
}

#[tokio::main]
async fn main() -> io::Result<()> {
    let state = qoo_tui::paths::state_path();
    let sock = qoo_tui::paths::socket_path(&state);
    let runs = qoo_tui::paths::runs_path(&state);

    // Task 1's guard: alt-screen + mouse capture on; Drop/panic/SIGINT restore.
    let _guard = qoo_tui::terminal_guard()?;
    let mut terminal = ratatui::init();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Event>();
    let _subscription = qoo_tui::ipc::client::spawn_subscription(sock.clone(), tx.clone());

    let mut app = App::new(runs.clone(), sock.clone());
    if let Ok((w, h)) = crossterm::terminal::size() {
        app.size = (w, h);
    }
    let mut reader = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    draw(&mut terminal, &mut app)?;

    'outer: loop {
        let update = tokio::select! {
            maybe = reader.next() => match maybe {
                Some(Ok(CtEvent::Key(k))) => app.update(Event::Key(k)),
                Some(Ok(CtEvent::Mouse(m))) => app.update(Event::Mouse(m)),
                Some(Ok(CtEvent::Resize(w, h))) => {
                    app.size = (w, h);
                    app.update(Event::Resize)
                }
                Some(Ok(_)) => continue,           // focus/paste events: ignore
                Some(Err(_)) | None => break 'outer, // stdin gone
            },
            maybe = rx.recv() => match maybe {
                Some(ev) = maybe => app.update(ev),
                None => break 'outer,
            },
            _ = tick.tick() => {
                // Tick is armed only while the active project has a running
                // task — zero idle renders (update() also returns dirty:false
                // defensively; this check skips even the update call).
                if app.active_has_running() {
                    app.update(Event::Tick)
                } else {
                    continue;
                }
            }
        };
        let mut quit = false;
        for cmd in update.cmds {
            if matches!(cmd, Cmd::Quit) {
                quit = true;
                continue;
            }
            // Task 4's executor: performs the Cmd on a tokio task, results
            // re-enter through tx as Events.
            qoo_tui::event::execute(cmd, tx.clone(), &runs, &sock);
        }
        if quit {
            break;
        }
        if update.dirty {
            draw(&mut terminal, &mut app)?;
        }
    }
    ratatui::restore();
    Ok(())
}
```

(`App::active_has_running(&self) -> bool` — add if Task 5's skeleton lacks it: `true` iff the snapshot has a task with `status == Running` and `target.repo == active project name`. One small method mirroring `activeHasRunningRef`:

```rust
impl App {
    pub fn active_has_running(&self) -> bool {
        let Some(name) = self.active_project_name() else { return false };
        self.snapshot
            .as_ref()
            .map(|s| {
                s.tasks
                    .iter()
                    .any(|t| t.target.repo == name && matches!(t.status, crate::ipc::types::TaskStatus::Running))
            })
            .unwrap_or(false)
    }
}
```
)

**Version notes:** `ratatui::init()`/`ratatui::restore()` are the 0.29 convenience pair — if Task 1's guard already owns terminal setup, use its terminal instead and drop these two calls. `select!` binding `Some(ev) = maybe` syntax: use a plain `match maybe { Some(ev) => app.update(ev), None => break 'outer }`.

- [ ] **Step 7: Build + full test run.** `cargo build --release` and `cargo test -p qoo-tui` — everything green (M1 complete: 12 tasks).

- [ ] **Step 8: Manual verification checklist (against the live daemon).** Run `mise run tui:rs` (or `cargo run -p qoo-tui --release`) with the daemon up, and verify each item:
  - [ ] tabs render; click a tab switches project; `1`–`9` and `[`/`]` also switch
  - [ ] click rows in all three list panes: focus + cursor follow the click; click the selected row again → `actions arrive in M2` status line
  - [ ] shift+click extends the selection; footer shows `N selected …`
  - [ ] wheel over an **unfocused** pane moves that pane's cursor; wheel over detail scrolls the transcript (inverted: up = older)
  - [ ] scrollbar drag on an overflowing pane tracks the pointer proportionally
  - [ ] select a running task: transcript live-tails (1s), `g`/`G` jump, sub-tab chips click, `{`/`}` cycle
  - [ ] `/` filters, title shows `QUEUE /q█`, enter applies, staged esc (range → filter → noop)
  - [ ] `?` opens help; any key or outside-click closes
  - [ ] stop the daemon → yellow `daemon unreachable — retrying…`, last snapshot stays; restart → green `●` returns
  - [ ] shrink the terminal below 60×15 → guard text; resize back → full redraw
  - [ ] quit with `q` → terminal fully restored (no mouse-reporting garbage, no alt-screen residue)

- [ ] **Step 9: Commit.**
  `git add crates/qoo-tui/src/app.rs crates/qoo-tui/src/view/detail.rs crates/qoo-tui/src/main.rs crates/qoo-tui/src/lib.rs`
  `git commit -m "feat(tui-rs): mouse click/wheel/drag routing + M1 main loop assembly"`

---
## Milestone 2 — Actions (Tasks 13–16)

> Ports the Ink TUI's mutation surface: the RPC executor plumbing + status line
> (Task 13), single-target action menus (Task 14), text-input modals (Task 15),
> and bulk selection/menus/execution (Task 16). Parity oracle: `packages/tui/src/action-menu.ts`,
> `App.tsx` (`openActionMenu`/`openBulkMenu`/`runBulk`/`runMenuAction`), and the
> TS suites `action-menu.test.ts`, `app.test.tsx`, `modal.test.tsx`.
>
> **Parity note carried through M2:** App.tsx `runBulk` for task-definitions uses
> the past-tense verb **`"started"`** (`App.tsx:698`), and `app.test.tsx:1573`
> asserts the status line `"started 1"`. The Task 16 brief says `"ran"`; the TS
> suite is the binding parity oracle (Global Constraints), so **BulkRunDefs uses
> verb `"started"`**. Every other verb matches the brief: `"reran"`, `"skipped"`,
> `"removed"`.

---

### Task 13: mutation plumbing + status line behavior

Adds the `App::dispatch_rpc` builder (per-method timeout/timeout-ok/invalidate
defaults) and the `Event::ActionResult` handler (status line + defs-cache
invalidation round-trip). The event executor that turns `Cmd::Rpc`/`Cmd::RpcSeq`
into `Event::ActionResult` already exists from Task 4; the footer already renders
`status_line` red from Task 8; the list-mode keypress that clears `status_line`
already exists from Task 11 — this task verifies it also clears
ActionResult-set lines and pins that with a test.

**Files:**
- Modify: `crates/qoo-tui/src/app.rs` (add `RpcOpts`, `App::dispatch_rpc`, `Event::ActionResult` arm in `update`)
- Test: `crates/qoo-tui/src/app.rs` (`#[cfg(test)] mod action_result_tests`)

**Interfaces:**
- Consumes (Shared Type Contract, verbatim): `Event::ActionResult { status: Option<String>, invalidate_defs_for: Option<String> }`; `Cmd::{Rpc { label, call, timeout_ms, timeout_is_ok, invalidate_defs_for }, FetchDefinitions { repo }}`; `RpcCall { method, params }`; `App.status_line: Option<String>`, `App.defs_by_project: HashMap<String, Vec<DefinitionSummary>>`; `Update { dirty, cmds }`.
- Produces (**contract addition** — flag): `pub struct RpcOpts { pub timeout_ms: Option<u64>, pub timeout_is_ok: bool, pub invalidate_defs_for: Option<String> }` with `impl Default`, and `impl App { fn dispatch_rpc(&mut self, label: impl Into<String>, method: &str, params: serde_json::Value, opts: RpcOpts) -> Cmd }` (private to the crate). No public-shape change to any contracted type.

**Steps:**

- [ ] **Step 1: Failing test — `dispatch_rpc` per-method defaults.** Append to `app.rs`:
  ```rust
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
  }
  ```
- [ ] **Step 2: Run (expect FAIL).** `cargo test -p qoo-tui action_result_tests::dispatch_rpc_applies_per_method_defaults` — fails to compile (`RpcOpts`, `dispatch_rpc` undefined).
- [ ] **Step 3: Implement `RpcOpts` + `dispatch_rpc`.** Add near the top of `app.rs`:
  ```rust
  /// Per-call overrides for `App::dispatch_rpc`. Contract addition (M2).
  #[derive(Debug, Default, Clone)]
  pub struct RpcOpts {
      pub timeout_ms: Option<u64>,
      pub timeout_is_ok: bool,
      pub invalidate_defs_for: Option<String>,
  }
  ```
  and, in `impl App`:
  ```rust
  /// Build a `Cmd::Rpc` with the same defaults the Ink `createActions` layer
  /// baked in: 5s default timeout, a 10-minute budget for `createWorktree`
  /// (post-create hooks run for minutes), and `runDefinition` treated as
  /// timeout-ok (discovery can outlive the client; the push sub re-syncs).
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
  ```
- [ ] **Step 4: Run (expect PASS).** `cargo test -p qoo-tui action_result_tests::dispatch_rpc_applies_per_method_defaults`.
- [ ] **Step 5: Failing test — `Event::ActionResult` sets/clears status + invalidation round-trip.** Add to the test module:
  ```rust
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
  ```
- [ ] **Step 6: Run (expect FAIL).** `cargo test -p qoo-tui action_result_tests` — the two new tests fail (no `ActionResult` arm).
- [ ] **Step 7: Implement the `Event::ActionResult` arm in `App::update`.** In the `match event` inside `update`:
  ```rust
  Event::ActionResult { status, invalidate_defs_for } => {
      // Success carries status = None → leave the line untouched (never clobber
      // a heal/create message with an empty). Failure carries the message.
      if status.is_some() {
          self.status_line = status;
      }
      let mut cmds = Vec::new();
      if let Some(repo) = invalidate_defs_for {
          // A run may change dedup state, so drop the cached defs and re-fetch
          // eagerly (ports App.tsx `invalidateDefs` + the lazy re-fetch effect).
          self.defs_by_project.remove(&repo);
          cmds.push(Cmd::FetchDefinitions { repo });
      }
      Update { dirty: true, cmds }
  }
  ```
  (The list-mode key handler from Task 11 already runs `self.status_line = None;` before dispatching a keymap action; the Step-5 test pins that it also clears ActionResult-set lines — no change needed there.)
- [ ] **Step 8: Run (expect PASS).** `cargo test -p qoo-tui action_result_tests`.
- [ ] **Step 9: Commit.**
  `git add crates/qoo-tui/src/app.rs`
  `git commit -m "feat(tui-rs): RPC dispatch defaults + ActionResult status/invalidation"`

---

### Task 14: single-target action menus

Ports `action-menu.ts` `buildActions` (per-pane single-target menus) into
`action_menu.rs`, wires `Mode::ActionMenu` open/navigate/execute in `app.rs`
(replacing the Task 12 click-selected-row stub), implements `Mode::ConfirmRemove`
fully, and renders the centered menu popup in `view/menu.rs` with per-row hit
targets. `RunNamedDef`/`RunDef`/`CreateWorktree`/`SquashMerge` are M3 stubs —
each sets a status line naming the exact replacing task.

**Files:**
- Modify: `crates/qoo-tui/src/action_menu.rs` (menu builders)
- Modify: `crates/qoo-tui/src/app.rs` (`open_action_menu`, `Mode::ActionMenu` + `Mode::ConfirmRemove` handling, mouse routing, execute)
- Create: `crates/qoo-tui/src/view/menu.rs` (`render_menu`)
- Modify: `crates/qoo-tui/src/view/mod.rs` (dispatch `render_menu` for `Mode::ActionMenu`)
- Test: `crates/qoo-tui/src/action_menu.rs` (`mod builder_tests`), `crates/qoo-tui/src/app.rs` (`mod menu_flow_tests`), `crates/qoo-tui/src/view/menu.rs` (`mod menu_view_tests`)
- Snapshot: `crates/qoo-tui/src/snapshots/` (insta accepts)

**Interfaces:**
- Consumes (contract): `ActionItem { label, disabled, action }`, `MenuAction::{Rerun,Skip,AssignWorktree,TaskFresh,TaskMain,RunDef,RunNamedDef,OpenTmux,RemoveWorktree,CreateWorktree,SquashMerge}`; `selectors::{QueueRow, WorktreeRow, WtState, queue_rows, worktree_rows, filter_rows, build_tabs}`; `ipc::types::{TaskInstance, TaskStatus, DefinitionSummary}`; `Mode::{ActionMenu, AddTask, WorktreeInput, ConfirmRemove, List}`; `SessionMode`; `HitTarget::{MenuItem, Modal, Row}`; `Cmd::OpenTmux`; `RpcOpts`, `App::dispatch_rpc` (Task 13).
- Produces (`action_menu.rs`): `pub fn queue_menu(row: &QueueRow, full: &TaskInstance) -> (String, Vec<ActionItem>)`; `pub fn tasks_menu(def: &DefinitionSummary) -> (String, Vec<ActionItem>)`; `pub fn worktree_menu(repo: &str, row: &WorktreeRow, inside_tmux: bool) -> (String, Vec<ActionItem>)` (**signature clarification** — the brief sketch omitted `repo`, needed for `RemoveWorktree { repo, .. }`).
- Produces (`view/menu.rs`): `pub fn render_menu(frame: &mut ratatui::Frame, hit: &mut HitMap, title: &str, items: &[ActionItem], index: usize)`.
- Produces (`app.rs`, private): `fn open_action_menu(&mut self) -> Option<Mode>`; `fn execute_menu_action(&mut self, action: MenuAction) -> Update`.

**Steps:**

- [ ] **Step 1: Failing tests — menu builders mirror `action-menu.test.ts`.** In `action_menu.rs`:
  ```rust
  #[cfg(test)]
  mod builder_tests {
      use super::*;
      use crate::ipc::types::{TaskInstance, TaskStatus};
      use crate::selectors::{QueueRow, WorktreeRow, WtState};

      fn qrow(archived: bool) -> QueueRow {
          QueueRow { task_id: "t1".into(), glyph: '?', running: false, main_session: false,
              lane: "platform:main".into(), summary: "do the thing".into(), detail: String::new(), archived }
      }
      fn task(status: TaskStatus) -> TaskInstance {
          let mut t = TaskInstance::default();
          t.id = "t1".into();
          t.status = status;
          t
      }
      fn labels(items: &[ActionItem]) -> Vec<&str> { items.iter().map(|i| i.label.as_str()).collect() }
      fn enabled(items: &[ActionItem]) -> Vec<&str> {
          items.iter().filter(|i| i.disabled.is_none()).map(|i| i.label.as_str()).collect()
      }

      #[test]
      fn queue_stable_order_and_status_gating() {
          // Stable order regardless of status.
          let (title, items) = queue_menu(&qrow(false), &task(TaskStatus::Running));
          assert_eq!(title, "do the thing");
          assert_eq!(labels(&items), ["Rerun", "Skip", "Assign worktree…"]);
          assert!(enabled(&items).is_empty()); // running: nothing enabled

          assert_eq!(enabled(&queue_menu(&qrow(false), &task(TaskStatus::Failed)).1), ["Rerun", "Skip"]);
          assert_eq!(enabled(&queue_menu(&qrow(false), &task(TaskStatus::NeedsInput)).1),
              ["Rerun", "Skip", "Assign worktree…"]);
          assert_eq!(enabled(&queue_menu(&qrow(false), &task(TaskStatus::Done)).1), ["Skip"]);
          assert!(enabled(&queue_menu(&qrow(false), &task(TaskStatus::Queued)).1).is_empty());
      }

      #[test]
      fn queue_archived_all_disabled_with_reason() {
          let (_t, items) = queue_menu(&qrow(true), &task(TaskStatus::Done));
          assert!(items.iter().all(|i| i.disabled.as_deref() == Some("archived")));
      }

      #[test]
      fn queue_disabled_reasons_name_the_status() {
          let (_t, items) = queue_menu(&qrow(false), &task(TaskStatus::Running));
          assert_eq!(items[0].disabled.as_deref(), Some("cannot rerun a running task"));
          assert_eq!(items[1].disabled.as_deref(), Some("cannot skip a running task"));
          assert_eq!(items[2].disabled.as_deref(), Some("only for needs-input tasks"));
      }

      #[test]
      fn tasks_menu_offers_run() {
          let mut d = DefinitionSummary::default();
          d.repo = "platform".into();
          d.name = "pr-ready".into();
          let (title, items) = tasks_menu(&d);
          assert_eq!(title, "pr-ready");
          assert_eq!(labels(&items), ["Run"]);
          assert!(matches!(items[0].action, MenuAction::RunNamedDef { .. }));
      }

      fn wrow(state: WtState, branch: &str, is_session: bool) -> WorktreeRow {
          WorktreeRow { name: "wt-a".into(), raw_name: "platform.wt-a".into(), path: "/wt/wt-a".into(),
              branch: branch.into(), state, has_main_session: false, queued: 0, is_session }
      }

      #[test]
      fn worktree_menu_order_and_all_enabled() {
          let (title, items) = worktree_menu("platform", &wrow(WtState::Free, "wt-a", false), true);
          assert_eq!(title, "wt-a");
          assert_eq!(labels(&items), [
              "New task (fresh session)…", "New task (main session)…", "Run task definition…",
              "Open in tmux window", "Squash merge into…", "Remove worktree…", "Create worktree…",
          ]);
          assert_eq!(enabled(&items), labels(&items));
      }

      #[test]
      fn worktree_menu_busy_disables_remove_and_squash_create_stays() {
          let (_t, items) = worktree_menu("platform", &wrow(WtState::Busy, "wt-a", false), true);
          let by = |lbl: &str| items.iter().find(|i| i.label == lbl).unwrap();
          assert_eq!(by("Remove worktree…").disabled.as_deref(), Some("a task is running here"));
          assert_eq!(by("Squash merge into…").disabled.as_deref(), Some("a task is running here"));
          assert_eq!(by("Create worktree…").disabled, None);
      }

      #[test]
      fn worktree_menu_branchless_disables_only_squash() {
          let (_t, items) = worktree_menu("platform", &wrow(WtState::Free, "", false), true);
          let by = |lbl: &str| items.iter().find(|i| i.label == lbl).unwrap();
          assert_eq!(by("Squash merge into…").disabled.as_deref(), Some("worktree has no branch"));
          assert_eq!(by("Remove worktree…").disabled, None);
      }

      #[test]
      fn worktree_menu_outside_tmux_disables_open() {
          let (_t, items) = worktree_menu("platform", &wrow(WtState::Free, "wt-a", false), false);
          let by = |lbl: &str| items.iter().find(|i| i.label == lbl).unwrap();
          assert_eq!(by("Open in tmux window").disabled.as_deref(), Some("not inside tmux"));
      }

      #[test]
      fn session_row_offers_only_tmux_open() {
          let (title, items) = worktree_menu("platform", &wrow(WtState::You, "", true), true);
          assert_eq!(title, "wt-a");
          assert_eq!(labels(&items), ["Open in tmux window"]);
          assert_eq!(enabled(&items), ["Open in tmux window"]);
      }
  }
  ```
- [ ] **Step 2: Run (expect FAIL).** `cargo test -p qoo-tui builder_tests` — builders undefined.
- [ ] **Step 3: Implement the builders in `action_menu.rs`.** Add above the test module:
  ```rust
  use crate::ipc::types::{DefinitionSummary, TaskInstance, TaskStatus};
  use crate::selectors::{QueueRow, WorktreeRow, WtState};

  fn item(label: &str, applicable: bool, reason: &str, action: MenuAction) -> ActionItem {
      ActionItem {
          label: label.to_string(),
          disabled: if applicable { None } else { Some(reason.to_string()) },
          action,
      }
  }

  fn status_kebab(s: TaskStatus) -> &'static str {
      match s {
          TaskStatus::Queued => "queued",
          TaskStatus::NeedsInput => "needs-input",
          TaskStatus::Running => "running",
          TaskStatus::Done => "done",
          TaskStatus::Failed => "failed",
          TaskStatus::Unknown => "unknown",
      }
  }

  /// Single-target queue menu. Shape is stable per status (disabled rows keep
  /// their slot); archived rows disable everything with reason "archived".
  pub fn queue_menu(row: &QueueRow, full: &TaskInstance) -> (String, Vec<ActionItem>) {
      let title = row.summary.clone();
      let id = full.id.clone();
      if row.archived {
          return (title, vec![
              item("Rerun", false, "archived", MenuAction::Rerun { id: id.clone() }),
              item("Skip", false, "archived", MenuAction::Skip { id: id.clone() }),
              item("Assign worktree…", false, "archived", MenuAction::AssignWorktree { id }),
          ]);
      }
      let s = full.status;
      let k = status_kebab(s);
      let rerun_ok = matches!(s, TaskStatus::Failed | TaskStatus::NeedsInput);
      let skip_ok = matches!(s, TaskStatus::Failed | TaskStatus::NeedsInput | TaskStatus::Done);
      let assign_ok = matches!(s, TaskStatus::NeedsInput);
      (title, vec![
          item("Rerun", rerun_ok, &format!("cannot rerun a {k} task"), MenuAction::Rerun { id: id.clone() }),
          item("Skip", skip_ok, &format!("cannot skip a {k} task"), MenuAction::Skip { id: id.clone() }),
          item("Assign worktree…", assign_ok, "only for needs-input tasks", MenuAction::AssignWorktree { id }),
      ])
  }

  /// Single-target tasks menu: one "Run" row → the named-def run (ambient args
  /// form is M3, Task 19; the action carries repo/name so that task can dispatch).
  pub fn tasks_menu(def: &DefinitionSummary) -> (String, Vec<ActionItem>) {
      (def.name.clone(), vec![ActionItem {
          label: "Run".into(),
          disabled: None,
          action: MenuAction::RunNamedDef { repo: def.repo.clone(), name: def.name.clone() },
      }])
  }

  /// Single-target worktree menu (or session menu when the row is an interactive
  /// session). `repo` is the active project — needed for `RemoveWorktree`.
  pub fn worktree_menu(repo: &str, row: &WorktreeRow, inside_tmux: bool) -> (String, Vec<ActionItem>) {
      if row.is_session {
          return (row.name.clone(), vec![
              item("Open in tmux window", inside_tmux, "not inside tmux", MenuAction::OpenTmux { path: row.path.clone() }),
          ]);
      }
      let busy = matches!(row.state, WtState::Busy);
      let has_branch = !row.branch.is_empty();
      let branch_opt = if has_branch { Some(row.branch.clone()) } else { None };
      let squash_reason = if busy { "a task is running here" } else { "worktree has no branch" };
      (row.name.clone(), vec![
          ActionItem { label: "New task (fresh session)…".into(), disabled: None,
              action: MenuAction::TaskFresh { worktree: Some(row.raw_name.clone()) } },
          ActionItem { label: "New task (main session)…".into(), disabled: None,
              action: MenuAction::TaskMain { worktree: Some(row.raw_name.clone()) } },
          ActionItem { label: "Run task definition…".into(), disabled: None,
              action: MenuAction::RunDef { worktree: Some(row.raw_name.clone()), branch: branch_opt.clone() } },
          item("Open in tmux window", inside_tmux, "not inside tmux", MenuAction::OpenTmux { path: row.path.clone() }),
          item("Squash merge into…", !busy && has_branch, squash_reason,
              MenuAction::SquashMerge { branch: row.branch.clone() }),
          item("Remove worktree…", !busy, "a task is running here",
              MenuAction::RemoveWorktree { repo: repo.to_string(), name: row.raw_name.clone(), branch: row.branch.clone() }),
          ActionItem { label: "Create worktree…".into(), disabled: None, action: MenuAction::CreateWorktree },
      ])
  }
  ```
- [ ] **Step 4: Run (expect PASS).** `cargo test -p qoo-tui builder_tests`.
- [ ] **Step 5: Commit builders.**
  `git add crates/qoo-tui/src/action_menu.rs`
  `git commit -m "feat(tui-rs): single-target action-menu builders"`

- [ ] **Step 6: Failing tests — `Mode::ActionMenu` open/navigate/execute + `ConfirmRemove`.** In `app.rs`:
  ```rust
  #[cfg(test)]
  mod menu_flow_tests {
      use super::*;
      use crate::action_menu::MenuAction;
      use crate::ipc::types::{Project, StateSnapshot, TaskInstance, TaskStatus, WorktreeInfo};
      use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
      use std::collections::HashMap;

      fn key(c: char) -> Event { Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)) }
      fn enter() -> Event { Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)) }

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
          StateSnapshot { tasks: vec![t], projects: vec![Project { name: "platform".into() }], ..Default::default() }
      }

      fn worktree_snapshot() -> StateSnapshot {
          let mut wts = HashMap::new();
          wts.insert("platform".into(), vec![WorktreeInfo { name: "platform.wt-a".into(), path: "/wt/wt-a".into(), branch: "wt-a".into() }]);
          StateSnapshot { projects: vec![Project { name: "platform".into() }], worktrees: wts, ..Default::default() }
      }

      #[test]
      fn enter_opens_queue_menu_then_navigate_and_execute_rerun() {
          let mut a = app_with(failed_task_snapshot());
          // queue is the default focus/last pane; the failed task is selected.
          a.update(enter());
          match &a.mode {
              Mode::ActionMenu { title, items, index } => {
                  assert_eq!(title, ""); // prompt "" for the default TaskInstance → summary "" (fixture)
                  assert_eq!(items.len(), 3);
                  assert_eq!(*index, 0);
              }
              other => panic!("expected ActionMenu, got {other:?}"),
          }
          // j moves the highlight; Enter on "Rerun" (index 0) dispatches retry.
          a.update(enter_on(&mut a2_noop())); // placeholder replaced below
      }
  }
  ```
  Replace the trailing placeholder line — the real assertions:
  ```rust
      #[test]
      fn queue_menu_execute_rerun_emits_retry_and_closes() {
          let mut a = app_with(failed_task_snapshot());
          a.update(enter()); // open menu, index 0 = Rerun (enabled: failed)
          let u = a.update(enter()); // execute Rerun
          assert!(matches!(a.mode, Mode::List));
          assert!(
              u.cmds.iter().any(|c| matches!(c, Cmd::Rpc { call, .. } if call.method == "retry" && call.params == serde_json::json!({ "id": "t1" }))),
              "expected retry Cmd, got {:?}", u.cmds,
          );
      }

      #[test]
      fn queue_menu_j_moves_highlight() {
          let mut a = app_with(failed_task_snapshot());
          a.update(enter());
          a.update(key('j'));
          match &a.mode { Mode::ActionMenu { index, .. } => assert_eq!(*index, 1), _ => panic!() }
          a.update(key('k'));
          match &a.mode { Mode::ActionMenu { index, .. } => assert_eq!(*index, 0), _ => panic!() }
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
          a.update(key('j')); a.update(key('j')); // -> Assign worktree…
          a.update(enter());
          match &a.mode { Mode::WorktreeInput { task_id, .. } => assert_eq!(task_id, "t1"), other => panic!("{other:?}") }
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
          for _ in 0..5 { a.update(key('j')); } // -> Remove worktree… (index 5)
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
          for _ in 0..5 { a.update(key('j')); }
          a.update(enter()); // ConfirmRemove
          let u = a.update(key('y'));
          assert!(matches!(a.mode, Mode::List));
          assert!(u.cmds.iter().any(|c| matches!(c, Cmd::Rpc { call, .. }
              if call.method == "removeWorktree" && call.params == serde_json::json!({ "repo": "platform", "name": "platform.wt-a" }))));

          // n cancels without a cmd
          a.update(enter());
          for _ in 0..5 { a.update(key('j')); }
          a.update(enter());
          let u2 = a.update(key('n'));
          assert!(matches!(a.mode, Mode::List));
          assert!(u2.cmds.is_empty());
      }

      #[test]
      fn m3_stub_actions_set_status_and_close() {
          // tasks-pane Run → RunNamedDef stub (Task 18).
          let mut snap = StateSnapshot { projects: vec![Project { name: "platform".into() }], ..Default::default() };
          snap.tasks = vec![]; // ensure definition selection path via defs cache
          let mut a = app_with(snap);
          a.defs_by_project.insert("platform".into(), vec![{
              let mut d = crate::ipc::types::DefinitionSummary::default();
              d.repo = "platform".into(); d.name = "lint".into(); d
          }]);
          focus_tasks(&mut a);
          a.update(enter()); // open tasks menu
          let u = a.update(enter()); // execute Run → M3 stub
          assert!(matches!(a.mode, Mode::List));
          assert!(u.cmds.is_empty());
          assert!(a.status_line.as_deref().unwrap_or("").contains("Task 19"));
      }

      // --- focus helpers (Tab cycles panes; ports moveFocus order queue→tasks→worktrees) ---
      fn tab(a: &mut App) { a.update(Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))); }
      fn focus_tasks(a: &mut App) { tab(a); }
      fn focus_worktrees(a: &mut App) { tab(a); tab(a); }

      // scaffolding stub removed in the real impl
      fn a2_noop() -> App { App::new("/tmp/r".into(), "/tmp/s".into()) }
      fn enter_on(_: &mut App) -> Event { Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)) }
  }
  ```
  (Delete the two scaffolding helpers `a2_noop`/`enter_on` and the first exploratory `enter_opens_queue_menu…` test body's placeholder line before running; they exist only to show the shape and are superseded by `queue_menu_execute_rerun_emits_retry_and_closes`.)
- [ ] **Step 7: Run (expect FAIL).** `cargo test -p qoo-tui menu_flow_tests` — `open_action_menu`/`ActionMenu`/`ConfirmRemove` handling missing.
- [ ] **Step 8: Implement `open_action_menu`, menu key handling, execute, and `ConfirmRemove`.** In `impl App`:
  ```rust
  fn active_repo(&self) -> Option<String> {
      let snap = self.snapshot.as_ref()?;
      let tabs = crate::selectors::build_tabs(snap);
      if tabs.is_empty() { return None; }
      let idx = self.active_tab.min(tabs.len() - 1);
      Some(tabs[idx].name.clone())
  }

  fn active_ui(&self) -> TabUiState {
      self.active_repo()
          .and_then(|r| self.ui_by_tab.get(&r).cloned())
          .unwrap_or_default()
  }

  /// The action-menu targets the last-focused list pane's selection — exactly
  /// what the detail pane shows. Bulk (range > 1) is added in Task 16 by
  /// prepending a guard here; Task 14 handles the single-target case.
  fn open_action_menu(&mut self) -> Option<Mode> {
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
              let task = snap.tasks.iter().chain(snap.archived_recent.iter()).find(|t| t.id == row.task_id)?;
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
          // --- M3 stubs (clearly marked with the replacing task) ---
          M::RunNamedDef { .. } => {
              self.status_line = Some("run task definition — arrives in M3 (Task 18)".into());
              Update { dirty: true, cmds: vec![] }
          }
          M::RunDef { .. } => {
              self.status_line = Some("run task definition — arrives in M3 (Task 18)".into());
              Update { dirty: true, cmds: vec![] }
          }
          M::CreateWorktree => {
              self.status_line = Some("create worktree — arrives in M3 (Task 21)".into());
              Update { dirty: true, cmds: vec![] }
          }
          M::SquashMerge { .. } => {
              self.status_line = Some("squash merge — arrives in M3 (Task 21)".into());
              Update { dirty: true, cmds: vec![] }
          }
          // Bulk execution is added in Task 16 (this state is never produced in Task 14).
          _ => Update { dirty: true, cmds: vec![] },
      }
  }
  ```
  Wire the dispatch of `AppAction::OpenActionMenu` (replacing any Task 12 stub) inside `update`'s list-mode action handling:
  ```rust
  AppAction::OpenActionMenu => {
      match self.open_action_menu() {
          Some(mode) => { self.mode = mode; }
          None => { self.status_line = Some("nothing selected".into()); }
      }
      Update { dirty: true, cmds: vec![] }
  }
  ```
  Add the `Mode::ActionMenu` and `Mode::ConfirmRemove` branches at the top of `update` (before list-mode key handling — these modes swallow keys):
  ```rust
  Event::Key(k) if matches!(self.mode, Mode::ActionMenu { .. }) => {
      use crossterm::event::KeyCode::*;
      let Mode::ActionMenu { items, index, .. } = &mut self.mode else { unreachable!() };
      match k.code {
          Esc | Char('q') => { self.mode = Mode::List; Update { dirty: true, cmds: vec![] } }
          Up | Char('k') => { *index = index.saturating_sub(1); Update { dirty: true, cmds: vec![] } }
          Down | Char('j') => { *index = (*index + 1).min(items.len().saturating_sub(1)); Update { dirty: true, cmds: vec![] } }
          Enter => {
              let it = items.get(*index).cloned();
              match it {
                  Some(it) if it.disabled.is_none() => self.execute_menu_action(it.action),
                  _ => Update { dirty: false, cmds: vec![] }, // disabled row is inert
              }
          }
          _ => Update { dirty: false, cmds: vec![] },
      }
  }
  Event::Key(k) if matches!(self.mode, Mode::ConfirmRemove { .. }) => {
      use crossterm::event::KeyCode::*;
      match k.code {
          Char('y') => {
              let Mode::ConfirmRemove { repo, worktree, .. } = &self.mode else { unreachable!() };
              let (repo, worktree) = (repo.clone(), worktree.clone());
              self.mode = Mode::List;
              let cmd = self.dispatch_rpc("remove worktree", "removeWorktree",
                  serde_json::json!({ "repo": repo, "name": worktree }), RpcOpts::default());
              Update { dirty: true, cmds: vec![cmd] }
          }
          Char('n') | Char('q') | Esc => { self.mode = Mode::List; Update { dirty: true, cmds: vec![] } }
          _ => Update { dirty: false, cmds: vec![] },
      }
  }
  ```
  (`ActionItem` must derive `Clone`; add it in `action_menu.rs` if absent — a private-shape addition, not a contract change.)
- [ ] **Step 9: Run (expect PASS).** `cargo test -p qoo-tui menu_flow_tests`.
- [ ] **Step 10: Commit menu logic.**
  `git add crates/qoo-tui/src/app.rs crates/qoo-tui/src/action_menu.rs`
  `git commit -m "feat(tui-rs): action-menu open/navigate/execute + confirm-remove"`

- [ ] **Step 11: Failing tests — `render_menu` popup geometry, disabled rows, hit targets + snapshot.** Create `crates/qoo-tui/src/view/menu.rs` with the test module first:
  ```rust
  #[cfg(test)]
  mod menu_view_tests {
      use super::*;
      use crate::action_menu::{ActionItem, MenuAction};
      use crate::hit::{HitMap, HitTarget};
      use ratatui::{backend::TestBackend, Terminal};

      fn items() -> Vec<ActionItem> {
          vec![
              ActionItem { label: "Rerun".into(), disabled: None, action: MenuAction::Rerun { id: "t1".into() } },
              ActionItem { label: "Skip".into(), disabled: Some("cannot skip a running task".into()), action: MenuAction::Skip { id: "t1".into() } },
          ]
      }

      fn draw(cols: u16, rows: u16, index: usize) -> (String, HitMap) {
          let mut term = Terminal::new(TestBackend::new(cols, rows)).unwrap();
          let mut hit = HitMap::default();
          term.draw(|f| render_menu(f, &mut hit, "do the thing", &items(), index)).unwrap();
          let buf = term.backend().buffer().clone();
          let mut s = String::new();
          for y in 0..rows {
              for x in 0..cols { s.push_str(buf[(x, y)].symbol()); }
              s.push('\n');
          }
          (s, hit)
      }

      #[test]
      fn disabled_row_shows_reason() {
          let (s, _hit) = draw(80, 20, 0);
          assert!(s.contains("Rerun"));
          assert!(s.contains("Skip — cannot skip a running task"));
      }

      #[test]
      fn hit_targets_cover_rows_and_modal_body() {
          let (_s, hit) = draw(80, 20, 0);
          // Somewhere inside the popup a click resolves to a MenuItem; the popup
          // body is also covered by a Modal target so clicks never leak through.
          let mut saw_item0 = false;
          let mut saw_modal = false;
          for y in 0..20 { for x in 0..80 {
              match hit.hit(x, y) {
                  Some(HitTarget::MenuItem(0)) => saw_item0 = true,
                  Some(HitTarget::Modal) => saw_modal = true,
                  _ => {}
              }
          }}
          assert!(saw_item0, "expected a MenuItem(0) hit region");
          assert!(saw_modal, "expected a Modal body region");
      }

      #[test]
      fn menu_snapshot() {
          let (s, _hit) = draw(60, 15, 0);
          insta::assert_snapshot!("action_menu_open", s);
      }
  }
  ```
- [ ] **Step 12: Run (expect FAIL).** `cargo test -p qoo-tui menu_view_tests` — `render_menu` missing.
- [ ] **Step 13: Implement `render_menu`.** Above the tests in `view/menu.rs`:
  ```rust
  use ratatui::layout::Rect;
  use ratatui::widgets::{Block, BorderType, Borders, Clear, List, ListItem};
  use ratatui::text::{Line, Span};
  use crate::action_menu::ActionItem;
  use crate::hit::{HitMap, HitTarget};
  use crate::view::theme::Palette;

  /// Centered popup: width = clamp(20, 72, cols - 8); height fits the rows +
  /// title + hint + borders. Registers a Modal target over the whole popup
  /// (clicks can't leak through) plus one MenuItem(i) target per row.
  pub fn render_menu(frame: &mut ratatui::Frame, hit: &mut HitMap, title: &str, items: &[ActionItem], index: usize) {
      let p = Palette::default();
      let area = frame.area();
      let width = (area.width.saturating_sub(8)).clamp(20, 72);
      // interior line count = title(via block) + items + hint; +2 for borders.
      let inner_h = items.len() as u16 + 1; // rows + hint line
      let height = (inner_h + 2).min(area.height);
      let x = area.x + (area.width.saturating_sub(width)) / 2;
      let y = area.y + (area.height.saturating_sub(height)) / 2;
      let rect = Rect { x, y, width, height };

      frame.render_widget(Clear, rect);
      hit.push(rect, HitTarget::Modal); // popup body: opaque to clicks

      let block = Block::default()
          .title(format!(" {title} "))
          .borders(Borders::ALL)
          .border_type(BorderType::Rounded)
          .border_style(p.accent);
      let inner = block.inner(rect);
      frame.render_widget(block, rect);

      // Each row gets a MenuItem hit rect; disabled rows dim + "— reason".
      let mut rows: Vec<ListItem> = Vec::with_capacity(items.len());
      for (i, it) in items.iter().enumerate() {
          let row_rect = Rect { x: inner.x, y: inner.y + i as u16, width: inner.width, height: 1 };
          if row_rect.y < inner.y + inner.height {
              hit.push(row_rect, HitTarget::MenuItem(i));
          }
          let text = match &it.disabled {
              Some(reason) => format!(" {} — {reason}", it.label),
              None => format!(" {}", it.label),
          };
          let style = if it.disabled.is_some() { p.dim } else if i == index { p.selected } else { p.normal };
          rows.push(ListItem::new(Line::from(Span::styled(text, style))));
      }
      let list = List::new(rows);
      let list_area = Rect { x: inner.x, y: inner.y, width: inner.width, height: (items.len() as u16).min(inner.height) };
      frame.render_widget(list, list_area);

      // Hint on the last interior line.
      let hint_y = inner.y + inner.height.saturating_sub(1);
      let hint = Line::from(Span::styled(" ↑/↓ move · enter run · esc close", p.dim));
      frame.render_widget(ratatui::widgets::Paragraph::new(hint),
          Rect { x: inner.x, y: hint_y, width: inner.width, height: 1 });
  }
  ```
  (Uses `Palette` fields `accent`/`dim`/`selected`/`normal` from `view/theme.rs`; if a field name differs in the resolved theme, adapt mechanically.)
- [ ] **Step 14: Wire `render_menu` in `view/mod.rs`.** In `render`, after the panes/detail/footer, before returning `hit`, add:
  ```rust
  if let Mode::ActionMenu { title, items, index } = &app.mode {
      crate::view::menu::render_menu(frame, &mut hit, title, items, *index);
  }
  ```
  and add `pub mod menu;` to `view/mod.rs`.
- [ ] **Step 15: Run (expect FAIL→accept snapshot→PASS).** `cargo test -p qoo-tui menu_view_tests`; on first run review the insta snapshot (`cargo insta accept` if faithful), then re-run to green.
- [ ] **Step 16: Mouse routing — click a menu item / outside.** In `update`'s `Event::Mouse` handling, when `Mode::ActionMenu`, route the click via the `HitMap` produced last frame (Task 12 already stores `self.hit`):
  ```rust
  // inside the mouse-down branch, checked before the list-pane routing:
  if matches!(self.mode, Mode::ActionMenu { .. }) {
      match self.hit.hit(col, row) {
          Some(HitTarget::MenuItem(i)) => {
              let i = *i;
              if let Mode::ActionMenu { items, index, .. } = &mut self.mode {
                  *index = i;
                  let it = items.get(i).cloned();
                  if let Some(it) = it { if it.disabled.is_none() { return self.execute_menu_action(it.action); } }
              }
              return Update { dirty: true, cmds: vec![] };
          }
          Some(HitTarget::Modal) => return Update { dirty: false, cmds: vec![] }, // body click: inert
          _ => { self.mode = Mode::List; return Update { dirty: true, cmds: vec![] }; } // outside: close
      }
  }
  ```
  Replace the Task 12 click-on-already-selected-row stub in the `Row(pane, i)` arm so a click on the row that is already the cursor opens the menu:
  ```rust
  Some(HitTarget::Row(pane, i)) => {
      let already = self.active_ui().selections[pane.idx()].cursor == *i
          && self.active_ui().last_list_pane == *pane;
      // focus + move cursor (existing Task 12 behavior)
      self.focus_and_select(*pane, *i);
      if already {
          match self.open_action_menu() {
              Some(mode) => self.mode = mode,
              None => self.status_line = Some("nothing selected".into()),
          }
      }
      Update { dirty: true, cmds: vec![] }
  }
  ```
  Add a hit-routing test:
  ```rust
  #[test]
  fn click_menu_item_executes() {
      let mut a = app_with(failed_task_snapshot());
      a.size = (120, 40);
      // render once to populate the hit map, then open the menu and render again.
      // (helper mirrors the M1 test harness: render_to_hitmap(&a) fills a.hit)
      a.update(super::menu_flow_tests_enter()); // placeholder: use Event::Key Enter
      // resolve the MenuItem(0) region and synthesize a click there — see harness.
  }
  ```
  (If the M1 mouse test harness already exposes a `render_and_hit(&mut App)` helper, reuse it; otherwise fold the `Terminal::draw(view::render)` + `a.hit = hit` step into a test helper. The behavioral assertion mirrors `queue_menu_execute_rerun_emits_retry_and_closes` but triggered via a synthesized `Event::Mouse` at the MenuItem(0) coordinates.)
- [ ] **Step 17: Run (expect PASS).** `cargo test -p qoo-tui menu`.
- [ ] **Step 18: Commit view + mouse.**
  `git add crates/qoo-tui/src/view/menu.rs crates/qoo-tui/src/view/mod.rs crates/qoo-tui/src/app.rs crates/qoo-tui/src/snapshots`
  `git commit -m "feat(tui-rs): action-menu popup render + mouse routing"`

---

### Task 15: text-input modals (add-task, assign-worktree)

Renders the shared modal frame + input modal (clickable `[ OK ] [ Cancel ]`),
wires `Mode::AddTask` (queue-pane `c` adhoc + worktree-menu TaskFresh/TaskMain)
and `Mode::WorktreeInput` (from the queue menu's Assign worktree…), and forwards
typing to `tui_input::Input`. Mouse events are intercepted globally in `update`
before typing so a stray click never lands in the field.

**Files:**
- Create: `crates/qoo-tui/src/view/modal.rs` (`modal_frame`, `render_input_modal`)
- Modify: `crates/qoo-tui/src/view/mod.rs` (dispatch input modals for `AddTask`/`WorktreeInput`)
- Modify: `crates/qoo-tui/src/app.rs` (`AddTask`/`WorktreeInput` key + mouse handling, `Create` → adhoc AddTask)
- Test: `crates/qoo-tui/src/app.rs` (`mod input_modal_tests`), `crates/qoo-tui/src/view/modal.rs` (`mod modal_view_tests`)

**Interfaces:**
- Consumes (contract): `Mode::{AddTask { worktree, session, input }, WorktreeInput { task_id, input }, List}`; `SessionMode::{Fresh, Main}`; `tui_input::Input`; `HitTarget::{Button(ButtonKind), Modal}`; `ButtonKind::{Confirm, Cancel}`; `selectors::strip_repo_prefix`; `RpcOpts`, `App::dispatch_rpc`.
- Produces (`view/modal.rs`): `pub fn modal_frame(frame: &mut ratatui::Frame, area: Rect, title: &str, height: u16) -> Rect` (Clear + rounded Block + centered geometry, width = clamp(20, 72, cols − 8); returns the interior Rect); `pub fn render_input_modal(frame: &mut ratatui::Frame, hit: &mut HitMap, title: &str, label: &str, input: &tui_input::Input)`.
- Produces (`app.rs`, private): `AddTask`/`WorktreeInput` handlers folded into `update`.

**Steps:**

- [ ] **Step 1: Failing tests — modal geometry + input modal buttons/hit targets + snapshot.** Create `view/modal.rs` with tests first:
  ```rust
  #[cfg(test)]
  mod modal_view_tests {
      use super::*;
      use crate::hit::{HitMap, HitTarget, ButtonKind};
      use ratatui::{backend::TestBackend, layout::Rect, Terminal};
      use tui_input::Input;

      fn render_input(cols: u16, rows: u16, value: &str) -> (String, HitMap) {
          let mut term = Terminal::new(TestBackend::new(cols, rows)).unwrap();
          let mut hit = HitMap::default();
          let input = Input::new(value.to_string());
          term.draw(|f| render_input_modal(f, &mut hit, "New task — fresh session — platform (adhoc)", "prompt", &input)).unwrap();
          let buf = term.backend().buffer().clone();
          let mut s = String::new();
          for y in 0..rows { for x in 0..cols { s.push_str(buf[(x, y)].symbol()); } s.push('\n'); }
          (s, hit)
      }

      #[test]
      fn width_clamps_to_72_and_centers() {
          let mut term = Terminal::new(TestBackend::new(200, 40)).unwrap();
          let mut r = Rect::default();
          term.draw(|f| { r = modal_frame(f, f.area(), "t", 3); }).unwrap();
          // interior width = 72 - 2 border cols = 70; left border at (200-72)/2 = 64.
          assert_eq!(r.width, 70);
          assert_eq!(r.x, 65); // 64 border + 1
      }

      #[test]
      fn shows_label_value_and_buttons() {
          let (s, _hit) = render_input(80, 15, "hello");
          assert!(s.contains("prompt"));
          assert!(s.contains("hello"));
          assert!(s.contains("[ OK ]"));
          assert!(s.contains("[ Cancel ]"));
      }

      #[test]
      fn buttons_register_hit_targets() {
          let (_s, hit) = render_input(80, 15, "");
          let mut ok = false; let mut cancel = false; let mut modal = false;
          for y in 0..15 { for x in 0..80 {
              match hit.hit(x, y) {
                  Some(HitTarget::Button(ButtonKind::Confirm)) => ok = true,
                  Some(HitTarget::Button(ButtonKind::Cancel)) => cancel = true,
                  Some(HitTarget::Modal) => modal = true,
                  _ => {}
              }
          }}
          assert!(ok && cancel && modal);
      }

      #[test]
      fn add_task_modal_snapshot() {
          let (s, _hit) = render_input(60, 15, "run this now");
          insta::assert_snapshot!("add_task_modal", s);
      }
  }
  ```
- [ ] **Step 2: Run (expect FAIL).** `cargo test -p qoo-tui modal_view_tests`.
- [ ] **Step 3: Implement `modal_frame` + `render_input_modal`.** Above the tests:
  ```rust
  use ratatui::layout::Rect;
  use ratatui::text::{Line, Span};
  use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
  use crate::hit::{ButtonKind, HitMap, HitTarget};
  use crate::view::theme::Palette;

  /// Clear + rounded, accent-bordered, centered popup. `height` is the interior
  /// line count (borders added here). Returns the interior Rect for content.
  pub fn modal_frame(frame: &mut ratatui::Frame, _area: Rect, title: &str, height: u16) -> Rect {
      let p = Palette::default();
      let area = frame.area();
      let width = (area.width.saturating_sub(8)).clamp(20, 72);
      let outer_h = (height + 2).min(area.height);
      let x = area.x + (area.width.saturating_sub(width)) / 2;
      let y = area.y + (area.height.saturating_sub(outer_h)) / 2;
      let rect = Rect { x, y, width, height: outer_h };
      frame.render_widget(Clear, rect);
      let block = Block::default()
          .title(format!(" {title} "))
          .borders(Borders::ALL)
          .border_type(BorderType::Rounded)
          .border_style(p.accent);
      let inner = block.inner(rect);
      frame.render_widget(block, rect);
      inner
  }

  /// Single-field input modal with clickable [ OK ] / [ Cancel ] buttons.
  /// Layout: label+value line, blank, buttons line. The whole popup registers a
  /// Modal target; the two buttons register Button targets on top.
  pub fn render_input_modal(frame: &mut ratatui::Frame, hit: &mut HitMap, title: &str, label: &str, input: &tui_input::Input) {
      let p = Palette::default();
      let inner = modal_frame(frame, frame.area(), title, 3);
      // Register the popup body (outer rect = inner grown by the border ring).
      let body = Rect {
          x: inner.x.saturating_sub(1), y: inner.y.saturating_sub(1),
          width: inner.width + 2, height: inner.height + 2,
      };
      hit.push(body, HitTarget::Modal);

      // Field line: "label> value█"
      let field = Line::from(vec![
          Span::styled(format!(" {label}> "), p.dim),
          Span::styled(input.value().to_string(), p.normal),
          Span::styled("█", p.accent),
      ]);
      frame.render_widget(Paragraph::new(field), Rect { x: inner.x, y: inner.y, width: inner.width, height: 1 });

      // Buttons line (bottom interior row).
      let btn_y = inner.y + inner.height.saturating_sub(1);
      let ok = " [ OK ] ";
      let cancel = "[ Cancel ] ";
      let ok_rect = Rect { x: inner.x, y: btn_y, width: ok.len() as u16, height: 1 };
      let cancel_rect = Rect { x: inner.x + ok.len() as u16, y: btn_y, width: cancel.len() as u16, height: 1 };
      frame.render_widget(Paragraph::new(Line::from(Span::styled(ok, p.selected))), ok_rect);
      frame.render_widget(Paragraph::new(Line::from(Span::styled(cancel, p.normal))), cancel_rect);
      hit.push(ok_rect, HitTarget::Button(ButtonKind::Confirm));
      hit.push(cancel_rect, HitTarget::Button(ButtonKind::Cancel));

      // Hint line just under the field (uses the middle interior row).
      if inner.height >= 3 {
          let hint = Line::from(Span::styled(" enter submit · esc cancel", p.dim));
          frame.render_widget(Paragraph::new(hint), Rect { x: inner.x, y: inner.y + 1, width: inner.width, height: 1 });
      }
  }
  ```
- [ ] **Step 4: Run (accept snapshot, expect PASS).** `cargo test -p qoo-tui modal_view_tests`; review + `cargo insta accept`.
- [ ] **Step 5: Wire input modals in `view/mod.rs`.** Add `pub mod modal;` and, in `render`, after the action menu dispatch:
  ```rust
  match &app.mode {
      Mode::AddTask { worktree, session, input } => {
          let repo = app.active_repo().unwrap_or_default();
          let target = match worktree {
              Some(w) => format!("{repo}:{}", crate::selectors::strip_repo_prefix(w, &repo)),
              None => format!("{repo} (adhoc)"),
          };
          let sess = match session { SessionMode::Fresh => "fresh", SessionMode::Main => "main" };
          crate::view::modal::render_input_modal(frame, &mut hit,
              &format!("New task — {sess} session — {target}"), "prompt", input);
      }
      Mode::WorktreeInput { task_id, input } => {
          let last6: String = task_id.chars().rev().take(6).collect::<Vec<_>>().into_iter().rev().collect();
          crate::view::modal::render_input_modal(frame, &mut hit,
              &format!("Assign worktree — task {last6}"), "worktree", input);
      }
      _ => {}
  }
  ```
  (`App::active_repo` from Task 14 must be `pub(crate)` for the view; widen its visibility.)
- [ ] **Step 6: Failing tests — enqueue/setWorktree param shapes, esc, button clicks, typing, mouse-not-in-input.** In `app.rs`:
  ```rust
  #[cfg(test)]
  mod input_modal_tests {
      use super::*;
      use crate::action_menu::MenuAction;
      use crate::hit::{ButtonKind, HitTarget};
      use crate::ipc::types::{Project, StateSnapshot, WorktreeInfo};
      use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
      use std::collections::HashMap;

      fn key(c: char) -> Event { Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)) }
      fn enter() -> Event { Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)) }
      fn esc() -> Event { Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)) }

      fn app() -> App {
          let mut a = App::new("/tmp/runs".into(), "/tmp/s.sock".into());
          a.size = (120, 40);
          let mut wts = HashMap::new();
          wts.insert("platform".into(), vec![WorktreeInfo { name: "platform.wt-a".into(), path: "/wt/wt-a".into(), branch: "jus-42".into() }]);
          a.update(Event::Snapshot(StateSnapshot { projects: vec![Project { name: "platform".into() }], worktrees: wts, ..Default::default() }));
          a
      }

      fn type_str(a: &mut App, s: &str) { for c in s.chars() { a.update(key(c)); } }

      #[test]
      fn add_task_worktree_targeted_enqueue_carries_worktree() {
          let mut a = app();
          a.mode = Mode::AddTask { worktree: Some("platform.wt-a".into()), session: SessionMode::Fresh, input: tui_input::Input::default() };
          type_str(&mut a, "do a thing");
          let u = a.update(enter());
          assert!(matches!(a.mode, Mode::List));
          let call = u.cmds.iter().find_map(|c| if let Cmd::Rpc { call, .. } = c { Some(call) } else { None }).unwrap();
          assert_eq!(call.method, "enqueue");
          assert_eq!(call.params, serde_json::json!({
              "prompt": "do a thing", "repo": "platform", "worktree": "platform.wt-a", "session": "fresh"
          }));
      }

      #[test]
      fn add_task_adhoc_omits_worktree() {
          let mut a = app();
          a.mode = Mode::AddTask { worktree: None, session: SessionMode::Fresh, input: tui_input::Input::default() };
          type_str(&mut a, "run this now");
          let u = a.update(enter());
          let call = u.cmds.iter().find_map(|c| if let Cmd::Rpc { call, .. } = c { Some(call) } else { None }).unwrap();
          assert_eq!(call.params, serde_json::json!({
              "prompt": "run this now", "repo": "platform", "session": "fresh"
          }));
      }

      #[test]
      fn queue_c_opens_adhoc_add_task() {
          let mut a = app(); // queue focused by default
          a.update(key('c'));
          match &a.mode {
              Mode::AddTask { worktree, session, .. } => { assert!(worktree.is_none()); assert!(matches!(session, SessionMode::Fresh)); }
              other => panic!("{other:?}"),
          }
      }

      #[test]
      fn add_task_esc_cancels_without_cmd() {
          let mut a = app();
          a.mode = Mode::AddTask { worktree: None, session: SessionMode::Fresh, input: tui_input::Input::default() };
          let u = a.update(esc());
          assert!(matches!(a.mode, Mode::List));
          assert!(u.cmds.is_empty());
      }

      #[test]
      fn typing_q_inserts_literal_and_backspace_edits() {
          let mut a = app();
          a.mode = Mode::AddTask { worktree: None, session: SessionMode::Fresh, input: tui_input::Input::default() };
          a.update(key('q'));
          match &a.mode { Mode::AddTask { input, .. } => assert_eq!(input.value(), "q"), _ => panic!() }
          a.update(Event::Key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)));
          match &a.mode { Mode::AddTask { input, .. } => assert_eq!(input.value(), ""), _ => panic!() }
      }

      #[test]
      fn worktree_input_enter_dispatches_set_worktree() {
          let mut a = app();
          a.mode = Mode::WorktreeInput { task_id: "abc123".into(), input: tui_input::Input::default() };
          type_str(&mut a, "wt-x");
          let u = a.update(enter());
          let call = u.cmds.iter().find_map(|c| if let Cmd::Rpc { call, .. } = c { Some(call) } else { None }).unwrap();
          assert_eq!(call.method, "setWorktree");
          assert_eq!(call.params, serde_json::json!({ "id": "abc123", "worktree": "wt-x" }));
      }

      #[test]
      fn mouse_event_never_reaches_the_input() {
          let mut a = app();
          a.mode = Mode::AddTask { worktree: None, session: SessionMode::Fresh, input: tui_input::Input::default() };
          type_str(&mut a, "hi");
          // A drag/motion mouse event over the field must not append glyphs.
          a.update(Event::Mouse(MouseEvent { kind: MouseEventKind::Moved, column: 10, row: 5, modifiers: KeyModifiers::NONE }));
          match &a.mode { Mode::AddTask { input, .. } => assert_eq!(input.value(), "hi"), _ => panic!() }
      }
  }
  ```
- [ ] **Step 7: Run (expect FAIL).** `cargo test -p qoo-tui input_modal_tests`.
- [ ] **Step 8: Implement `AddTask`/`WorktreeInput` handling + adhoc `Create`.** Add mode branches at the top of `update` (before list handling), mirroring the ActionMenu branches. Mouse is handled globally first (the existing `Event::Mouse` arm runs before these `Event::Key` arms), so a mouse event can never fall into the typing path:
  ```rust
  Event::Key(k) if matches!(self.mode, Mode::AddTask { .. }) => {
      use crossterm::event::KeyCode::*;
      match k.code {
          Enter => {
              let Mode::AddTask { worktree, session, input } = &self.mode else { unreachable!() };
              let repo = match self.active_repo() { Some(r) => r, None => { self.mode = Mode::List; return Update { dirty: true, cmds: vec![] }; } };
              let sess = match session { SessionMode::Fresh => "fresh", SessionMode::Main => "main" };
              let mut params = serde_json::json!({ "prompt": input.value(), "repo": repo, "session": sess });
              if let Some(w) = worktree { params["worktree"] = serde_json::Value::String(w.clone()); }
              self.mode = Mode::List;
              let cmd = self.dispatch_rpc("enqueue task", "enqueue", params, RpcOpts::default());
              Update { dirty: true, cmds: vec![cmd] }
          }
          Esc => { self.mode = Mode::List; Update { dirty: true, cmds: vec![] } }
          _ => {
              if let Mode::AddTask { input, .. } = &mut self.mode {
                  input.handle_event(&crossterm::event::Event::Key(k));
              }
              Update { dirty: true, cmds: vec![] }
          }
      }
  }
  Event::Key(k) if matches!(self.mode, Mode::WorktreeInput { .. }) => {
      use crossterm::event::KeyCode::*;
      match k.code {
          Enter => {
              let Mode::WorktreeInput { task_id, input } = &self.mode else { unreachable!() };
              let (id, wt) = (task_id.clone(), input.value().to_string());
              self.mode = Mode::List;
              let cmd = self.dispatch_rpc("assign worktree", "setWorktree",
                  serde_json::json!({ "id": id, "worktree": wt }), RpcOpts::default());
              Update { dirty: true, cmds: vec![cmd] }
          }
          Esc => { self.mode = Mode::List; Update { dirty: true, cmds: vec![] } }
          _ => {
              if let Mode::WorktreeInput { input, .. } = &mut self.mode {
                  input.handle_event(&crossterm::event::Event::Key(k));
              }
              Update { dirty: true, cmds: vec![] }
          }
      }
  }
  ```
  In the list-mode `AppAction::Create` dispatch (queue → adhoc AddTask; worktrees → M3 stub for Task 21):
  ```rust
  AppAction::Create => {
      match self.active_ui().last_list_pane {
          ListPane::Queue => {
              self.mode = Mode::AddTask { worktree: None, session: SessionMode::Fresh, input: tui_input::Input::default() };
          }
          ListPane::Worktrees => {
              self.status_line = Some("create worktree — arrives in M3 (Task 21)".into());
          }
          ListPane::Tasks => { /* no create on tasks pane */ }
      }
      Update { dirty: true, cmds: vec![] }
  }
  ```
- [ ] **Step 9: Add mouse Confirm/Cancel routing for input modals.** In the `Event::Mouse` arm, before other routing, when in an input mode:
  ```rust
  if matches!(self.mode, Mode::AddTask { .. } | Mode::WorktreeInput { .. }) {
      match self.hit.hit(col, row) {
          Some(HitTarget::Button(ButtonKind::Confirm)) =>
              return self.update(Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))),
          Some(HitTarget::Button(ButtonKind::Cancel)) =>
              return self.update(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))),
          Some(HitTarget::Modal) => return Update { dirty: false, cmds: vec![] },
          _ => { self.mode = Mode::List; return Update { dirty: true, cmds: vec![] }; } // outside → cancel
      }
  }
  ```
  Add a test asserting click-Confirm ≡ Enter and click-Cancel ≡ Esc (synthesize a `MouseEventKind::Down(MouseButton::Left)` at the button coords resolved from a prior `render`+`self.hit` fill, mirroring the Task 14 hit harness):
  ```rust
  #[test]
  fn click_confirm_equals_enter_and_cancel_equals_esc() {
      // Render to fill a.hit, click Confirm → enqueue Cmd emitted (same as Enter);
      // re-open, click Cancel → Mode::List, no Cmd. Uses the shared render_and_hit
      // helper (Task 14). Coordinates come from scanning a.hit for the Button rects.
  }
  ```
  (Flesh out with the shared harness; the assertions equal the `add_task_adhoc_omits_worktree` / `add_task_esc_cancels_without_cmd` outcomes.)
- [ ] **Step 10: Run (expect PASS).** `cargo test -p qoo-tui input_modal_tests`.
- [ ] **Step 11: Commit.**
  `git add crates/qoo-tui/src/view/modal.rs crates/qoo-tui/src/view/mod.rs crates/qoo-tui/src/app.rs crates/qoo-tui/src/snapshots`
  `git commit -m "feat(tui-rs): add-task + assign-worktree input modals"`

---

### Task 16: bulk selection execution

Adds the bulk menu (`bulk_menu`) and the range→bulk branch in `open_action_menu`,
`Cmd::RpcSeq` execution for rerun/skip/run-defs, and `Mode::ConfirmBulkRemove`.
Targets are frozen at menu-open time (ids captured inside the `MenuAction`), and
the range is cleared (anchor → None) before dispatch, matching `App.tsx` `runBulk`.
Extend-selection (anchor tracking) already exists from Task 11; the footer bulk
hint already exists from Task 8.

**Files:**
- Modify: `crates/qoo-tui/src/action_menu.rs` (`bulk_menu` + `BulkSelection`)
- Modify: `crates/qoo-tui/src/app.rs` (`open_bulk_menu`, range guard in `open_action_menu`, bulk arms in `execute_menu_action`, `ConfirmBulkRemove`)
- Modify: `crates/qoo-tui/src/view/mod.rs` (render `ConfirmBulkRemove`)
- Modify: `crates/qoo-tui/src/view/modal.rs` (`render_confirm_bulk_remove`)
- Test: `crates/qoo-tui/src/action_menu.rs` (`mod bulk_builder_tests`), `crates/qoo-tui/src/app.rs` (`mod bulk_flow_tests`)

**Interfaces:**
- Consumes (contract): `MenuAction::{BulkRerun { ids }, BulkSkip { ids }, BulkRunDefs { repo, names }, BulkRemove { repo, names }}`; `Cmd::RpcSeq { verb, calls, invalidate_defs_for }`; `RpcCall`; `Selection { cursor, anchor }`; `Mode::{ActionMenu, ConfirmBulkRemove { repo, names }, List}`; `ListPane`; `selectors::{QueueRow, WtState, WorktreeRow, DefinitionSummary}`.
- Produces (`action_menu.rs`, **contract addition** — flag): `pub enum BulkSelection { Queue { rerun_ids: Vec<String>, skip_ids: Vec<String>, total: usize }, Tasks { repo: String, run_names: Vec<String>, total: usize }, Worktrees { repo: String, remove_names: Vec<String>, total: usize } }` and `pub fn bulk_menu(sel: BulkSelection) -> (String, Vec<ActionItem>)` (the brief sketch `bulk_menu(pane, selected rows)` is realized as this pre-resolved enum so eligibility is computed once at open time and the ids are frozen into the returned `MenuAction`s).
- Produces (`app.rs`, private): `fn open_bulk_menu(&self, pane: ListPane, start: usize, end: usize) -> Option<Mode>`; bulk arms of `execute_menu_action`.
- Produces (`view/modal.rs`): `pub fn render_confirm_bulk_remove(frame, hit, names: &[String])`.

**Steps:**

- [ ] **Step 1: Failing tests — `bulk_menu` builder counts + disabled + frozen ids.** In `action_menu.rs`:
  ```rust
  #[cfg(test)]
  mod bulk_builder_tests {
      use super::*;

      fn labels(items: &[ActionItem]) -> Vec<String> { items.iter().map(|i| i.label.clone()).collect() }

      #[test]
      fn bulk_queue_rerun_and_skip_with_counts() {
          let (title, items) = bulk_menu(BulkSelection::Queue {
              rerun_ids: vec!["a".into(), "b".into()],
              skip_ids: vec!["a".into(), "b".into(), "c".into()],
              total: 5,
          });
          assert_eq!(title, "5 selected");
          assert_eq!(labels(&items), ["Rerun (2 of 5)", "Skip (3 of 5)"]);
          assert!(items.iter().all(|i| i.disabled.is_none()));
          // Frozen ids live inside the action.
          assert!(matches!(&items[0].action, MenuAction::BulkRerun { ids } if ids == &["a".to_string(), "b".to_string()]));
          assert!(matches!(&items[1].action, MenuAction::BulkSkip { ids } if ids.len() == 3));
      }

      #[test]
      fn bulk_queue_zero_eligible_disables() {
          let (_t, items) = bulk_menu(BulkSelection::Queue { rerun_ids: vec![], skip_ids: vec!["a".into()], total: 4 });
          assert_eq!(items[0].label, "Rerun (0 of 4)");
          assert_eq!(items[0].disabled.as_deref(), Some("no eligible rows"));
          assert_eq!(items[1].disabled, None);
      }

      #[test]
      fn bulk_tasks_run_only() {
          let (title, items) = bulk_menu(BulkSelection::Tasks { repo: "platform".into(), run_names: vec!["lint".into()], total: 3 });
          assert_eq!(title, "3 selected");
          assert_eq!(labels(&items), ["Run (1 of 3)"]);
          assert!(matches!(&items[0].action, MenuAction::BulkRunDefs { repo, names } if repo == "platform" && names == &["lint".to_string()]));
      }

      #[test]
      fn bulk_worktrees_remove_only() {
          let (_t, items) = bulk_menu(BulkSelection::Worktrees { repo: "platform".into(), remove_names: vec!["wt-a".into(), "wt-b".into()], total: 4 });
          assert_eq!(labels(&items), ["Remove worktrees… (2 of 4)"]);
          assert!(matches!(&items[0].action, MenuAction::BulkRemove { repo, names } if repo == "platform" && names.len() == 2));
      }
  }
  ```
- [ ] **Step 2: Run (expect FAIL).** `cargo test -p qoo-tui bulk_builder_tests`.
- [ ] **Step 3: Implement `BulkSelection` + `bulk_menu`.** In `action_menu.rs`:
  ```rust
  /// Pre-resolved bulk selection: eligibility is computed once by the caller at
  /// menu-open time so the ids/names are frozen into the returned actions.
  pub enum BulkSelection {
      Queue { rerun_ids: Vec<String>, skip_ids: Vec<String>, total: usize },
      Tasks { repo: String, run_names: Vec<String>, total: usize },
      Worktrees { repo: String, remove_names: Vec<String>, total: usize },
  }

  fn bulk_item(verb: &str, eligible: usize, total: usize, action: MenuAction) -> ActionItem {
      ActionItem {
          label: format!("{verb} ({eligible} of {total})"),
          disabled: if eligible > 0 { None } else { Some("no eligible rows".into()) },
          action,
      }
  }

  pub fn bulk_menu(sel: BulkSelection) -> (String, Vec<ActionItem>) {
      match sel {
          BulkSelection::Queue { rerun_ids, skip_ids, total } => (
              format!("{total} selected"),
              vec![
                  bulk_item("Rerun", rerun_ids.len(), total, MenuAction::BulkRerun { ids: rerun_ids }),
                  bulk_item("Skip", skip_ids.len(), total, MenuAction::BulkSkip { ids: skip_ids }),
              ],
          ),
          BulkSelection::Tasks { repo, run_names, total } => (
              format!("{total} selected"),
              vec![bulk_item("Run", run_names.len(), total, MenuAction::BulkRunDefs { repo, names: run_names })],
          ),
          BulkSelection::Worktrees { repo, remove_names, total } => (
              format!("{total} selected"),
              vec![bulk_item("Remove worktrees…", remove_names.len(), total, MenuAction::BulkRemove { repo, names: remove_names })],
          ),
      }
  }
  ```
- [ ] **Step 4: Run (expect PASS).** `cargo test -p qoo-tui bulk_builder_tests`.
- [ ] **Step 5: Commit builder.**
  `git add crates/qoo-tui/src/action_menu.rs`
  `git commit -m "feat(tui-rs): bulk action-menu builder"`

- [ ] **Step 6: Failing tests — open bulk, frozen targets, RpcSeq composition, confirm-bulk-remove.** In `app.rs`:
  ```rust
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
              WorktreeInfo { name: "wt-a".into(), path: "/wt/a".into(), branch: "wt-a".into() },
              WorktreeInfo { name: "wt-b".into(), path: "/wt/b".into(), branch: "wt-b".into() },
              WorktreeInfo { name: "wt-c".into(), path: "/wt/c".into(), branch: "wt-c".into() },
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
  }
  ```
- [ ] **Step 7: Run (expect FAIL).** `cargo test -p qoo-tui bulk_flow_tests`.
- [ ] **Step 8: Implement `open_bulk_menu`, the range guard, bulk execution, and `ConfirmBulkRemove`.** Add to `impl App`:
  ```rust
  fn selection_range(sel: &Selection) -> (usize, usize) {
      match sel.anchor {
          Some(a) => (a.min(sel.cursor), a.max(sel.cursor)),
          None => (sel.cursor, sel.cursor),
      }
  }

  fn clear_range(&mut self, pane: ListPane) {
      if let Some(repo) = self.active_repo() {
          if let Some(ui) = self.ui_by_tab.get_mut(&repo) {
              ui.selections[pane.idx()].anchor = None;
          }
      }
  }

  /// Freeze eligibility at open time (ids captured into the MenuAction), mirroring
  /// App.tsx openBulkMenu — a daemon push reshuffling rows mid-menu can't retarget.
  fn open_bulk_menu(&self, pane: ListPane, start: usize, end: usize) -> Option<Mode> {
      use crate::action_menu::{bulk_menu, BulkSelection};
      let snap = self.snapshot.as_ref()?;
      let repo = self.active_repo()?;
      let ui = self.active_ui();
      let total = end - start + 1;
      let (title, items) = match pane {
          ListPane::Queue => {
              let rows = crate::selectors::queue_rows(snap, &repo, self.now_epoch_s);
              let vis = crate::selectors::filter_rows(&rows, &ui.search[0], |r| r.summary.clone());
              let slice: Vec<&crate::selectors::QueueRow> = vis[start..=end.min(vis.len().saturating_sub(1))]
                  .iter().filter_map(|&i| rows.get(i)).collect();
              let status_of = |id: &str| snap.tasks.iter().find(|t| t.id == id).map(|t| t.status);
              let live = || slice.iter().filter(|r| !r.archived);
              let rerun_ids: Vec<String> = live().filter(|r| matches!(status_of(&r.task_id), Some(crate::ipc::types::TaskStatus::Failed) | Some(crate::ipc::types::TaskStatus::NeedsInput))).map(|r| r.task_id.clone()).collect();
              let skip_ids: Vec<String> = live().filter(|r| matches!(status_of(&r.task_id), Some(crate::ipc::types::TaskStatus::Failed) | Some(crate::ipc::types::TaskStatus::NeedsInput) | Some(crate::ipc::types::TaskStatus::Done))).map(|r| r.task_id.clone()).collect();
              bulk_menu(BulkSelection::Queue { rerun_ids, skip_ids, total })
          }
          ListPane::Tasks => {
              let defs = self.defs_by_project.get(&repo).cloned().unwrap_or_default();
              let vis = crate::selectors::filter_rows(&defs, &ui.search[1], |d| d.name.clone());
              let run_names: Vec<String> = vis[start..=end.min(vis.len().saturating_sub(1))]
                  .iter().filter_map(|&i| defs.get(i)).filter(|d| d.args.is_empty()).map(|d| d.name.clone()).collect();
              bulk_menu(BulkSelection::Tasks { repo: repo.clone(), run_names, total })
          }
          ListPane::Worktrees => {
              let rows = crate::selectors::worktree_rows(snap, &repo);
              let vis = crate::selectors::filter_rows(&rows, &ui.search[2], |r| r.name.clone());
              let remove_names: Vec<String> = vis[start..=end.min(vis.len().saturating_sub(1))]
                  .iter().filter_map(|&i| rows.get(i))
                  .filter(|r| !r.is_session && !matches!(r.state, crate::selectors::WtState::Busy))
                  .map(|r| r.name.clone()).collect();
              bulk_menu(BulkSelection::Worktrees { repo: repo.clone(), remove_names, total })
          }
      };
      Some(Mode::ActionMenu { title, items, index: 0 })
  }
  ```
  Prepend the range guard to `open_action_menu` (Task 14):
  ```rust
  fn open_action_menu(&mut self) -> Option<Mode> {
      let ui = self.active_ui();
      let pane = ui.last_list_pane;
      let (start, end) = Self::selection_range(&ui.selections[pane.idx()]);
      if end > start {
          return self.open_bulk_menu(pane, start, end);
      }
      // …existing single-target body from Task 14…
  }
  ```
  Replace the Task 14 catch-all `_ => …` bulk placeholder in `execute_menu_action` with real arms:
  ```rust
  M::BulkRerun { ids } => {
      self.clear_range(ListPane::Queue);
      Update { dirty: true, cmds: vec![Cmd::RpcSeq {
          verb: "reran".into(),
          calls: ids.into_iter().map(|id| RpcCall { method: "retry".into(), params: serde_json::json!({ "id": id }) }).collect(),
          invalidate_defs_for: None,
      }] }
  }
  M::BulkSkip { ids } => {
      self.clear_range(ListPane::Queue);
      Update { dirty: true, cmds: vec![Cmd::RpcSeq {
          verb: "skipped".into(),
          calls: ids.into_iter().map(|id| RpcCall { method: "skip".into(), params: serde_json::json!({ "id": id }) }).collect(),
          invalidate_defs_for: None,
      }] }
  }
  M::BulkRunDefs { repo, names } => {
      self.clear_range(ListPane::Tasks);
      // Verb "started" per parity oracle (App.tsx:698 / app.test.tsx:1573).
      Update { dirty: true, cmds: vec![Cmd::RpcSeq {
          verb: "started".into(),
          calls: names.into_iter().map(|name| RpcCall {
              method: "runDefinition".into(),
              params: serde_json::json!({ "repo": repo, "name": name, "args": [], "source": "tui" }),
          }).collect(),
          invalidate_defs_for: Some(repo),
      }] }
  }
  M::BulkRemove { repo, names } => {
      self.mode = Mode::ConfirmBulkRemove { repo, names };
      Update { dirty: true, cmds: vec![] }
  }
  ```
  Add the `Mode::ConfirmBulkRemove` key branch near the other confirm branch:
  ```rust
  Event::Key(k) if matches!(self.mode, Mode::ConfirmBulkRemove { .. }) => {
      use crossterm::event::KeyCode::*;
      match k.code {
          Char('y') => {
              let Mode::ConfirmBulkRemove { repo, names } = &self.mode else { unreachable!() };
              let (repo, names) = (repo.clone(), names.clone());
              self.clear_range(ListPane::Worktrees);
              self.mode = Mode::List;
              Update { dirty: true, cmds: vec![Cmd::RpcSeq {
                  verb: "removed".into(),
                  calls: names.into_iter().map(|name| RpcCall { method: "removeWorktree".into(), params: serde_json::json!({ "repo": repo, "name": name }) }).collect(),
                  invalidate_defs_for: None,
              }] }
          }
          Char('n') | Char('q') | Esc => { self.mode = Mode::List; Update { dirty: true, cmds: vec![] } }
          _ => Update { dirty: false, cmds: vec![] },
      }
  }
  ```
- [ ] **Step 9: Run (expect PASS).** `cargo test -p qoo-tui bulk_flow_tests`.
- [ ] **Step 10: Commit bulk logic.**
  `git add crates/qoo-tui/src/app.rs crates/qoo-tui/src/action_menu.rs`
  `git commit -m "feat(tui-rs): bulk menu open + RpcSeq execution + confirm-bulk-remove"`

- [ ] **Step 11: Failing test — `render_confirm_bulk_remove` truncation.** In `view/modal.rs`:
  ```rust
  #[cfg(test)]
  mod bulk_confirm_view_tests {
      use super::*;
      use crate::hit::HitMap;
      use ratatui::{backend::TestBackend, Terminal};

      fn draw(names: &[String]) -> String {
          let mut term = Terminal::new(TestBackend::new(60, 20)).unwrap();
          let mut hit = HitMap::default();
          term.draw(|f| render_confirm_bulk_remove(f, &mut hit, names)).unwrap();
          let buf = term.backend().buffer().clone();
          let mut s = String::new();
          for y in 0..20 { for x in 0..60 { s.push_str(buf[(x, y)].symbol()); } s.push('\n'); }
          s
      }

      #[test]
      fn lists_up_to_eight_names_then_and_n_more() {
          let names: Vec<String> = (0..10).map(|i| format!("wt-{i}")).collect();
          let s = draw(&names);
          assert!(s.contains("Remove 10 worktrees"));
          assert!(s.contains("discards uncommitted changes and deletes each local branch"));
          assert!(s.contains("wt-0"));
          assert!(s.contains("wt-7"));
          assert!(!s.contains("wt-8")); // truncated after 8
          assert!(s.contains("…and 2 more"));
      }
  }
  ```
- [ ] **Step 12: Run (expect FAIL).** `cargo test -p qoo-tui bulk_confirm_view_tests`.
- [ ] **Step 13: Implement `render_confirm_bulk_remove` + wire it.** In `view/modal.rs`:
  ```rust
  /// Bulk-remove confirmation: warning + up to 8 names + "…and N more".
  pub fn render_confirm_bulk_remove(frame: &mut ratatui::Frame, hit: &mut HitMap, names: &[String]) {
      let p = Palette::default();
      let shown = names.len().min(8);
      let extra = names.len().saturating_sub(8);
      let height = (2 + shown + if extra > 0 { 1 } else { 0 } + 1) as u16; // warn + names + more? + hint
      let inner = modal_frame(frame, frame.area(), &format!("Remove {} worktrees", names.len()), height);
      let body = Rect { x: inner.x.saturating_sub(1), y: inner.y.saturating_sub(1), width: inner.width + 2, height: inner.height + 2 };
      hit.push(body, HitTarget::Modal);

      let mut lines: Vec<Line> = Vec::new();
      lines.push(Line::from(Span::styled(" discards uncommitted changes and deletes each local branch", p.normal)));
      for name in names.iter().take(8) {
          lines.push(Line::from(Span::styled(format!("  {name}"), p.normal)));
      }
      if extra > 0 {
          lines.push(Line::from(Span::styled(format!("  …and {extra} more"), p.dim)));
      }
      lines.push(Line::from(Span::styled(" y confirm · n/esc cancel", p.dim)));
      frame.render_widget(Paragraph::new(lines), inner);
  }
  ```
  In `view/mod.rs` `render`, add a branch alongside the input modals:
  ```rust
  if let Mode::ConfirmBulkRemove { names, .. } = &app.mode {
      crate::view::modal::render_confirm_bulk_remove(frame, &mut hit, names);
  }
  ```
  Also verify (add an assertion in an existing footer test, no new file) that with an active range the Task 8 footer shows the bulk hint `"N selected · [a] bulk actions · [shift+↑↓] extend · [esc] clear"` — a read-only confirmation that Task 8's hint survives; if the exact copy differs, this is the pin.
- [ ] **Step 14: Run (accept snapshot if any, expect PASS).** `cargo test -p qoo-tui bulk_confirm_view_tests` and the full `cargo test -p qoo-tui`.
- [ ] **Step 15: Commit view.**
  `git add crates/qoo-tui/src/view/modal.rs crates/qoo-tui/src/view/mod.rs`
  `git commit -m "feat(tui-rs): confirm-bulk-remove modal render"`

- [ ] **Step 16: Milestone gate.** Run `cargo test -p qoo-tui` (all green) and `cargo build --release` (workspace builds). Both must pass to close M2.
## Milestone 3 — Forms & worktrees (Tasks 17–21)

M3 makes the TUI act: it ports the worktree-context auto-fill helpers, the definitions cache trigger with in-flight dedup, the def-picker, the per-arg form with real dropdowns, and the create/remove/squash worktree flows. These replace the four M3 stubs Task 14 left in the `MenuAction` dispatch (`RunNamedDef`, `RunDef`, `CreateWorktree`, `SquashMerge` currently set a `status_line` "arrives in M3").

**Cross-task ordering note (co-dependency 18↔19):** Task 18 opens `Mode::DefArgs`, which needs the `ArgsForm` type to exist. To keep every task green in listed order, the `ArgsForm` **struct + `ArgsForm::new`** (with its value-precedence logic and precedence tests) land in **Task 18** — its first consumer — and **Task 19** adds the interaction methods (`is_enum`/`is_fixed`/focus/cycle/edit/`validate`/dropdown). This is the only deviation from the prompt's "`new` in Task 19"; it is a pure ordering concession and changes no public shape.

**Assumed M2/earlier signatures this milestone consumes** (adapt mechanically to the resolved names):
- `crate::view::modal::modal_frame(frame: &mut Frame, title: &str, hint: &str, hit: &mut HitMap) -> Rect` (Task 15) — draws backdrop + bordered popup, registers the backdrop as `HitTarget::Modal`, returns the inner content `Rect`.
- `crate::view::theme::Palette` (Task 7) with fields `accent`, `dim`, `error`, `text` and glyph consts `GLYPH_DISCOVERY` (`⏰`), `MARKER_GLOBAL` (`(g)`); no inline color/glyph literals in view code (Global Constraint).
- `App::active_repo_name(&self) -> Option<String>` — active tab's project name (from `build_tabs(snapshot)[active_tab]`). If an equivalent M1 accessor already exists, reuse it and drop the local copy.
- The `update()` dispatcher (Task 14) matches on `self.mode`; M3 replaces the stub `MenuAction` arms and adds `Mode::DefPick`/`Mode::DefArgs`/`Mode::CreateWorktree` handling. Mouse routing (Task 12) yields a `HitTarget` from `app.hit.hit(col,row)`; M3 handles the mode-specific targets.

---

### Task 17: worktree_context.rs

Port `packages/core/src/worktree-context.ts` (`extractTicket`, `contextArgValues`) and the ambient overlay from `packages/tui/src/selectors.ts` (`ambientContextArgValues`, `branchCandidates`, `ambientRunArgs`). Pure module, no I/O; mirrors `core/src/__tests__/worktree-context.test.ts` and the `ambient*` blocks of `packages/tui/src/__tests__/selectors.test.ts`.

**Files:**
- Create: `crates/qoo-tui/src/worktree_context.rs`
- Modify: `crates/qoo-tui/src/lib.rs` (add `pub mod worktree_context;`)
- Test: inline `#[cfg(test)] mod tests` in `crates/qoo-tui/src/worktree_context.rs`

**Interfaces:**
- Consumes: `crate::ipc::types::ArgSpec`, `crate::selectors::{WorktreeRow, WtState}`.
- Produces (Shared Type Contract, verbatim):
  - `pub fn extract_ticket(branch: &str) -> Option<String>` — TS returns `""` for "no ticket"; the Rust contract uses `None`.
  - `pub fn context_arg_values(branch: &str) -> HashMap<String, String>` — keys among `{"source","branch","ticket"}`; empty branch → empty map; `ticket` key omitted when absent.
  - `pub fn ambient_run_args(args: &[ArgSpec], worktrees: &[WorktreeRow], selected: Option<&WorktreeRow>) -> (Vec<ArgSpec>, HashMap<String, String>)`.
- Private: `fn ambient_context_arg_values(row: Option<&WorktreeRow>) -> HashMap<String,String>`, `fn branch_candidates(rows: &[WorktreeRow]) -> Vec<String>` (tested via the same module).
- No new deps: `extract_ticket` hand-rolls the `[A-Za-z]+-\d+` scan (no `regex` crate).

**Steps:**

- [ ] **Step 1: Failing tests for `extract_ticket` + `context_arg_values`.** Add to `worktree_context.rs`:
  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;
      use crate::ipc::types::ArgSpec;
      use crate::selectors::{WorktreeRow, WtState};
      use std::collections::HashMap;

      fn map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
          pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
      }

      #[test]
      fn extract_ticket_cases() {
          assert_eq!(extract_ticket("JUS-1008").as_deref(), Some("JUS-1008"));
          assert_eq!(extract_ticket("jus-1008-fix-thing").as_deref(), Some("JUS-1008"));
          assert_eq!(extract_ticket("jus-1008").as_deref(), Some("JUS-1008"));
          assert_eq!(extract_ticket("main"), None);
          assert_eq!(extract_ticket("feature/no-number"), None);
          assert_eq!(extract_ticket(""), None);
          assert_eq!(extract_ticket("jus-1008-then-abc-42").as_deref(), Some("JUS-1008"));
      }

      #[test]
      fn context_arg_values_cases() {
          assert_eq!(
              context_arg_values("jus-1008-fix-thing"),
              map(&[("source", "jus-1008-fix-thing"), ("branch", "jus-1008-fix-thing"), ("ticket", "JUS-1008")])
          );
          assert_eq!(
              context_arg_values("feature/no-number"),
              map(&[("source", "feature/no-number"), ("branch", "feature/no-number")])
          );
          assert!(context_arg_values("").is_empty());
      }
  }
  ```
- [ ] **Step 2: Run (expect FAIL).** `cargo test -p qoo-tui worktree_context` — fails to compile (functions missing).
- [ ] **Step 3: Implement `extract_ticket` + `context_arg_values`.** Add above the test module:
  ```rust
  use std::collections::HashMap;

  use crate::ipc::types::ArgSpec;
  use crate::selectors::WorktreeRow;

  /// First `[A-Za-z]+-\d+` token, uppercased; `None` when the branch carries no
  /// ticket-shaped token. Hand-rolled scan (no regex dep): greedy letter run,
  /// a single `-`, then one-or-more digits.
  pub fn extract_ticket(branch: &str) -> Option<String> {
      let b = branch.as_bytes();
      let n = b.len();
      let mut i = 0;
      while i < n {
          if b[i].is_ascii_alphabetic() {
              let start = i;
              while i < n && b[i].is_ascii_alphabetic() {
                  i += 1;
              }
              if i < n && b[i] == b'-' {
                  let mut j = i + 1;
                  while j < n && b[j].is_ascii_digit() {
                      j += 1;
                  }
                  if j > i + 1 {
                      return Some(branch[start..j].to_ascii_uppercase());
                  }
              }
              // letters not followed by `-<digits>`: keep scanning from `i`.
          } else {
              i += 1;
          }
      }
      None
  }

  /// Arg values implied by a worktree branch: `source`/`branch` are the branch,
  /// `ticket` the extracted token (key omitted when absent so a def default
  /// wins). Empty branch → empty map.
  pub fn context_arg_values(branch: &str) -> HashMap<String, String> {
      let mut values = HashMap::new();
      if branch.is_empty() {
          return values;
      }
      values.insert("source".to_string(), branch.to_string());
      values.insert("branch".to_string(), branch.to_string());
      if let Some(ticket) = extract_ticket(branch) {
          values.insert("ticket".to_string(), ticket);
      }
      values
  }
  ```
  Add `pub mod worktree_context;` to `lib.rs`.
- [ ] **Step 4: Run (expect PASS).** `cargo test -p qoo-tui worktree_context`.
- [ ] **Step 5: Commit.** `git add crates/qoo-tui/src/worktree_context.rs crates/qoo-tui/src/lib.rs` · `git commit -m "feat(tui-rs): port extract_ticket + context_arg_values"`
- [ ] **Step 6: Failing tests for ambient overlay.** Extend the test module:
  ```rust
  fn wt(name: &str, branch: &str, is_session: bool) -> WorktreeRow {
      WorktreeRow {
          name: name.to_string(),
          raw_name: name.to_string(),
          path: format!("/wt/{name}"),
          branch: branch.to_string(),
          state: WtState::Free,
          has_main_session: false,
          queued: 0,
          is_session,
      }
  }
  fn arg(name: &str) -> ArgSpec {
      ArgSpec { name: name.to_string(), default: None, options: None, description: None }
  }
  fn rows() -> Vec<WorktreeRow> {
      vec![
          wt("a", "jus-1-a", false),
          wt("main", "main", false),
          wt("b", "feat-b", false),
          wt("sess", "", true),
      ]
  }

  #[test]
  fn ambient_context_arg_values_cases() {
      assert_eq!(
          ambient_context_arg_values(Some(&wt("a", "jus-1008-fix", false))),
          map(&[("source", "jus-1008-fix"), ("branch", "jus-1008-fix"), ("ticket", "JUS-1008")])
      );
      assert!(ambient_context_arg_values(Some(&wt("s", "", true))).is_empty()); // session row
      assert!(ambient_context_arg_values(Some(&wt("x", "", false))).is_empty()); // branchless
      assert!(ambient_context_arg_values(Some(&wt("m", "main", false))).is_empty());
      assert!(ambient_context_arg_values(Some(&wt("m", "master", false))).is_empty());
      assert!(ambient_context_arg_values(None).is_empty());
  }

  #[test]
  fn ambient_run_args_injects_source_dropdown_excluding_main_and_sessions() {
      let r = rows();
      let (args, _) = ambient_run_args(
          &[arg("source"), ArgSpec { default: Some("main".into()), ..arg("target") }],
          &r,
          Some(&r[0]),
      );
      assert_eq!(args[0].name, "source");
      assert_eq!(args[0].options.as_deref(), Some(&["jus-1-a".to_string(), "feat-b".to_string()][..]));
      assert_eq!(args[1].name, "target");
      assert_eq!(args[1].options, None);
      assert_eq!(args[1].default.as_deref(), Some("main"));
  }

  #[test]
  fn ambient_run_args_injects_for_branch_and_prefills_initial() {
      let r = rows();
      let (args, _) = ambient_run_args(&[arg("branch")], &r, Some(&r[0]));
      assert_eq!(args[0].options.as_deref(), Some(&["jus-1-a".to_string(), "feat-b".to_string()][..]));
      let (_, initial) = ambient_run_args(&[arg("source")], &r, Some(&r[0]));
      assert_eq!(initial, map(&[("source", "jus-1-a"), ("branch", "jus-1-a"), ("ticket", "JUS-1")]));
  }

  #[test]
  fn ambient_run_args_leaves_declared_options_and_freetext_untouched() {
      let r = rows();
      let declared = ArgSpec { options: Some(vec!["x".into(), "y".into()]), ..arg("source") };
      let (args, _) = ambient_run_args(&[declared.clone()], &r, Some(&r[0]));
      assert_eq!(args[0], declared);

      let only_main = vec![wt("main", "main", false)];
      let (args, initial) = ambient_run_args(&[arg("source")], &only_main, Some(&only_main[0]));
      assert_eq!(args[0], arg("source")); // no options injected
      assert!(initial.is_empty());

      let (args, _) = ambient_run_args(&[arg("pr")], &r, Some(&r[0]));
      assert_eq!(args[0], arg("pr")); // non source/branch untouched
  }
  ```
- [ ] **Step 7: Run (expect FAIL).** `cargo test -p qoo-tui worktree_context`.
- [ ] **Step 8: Implement the ambient overlay.** Append to `worktree_context.rs`:
  ```rust
  /// Prefill from the selected worktree row: only a real worktree row with a
  /// branch other than main/master contributes (session/branchless/main rows
  /// borrow nothing).
  fn ambient_context_arg_values(row: Option<&WorktreeRow>) -> HashMap<String, String> {
      let Some(row) = row else { return HashMap::new() };
      if row.is_session || row.branch.is_empty() || row.branch == "main" || row.branch == "master" {
          return HashMap::new();
      }
      context_arg_values(&row.branch)
  }

  /// Every real worktree's branch in row order, minus session/branchless rows
  /// and the primary checkout (main/master is never a sensible source).
  fn branch_candidates(rows: &[WorktreeRow]) -> Vec<String> {
      rows.iter()
          .filter(|r| !r.is_session && !r.branch.is_empty() && r.branch != "main" && r.branch != "master")
          .map(|r| r.branch.clone())
          .collect()
  }

  /// Overlay worktree context onto a def's args for an ambient (TASKS-pane) run:
  /// an arg named `source`/`branch` with no declared options becomes a dropdown
  /// of the repo's worktree branches; `initial` prefills from the selected row.
  /// Nothing is fixed — submission stays positional and the daemon never sees the
  /// injected options.
  pub fn ambient_run_args(
      args: &[ArgSpec],
      worktrees: &[WorktreeRow],
      selected: Option<&WorktreeRow>,
  ) -> (Vec<ArgSpec>, HashMap<String, String>) {
      let candidates = branch_candidates(worktrees);
      let out = args
          .iter()
          .map(|arg| {
              let named = arg.name == "source" || arg.name == "branch";
              let has_options = arg.options.as_ref().is_some_and(|o| !o.is_empty());
              if !named || has_options || candidates.is_empty() {
                  arg.clone()
              } else {
                  ArgSpec { options: Some(candidates.clone()), ..arg.clone() }
              }
          })
          .collect();
      (out, ambient_context_arg_values(selected))
  }
  ```
- [ ] **Step 9: Run (expect PASS).** `cargo test -p qoo-tui worktree_context`.
- [ ] **Step 10: Commit.** `git add crates/qoo-tui/src/worktree_context.rs` · `git commit -m "feat(tui-rs): port ambient worktree-context overlay for args forms"`

---

### Task 18: def-pick + definitions cache wiring (+ ArgsForm struct/`new`)

Wires the lazy per-tab `definitions` fetch with in-flight dedup, replaces Task 14's `RunNamedDef`/`RunDef` stubs, and adds the `Mode::DefPick` picker. Also introduces the `ArgsForm` struct + `ArgsForm::new` (Task 18 is its first consumer; Task 19 adds the interaction methods). Ports `App.tsx` def-fetch effect (~L401), the `run`/`run-def` action cases (~L695/744), and the def-pick key handler + render (~L977, L1276).

**Files:**
- Modify: `crates/qoo-tui/src/app.rs` (add `defs_inflight`, `reconcile_defs`, `MenuAction` arms, `Mode::DefPick` handling, helpers)
- Create: `crates/qoo-tui/src/view/args_form.rs` (struct + `new` only)
- Modify: `crates/qoo-tui/src/view/menu.rs` (`render_def_pick`), `crates/qoo-tui/src/view/mod.rs` (dispatch `Mode::DefPick`/`Mode::DefArgs`), `crates/qoo-tui/src/lib.rs`/`view/mod.rs` (`pub mod args_form;`)
- Test: inline `#[cfg(test)] mod tests` in `app.rs`; snapshot test in `crates/qoo-tui/tests/def_pick_snapshot.rs`

**Interfaces:**
- **Contract addition:** `App` gains `pub defs_inflight: std::collections::HashSet<String>` (repos with a `FetchDefinitions` in flight). Initialize empty in `App::new`.
- Consumes: `Cmd::{FetchDefinitions, Rpc}`, `Event::Definitions`, `MenuAction::{RunNamedDef, RunDef}`, `crate::selectors::{arg_summary, worktree_rows, WorktreeRow}`, `crate::worktree_context::context_arg_values`, `crate::view::modal::modal_frame`.
- Produces: `crate::view::args_form::ArgsForm` (struct fields per Shared Type Contract) + `ArgsForm::new`; `App::reconcile_defs`, `App::run_definition_cmd`, `App::open_def_args`.
- Ordering: `RunNamedDef`/`RunDef` "with args" transitions consume `ArgsForm::new` (introduced in this task).

**Steps:**

- [ ] **Step 1: Failing test — `ArgsForm::new` value precedence.** Create `crates/qoo-tui/src/view/args_form.rs`:
  ```rust
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
  }
  ```
- [ ] **Step 2: Run (expect FAIL).** `cargo test -p qoo-tui args_form` — `ArgsForm` undefined.
- [ ] **Step 3: Implement `ArgsForm` struct + `new`.** Prepend to `args_form.rs`:
  ```rust
  use std::collections::HashMap;

  use crate::ipc::types::ArgSpec;

  /// Per-arg form state. Interaction methods (focus/cycle/edit/validate/dropdown)
  /// are added in Task 19; this file introduces the struct + constructor.
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
  fn initial_value(arg: &ArgSpec, fixed: &HashMap<String, String>, initial: &HashMap<String, String>) -> String {
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
  }
  ```
  Add `pub mod args_form;` under `view/mod.rs` (or wherever `view` submodules are declared).
- [ ] **Step 4: Run (expect PASS).** `cargo test -p qoo-tui args_form`.
- [ ] **Step 5: Commit.** `git add crates/qoo-tui/src/view/args_form.rs crates/qoo-tui/src/view/mod.rs` · `git commit -m "feat(tui-rs): ArgsForm struct + new() with value precedence"`

- [ ] **Step 6: Failing test — defs lazy fetch + in-flight dedup.** In `app.rs` test module (build an `App` with a fixture snapshot exposing one project "platform"; reuse M1/M2 test fixtures):
  ```rust
  #[test]
  fn reconcile_defs_fetches_once_and_dedups() {
      let mut app = fixture_app_one_project("platform"); // helper from M1/M2 tests
      // First reconcile: repo uncached, not in flight -> emits FetchDefinitions + marks in flight.
      let cmd = app.reconcile_defs();
      assert!(matches!(cmd, Some(Cmd::FetchDefinitions { ref repo }) if repo == "platform"));
      assert!(app.defs_inflight.contains("platform"));
      // Second reconcile before the reply: no duplicate.
      assert!(app.reconcile_defs().is_none());
      // Reply lands: cache set, in-flight cleared.
      app.update(Event::Definitions { repo: "platform".into(), defs: vec![] });
      assert!(app.defs_by_project.contains_key("platform"));
      assert!(!app.defs_inflight.contains("platform"));
      // Now cached -> no fetch.
      assert!(app.reconcile_defs().is_none());
  }
  ```
- [ ] **Step 7: Run (expect FAIL).** `cargo test -p qoo-tui reconcile_defs`.
- [ ] **Step 8: Implement `defs_inflight` + `reconcile_defs` + `Event::Definitions` handling.** In `app.rs`:
  - Add field `pub defs_inflight: std::collections::HashSet<String>` to `App`; init `HashSet::new()` in `App::new`.
  - Add the reconcile helper and call it at the tail of `update()` (mirrors the TS effect that re-fires after every render, incl. after invalidation drops the cache):
    ```rust
    /// Emit a lazy `definitions` fetch for the active repo when its summaries are
    /// neither cached nor already in flight. Called at the end of `update()`.
    fn reconcile_defs(&mut self) -> Option<Cmd> {
        let repo = self.active_repo_name()?;
        if self.defs_by_project.contains_key(&repo) || self.defs_inflight.contains(&repo) {
            return None;
        }
        self.defs_inflight.insert(repo.clone());
        Some(Cmd::FetchDefinitions { repo })
    }
    ```
    At the end of `update()`, just before returning `update`:
    ```rust
    if let Some(cmd) = self.reconcile_defs() {
        update.cmds.push(cmd);
    }
    ```
  - Ensure the `Event::Definitions { repo, defs }` arm stores **and** clears in-flight (extend Task 13's arm, or create it):
    ```rust
    Event::Definitions { repo, defs } => {
        self.defs_by_project.insert(repo.clone(), defs);
        self.defs_inflight.remove(&repo);
        dirty = true;
    }
    ```
    (The `ActionResult { invalidate_defs_for: Some(repo), .. }` arm from Task 13 drops `defs_by_project[repo]`; the next `reconcile_defs` re-fetches, matching `invalidateDefs()`.)
- [ ] **Step 9: Run (expect PASS).** `cargo test -p qoo-tui reconcile_defs`.
- [ ] **Step 10: Commit.** `git add crates/qoo-tui/src/app.rs` · `git commit -m "feat(tui-rs): lazy per-tab definitions fetch with in-flight dedup"`

- [ ] **Step 11: Failing tests — `RunNamedDef` (tasks pane) dispatch shapes.** In `app.rs` tests:
  ```rust
  #[test]
  fn run_named_def_zero_arg_dispatches_immediately() {
      let mut app = fixture_app_with_defs("platform", vec![
          DefinitionSummary { repo: "platform".into(), name: "noargs".into(), scope: "project".into(), args: vec![], has_discovery: false },
      ]);
      let update = app.dispatch_menu_action(MenuAction::RunNamedDef { repo: "platform".into(), name: "noargs".into() });
      // args:[], source:"tui", no worktree override; invalidates defs for the repo.
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
      // A worktree row with a ticket branch is selected in the worktrees pane.
      let mut app = fixture_app_with_defs_and_worktree(
          "platform",
          vec![DefinitionSummary { repo: "platform".into(), name: "deploy".into(), scope: "project".into(),
              args: vec![ArgSpec { name: "source".into(), default: None, options: None, description: None }],
              has_discovery: false }],
          ("wt-a", "jus-9-x"), // selected worktree name/branch
      );
      let update = app.dispatch_menu_action(MenuAction::RunNamedDef { repo: "platform".into(), name: "deploy".into() });
      assert!(update.cmds.is_empty()); // no dispatch yet — form opens
      match &app.mode {
          Mode::DefArgs { form } => {
              // ambient: `source` gains worktree-branch options, prefilled, no fixed rows, no worktree override.
              assert_eq!(form.args[0].options.as_deref(), Some(&["jus-9-x".to_string()][..]));
              assert_eq!(form.values[0], "jus-9-x");
              assert!(form.fixed.is_empty());
              assert_eq!(form.initial_worktree, None);
          }
          other => panic!("expected DefArgs, got {other:?}"),
      }
  }
  ```
- [ ] **Step 12: Run (expect FAIL).** `cargo test -p qoo-tui run_named_def`.
- [ ] **Step 13: Implement `run_definition_cmd`, `open_def_args`, and the `RunNamedDef` arm.** In `app.rs`:
  ```rust
  /// Build the fire-and-forget `runDefinition` command. Client timeout is treated
  /// as success (discovery can outlive it; the push subscription re-syncs), and a
  /// successful run invalidates the repo's def summaries.
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

  /// Active project's worktree rows (unfiltered), used for ambient overlays.
  fn active_worktree_rows(&self) -> Vec<crate::selectors::WorktreeRow> {
      match (&self.snapshot, self.active_repo_name()) {
          (Some(snap), Some(repo)) => crate::selectors::worktree_rows(snap, &repo),
          _ => Vec::new(),
      }
  }

  /// Currently-selected worktree row (clamped cursor into the pane's rows).
  fn selected_worktree_row(&self) -> Option<crate::selectors::WorktreeRow> {
      let rows = self.active_worktree_rows();
      let cursor = self
          .ui_by_tab
          .get(&self.active_repo_name()?)
          .map(|ui| ui.selections[crate::app::ListPane::Worktrees.idx()].cursor)
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
  ```
  In `dispatch_menu_action` (the `MenuAction` match; replace the Task 14 `RunNamedDef` stub):
  ```rust
  MenuAction::RunNamedDef { repo, name } => {
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
          self.mode = Mode::List;
          return Update { dirty: true, cmds: vec![Self::run_definition_cmd(&repo, &name, &[], None)] };
      }
      // Ambient run: overlay branch dropdown + prefill from the selected worktree row.
      let rows = self.active_worktree_rows();
      let selected = self.selected_worktree_row();
      let (args, initial) = crate::worktree_context::ambient_run_args(&def.args, &rows, selected.as_ref());
      self.open_def_args(repo, name, args, HashMap::new(), initial, None);
      Update { dirty: true, cmds: vec![] }
  }
  ```
  (`dispatch_menu_action` returns an `Update`; adapt to Task 14's actual signature — if it mutates `self` and pushes into a shared `cmds` vec, inline the same bodies.)
- [ ] **Step 14: Run (expect PASS).** `cargo test -p qoo-tui run_named_def`.
- [ ] **Step 15: Commit.** `git add crates/qoo-tui/src/app.rs` · `git commit -m "feat(tui-rs): RunNamedDef — zero-arg dispatch + ambient args form"`

- [ ] **Step 16: Failing tests — `RunDef` → `Mode::DefPick`, ordering, empty guard.** In `app.rs` tests:
  ```rust
  #[test]
  fn run_def_opens_def_pick_in_server_order() {
      // Daemon returns per-repo defs sorted by name; the client preserves that order.
      let mut app = fixture_app_with_defs("platform", vec![
          DefinitionSummary { repo: "platform".into(), name: "autotest".into(), scope: "project".into(), args: vec![], has_discovery: false },
          DefinitionSummary { repo: "platform".into(), name: "squash-merge".into(), scope: "global".into(),
              args: vec![ArgSpec { name: "source".into(), default: None, options: None, description: None }], has_discovery: false },
      ]);
      let _ = app.dispatch_menu_action(MenuAction::RunDef { worktree: Some("platform.wt-a".into()), branch: Some("jus-4-x".into()) });
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
      let update = app.dispatch_menu_action(MenuAction::RunDef { worktree: Some("wt-a".into()), branch: None });
      assert_eq!(app.status_line.as_deref(), Some("no task definitions found"));
      assert!(update.cmds.is_empty());
      assert!(matches!(app.mode, Mode::List));
  }
  ```
- [ ] **Step 17: Run (expect FAIL).** `cargo test -p qoo-tui run_def`.
- [ ] **Step 18: Implement the `RunDef` arm.** Replace the Task 14 `RunDef` stub:
  ```rust
  MenuAction::RunDef { worktree, branch } => {
      let repo = match self.active_repo_name() {
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
  ```
  (Server order is alphabetical-by-name per `packages/daemon/src/api.ts` `definitions` — no client re-sort, matching `App.tsx`.)
- [ ] **Step 19: Run (expect PASS).** `cargo test -p qoo-tui run_def`.
- [ ] **Step 20: Commit.** `git add crates/qoo-tui/src/app.rs` · `git commit -m "feat(tui-rs): RunDef opens the def picker for the targeted worktree"`

- [ ] **Step 21: Failing tests — `Mode::DefPick` navigation + Enter (fixed target).** In `app.rs` tests:
  ```rust
  fn key(code: crossterm::event::KeyCode) -> Event {
      Event::Key(crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::NONE))
  }

  #[test]
  fn def_pick_moves_clamped_and_closes_on_q_esc() {
      let mut app = fixture_def_pick(vec!["a", "b"], Some("platform.wt"), Some("jus-1-x"));
      app.update(key(crossterm::event::KeyCode::Char('j'))); // 0 -> 1
      assert!(matches!(app.mode, Mode::DefPick { index: 1, .. }));
      app.update(key(crossterm::event::KeyCode::Char('j'))); // clamp at last
      assert!(matches!(app.mode, Mode::DefPick { index: 1, .. }));
      app.update(key(crossterm::event::KeyCode::Char('k'))); // 1 -> 0
      assert!(matches!(app.mode, Mode::DefPick { index: 0, .. }));
      app.update(key(crossterm::event::KeyCode::Char('q')));
      assert!(matches!(app.mode, Mode::List));
  }

  #[test]
  fn def_pick_enter_zero_arg_dispatches_with_worktree() {
      let mut app = fixture_def_pick_defs(
          vec![DefinitionSummary { repo: "platform".into(), name: "autotest".into(), scope: "project".into(), args: vec![], has_discovery: false }],
          Some("platform.wt-a".into()), Some("jus-1-x".into()),
      );
      let update = app.update(key(crossterm::event::KeyCode::Enter));
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
          vec![DefinitionSummary { repo: "platform".into(), name: "deploy".into(), scope: "project".into(),
              args: vec![
                  ArgSpec { name: "source".into(), default: None, options: None, description: None },
                  ArgSpec { name: "target".into(), default: Some("main".into()), options: None, description: None },
              ], has_discovery: false }],
          Some("platform.wt-a".into()), Some("jus-9-x".into()),
      );
      app.update(key(crossterm::event::KeyCode::Enter));
      match &app.mode {
          Mode::DefArgs { form } => {
              // explicit target: source/branch/ticket fixed from the branch; source shown from fixed.
              assert_eq!(form.fixed.get("source").map(String::as_str), Some("jus-9-x"));
              assert_eq!(form.fixed.get("ticket").map(String::as_str), Some("JUS-9"));
              assert_eq!(form.values[0], "jus-9-x"); // source row prefilled from fixed
              assert_eq!(form.values[1], "main");     // target from default (editable)
              assert_eq!(form.initial_worktree.as_deref(), Some("platform.wt-a"));
          }
          other => panic!("expected DefArgs, got {other:?}"),
      }
  }
  ```
- [ ] **Step 22: Run (expect FAIL).** `cargo test -p qoo-tui def_pick`.
- [ ] **Step 23: Implement `Mode::DefPick` key handling.** In `update()` add a `Mode::DefPick` arm handling `Event::Key` (and `Event::Mouse` click → `HitTarget::MenuItem(i)` picks that row via the same Enter path). Factor the body into a method:
  ```rust
  fn def_pick_key(&mut self, key: crossterm::event::KeyCode) -> Update {
      use crossterm::event::KeyCode::*;
      let Mode::DefPick { defs, index, worktree, branch } = &self.mode else {
          return Update { dirty: false, cmds: vec![] };
      };
      let (defs, index, worktree, branch) = (defs.clone(), *index, worktree.clone(), branch.clone());
      match key {
          Esc | Char('q') => {
              self.mode = Mode::List;
              Update { dirty: true, cmds: vec![] }
          }
          Up | Char('k') => {
              let i = index.saturating_sub(1);
              if let Mode::DefPick { index, .. } = &mut self.mode { *index = i; }
              Update { dirty: true, cmds: vec![] }
          }
          Down | Char('j') => {
              let i = (index + 1).min(defs.len().saturating_sub(1));
              if let Mode::DefPick { index, .. } = &mut self.mode { *index = i; }
              Update { dirty: true, cmds: vec![] }
          }
          Enter => self.def_pick_activate(index),
          _ => Update { dirty: false, cmds: vec![] },
      }
  }

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
      // Explicit target: this worktree's branch drives source/branch/ticket as FIXED.
      let fixed = branch.as_deref().map(context_arg_values).unwrap_or_default();
      self.open_def_args(def.repo, def.name, def.args, fixed, HashMap::new(), worktree);
      Update { dirty: true, cmds: vec![] }
  }
  ```
  Route mouse: on `HitTarget::MenuItem(i)` while `Mode::DefPick`, call `self.def_pick_activate(i)` (register `MenuItem` in `render_def_pick`, Step 26). Add `use crate::worktree_context::context_arg_values;`.
- [ ] **Step 24: Run (expect PASS).** `cargo test -p qoo-tui def_pick`.
- [ ] **Step 25: Commit.** `git add crates/qoo-tui/src/app.rs` · `git commit -m "feat(tui-rs): def-pick navigation, zero-arg dispatch, fixed-context args"`

- [ ] **Step 26: Failing snapshot — def-pick popup render.** Create `crates/qoo-tui/tests/def_pick_snapshot.rs`:
  ```rust
  use qoo_tui::app::{App, Mode};
  use qoo_tui::ipc::types::{ArgSpec, DefinitionSummary};
  use ratatui::{Terminal, backend::TestBackend};

  #[test]
  fn def_pick_popup_snapshot() {
      let mut app = App::new("/tmp/runs".into(), "/tmp/d.sock".into());
      app.size = (80, 24);
      app.mode = Mode::DefPick {
          defs: vec![
              DefinitionSummary { repo: "platform".into(), name: "autotest".into(), scope: "project".into(),
                  args: vec![], has_discovery: true },
              DefinitionSummary { repo: "platform".into(), name: "squash-merge".into(), scope: "global".into(),
                  args: vec![
                      ArgSpec { name: "source".into(), default: None, options: None, description: None },
                      ArgSpec { name: "target".into(), default: Some("main".into()), options: None, description: None },
                  ], has_discovery: false },
          ],
          index: 1,
          worktree: Some("platform.wt-a".into()),
          branch: Some("jus-1-x".into()),
      };
      let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
      term.draw(|f| { qoo_tui::view::render(&app, f); }).unwrap();
      insta::assert_snapshot!(term.backend());
  }
  ```
- [ ] **Step 27: Run (expect FAIL).** `cargo test -p qoo-tui --test def_pick_snapshot` — no render for `Mode::DefPick`.
- [ ] **Step 28: Implement `render_def_pick` + view dispatch.** In `view/menu.rs`:
  ```rust
  use ratatui::Frame;
  use ratatui::layout::Rect;
  use ratatui::style::{Modifier, Style};
  use ratatui::text::{Line, Span};

  use crate::hit::{HitMap, HitTarget};
  use crate::ipc::types::DefinitionSummary;
  use crate::selectors::arg_summary;
  use crate::view::modal::modal_frame;
  use crate::view::theme::{Palette, GLYPH_DISCOVERY, MARKER_GLOBAL};

  /// Render the "Run task definition" picker. `(g)` marks global-scope defs,
  /// `⏰` marks discovery, and the arg summary trails the name. Each row registers
  /// `HitTarget::MenuItem(i)` so a click picks it.
  pub fn render_def_pick(
      frame: &mut Frame,
      hit: &mut HitMap,
      p: &Palette,
      title: &str,
      defs: &[DefinitionSummary],
      index: usize,
  ) {
      let inner: Rect = modal_frame(frame, title, "↑/↓ move · enter run · q/esc close", hit);
      for (i, def) in defs.iter().enumerate() {
          if i as u16 >= inner.height {
              break;
          }
          let row = Rect { x: inner.x, y: inner.y + i as u16, width: inner.width, height: 1 };
          let mut text = format!(" {}", def.name);
          if !def.args.is_empty() {
              text.push_str(&format!(" ({})", arg_summary(&def.args)));
          }
          if def.has_discovery {
              text.push(' ');
              text.push_str(GLYPH_DISCOVERY);
          }
          let marker = if def.scope == "global" { MARKER_GLOBAL } else { "    " };
          let base = if i == index {
              Style::default().add_modifier(Modifier::REVERSED)
          } else {
              Style::default().fg(p.text)
          };
          let line = Line::from(vec![
              Span::styled(pad_to(&text, inner.width.saturating_sub(4) as usize), base),
              Span::styled(marker.to_string(), base.fg(p.dim)),
          ]);
          frame.render_widget(line, row);
          hit.push(row, HitTarget::MenuItem(i));
      }
  }

  /// Left-pad/truncate to an exact width so the popup stays opaque.
  fn pad_to(s: &str, width: usize) -> String {
      let mut out: String = s.chars().take(width).collect();
      while out.chars().count() < width {
          out.push(' ');
      }
      out
  }
  ```
  Add `MARKER_GLOBAL: &str = "(g)"` and `GLYPH_DISCOVERY: &str = "⏰"` to `theme.rs` if not already present (glyphs centralized per Global Constraint). In `view/mod.rs` `render`, dispatch after drawing the base layout:
  ```rust
  match &app.mode {
      Mode::DefPick { defs, index, worktree, branch } => {
          let repo = app.active_repo_name().unwrap_or_default();
          let title = match worktree {
              Some(wt) => format!("Run task definition — {}:{}", repo, crate::selectors::strip_repo_prefix(wt, &repo)),
              None => format!("Run task definition — {repo}"),
          };
          let _ = branch;
          crate::view::menu::render_def_pick(frame, &mut hit, &palette, &title, defs, *index);
      }
      Mode::DefArgs { form } => {
          crate::view::args_form::render_args_form(frame, &mut hit, &palette, form); // Task 20
      }
      _ => {}
  }
  ```
  (A stub `render_args_form` may be needed to compile until Task 20; add a one-line placeholder `pub fn render_args_form(_: &mut Frame, _: &mut HitMap, _: &Palette, _: &ArgsForm) {}` in `args_form.rs` now, filled in Task 20.)
- [ ] **Step 29: Run + review snapshot (expect PASS after accept).** `cargo test -p qoo-tui --test def_pick_snapshot`; `cargo insta review` (accept the popup showing `autotest ⏰`, selected `squash-merge (source, target=main) (g)`).
- [ ] **Step 30: Commit.** `git add crates/qoo-tui/src/view/menu.rs crates/qoo-tui/src/view/mod.rs crates/qoo-tui/src/view/theme.rs crates/qoo-tui/src/view/args_form.rs crates/qoo-tui/tests/def_pick_snapshot.rs crates/qoo-tui/tests/snapshots/` · `git commit -m "feat(tui-rs): render def-pick popup with clickable rows"`

---

### Task 19: ArgsForm logic (pure)

Adds the interaction methods to `ArgsForm` (struct + `new` landed in Task 18): enum detection, focus traversal skipping fixed rows, enum cycling, text editing with error-clear, positional validation with focus-on-first-missing, and dropdown open/move/pick. Mirrors `packages/tui/src/components/ArgsForm.tsx` and the `ArgsForm` block of `packages/tui/src/__tests__/components.test.tsx`, test-by-test.

**Files:**
- Modify: `crates/qoo-tui/src/view/args_form.rs`
- Test: inline `#[cfg(test)] mod tests` in `args_form.rs`

**Interfaces:**
- Consumes: `crate::ipc::types::ArgSpec`, the module-private `arg_is_enum` (Task 18).
- Produces (methods on `ArgsForm`):
  - `pub fn is_enum(&self, i: usize) -> bool`
  - `pub fn is_fixed(&self, i: usize) -> bool`
  - `pub fn next_focus(&mut self)` / `pub fn prev_focus(&mut self)` (wrap; skip fixed; no-op if all-fixed)
  - `pub fn cycle_option(&mut self, i: usize, delta: i32)` (enum rows only; wraps)
  - `pub fn input_char(&mut self, c: char)` / `pub fn backspace(&mut self)` (text focus row only; clears `error` on the edited row)
  - `pub fn validate(&mut self) -> Result<Vec<String>, usize>` (first required-and-empty → `Err(index)`, sets `self.error`, focuses it if editable; else `Ok(values)`)
  - `pub fn open_dropdown(&mut self, i: usize)` / `pub fn close_dropdown(&mut self)` / `pub fn dropdown_move(&mut self, delta: i32)` / `pub fn dropdown_pick(&mut self)`

**Steps:**

- [ ] **Step 1: Failing tests — enum/fixed detection, focus traversal, cycling.** In the `args_form.rs` test module:
  ```rust
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
  ```
- [ ] **Step 2: Run (expect FAIL).** `cargo test -p qoo-tui args_form`.
- [ ] **Step 3: Implement detection + focus + cycling.** In `impl ArgsForm`:
  ```rust
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
  pub fn next_focus(&mut self) { self.step_focus(1); }
  pub fn prev_focus(&mut self) { self.step_focus(-1); }

  pub fn cycle_option(&mut self, i: usize, delta: i32) {
      if !self.is_enum(i) {
          return;
      }
      let opts = match self.args[i].options.as_ref() {
          Some(o) if !o.is_empty() => o.clone(),
          _ => return,
      };
      let len = opts.len() as i32;
      let cur = opts.iter().position(|o| Some(o) == self.values.get(i)).map(|p| p as i32).unwrap_or(0);
      let next = ((cur + delta).rem_euclid(len)) as usize;
      self.values[i] = opts[next].clone();
  }
  ```
- [ ] **Step 4: Run (expect PASS).** `cargo test -p qoo-tui args_form`.
- [ ] **Step 5: Commit.** `git add crates/qoo-tui/src/view/args_form.rs` · `git commit -m "feat(tui-rs): ArgsForm enum detection, focus traversal, option cycling"`

- [ ] **Step 6: Failing tests — text edit, validation, dropdown.** Extend the test module:
  ```rust
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
  ```
- [ ] **Step 7: Run (expect FAIL).** `cargo test -p qoo-tui args_form`.
- [ ] **Step 8: Implement edit + validate + dropdown.** In `impl ArgsForm`:
  ```rust
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
      let len = self.args.get(self.focus).and_then(|a| a.options.as_ref()).map(|o| o.len()).unwrap_or(0);
      if len == 0 {
          return;
      }
      let next = (cur as i32 + delta).clamp(0, len as i32 - 1) as usize;
      self.dropdown = Some(next);
  }
  pub fn dropdown_pick(&mut self) {
      let Some(hl) = self.dropdown else { return };
      let i = self.focus;
      if let Some(opt) = self.args.get(i).and_then(|a| a.options.as_ref()).and_then(|o| o.get(hl)).cloned() {
          self.values[i] = opt;
          if self.error == Some(i) {
              self.error = None;
          }
      }
      self.dropdown = None;
  }
  ```
- [ ] **Step 9: Run (expect PASS).** `cargo test -p qoo-tui args_form`.
- [ ] **Step 10: Commit.** `git add crates/qoo-tui/src/view/args_form.rs` · `git commit -m "feat(tui-rs): ArgsForm text edit, positional validation, dropdown state"`

---

### Task 20: ArgsForm view + dropdown popup + keys/mouse

Renders the form inside `modal_frame`, draws the dropdown popup over it, and wires `Mode::DefArgs` key/mouse handling to the Task 19 methods. Layout mirrors `ArgsForm.tsx`: label col = longest arg name; value cell (cursor block on text, `‹value›` chip on enums, dimmed on fixed); hint col = `min(width/2, 40)` showing `opt1 | opt2 | opt3 — description` (enum) or description (text), red ` required` on the error row. Submit dispatches `runDefinition` and invalidates defs (mirrors `App.tsx` def-args submit ~L1260).

**Files:**
- Modify: `crates/qoo-tui/src/view/args_form.rs` (`render_args_form` replaces the Task 18 stub, `render_dropdown`)
- Modify: `crates/qoo-tui/src/app.rs` (`Mode::DefArgs` key + mouse handling)
- Test: inline `#[cfg(test)] mod tests` in `app.rs`; snapshots in `crates/qoo-tui/tests/args_form_snapshot.rs`

**Interfaces:**
- Consumes: `ArgsForm` methods (Task 19), `modal_frame`, `Palette`, `crate::hit::{HitMap, HitTarget}` (`FormField`, `DropdownItem`, `Button(ButtonKind)`, `Modal`), `App::run_definition_cmd`.
- Produces: `pub fn render_args_form(frame, hit, p, form)`, `fn render_dropdown(...)`; `App::def_args_key`, `App::def_args_click`.
- Key contract (dropdown **closed**): `Tab`/`↓` → `next_focus`; `Shift+Tab`/`↑` → `prev_focus`; `←`/`→` → `cycle_option` (enum focus); `Enter` → enum focus opens dropdown, else `validate` + submit; `Esc` → cancel; printable → `input_char`; `Backspace` → `backspace`. Dropdown **open**: `↑`/`↓` → `dropdown_move`; `Enter` → `dropdown_pick`; `Esc` → `close_dropdown`.

**Steps:**

- [ ] **Step 1: Failing snapshots — form + open dropdown.** Create `crates/qoo-tui/tests/args_form_snapshot.rs`:
  ```rust
  use qoo_tui::app::{App, Mode};
  use qoo_tui::ipc::types::ArgSpec;
  use qoo_tui::view::args_form::ArgsForm;
  use ratatui::{Terminal, backend::TestBackend};
  use std::collections::HashMap;

  fn form_app() -> App {
      let mut app = App::new("/tmp/runs".into(), "/tmp/d.sock".into());
      app.size = (80, 24);
      let args = vec![
          ArgSpec { name: "pr".into(), default: None, options: None, description: Some("PR number".into()) },
          ArgSpec { name: "mode".into(), default: Some("ready".into()), options: Some(vec!["ready".into(), "create".into()]), description: None },
          ArgSpec { name: "source".into(), default: None, options: None, description: None },
      ];
      app.mode = Mode::DefArgs {
          form: ArgsForm::new("platform".into(), "pr-ready".into(), args, HashMap::from([("source".into(), "wt-a".into())]), HashMap::new(), None),
      };
      app
  }

  #[test]
  fn args_form_snapshot() {
      let app = form_app();
      let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
      term.draw(|f| { qoo_tui::view::render(&app, f); }).unwrap();
      insta::assert_snapshot!(term.backend());
  }

  #[test]
  fn args_form_open_dropdown_snapshot() {
      let mut app = form_app();
      if let Mode::DefArgs { form } = &mut app.mode {
          form.open_dropdown(1); // open the enum dropdown
      }
      let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
      term.draw(|f| { qoo_tui::view::render(&app, f); }).unwrap();
      insta::assert_snapshot!(term.backend());
  }
  ```
- [ ] **Step 2: Run (expect FAIL).** `cargo test -p qoo-tui --test args_form_snapshot` — stub render draws nothing.
- [ ] **Step 3: Implement `render_args_form` + `render_dropdown`.** Replace the Task 18 stub in `args_form.rs`:
  ```rust
  use ratatui::Frame;
  use ratatui::layout::Rect;
  use ratatui::style::{Modifier, Style};
  use ratatui::text::{Line, Span};
  use ratatui::widgets::{Block, Borders, Clear};

  use crate::hit::{ButtonKind, HitMap, HitTarget};
  use crate::view::modal::modal_frame;
  use crate::view::theme::Palette;

  fn pad(s: &str, width: usize) -> String {
      let mut out: String = s.chars().take(width).collect();
      while out.chars().count() < width {
          out.push(' ');
      }
      out
  }

  /// Dimmed hint for a row: `opt1 | opt2 — description` (enum) or the description.
  fn row_hint(arg: &crate::ipc::types::ArgSpec) -> String {
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

  pub fn render_args_form(frame: &mut Frame, hit: &mut HitMap, p: &Palette, form: &ArgsForm) {
      let title = format!("{} args", form.def_name);
      let inner: Rect = modal_frame(frame, &title, "tab/↓ next · ←/→ cycle · enter run · esc cancel", hit);
      let width = inner.width as usize;
      let hint_col = (width / 2).min(40);
      let main_col = width.saturating_sub(hint_col).max(1);
      let label_w = form.args.iter().map(|a| a.name.chars().count()).max().unwrap_or(0);

      for (i, arg) in form.args.iter().enumerate() {
          if i as u16 >= inner.height.saturating_sub(2) {
              break;
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
              Style::default().fg(p.text)
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

      // [ Run ] [ Cancel ] under the rows.
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

  /// Option-list popup anchored under the focused enum row.
  fn render_dropdown(frame: &mut Frame, hit: &mut HitMap, p: &Palette, form: &ArgsForm, inner: Rect, label_w: usize, main_col: usize) {
      let Some(hl) = form.dropdown else { return };
      let Some(opts) = form.args.get(form.focus).and_then(|a| a.options.as_ref()) else { return };
      let x = inner.x + 1 + label_w as u16 + 2;
      let y = inner.y + form.focus as u16 + 1;
      let w = (main_col as u16).min(inner.width.saturating_sub(3)).max(6);
      let h = (opts.len() as u16 + 2).min(inner.height.saturating_sub(form.focus as u16 + 1)).max(3);
      let area = Rect { x, y, width: w, height: h };
      frame.render_widget(Clear, area);
      frame.render_widget(Block::default().borders(Borders::ALL).border_style(Style::default().fg(p.accent)), area);
      for (i, opt) in opts.iter().enumerate() {
          if i as u16 + 1 >= h.saturating_sub(1) {
              break;
          }
          let row = Rect { x: x + 1, y: y + 1 + i as u16, width: w.saturating_sub(2), height: 1 };
          let style = if i == hl {
              Style::default().add_modifier(Modifier::REVERSED)
          } else {
              Style::default().fg(p.text)
          };
          frame.render_widget(Line::from(Span::styled(pad(&format!(" {opt}"), row.width as usize), style)), row);
          hit.push(row, HitTarget::DropdownItem(i));
      }
  }
  ```
- [ ] **Step 4: Run + review snapshots (expect PASS after accept).** `cargo test -p qoo-tui --test args_form_snapshot`; `cargo insta review` (form shows `pr>` cursor, `mode> ‹ready›`, `source> wt-a` dimmed, `PR number`/`ready | create` hints, `[ Run ] [ Cancel ]`; second snapshot shows the open dropdown listing `ready`/`create`).
- [ ] **Step 5: Commit.** `git add crates/qoo-tui/src/view/args_form.rs crates/qoo-tui/tests/args_form_snapshot.rs crates/qoo-tui/tests/snapshots/` · `git commit -m "feat(tui-rs): render args form + dropdown popup with hit targets"`

- [ ] **Step 6: Failing tests — key flow through `update()`.** In `app.rs` tests (reuse the `key`/modifier helpers):
  ```rust
  fn shift(code: crossterm::event::KeyCode) -> Event {
      Event::Key(crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::SHIFT))
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
      // Now submit via the Run button click.
      // (fields tested separately; enum-only forms submit through the button)
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
  ```
- [ ] **Step 7: Run (expect FAIL).** `cargo test -p qoo-tui def_args`.
- [ ] **Step 8: Implement `Mode::DefArgs` key + mouse handling.** In `update()`, add a `Mode::DefArgs` arm delegating to:
  ```rust
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

  /// Validate and dispatch, or keep the form open on the first missing field.
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

  /// Mouse targets while the form is open (from Task 12's HitTarget routing).
  fn def_args_click(&mut self, target: &crate::hit::HitTarget) -> Update {
      use crate::hit::{ButtonKind, HitTarget};
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
          HitTarget::Button(ButtonKind::Confirm) => self.submit_def_args(),
          HitTarget::Button(ButtonKind::Cancel) | HitTarget::Modal => {
              self.mode = Mode::List;
              Update { dirty: true, cmds: vec![] }
          }
          _ => Update { dirty: false, cmds: vec![] },
      }
  }
  ```
  Route `Event::Key`→`def_args_key`, `Event::Mouse`(down)→resolve hit→`def_args_click` inside the `Mode::DefArgs` arm.
- [ ] **Step 9: Run (expect PASS).** `cargo test -p qoo-tui def_args`.
- [ ] **Step 10: Failing test — click hit-target routing.** In `app.rs` tests, render the form to a `TestBackend`, capture the `HitMap`, then assert a click on a form-field rect focuses it and a click on the enum field opens the dropdown; a click on `[ Run ]` submits:
  ```rust
  #[test]
  fn def_args_click_focuses_field_and_run_submits() {
      use crate::hit::{ButtonKind, HitTarget};
      let mut app = def_args_app(vec![ArgSpec { name: "pr".into(), default: None, options: None, description: None }], HashMap::new(), None);
      app.def_args_click(&HitTarget::FormField(0));
      if let Mode::DefArgs { form } = &app.mode { assert_eq!(form.focus, 0); }
      // fill then Run
      app.update(Event::Key(crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Char('7'), crossterm::event::KeyModifiers::NONE)));
      let update = app.def_args_click(&HitTarget::Button(ButtonKind::Confirm));
      assert!(matches!(update.cmds[0], Cmd::Rpc { .. }));
      assert!(matches!(app.mode, Mode::List));
  }
  ```
- [ ] **Step 11: Run (expect PASS).** `cargo test -p qoo-tui def_args_click`.
- [ ] **Step 12: Commit.** `git add crates/qoo-tui/src/app.rs` · `git commit -m "feat(tui-rs): def-args key + mouse handling, submit dispatches runDefinition"`

---

### Task 21: create-worktree, squash-merge, remove polish, tmux verify

Adds `validate_branch` (contract addition in `worktree_context.rs`, ported from `branch.ts`), the `Mode::CreateWorktree` flow (replaces Task 14's `CreateWorktree` stub; also entered via `c` in the worktrees pane), the `SquashMerge` flow (replaces the stub), a regression test that a busy worktree's Remove menu row is disabled (the `ConfirmRemove` flow itself landed in Task 14), and the M3 manual verification checklist.

**Files:**
- Modify: `crates/qoo-tui/src/worktree_context.rs` (`validate_branch`)
- Modify: `crates/qoo-tui/src/app.rs` (`Mode::CreateWorktree` handling, `CreateWorktree`/`SquashMerge` arms, `create_worktree_cmd`)
- Modify: `crates/qoo-tui/src/view/modal.rs` (`render_create_worktree` — bordered input + inline error), `crates/qoo-tui/src/view/mod.rs` (dispatch)
- Test: inline `#[cfg(test)] mod tests` in `worktree_context.rs` and `app.rs`
- Modify: `docs/superpowers/plans/2026-07-09-ratatui-tui-rewrite.md` (append the M3 manual-verification checklist)

**Interfaces:**
- **Contract addition:** `pub fn validate_branch(name: &str) -> Option<String>` in `worktree_context.rs` — `Some(first-failing message)` or `None` when git-ref-safe. (Rust uses `Option<String>` where TS returns `string | null`.)
- Consumes: `Cmd::Rpc` (createWorktree, timeout 600000ms, `timeout_is_ok: false`), `crate::worktree_context::context_arg_values`, `App::run_definition_cmd`/`open_def_args`, `ArgsForm` (Tasks 18/19), `tui_input::Input`.
- Produces: `App::create_worktree_cmd`, `Mode::CreateWorktree` handling, `SquashMerge` arm.

**Steps:**

- [ ] **Step 1: Failing tests — `validate_branch` (mirror `branch.test.ts`).** In `worktree_context.rs` tests:
  ```rust
  #[test]
  fn validate_branch_table() {
      assert_eq!(validate_branch("feature-x"), None);
      assert_eq!(validate_branch("JUS-1423/fix-auth"), None);
      assert!(validate_branch("").unwrap().contains("required"));
      assert!(validate_branch("fix login").unwrap().contains("whitespace"));
      assert!(validate_branch("fix\tlogin").unwrap().contains("whitespace"));
      assert!(validate_branch("fix..auth").unwrap().contains(".."));
      assert!(validate_branch("-fix").unwrap().contains("start"));
      assert!(validate_branch("/fix").unwrap().contains("start"));
      assert!(validate_branch("fix.lock").unwrap().contains(".lock"));
      assert!(validate_branch("fix\u{1}").unwrap().contains("printable ASCII"));
      assert!(validate_branch("fïx").unwrap().contains("printable ASCII"));
  }
  ```
- [ ] **Step 2: Run (expect FAIL).** `cargo test -p qoo-tui validate_branch`.
- [ ] **Step 3: Implement `validate_branch`.** Append to `worktree_context.rs` (checks in message-surfacing order, per `branch.ts`):
  ```rust
  /// Validate a branch name for the create-worktree modal: `Some(message)` when
  /// not git-ref-safe, `None` when acceptable. Order: non-empty, no whitespace,
  /// no `..`, no leading `-`/`/`, no trailing `.lock`, printable ASCII only.
  pub fn validate_branch(name: &str) -> Option<String> {
      if name.is_empty() {
          return Some("branch name required".into());
      }
      if name.chars().any(|c| c.is_whitespace()) {
          return Some("no whitespace allowed".into());
      }
      if name.contains("..") {
          return Some("no '..' allowed".into());
      }
      if name.starts_with('-') || name.starts_with('/') {
          return Some("cannot start with '-' or '/'".into());
      }
      if name.ends_with(".lock") {
          return Some("cannot end with '.lock'".into());
      }
      if name.chars().any(|c| !('\u{20}'..='\u{7e}').contains(&c)) {
          return Some("printable ASCII only".into());
      }
      None
  }
  ```
- [ ] **Step 4: Run (expect PASS).** `cargo test -p qoo-tui validate_branch`.
- [ ] **Step 5: Commit.** `git add crates/qoo-tui/src/worktree_context.rs` · `git commit -m "feat(tui-rs): port branch-name validation for create-worktree"`

- [ ] **Step 6: Failing tests — create-worktree flow.** In `app.rs` tests:
  ```rust
  #[test]
  fn create_worktree_entered_by_c_in_worktrees_pane() {
      let mut app = fixture_app_worktrees_focused("platform"); // worktrees pane focused
      app.update(key(crossterm::event::KeyCode::Char('c')));
      assert!(matches!(app.mode, Mode::CreateWorktree { .. }));
  }

  #[test]
  fn create_worktree_invalid_stays_open_with_error() {
      let mut app = fixture_create_worktree("platform");
      for c in "bad name".chars() { app.update(key(crossterm::event::KeyCode::Char(c))); }
      let update = app.update(key(crossterm::event::KeyCode::Enter));
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
      for c in "feature-x".chars() { app.update(key(crossterm::event::KeyCode::Char(c))); }
      let update = app.update(key(crossterm::event::KeyCode::Enter));
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
  ```
- [ ] **Step 7: Run (expect FAIL).** `cargo test -p qoo-tui create_worktree`.
- [ ] **Step 8: Implement create-worktree.** In `app.rs`:
  - `create_worktree_cmd`:
    ```rust
    fn create_worktree_cmd(repo: &str, name: &str) -> Cmd {
        Cmd::Rpc {
            label: format!("create worktree {name}"), // executor formats "<label>: <error>" on failure
            call: RpcCall {
                method: "createWorktree".into(),
                params: serde_json::json!({ "repo": repo, "name": name }),
            },
            timeout_ms: 600_000, // wt.toml post-create hooks routinely take minutes
            timeout_is_ok: false,
            invalidate_defs_for: None,
        }
    }
    ```
  - Enter into the mode: in the worktrees-pane `AppAction::Create` handler (Task 14) and the `MenuAction::CreateWorktree` arm (replace stub), set `self.mode = Mode::CreateWorktree { input: tui_input::Input::default(), error: None }`.
  - `Mode::CreateWorktree` key handling:
    ```rust
    fn create_worktree_key(&mut self, ev: &crossterm::event::KeyEvent) -> Update {
        use crossterm::event::KeyCode::*;
        let repo = match self.active_repo_name() { Some(r) => r, None => return Update { dirty: false, cmds: vec![] } };
        let Mode::CreateWorktree { input, error } = &mut self.mode else {
            return Update { dirty: false, cmds: vec![] };
        };
        match ev.code {
            Esc => { self.mode = Mode::List; Update { dirty: true, cmds: vec![] } }
            Enter => {
                let name = input.value().to_string();
                if let Some(msg) = crate::worktree_context::validate_branch(&name) {
                    *error = Some(msg);
                    return Update { dirty: true, cmds: vec![] };
                }
                // Close immediately — creation can take minutes; progress + result
                // live on the status line, not a blocked modal.
                self.mode = Mode::List;
                self.status_line = Some(format!("creating worktree {name}…"));
                Update { dirty: true, cmds: vec![Self::create_worktree_cmd(&repo, &name)] }
            }
            _ => {
                // Feed the key to tui-input; ignore SGR mouse noise (Task 12 filters mouse events before this).
                input.handle_event(&crossterm::event::Event::Key(*ev));
                *error = None;
                Update { dirty: true, cmds: vec![] }
            }
        }
    }
    ```
    (`invalidate_defs_for: None`; the ActionResult status "create worktree <name>: <err>" comes from the executor's `<label>: <error>` format — verify against Task 13; if the executor emits only the raw error, prepend the label here instead.)
- [ ] **Step 9: Run (expect PASS).** `cargo test -p qoo-tui create_worktree`.
- [ ] **Step 10: Commit.** `git add crates/qoo-tui/src/app.rs` · `git commit -m "feat(tui-rs): create-worktree flow — validate, dispatch async, close"`

- [ ] **Step 11: Failing snapshot + error render — create-worktree modal.** Add `crates/qoo-tui/tests/create_worktree_snapshot.rs`:
  ```rust
  use qoo_tui::app::{App, Mode};
  use ratatui::{Terminal, backend::TestBackend};

  #[test]
  fn create_worktree_modal_with_error_snapshot() {
      let mut app = App::new("/tmp/runs".into(), "/tmp/d.sock".into());
      app.size = (80, 24);
      // seed a snapshot so active_repo_name() yields "platform" (reuse M1 fixture builder)
      app.snapshot = Some(qoo_tui::testkit::snapshot_one_project("platform"));
      let mut input = tui_input::Input::default();
      input = input.with_value("bad name".into());
      app.mode = Mode::CreateWorktree { input, error: Some("no whitespace allowed".into()) };
      let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
      term.draw(|f| { qoo_tui::view::render(&app, f); }).unwrap();
      insta::assert_snapshot!(term.backend());
  }
  ```
- [ ] **Step 12: Run (expect FAIL).** `cargo test -p qoo-tui --test create_worktree_snapshot`.
- [ ] **Step 13: Implement `render_create_worktree` + dispatch.** In `view/modal.rs`:
  ```rust
  pub fn render_create_worktree(frame: &mut Frame, hit: &mut HitMap, p: &Palette, repo: &str, input: &tui_input::Input, error: Option<&str>) {
      let inner = modal_frame(frame, &format!("Create worktree — {repo}"), "enter submit · esc cancel", hit);
      let row = Rect { x: inner.x, y: inner.y, width: inner.width, height: 1 };
      let text = format!(" branch> {}█", input.value());
      frame.render_widget(Line::from(Span::styled(text, Style::default().fg(p.text))), row);
      if let Some(msg) = error {
          let er = Rect { x: inner.x, y: inner.y + 1, width: inner.width, height: 1 };
          frame.render_widget(Line::from(Span::styled(format!(" {msg}"), Style::default().fg(p.error))), er);
      }
  }
  ```
  Dispatch in `view/mod.rs`: `Mode::CreateWorktree { input, error } => modal::render_create_worktree(frame, &mut hit, &palette, &app.active_repo_name().unwrap_or_default(), input, error.as_deref())`.
- [ ] **Step 14: Run + review snapshot (expect PASS).** `cargo test -p qoo-tui --test create_worktree_snapshot`; `cargo insta review` (shows `branch> bad name█` and red `no whitespace allowed`).
- [ ] **Step 15: Commit.** `git add crates/qoo-tui/src/view/modal.rs crates/qoo-tui/src/view/mod.rs crates/qoo-tui/tests/create_worktree_snapshot.rs crates/qoo-tui/tests/snapshots/` · `git commit -m "feat(tui-rs): render create-worktree modal with inline error"`

- [ ] **Step 16: Failing tests — squash-merge flow.** In `app.rs` tests:
  ```rust
  #[test]
  fn squash_merge_opens_def_args_with_source_fixed_to_branch() {
      let mut app = fixture_app_with_defs("platform", vec![
          DefinitionSummary { repo: "platform".into(), name: "squash-merge".into(), scope: "global".into(),
              args: vec![
                  ArgSpec { name: "source".into(), default: None, options: None, description: None },
                  ArgSpec { name: "target".into(), default: Some("main".into()), options: None, description: None },
              ], has_discovery: false },
      ]);
      let update = app.dispatch_menu_action(MenuAction::SquashMerge { branch: "wt-a".into() });
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
      let update = app.dispatch_menu_action(MenuAction::SquashMerge { branch: "wt-a".into() });
      assert!(update.cmds.is_empty());
      assert!(app.status_line.as_deref().unwrap().contains("squash-merge definition not found"));
      assert!(matches!(app.mode, Mode::List));
  }
  ```
- [ ] **Step 17: Run (expect FAIL).** `cargo test -p qoo-tui squash_merge`.
- [ ] **Step 18: Implement the `SquashMerge` arm.** Replace the Task 14 stub:
  ```rust
  MenuAction::SquashMerge { branch } => {
      let repo = match self.active_repo_name() {
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
      // source is the selected worktree's branch — FIXED, not asked; no worktree
      // override (the def's `worktree: repo` runs in the primary checkout). `target`
      // stays editable (default main). context_arg_values also fixes branch/ticket
      // when the def declares those args.
      let fixed = crate::worktree_context::context_arg_values(&branch);
      self.open_def_args(def.repo, def.name, def.args, fixed, HashMap::new(), None);
      Update { dirty: true, cmds: vec![] }
  }
  ```
  (Note: `App.tsx` re-fetches `definitions()` here so a just-added global def is seen; in the Elm model the cache is already kept fresh by `reconcile_defs`/invalidation, so reading `defs_by_project` is faithful. The disabled-while-busy guard already lives on the menu item — Task 14.)
- [ ] **Step 19: Run (expect PASS).** `cargo test -p qoo-tui squash_merge`.
- [ ] **Step 20: Commit.** `git add crates/qoo-tui/src/app.rs` · `git commit -m "feat(tui-rs): squash-merge opens args form with source fixed to branch"`

- [ ] **Step 21: Failing test — busy worktree Remove row disabled (regression).** In `app.rs` tests (ConfirmRemove flow itself is Task 14; this pins the menu-eligibility rule):
  ```rust
  #[test]
  fn busy_worktree_remove_menu_row_is_disabled() {
      // A worktree with a running task in its lane is busy.
      let mut app = fixture_app_busy_worktree("platform", "wt-a"); // running task on lane platform:wt-a
      app.focus_worktrees();          // select the busy worktree (M1/M2 helper)
      app.open_action_menu();         // Enter/`a`
      let items = match &app.mode {
          Mode::ActionMenu { items, .. } => items.clone(),
          other => panic!("expected ActionMenu, got {other:?}"),
      };
      let remove = items.iter().find(|it| it.label.starts_with("Remove worktree")).expect("remove row present");
      assert_eq!(remove.disabled.as_deref(), Some("a task is running here"));
      // squash-merge is likewise disabled while busy.
      let squash = items.iter().find(|it| it.label.starts_with("Squash merge")).unwrap();
      assert_eq!(squash.disabled.as_deref(), Some("a task is running here"));
  }
  ```
- [ ] **Step 22: Run (expect PASS or FAIL).** `cargo test -p qoo-tui busy_worktree_remove` — should PASS against Task 14's `action_menu` builder (busy → `disabled`); if it FAILs, the eligibility wiring regressed and the fix belongs in `action_menu.rs` (mirror `buildActions` worktree case: `remove-worktree`/`squash-merge` disabled when `busy`).
- [ ] **Step 23: Commit.** `git add crates/qoo-tui/src/app.rs` · `git commit -m "test(tui-rs): pin busy-worktree remove/squash menu rows disabled"`

- [ ] **Step 24: Append the M3 manual-verification checklist.** In `docs/superpowers/plans/2026-07-09-ratatui-tui-rewrite.md`, under Task 21 (or an "M3 verification" note), add — to run against a live daemon (`cargo run -p qoo-tui` with a real `queohoh` daemon + at least one repo with worktrees and a `squash-merge` global def):
  - [ ] TASKS pane → `Enter`/`a` → Run a def **with args** via the ambient path: `source` shows a worktree-branch dropdown, prefilled from the selected worktree; typing/`←`/`→`/dropdown click all work; `Enter` on a text field submits; task appears in the queue.
  - [ ] WORKTREES pane → `Run task definition…` → def-pick shows `(g)`/`⏰` markers and arg summaries; picking a zero-arg def dispatches immediately; picking an arg def opens the form with `source`/`branch`/`ticket` **fixed** from the worktree branch (dimmed, focus skips them).
  - [ ] Dropdown by **mouse**: click an enum field opens the popup; click an option picks it; click outside / `esc` closes only the dropdown; second `esc` cancels the form.
  - [ ] `c` in the worktrees pane and `Create worktree…` both open the create modal; an invalid branch shows the inline error and keeps the input; a valid branch closes immediately, shows `creating worktree <name>…`, and the new worktree lands (status clears / errors to the status line).
  - [ ] Remove a worktree via `Remove worktree…` → confirm; a busy worktree's Remove/Squash rows are disabled with a reason.
  - [ ] `Squash merge into…` opens the form with `source` fixed to the branch and `target` editable (default `main`); submitting runs the squash task in the primary checkout.
  - [ ] tmux: with `$TMUX` set, `Open in tmux window` opens a new window at the worktree path; **outside** tmux the row is disabled with `not inside tmux`.
- [ ] **Step 25: Verify green + commit.** `cargo test -p qoo-tui` (all M3 tests pass) and `cargo build --release`. `git add docs/superpowers/plans/2026-07-09-ratatui-tui-rewrite.md` · `git commit -m "docs(tui-rs): M3 manual verification checklist"`

---
## M3 — Manual Verification Checklist

Run against a live daemon: `cargo run -p qoo-tui` with a real `queohoh` daemon +
at least one repo with worktrees and a `squash-merge` global def. Each step
below has automated coverage (unit/snapshot tests noted); this live pass confirms
the wiring against a real TTY + daemon and is **deferred to the Task 23 full
manual parity pass** (a live TTY is not available in the implementation
environment — same deferral pattern as the Task 12 M1 checklist). The non-live
behaviors were smoke-covered by `cargo test -p qoo-tui` (232 tests green,
including `task21_tests` and the create-worktree snapshot).

- [ ] TASKS pane → `Enter`/`a` → Run a def **with args** via the ambient path: `source` shows a worktree-branch dropdown, prefilled from the selected worktree; typing/`←`/`→`/dropdown click all work; `Enter` on a text field submits; task appears in the queue. _(auto: `run_named_def_with_args_opens_def_args_with_ambient_overlay`, `def_args_*`)_
- [ ] WORKTREES pane → `Run task definition…` → def-pick shows `(g)`/`⏰` markers and arg summaries; picking a zero-arg def dispatches immediately; picking an arg def opens the form with `source`/`branch`/`ticket` **fixed** from the worktree branch (dimmed, focus skips them). _(auto: `run_def_opens_def_pick_in_server_order`, `def_pick_enter_with_args_opens_def_args_with_fixed_context`)_
- [ ] Dropdown by **mouse**: click an enum field opens the popup; click an option picks it; click outside / `esc` closes only the dropdown; second `esc` cancels the form. _(auto: `def_args_click_focuses_field_and_run_submits`, `def_args_esc_closes_dropdown_then_cancels`)_
- [ ] `c` in the worktrees pane and `Create worktree…` both open the create modal; an invalid branch shows the inline error and keeps the input; a valid branch closes immediately, shows `creating worktree <name>…`, and the new worktree lands (status clears / errors to the status line). _(auto: `create_worktree_entered_by_c_in_worktrees_pane`, `create_worktree_invalid_stays_open_with_error`, `create_worktree_valid_dispatches_and_closes_immediately`, `create_worktree_modal_with_error_snapshot`)_
- [ ] Remove a worktree via `Remove worktree…` → confirm; a busy worktree's Remove/Squash rows are disabled with a reason. _(auto: `busy_worktree_remove_menu_row_is_disabled`, `worktree_menu_busy_disables_remove_and_squash_create_stays`)_
- [ ] `Squash merge into…` opens the form with `source` fixed to the branch and `target` editable (default `main`); submitting runs the squash task in the primary checkout. _(auto: `squash_merge_opens_def_args_with_source_fixed_to_branch`, `squash_merge_absent_def_sets_status_line`)_
- [ ] tmux: with `$TMUX` set, `Open in tmux window` opens a new window at the worktree path; **outside** tmux the row is disabled with `not inside tmux`. _(auto: `worktree_menu_outside_tmux_disables_open`; the `tmux new-window -c` executor arm landed in Task 4)_

---
## Milestone 4 — Self-heal & cutover (Tasks 22–24)

Milestone 4 finishes the rewrite: Task 22 ports the daemon self-heal (`heal.ts` → `heal.rs`) and wires `Cmd::Heal`; Task 23 runs the full parity gate and flips `mise run tui` to the Rust binary while keeping the Ink TUI as `tui:ink` for the bake; Task 24 (user-gated) deletes `packages/tui` once the Rust TUI is trusted.

---

### Task 22: heal.rs

Port the pure self-heal decision + orchestration from `packages/tui/src/heal.ts` and `packages/daemon/src/build-id.ts`, then wire it into `App::update` on every `Event::Snapshot` and implement the `Cmd::Heal` executor arm (currently a no-op left by Task 4). The daemon's build fingerprint (`snapshot.build_id`) is compared to the newest `.js` mtime on disk; when the daemon is stale it is restarted — deferred while a task runs, fired when idle, guarded against restart loops.

**Files:**

- **Create:** `crates/qoo-tui/src/heal.rs`
- **Modify:** `crates/qoo-tui/src/lib.rs` (add `pub mod heal;`)
- **Modify:** `crates/qoo-tui/src/app.rs` (App private fields `healing` + `heal_status_shown`; init in `App::new`; `heal_on_snapshot` + `set_heal_status` helpers; `Event::Snapshot` arm append; `Event::ActionResult` arm heal-reset; `#[cfg(test)]` update() transition tests)
- **Modify:** `crates/qoo-tui/src/event.rs` (replace the `Cmd::Heal => {}` no-op executor arm)
- **Modify:** `crates/qoo-tui/Cargo.toml` (ensure `tempfile` under `[dev-dependencies]`; no runtime dep added — SIGTERM fallback shells out to `kill`)

**Interfaces:**

Consumes (verbatim from contract):
- `crate::ipc::client::RpcClient::{connect, call}` — `call(&mut self, method: &str, params: serde_json::Value, timeout: Duration) -> Result<serde_json::Value, String>`
- `crate::paths::{state_path, socket_path, pid_path, daemon_dist_dir, daemon_cli_path}`
- `crate::ipc::types::StateSnapshot` (`.build_id: Option<String>`, `.running: Vec<String>`)
- `crate::event::{Event::{Snapshot, ActionResult}, Cmd::Heal}`
- `crate::app::App` (`.status_line: Option<String>`, `.last_healed_build_id: Option<String>`)

Produces (contract shapes, verbatim):
- `pub enum HealDecision { None, Defer, RestartNow }`
- `pub fn decide_heal(snapshot_build_id: Option<&str>, disk_build_id: &str, running: usize, last_healed: Option<&str>) -> HealDecision`
- `pub fn disk_build_id(dist_dir: &Path) -> String`
- `pub async fn perform_heal(sock: &Path, pid_file: &Path, daemon_cli: &Path) -> Result<(), String>`

**Contract additions** (called out per Global Constraints — additive, backward-compatible):
- `heal.rs` adds `pub fn is_stale(snapshot_build_id: Option<&str>, disk_build_id: &str) -> bool` (mirrors `isStale`), `pub fn classify_shutdown_error(err: &str) -> ShutdownOutcome` (pure split-out so the shutdown branch is unit-testable), and `pub enum ShutdownOutcome { Busy, Fallback }`.
- `HealDecision` derives `Debug, Clone, Copy, PartialEq, Eq`.
- `App` gains two **private** fields `healing: bool` and `heal_status_shown: bool` (mirror `heal.ts` App effect's `healing`/`healStatusShown` refs). Public field shapes are unchanged.

**Parity note — disk_build_id float formatting.** `build-id.ts` fingerprints the build as `String(mtimeMs)`, where Node's `mtimeMs` is an IEEE-754 `f64` (`sec*1000 + nsec/1e6`, usually fractional). The daemon computes its own `buildId` this way and the TUI must produce the byte-identical string or the daemon looks perpetually stale. `disk_build_id` therefore computes the same `f64` (`secs as f64 * 1000.0 + subsec_nanos as f64 / 1_000_000.0`) and formats it with Rust's default `f64` `Display`, which — like V8's `Number.toString` — emits the unique shortest round-trip decimal for the same bit pattern. No `.js` files (or a read error) → `"0"`, matching `currentBuildId`. Only `.js` siblings count.

**Wiring note — no disk-id cache.** `heal.ts` recomputes `currentBuildId()` on *every* snapshot effect. We do the same (a `read_dir` over a handful of files, run only on mutation-driven snapshots, never in a hot loop). Caching the disk id keyed on `snapshot.build_id` would break the "re-heals when a fresh build lands after a prior attempt" case (snapshot stays `100` while disk advances `200 → 300`), so `heal_on_snapshot` reads the dir each snapshot.

- [ ] **Step 1: Failing pure-function tests (heal.rs).** Create `crates/qoo-tui/src/heal.rs` with the full test module below and only stub signatures (`unimplemented!()`) so it compiles-and-fails. Add `pub mod heal;` to `lib.rs`.

  ```rust
  //! Daemon self-heal: pure decision + async orchestration.
  //! Port of packages/tui/src/heal.ts + packages/daemon/src/build-id.ts (parity oracle:
  //! packages/tui/src/__tests__/heal.test.ts).

  use std::path::Path;
  use std::process::Stdio;
  use std::time::Duration;

  use crate::ipc::client::RpcClient;

  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum HealDecision {
      None,
      Defer,
      RestartNow,
  }

  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum ShutdownOutcome {
      Busy,
      Fallback,
  }

  /// True when the daemon's reported build differs from disk. A pre-feature daemon
  /// sends `None`, which is always stale — it definitionally predates the field.
  pub fn is_stale(snapshot_build_id: Option<&str>, disk_build_id: &str) -> bool {
      snapshot_build_id != Some(disk_build_id)
  }

  /// Pure self-heal decision (mirrors decideHeal). The loop guard keys on the
  /// *target* disk build: once we attempt to reach build X we won't retry X, but a
  /// fresh build (disk moves to Y) is a new mismatch worth healing.
  pub fn decide_heal(
      snapshot_build_id: Option<&str>,
      disk_build_id: &str,
      running: usize,
      last_healed: Option<&str>,
  ) -> HealDecision {
      if !is_stale(snapshot_build_id, disk_build_id) {
          return HealDecision::None;
      }
      if last_healed == Some(disk_build_id) {
          return HealDecision::None; // already tried this build
      }
      if running > 0 {
          return HealDecision::Defer;
      }
      HealDecision::RestartNow
  }

  /// Build fingerprint: newest `.js` mtime (ms) in `dist_dir` as a string, `"0"`
  /// when the dir holds no `.js` files or can't be read. Mirrors currentBuildId.
  pub fn disk_build_id(dist_dir: &Path) -> String {
      let entries = match std::fs::read_dir(dist_dir) {
          Ok(e) => e,
          Err(_) => return "0".to_string(),
      };
      let mut newest: f64 = 0.0;
      for entry in entries.flatten() {
          let path = entry.path();
          if path.extension().and_then(|s| s.to_str()) != Some("js") {
              continue;
          }
          let Ok(meta) = entry.metadata() else { continue };
          let Ok(modified) = meta.modified() else { continue };
          let Ok(dur) = modified.duration_since(std::time::UNIX_EPOCH) else {
              continue;
          };
          // Same decomposition as Node's mtimeMs: sec*1000 + nsec/1e6, as f64.
          let ms = dur.as_secs() as f64 * 1000.0 + dur.subsec_nanos() as f64 / 1_000_000.0;
          if ms > newest {
              newest = ms;
          }
      }
      // Rust f64 Display == V8 Number.toString (unique shortest round-trip).
      newest.to_string()
  }

  /// Classify a failed `shutdown` RPC. "busy" → a task raced in after our idle
  /// check; respect the guard and abort. Anything else (unknown-method on an old
  /// daemon, or unreachable) → fall back to the pidfile SIGTERM path.
  pub fn classify_shutdown_error(err: &str) -> ShutdownOutcome {
      if err.contains("busy") {
          ShutdownOutcome::Busy
      } else {
          ShutdownOutcome::Fallback
      }
  }

  async fn socket_answers(sock: &Path) -> bool {
      match RpcClient::connect(sock).await {
          Ok(mut c) => matches!(
              c.call("ping", serde_json::Value::Null, Duration::from_millis(500)).await,
              Ok(v) if v == serde_json::json!("pong")
          ),
          Err(_) => false,
      }
  }

  /// Ask the daemon to shut down (SIGTERM fallback for an old daemon that lacks the
  /// RPC), wait for the socket to go quiet, then spawn a fresh detached daemon. The
  /// reconnect loop picks the new one up. Returns `Err("daemon busy — restart
  /// deferred")` without spawning when the daemon reports it is busy — we never
  /// force-kill a daemon with a task in flight.
  pub async fn perform_heal(sock: &Path, pid_file: &Path, daemon_cli: &Path) -> Result<(), String> {
      unimplemented!()
  }

  #[cfg(test)]
  mod tests {
      use super::*;

      // ---- is_stale (mirrors heal.test.ts `describe("isStale")`) ----
      #[test]
      fn is_stale_false_when_matching() {
          assert!(!is_stale(Some("100"), "100"));
      }
      #[test]
      fn is_stale_true_when_differing() {
          assert!(is_stale(Some("100"), "200"));
      }
      #[test]
      fn is_stale_true_for_pre_feature_daemon() {
          assert!(is_stale(None, "200"));
      }

      // ---- decide_heal (mirrors heal.test.ts `describe("decideHeal")`) ----
      #[test]
      fn decide_none_when_up_to_date() {
          assert_eq!(decide_heal(Some("100"), "100", 0, None), HealDecision::None);
      }
      #[test]
      fn decide_restart_now_when_stale_and_idle() {
          assert_eq!(decide_heal(Some("100"), "200", 0, None), HealDecision::RestartNow);
      }
      #[test]
      fn decide_defer_when_stale_but_running() {
          assert_eq!(decide_heal(Some("100"), "200", 1, None), HealDecision::Defer);
      }
      #[test]
      fn decide_restart_now_for_pre_feature_daemon_when_idle() {
          assert_eq!(decide_heal(None, "200", 0, None), HealDecision::RestartNow);
      }
      #[test]
      fn decide_none_loop_guard_already_attempted() {
          assert_eq!(decide_heal(Some("100"), "200", 0, Some("200")), HealDecision::None);
      }
      #[test]
      fn decide_restart_now_when_fresh_build_lands() {
          assert_eq!(decide_heal(Some("100"), "300", 0, Some("200")), HealDecision::RestartNow);
      }
      #[test]
      fn decide_none_loop_guard_wins_over_running() {
          assert_eq!(decide_heal(Some("100"), "200", 2, Some("200")), HealDecision::None);
      }

      // ---- disk_build_id ----
      #[test]
      fn disk_build_id_zero_when_no_js() {
          let d = tempfile::tempdir().unwrap();
          std::fs::write(d.path().join("readme.txt"), "x").unwrap();
          assert_eq!(disk_build_id(d.path()), "0");
      }
      #[test]
      fn disk_build_id_zero_when_dir_missing() {
          assert_eq!(disk_build_id(Path::new("/no/such/dir/qoo-xyz")), "0");
      }
      #[test]
      fn disk_build_id_picks_newest_js_ignoring_non_js() {
          let d = tempfile::tempdir().unwrap();
          std::fs::write(d.path().join("a.js"), "a").unwrap();
          std::thread::sleep(Duration::from_millis(25));
          std::fs::write(d.path().join("b.js"), "b").unwrap();
          // A newer *non*-.js sibling must be ignored.
          std::thread::sleep(Duration::from_millis(25));
          std::fs::write(d.path().join("c.ts"), "c").unwrap();

          let m = std::fs::metadata(d.path().join("b.js"))
              .unwrap()
              .modified()
              .unwrap()
              .duration_since(std::time::UNIX_EPOCH)
              .unwrap();
          let expect =
              (m.as_secs() as f64 * 1000.0 + m.subsec_nanos() as f64 / 1_000_000.0).to_string();
          assert_eq!(disk_build_id(d.path()), expect);
      }

      // ---- classify_shutdown_error ----
      #[test]
      fn classify_busy() {
          assert_eq!(classify_shutdown_error("busy: task running"), ShutdownOutcome::Busy);
      }
      #[test]
      fn classify_unknown_method_is_fallback() {
          assert_eq!(classify_shutdown_error("unknown method: shutdown"), ShutdownOutcome::Fallback);
      }
      #[test]
      fn classify_unreachable_is_fallback() {
          assert_eq!(classify_shutdown_error("connection refused"), ShutdownOutcome::Fallback);
      }
  }
  ```

  Ensure `tempfile` is a dev-dependency (add only if absent):

  ```bash
  grep -q '^tempfile' crates/qoo-tui/Cargo.toml || cargo add --dev tempfile -p qoo-tui
  ```

- [ ] **Step 2: Run pure tests — expect FAIL.**

  ```bash
  cargo test -p qoo-tui heal::tests
  ```

  Expected: the `decide_heal` / `is_stale` / `disk_build_id` / `classify_shutdown_error` tests pass against the real impls above, but the crate fails to compile because `perform_heal` bodies call `unimplemented!()` in a non-test path only if referenced — if it compiles, the suite is green except we have not yet exercised `perform_heal`. If `cargo test` reports the module unbuilt (e.g. `lib.rs` missing `pub mod heal;`), that is the expected red. Fix compilation to reach a state where the listed tests are the only work left.

- [ ] **Step 3: Implement `perform_heal` — run pure tests PASS.** Replace the `perform_heal` body:

  ```rust
  pub async fn perform_heal(sock: &Path, pid_file: &Path, daemon_cli: &Path) -> Result<(), String> {
      // 1. Ask the daemon to shut down. Busy → abort (never force-kill live work).
      let mut shutdown_accepted = false;
      if let Ok(mut client) = RpcClient::connect(sock).await {
          match client
              .call("shutdown", serde_json::Value::Null, Duration::from_secs(5))
              .await
          {
              Ok(_) => shutdown_accepted = true,
              Err(e) => match classify_shutdown_error(&e) {
                  ShutdownOutcome::Busy => {
                      return Err("daemon busy — restart deferred".to_string());
                  }
                  // unknown-method (old daemon) or unreachable → pidfile fallback below.
                  ShutdownOutcome::Fallback => {}
              },
          }
      }
      // (connect failure also falls through to the pidfile fallback)

      // 2. SIGTERM fallback for an old daemon that lacks the shutdown RPC. No new
      //    runtime dependency: POSIX `kill` sends SIGTERM. (nix::sys::signal::kill
      //    is an equivalent if the crate is already present.)
      if !shutdown_accepted {
          if let Ok(text) = std::fs::read_to_string(pid_file) {
              if let Ok(pid) = text.trim().parse::<i32>() {
                  if pid > 0 {
                      let _ = tokio::process::Command::new("kill")
                          .arg("-TERM")
                          .arg(pid.to_string())
                          .stdin(Stdio::null())
                          .stdout(Stdio::null())
                          .stderr(Stdio::null())
                          .status()
                          .await;
                  }
              }
          }
      }

      // 3. Poll ping until the old socket stops answering (bounded ~5s).
      let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
      while tokio::time::Instant::now() < deadline {
          if !socket_answers(sock).await {
              break;
          }
          tokio::time::sleep(Duration::from_millis(150)).await;
      }

      // 4. Spawn a fresh detached daemon: `node <daemon_cli> daemon`, own process
      //    group, all stdio to /dev/null. Drop the handle (kill_on_drop is false).
      let mut cmd = tokio::process::Command::new("node");
      cmd.arg(daemon_cli)
          .arg("daemon")
          .stdin(Stdio::null())
          .stdout(Stdio::null())
          .stderr(Stdio::null());
      #[cfg(unix)]
      {
          cmd.process_group(0); // detach from the TUI's group (tokio 1.21+)
      }
      cmd.spawn().map_err(|e| format!("failed to spawn daemon: {e}"))?;
      Ok(())
  }
  ```

  ```bash
  cargo test -p qoo-tui heal::tests
  ```

  Expected: PASS (all pure/decision/fixture tests green). `perform_heal`'s socket + spawn path is intentionally untested here — it drives real sockets and a `node` child, and is covered by the manual self-heal step in Task 23 (mirrors `heal.ts`'s "intentionally untested" orchestration comment).

- [ ] **Step 4: Failing `App::update` heal-transition tests (app.rs).** Add these to the existing `#[cfg(test)] mod tests` in `app.rs` (same module → can read/write the private `healing` / `heal_status_shown` fields). They set `$QUEOHOH_DAEMON_DIST` to a tempdir so `daemon_dist_dir()` resolves a controllable disk build id (env is process-global → serialize with a `Mutex`).

  ```rust
  #[cfg(test)]
  mod heal_wiring_tests {
      use super::*;
      use crate::event::{Cmd, Event};
      use crate::ipc::types::StateSnapshot;
      use std::path::{Path, PathBuf};
      use std::sync::Mutex;

      static ENV_LOCK: Mutex<()> = Mutex::new(());

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
      fn restart_now_sets_status_records_guard_and_emits_heal() {
          let _g = ENV_LOCK.lock().unwrap();
          let dist = dist_with_js();
          std::env::set_var("QUEOHOH_DAEMON_DIST", dist.path());
          let disk = crate::heal::disk_build_id(dist.path());

          let mut app = app();
          let upd = app.update(Event::Snapshot(snap(Some("stale-build"), vec![])));

          assert_eq!(app.status_line.as_deref(), Some("daemon outdated — restarting…"));
          assert_eq!(app.last_healed_build_id.as_deref(), Some(disk.as_str()));
          assert!(upd.cmds.iter().any(|c| matches!(c, Cmd::Heal)));
          std::env::remove_var("QUEOHOH_DAEMON_DIST");
      }

      #[test]
      fn defer_when_task_running_no_heal_cmd() {
          let _g = ENV_LOCK.lock().unwrap();
          let dist = dist_with_js();
          std::env::set_var("QUEOHOH_DAEMON_DIST", dist.path());

          let mut app = app();
          let upd = app.update(Event::Snapshot(snap(Some("stale-build"), vec!["t1".into()])));

          assert_eq!(app.status_line.as_deref(), Some("daemon outdated — will restart when idle"));
          assert!(app.last_healed_build_id.is_none());
          assert!(!upd.cmds.iter().any(|c| matches!(c, Cmd::Heal)));
          std::env::remove_var("QUEOHOH_DAEMON_DIST");
      }

      #[test]
      fn declined_loop_guard_says_restart_manually() {
          let _g = ENV_LOCK.lock().unwrap();
          let dist = dist_with_js();
          std::env::set_var("QUEOHOH_DAEMON_DIST", dist.path());
          let disk = crate::heal::disk_build_id(dist.path());

          let mut app = app();
          app.last_healed_build_id = Some(disk); // already attempted this build
          let upd = app.update(Event::Snapshot(snap(Some("stale-build"), vec![])));

          assert_eq!(app.status_line.as_deref(), Some("daemon still outdated — restart it manually"));
          assert!(!upd.cmds.iter().any(|c| matches!(c, Cmd::Heal)));
          std::env::remove_var("QUEOHOH_DAEMON_DIST");
      }

      #[test]
      fn healthy_snapshot_resets_guard_and_clears_heal_status() {
          let _g = ENV_LOCK.lock().unwrap();
          let dist = dist_with_js();
          std::env::set_var("QUEOHOH_DAEMON_DIST", dist.path());
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
          std::env::remove_var("QUEOHOH_DAEMON_DIST");
      }

      #[test]
      fn action_result_during_heal_resets_healing_and_owns_status() {
          let _g = ENV_LOCK.lock().unwrap();
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
  ```

  ```bash
  cargo test -p qoo-tui heal_wiring_tests
  ```

  Expected: FAIL (helpers/fields/arms not yet present → compile error or wrong status lines).

- [ ] **Step 5: Wire `heal_on_snapshot` + arms into `App` — run wiring tests PASS.**

  In the `App` struct, add the two private fields alongside `last_healed_build_id`:

  ```rust
      // self-heal effect state (mirror heal.ts App refs: `healing`, `healStatusShown`)
      healing: bool,
      heal_status_shown: bool,
  ```

  In `App::new`, initialize them (both `false`):

  ```rust
          healing: false,
          heal_status_shown: false,
  ```

  Add the two helper methods to `impl App`:

  ```rust
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
  ```

  In the `Event::Snapshot(snapshot) =>` arm of `update()`, after it stores `self.snapshot` / `self.connected` and builds its existing `cmds` vector, append the heal commands before returning:

  ```rust
          // (existing) self.snapshot = Some(snapshot); self.connected = true; … let mut cmds = …;
          cmds.extend(self.heal_on_snapshot());
          Update { dirty: true, cmds }
  ```

  In the `Event::ActionResult { status, invalidate_defs_for } =>` arm, after the existing `self.status_line = status;` (and `invalidate_defs_for` handling), add the heal-outcome reset. `healing` is true only while a `Cmd::Heal` is in flight, so it precisely identifies a heal result without a dedicated event:

  ```rust
          // A self-heal reported its outcome (success emits nothing; failure carries
          // "daemon busy — restart deferred"). Clear the in-flight flag and mark the
          // status heal-owned so the next healthy snapshot clears it.
          if self.healing {
              self.healing = false;
              self.heal_status_shown = true;
          }
  ```

  ```bash
  cargo test -p qoo-tui heal_wiring_tests
  ```

  Expected: PASS.

- [ ] **Step 6: Implement the `Cmd::Heal` executor arm (event.rs) — build PASS.** In the command executor (the `match cmd` inside the spawned executor task), replace the `Cmd::Heal => {}` no-op. It resolves the daemon paths from `crate::paths`, runs `perform_heal`, and reports failure via `Event::ActionResult` (success sends nothing — the reconnect loop picks up the fresh daemon, and its healthy snapshot clears the "restarting…" status):

  ```rust
          Cmd::Heal => {
              let state = crate::paths::state_path();
              let sock = crate::paths::socket_path(&state);
              let pid = crate::paths::pid_path(&state);
              let cli = crate::paths::daemon_cli_path();
              match crate::heal::perform_heal(&sock, &pid, &cli).await {
                  Ok(()) => { /* success: reconnect loop + healthy snapshot take over */ }
                  Err(msg) => {
                      let _ = tx.send(Event::ActionResult {
                          status: Some(msg),
                          invalidate_defs_for: None,
                      });
                  }
              }
          }
  ```

  (Adapt `tx` to the executor's actual sender name from Task 4, e.g. `event_tx`.)

  ```bash
  cargo test -p qoo-tui && cargo build --release -p qoo-tui
  ```

  Expected: full suite PASS, release build PASS.

- [ ] **Step 7: Commit.**

  ```bash
  git add crates/qoo-tui/src/heal.rs crates/qoo-tui/src/lib.rs crates/qoo-tui/src/app.rs crates/qoo-tui/src/event.rs crates/qoo-tui/Cargo.toml crates/qoo-tui/Cargo.lock
  git commit -m "feat(tui-rs): daemon self-heal (decide_heal + perform_heal + Cmd::Heal wiring)"
  ```

---

### Task 23: End-to-end verification + launcher flip

No new features. Prove the Rust TUI reaches parity with the Ink TUI across every mode (automated suite + a manual parity walk), then make `mise run tui` launch the Rust binary while keeping the Ink TUI reachable as `tui:ink` for the bake period. Also prove the TS packages are untouched.

**Files:**

- **Modify:** `.mise.toml` (`[tasks.tui]` → Rust binary; add `[tasks."tui:ink"]` fallback)

**Interfaces:** none (verification + task wiring only). Consumes the `target/release/qoo-tui` artifact produced by `cargo build --release -p qoo-tui` and the existing `daemon:ensure` task.

- [ ] **Step 1: Automated gate — all suites green.** Run and confirm each command exits 0:

  ```bash
  cargo test -p qoo-tui          # full Rust suite
  cargo build --release          # workspace release build
  pnpm -r test                   # TS suites unchanged → still green (proves packages/* untouched)
  ```

  Do not proceed until all three pass.

- [ ] **Step 2: Manual parity checklist.** With the daemon running and at least one real project + a running task present, launch the Rust TUI (`cargo build --release -p qoo-tui && ./target/release/qoo-tui`) and confirm every item behaves identically to the Ink TUI (spec parity section). Check each box only after observing the behavior live:

  - [ ] Tabs: switch projects by number keys and by clicking a tab; the synthetic/all tab renders.
  - [ ] Filters: `/` opens per-pane search; typing filters rows; `Esc` clears; the pane title shows the active filter.
  - [ ] Bulk rerun / skip: multi-select a range (anchor + cursor) in the queue, open the bulk action menu, run bulk rerun and bulk skip; status line reports the verb.
  - [ ] Add task (fresh): `a` → fresh-session add-task modal → submit → task appears queued.
  - [ ] Add task (main): add-task in main-session mode → task lands on the main lane.
  - [ ] Assign worktree: assign-worktree modal on a task → target updates.
  - [ ] Def-pick → args → run (ambient): open def picker, choose a def, fill the args form with ambient worktree context auto-filled, run.
  - [ ] Def-pick → args → run (explicit): same but with an explicitly chosen worktree/branch.
  - [ ] Dropdowns via mouse: open an args-form dropdown by clicking; pick an option by clicking.
  - [ ] Create worktree: create-worktree modal → new worktree appears in the pane.
  - [ ] Remove worktree: remove-worktree confirm → worktree gone.
  - [ ] Squash-merge: squash-merge action on a branch → completes.
  - [ ] Tmux open: open-tmux action attaches/creates the session for the path.
  - [ ] Transcript live tail: enter a running task's detail → transcript sub-tab tails new lines while the task runs (bottom-anchored).
  - [ ] Self-heal (idle): with the TUI open and no task running, rebuild the daemon (`pnpm -F @queohoh/daemon build`); the daemon restarts automatically ("daemon outdated — restarting…" then clears).
  - [ ] Self-heal (busy): with a task running, rebuild the daemon; status shows "daemon outdated — will restart when idle"; it restarts once the task finishes.
  - [ ] Disconnect / reconnect: stop the daemon (connection indicator flips); restart it (`mise run daemon:ensure`) → TUI reconnects and repaints.
  - [ ] Resize: resize the terminal → layout reflows; no crash.
  - [ ] Too small: shrink below 60×15 → shows only `terminal too small (60x15 minimum)`.
  - [ ] Help overlay: `?` toggles the keymap overlay.
  - [ ] Scrollbar drag in every pane: drag the scrollbar thumb in the queue, tasks, worktrees, and detail panes.

- [ ] **Step 3: Flip `.mise.toml`.** Replace the existing `[tasks.tui]` block and add the `tui:ink` fallback. Exact TOML (replace lines 59–62; leave `dev:tui` as-is for Task 24 to handle):

  ```toml
  [tasks.tui]
  description = "Launch the qoo-tui cockpit (ratatui; builds the release binary, self-heals the daemon)"
  depends = ["daemon:ensure"]
  run = [
  	"cargo build --release -p qoo-tui",
  	"./target/release/qoo-tui",
  ]

  [tasks."tui:ink"]
  description = "Launch the legacy Ink TUI (fallback during the ratatui bake; rebuilds first, self-heals the daemon)"
  depends = ["build", "daemon:ensure"]
  run = "node packages/tui/dist/cli.js"
  ```

  (`daemon:ensure` transitively depends on `build`, so the daemon `dist/` — needed for self-heal's disk-id read and detached respawn — is present before the Rust binary launches.)

- [ ] **Step 4: Verify the flip.** Confirm the tasks resolve and the Rust binary launches via mise:

  ```bash
  mise tasks | grep -E '(^| )tui'      # expect: tui, tui:ink (and dev:tui)
  mise run tui                          # launches ./target/release/qoo-tui after building
  ```

  Sanity-check `tui:ink` still starts the Ink TUI. Quit both.

- [ ] **Step 5: Commit.**

  ```bash
  git add .mise.toml
  git commit -m "feat(tui-rs): make qoo-tui the default TUI launcher"
  ```

---

### Task 24: Cutover — delete packages/tui (USER-GATED)

- [ ] **Step 0: STOP — do not execute until the user confirms.** This task deletes the Ink TUI. **Do not run any step below until the user explicitly confirms they have baked on the Rust TUI and want the Ink TUI deleted.** If unconfirmed, halt and surface the question. There is no code to write here beyond removals and doc edits; the gate is a human decision.

**Files:**

- **Delete:** `packages/tui/` (entire directory)
- **Modify:** `.mise.toml` (remove `[tasks."tui:ink"]` and `[tasks."dev:tui"]`)
- **Modify:** `pnpm-lock.yaml` (regenerated by `pnpm install`)
- **Modify:** `docs/setup.md` (section 6 — point at the Rust launcher)

**Interfaces:** none. Removal + doc-sync only.

- [ ] **Step 1: Remove the package and its tasks.** Delete the Ink TUI package and both Ink-only mise tasks (Rust hot-reload is already covered by the existing `tui:rs:dev` cargo-watch task, so `dev:tui` is deleted, not repointed):

  ```bash
  git rm -r packages/tui
  ```

  In `.mise.toml`, delete the entire `[tasks."tui:ink"]` block (added in Task 23) and the entire `[tasks."dev:tui"]` block. Leave `[tasks.tui]` (now the sole TUI launcher) and `[tasks."tui:rs:dev"]` untouched.

- [ ] **Step 2: Prune the lockfile.**

  ```bash
  pnpm install     # drops @queohoh/tui from the workspace + prunes pnpm-lock.yaml
  ```

  Confirm root `package.json` scripts don't reference the removed package (the `packages/*` workspace glob needs no edit; a stale root script referencing `@queohoh/tui` or `packages/tui` must be removed if present).

- [ ] **Step 3: Grep for dangling references — expect none.**

  ```bash
  rg -l "@queohoh/tui" --glob '!pnpm-lock.yaml'
  ```

  Expected: no output. If anything matches (outside `pnpm-lock.yaml`), remove or repoint it before committing.

- [ ] **Step 4: Update `docs/setup.md`.** Section 6 ("TUI (the cockpit)") currently documents `node packages/tui/dist/cli.js`. Replace the build+run block (lines ~104–110) so it points at the Rust launcher:

  ```markdown
  ## 6. TUI (the cockpit)

  The cockpit is the `qoo-tui` Rust binary. `mise run tui` builds it (release) and
  launches it, ensuring the daemon is running first.

  ```bash
  mise run tui                      # builds ./target/release/qoo-tui and launches it
  ```
  ```

  Leave the "Run it in tmux tab 0…" paragraph as-is (still accurate).

- [ ] **Step 5: Full gate — everything green without the Ink TUI.**

  ```bash
  pnpm -r test && pnpm -r typecheck && cargo test -p qoo-tui
  ```

  Expected: all PASS (the workspace no longer contains `packages/tui`, so its suites are simply absent).

- [ ] **Step 6: Commit.**

  ```bash
  git add -A
  git commit -m "chore: remove Ink TUI after ratatui cutover"
  ```
