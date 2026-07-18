use std::path::{Path, PathBuf};
use std::time::Duration;

use futures::StreamExt;
use tokio::sync::mpsc::{self, UnboundedSender};

use crate::app::{App, Update};
use crate::ipc::client::{spawn_subscription, RpcClient};
use crate::ipc::types::{DefinitionSummary, SettingsPayload, StateSnapshot, TaskDefinition};
use crate::runfiles::RunFiles;

/// Everything enters the app through this one enum (contract-verbatim).
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Key(crossterm::event::KeyEvent),
    Mouse(crossterm::event::MouseEvent),
    /// Bracketed-paste payload (verbatim, newlines included). Enabled at startup
    /// so a multiline paste arrives as one event instead of a burst of Enter
    /// keypresses that would submit a text field mid-paste.
    Paste(String),
    Resize,
    Snapshot(StateSnapshot),
    Disconnected,
    Tick,
    /// Boxed: `RunFiles` carries the parsed run meta (~700 bytes), which would
    /// otherwise dominate every `Event` (clippy::large_enum_variant).
    RunFiles { task_id: String, files: Box<RunFiles> },
    Definitions { repo: String, defs: Vec<DefinitionSummary> },
    /// Boxed: a full `TaskDefinition` (~400 bytes) would otherwise dominate every
    /// `Event` (clippy::large_enum_variant).
    Definition { repo: String, name: String, def: Option<Box<TaskDefinition>> },
    /// Result of the on-demand `settings` RPC that backs the `s` overlay.
    /// `None` = the call failed or the daemon predates the RPC (stored as
    /// `Some(None)` in `App::settings` → overlay shows the "unavailable" line).
    Settings { payload: Option<SettingsPayload> },
    /// The post-copy selection fade fired. Stale (epoch < the current selection
    /// generation) expiries are ignored by `update`.
    SelectionExpired { epoch: u64 },
    ActionResult { status: Option<String>, invalidate_defs_for: Option<String> },
    /// Reply to a [`Cmd::FetchSessions`]: the resumable Claude sessions for a
    /// worktree (newest-first, capped by the daemon). `worktree` echoes the
    /// request so `update` can drop a stale reply that arrives after the picker
    /// moved on. `Err` carries the RPC/parse error for the status line.
    SessionsLoaded { worktree: String, result: Result<Vec<SessionChoice>, String> },
}

/// One resumable Claude session offered by the `listSessions` RPC. Field names
/// match the wire shape verbatim (serde snake_case).
#[derive(Debug, Clone, serde::Deserialize, PartialEq)]
pub struct SessionChoice {
    pub session_id: String,
    pub label: String,
    pub mtime_ms: u64,
    /// The model alias this session last ran on (from the daemon's run store,
    /// reverse-mapped from the resolved id). `None` for sessions with no run
    /// data (e.g. started outside queohoh) — the resume form then falls back to
    /// the project/global default. Absent on the wire → `None`.
    #[serde(default)]
    pub model: Option<String>,
    /// Provider that last owned this session (model ref's first segment, or
    /// lineage when the model is unknown). Used by interactive goto to pick the
    /// right CLI/`bin`. `None` when unknown or on an old daemon that omits it.
    #[serde(default)]
    pub provider: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RpcCall {
    pub method: String,
    pub params: serde_json::Value,
}

/// Post-create enqueue payload for [`Cmd::CreateWorktree`]. When present, the
/// handler enqueues a first task into the freshly-created worktree (resolving the
/// worktree name from the create reply's `path` basename — never reconstructed in
/// the TUI). Empty `model` means "daemon default".
#[derive(Debug, Clone, PartialEq)]
pub struct EnqueueAfter {
    pub prompt: String,
    pub model: String,
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
        /// Status message to surface when the call succeeds AND the response
        /// is an empty JSON array — e.g. `runDefinition`'s reply is the array
        /// of created tasks, so an empty array means every item deduped away
        /// (a real outcome, not a no-op). `None` on every other Cmd::Rpc
        /// (a normal success still leaves the status line untouched).
        report_empty_as: Option<String>,
    },
    RpcSeq {
        verb: String, // past tense, e.g. "reran"
        calls: Vec<RpcCall>,
        invalidate_defs_for: Option<String>,
    },
    FetchDefinitions { repo: String },
    FetchDefinition { repo: String, name: String },
    /// Fetch a worktree's resumable Claude sessions for the session picker via the
    /// `listSessions` RPC; the reply lands as [`Event::SessionsLoaded`] (echoing
    /// `worktree` so a stale reply is dropped). `repo`/`worktree` scope the query.
    FetchSessions { repo: String, worktree: String },
    /// One-shot fetch of the daemon's model-alias settings for the `s` overlay.
    /// Emitted once on first open (App::settings is None); the reply lands as
    /// [`Event::Settings`].
    FetchSettings,
    ReadRunFiles { task_id: String, tail_lines: usize, delay_ms: u64 },
    /// Create a worktree via the `createWorktree` RPC (10-minute budget —
    /// post-create `wt.toml` hooks routinely run for minutes). With
    /// `enqueue: None` (create-only), then, inside tmux, auto-open a tmux window
    /// in the returned path (user request: no manual open after a create);
    /// outside tmux, or against an old daemon whose reply carries no path, the
    /// open is silently skipped. With `enqueue: Some(_)` (the launcher's Create
    /// Worktree flow), the handler instead enqueues a first task into the new
    /// worktree — resolving its name from the reply `path` basename — and skips
    /// the auto-open (the task owns the worktree).
    CreateWorktree { repo: String, name: String, enqueue: Option<EnqueueAfter> },
    /// Open a worktree (or resume a session) in a first-class tmux layout: new
    /// window at `path`, left|right split, left = bare shell, right runs `cmd`
    /// (empty `cmd` leaves both panes as bare shells). Fired by worktree/queue
    /// `g` after provider resolution; replaces the retired OpenTmux/TmuxResume
    /// + init-tab path.
    Goto { path: String, cmd: String },
    /// Write-through of the per-project pane layout. Fire-and-forget off the UI
    /// thread; a failed write is silently tolerated (layout is a convenience).
    SaveLayout { path: PathBuf, json: String },
    /// Copy `text` to the system clipboard from the detail-pane text selection:
    /// an OSC 52 escape (works in modern terminals and over ssh/tmux) written
    /// synchronously on the UI thread before the next redraw, plus a best-effort
    /// `pbcopy` pipe on macOS. Fire-and-forget.
    CopyClipboard { text: String },
    /// Arm the post-copy selection fade: after `delay_ms`, deliver
    /// [`Event::SelectionExpired`] carrying `epoch` so a selection started in
    /// the meantime survives (its epoch is newer).
    ExpireSelection { epoch: u64, delay_ms: u64 },
    Heal,
    Quit,
}

fn map_terminal_event(ev: crossterm::event::Event) -> Option<Event> {
    use crossterm::event::Event as Ct;
    match ev {
        Ct::Key(k) => Some(Event::Key(k)),
        Ct::Mouse(m) => Some(Event::Mouse(m)),
        Ct::Paste(s) => Some(Event::Paste(s)),
        Ct::Resize(_, _) => Some(Event::Resize),
        _ => None, // focus events unused
    }
}

/// Draw one frame with the real view and store its hit geometry into `app.hit`
/// for the next event's mouse routing. Stale-free: every state change redraws,
/// so `app.hit` always matches what is on screen.
fn draw<B: ratatui::backend::Backend>(
    terminal: &mut ratatui::Terminal<B>,
    app: &mut App,
) -> std::io::Result<()> {
    let mut hits = crate::hit::HitMap::new();
    terminal.draw(|f| {
        hits = crate::view::render(app, f);
    })?;
    app.hit = hits;
    Ok(())
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

    draw(terminal, app)?; // first paint pre-snapshot

    // Monotonic program-start anchor for the millisecond clock. `App::now_ms` is
    // stamped from this before every `update()` so the update logic (double-click
    // timing) reads a wall-independent, always-increasing millisecond count.
    let start = std::time::Instant::now();

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

        // Stamp the millisecond clock before update() so double-click timing
        // (and any future time-based logic) reads a monotonic source.
        app.now_ms = start.elapsed().as_millis() as u64;
        let Update { dirty, mut cmds } = app.update(event);
        // Lazy per-tab definitions fetch: after every event, emit a fetch for the
        // active repo when its summaries are neither cached nor in flight (the TS
        // post-render effect). Kept out of `App::update` so unit tests that assert
        // on `cmds` stay deterministic; the dedup set makes repeat calls no-ops.
        if let Some(cmd) = app.reconcile_defs() {
            cmds.push(cmd);
        }
        // Same lazy pattern for the FULL definition backing the detail pane's
        // Definition context — without this, `full_defs` never fills and the
        // pane shows "(loading definition…)" forever.
        if let Some(cmd) = app.reconcile_full_def() {
            cmds.push(cmd);
        }
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
            draw(terminal, app)?;
        }
    }
}

/// Minimal standard base64 (RFC 4648, `+`/`/` alphabet, `=` padded) for the
/// clipboard OSC 52 payload — kept local so the copy path pulls in no new crate.
fn base64_encode(input: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(A[(n >> 18 & 63) as usize] as char);
        out.push(A[(n >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 { A[(n >> 6 & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { A[(n & 63) as usize] as char } else { '=' });
    }
    out
}

/// Compose the RpcSeq status line: "reran 3" or "reran 2, 1 failed: <first error>".
pub fn seq_summary(verb: &str, ok: usize, errs: &[String]) -> String {
    if errs.is_empty() {
        return format!("{verb} {ok}");
    }
    format!("{verb} {ok}, {} failed: {}", errs.len(), errs[0])
}

/// Map a successful RPC response to a status message when it is an empty JSON
/// array and the caller supplied `report_empty_as` — e.g. `runDefinition`
/// returns the array of created tasks, so an empty array means every item
/// deduped away (dedup.ts `filterNewItems`). Any other response shape (a
/// non-empty array, an object, a scalar) or a `None` `report_empty_as` is a
/// normal success: `None`, leave the status line untouched.
fn empty_array_status(value: &serde_json::Value, report_empty_as: Option<&str>) -> Option<String> {
    match (value.as_array(), report_empty_as) {
        (Some(arr), Some(msg)) if arr.is_empty() => Some(msg.to_string()),
        _ => None,
    }
}

async fn rpc_once(sock: &Path, call: &RpcCall, timeout_ms: u64) -> Result<serde_json::Value, String> {
    let mut client = RpcClient::connect(sock).await.map_err(|e| e.to_string())?;
    client
        .call(&call.method, call.params.clone(), Duration::from_millis(timeout_ms))
        .await
}

/// Open a tmux window rooted at `path`. Returns the status-line error text on
/// failure, `None` on success — shared by the worktree menu's "Open in tmux
/// window" action and the post-create auto-open.
async fn open_tmux_window(path: &str) -> Option<String> {
    let result = tokio::process::Command::new("tmux")
        .args(["new-window", "-c", path])
        .output()
        .await;
    match result {
        Ok(out) if out.status.success() => None,
        Ok(out) => Some(format!("tmux: {}", String::from_utf8_lossy(&out.stderr).trim())),
        Err(e) => Some(format!("tmux: {e}")),
    }
}

/// The tmux layout shape for a `goto`. Always a new window + left|right split
/// (left bare shell; right runs `cmd` when non-empty). Derived purely from the
/// path and the provider-resolved command so unit tests cover the plan without
/// shelling out to tmux.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum GotoPlan {
    /// `new-window -P -F #{window_id} -c path` then `split-window -h -t id -c path`;
    /// when `cmd` is non-empty, select the right pane and `send-keys` the command
    /// + Enter. Empty `cmd` leaves both panes as bare interactive shells.
    Split { path: String, cmd: String },
}

/// Build the first-class split goto plan. `cmd` is the right-pane command
/// (fresh interactive bin, or `{bin} --resume {session_id}`); empty means both
/// panes stay bare shells.
pub(crate) fn goto_split_plan(path: &str, cmd: &str) -> GotoPlan {
    GotoPlan::Split { path: path.to_string(), cmd: cmd.to_string() }
}

/// Whether the plan will type a command into the right pane (false for an empty
/// `cmd` → both panes bare shells). Pure helper so tests pin the send-keys gate
/// without running tmux.
#[cfg(test)]
pub(crate) fn goto_sends_keys(plan: &GotoPlan) -> bool {
    matches!(plan, GotoPlan::Split { cmd, .. } if !cmd.is_empty())
}

/// Execute a [`GotoPlan`] off the UI thread. On any tmux failure, reports the
/// stderr as a status line (mirrors `open_tmux_window`); success is silent.
///
/// Sequence (targets the NEW window's panes carefully via captured ids so a
/// failed capture never injects keys into the operator's own TUI pane):
/// 1. `new-window -P -F #{window_id} -c path` → window id
/// 2. `split-window -h -t <win> -c path -P -F #{pane_id}` → right pane id
/// 3. if `cmd` non-empty: `send-keys -t <pane> -l -- cmd` + `Enter`
async fn run_goto(plan: GotoPlan, tx: UnboundedSender<Event>) {
    async fn tmux(args: &[&str]) -> Result<std::process::Output, std::io::Error> {
        tokio::process::Command::new("tmux").args(args).output().await
    }
    let GotoPlan::Split { path, cmd } = plan;
    let status = match tmux(&[
        "new-window",
        "-P",
        "-F",
        "#{window_id}",
        "-c",
        &path,
    ])
    .await
    {
        Ok(out) if out.status.success() => {
            let win = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if win.is_empty() {
                // Empty window id → any `-t ""` retargets the CURRENT pane and
                // would inject into the operator's TUI. Bail instead.
                Some("tmux: new-window returned no window id".to_string())
            } else {
                // Horizontal split on the new window. Capture the new (right)
                // pane id so send-keys never targets the left bare shell or the
                // operator's TUI by accident. After `-h`, the new pane is the
                // right half of the window.
                match tmux(&["split-window", "-h", "-t", &win, "-c", &path, "-P", "-F", "#{pane_id}"])
                    .await
                {
                    Ok(sout) if sout.status.success() => {
                        if cmd.is_empty() {
                            // Both panes bare shells — nothing to type.
                            None
                        } else {
                            let pane = String::from_utf8_lossy(&sout.stdout).trim().to_string();
                            if pane.is_empty() {
                                Some("tmux: split-window returned no pane id".to_string())
                            } else {
                                // `-l` = literal keys (no key-name lookup); `--`
                                // guards a leading '-'. Enter is a separate
                                // key-name call.
                                let _ = tmux(&["send-keys", "-t", &pane, "-l", "--", &cmd]).await;
                                let _ = tmux(&["send-keys", "-t", &pane, "Enter"]).await;
                                None
                            }
                        }
                    }
                    Ok(sout) => {
                        Some(format!("tmux: {}", String::from_utf8_lossy(&sout.stderr).trim()))
                    }
                    Err(e) => Some(format!("tmux: {e}")),
                }
            }
        }
        Ok(out) => Some(format!("tmux: {}", String::from_utf8_lossy(&out.stderr).trim())),
        Err(e) => Some(format!("tmux: {e}")),
    };
    if let Some(status) = status {
        let _ = tx.send(Event::ActionResult { status: Some(status), invalidate_defs_for: None });
    }
}

/// Perform one Cmd on a detached tokio task; results come back as Events.
/// The UI thread never blocks (mutations are fire-and-forget).
pub fn execute(cmd: Cmd, tx: UnboundedSender<Event>, sock: PathBuf, runs_dir: PathBuf) {
    match cmd {
        Cmd::Rpc { label, call, timeout_ms, timeout_is_ok, invalidate_defs_for, report_empty_as } => {
            tokio::spawn(async move {
                let result = rpc_once(&sock, &call, timeout_ms).await;
                let status = match result {
                    // An empty-array success (e.g. runDefinition deduped every
                    // item) still surfaces when the caller asked for it —
                    // otherwise a normal success leaves the status untouched.
                    Ok(v) => empty_array_status(&v, report_empty_as.as_deref()),
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
                    Ok(v) => serde_json::from_value::<TaskDefinition>(v).ok().map(Box::new),
                    Err(_) => None,
                };
                let _ = tx.send(Event::Definition { repo, name, def });
            });
        }
        Cmd::FetchSessions { repo, worktree } => {
            tokio::spawn(async move {
                let call = RpcCall {
                    method: "listSessions".into(),
                    params: serde_json::json!({ "repo": &repo, "worktree": &worktree }),
                };
                // On success, pull `result.sessions` and deserialize the typed
                // choices; an RPC error OR a shape an old daemon can't produce →
                // Err(msg) so the picker shows the error and stays usable (the
                // "New session" row is always selectable).
                let result = match rpc_once(&sock, &call, 5_000).await {
                    Ok(v) => serde_json::from_value::<Vec<SessionChoice>>(v["sessions"].clone())
                        .map_err(|e| e.to_string()),
                    Err(e) => Err(e),
                };
                let _ = tx.send(Event::SessionsLoaded { worktree, result });
            });
        }
        Cmd::FetchSettings => {
            tokio::spawn(async move {
                let call = RpcCall { method: "settings".into(), params: serde_json::Value::Null };
                // A failed call OR a shape an old daemon can't produce → None,
                // stored as `Some(None)` so the overlay renders the "unavailable"
                // line instead of spinning forever (mirrors FetchDefinition's
                // catch → null pattern).
                let payload = match rpc_once(&sock, &call, 5_000).await {
                    Ok(v) => serde_json::from_value::<SettingsPayload>(v).ok(),
                    Err(_) => None,
                };
                let _ = tx.send(Event::Settings { payload });
            });
        }
        Cmd::ReadRunFiles { task_id, tail_lines, delay_ms } => {
            tokio::spawn(async move {
                // Selection-settle debounce lives here (caller just issues the Cmd
                // with delay_ms=120). runfiles::read_run_files is a stub until
                // Task 10 lands the real tail/report reader.
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                let files = crate::runfiles::read_run_files(&runs_dir, &task_id, tail_lines).await;
                let _ = tx.send(Event::RunFiles { task_id, files: Box::new(files) });
            });
        }
        Cmd::CreateWorktree { repo, name, enqueue } => {
            tokio::spawn(async move {
                let call = RpcCall {
                    method: "createWorktree".into(),
                    params: serde_json::json!({ "repo": repo, "name": name }),
                };
                let result = rpc_once(&sock, &call, 600_000).await;
                let status = match &result {
                    Ok(_) => None,
                    Err(e) => Some(format!("create worktree {name}: {e}")),
                };
                let failed = status.is_some();
                let _ = tx.send(Event::ActionResult { status, invalidate_defs_for: None });
                if failed {
                    return;
                }
                // The created worktree's name is the basename of the reply path
                // (matches the daemon's `listWorktrees` naming) — never
                // reconstructed in the TUI.
                let path = result
                    .ok()
                    .and_then(|v| v.get("path").and_then(|p| p.as_str()).map(str::to_string));
                // Option A: sequence create → enqueue into the new worktree using
                // the authoritative path. Skips the auto-open (the task owns it).
                if let Some(after) = enqueue {
                    let wt = path
                        .as_deref()
                        .and_then(|p| Path::new(p).file_name().and_then(|s| s.to_str()));
                    let status = match wt {
                        Some(wt) => {
                            let mut params =
                                serde_json::json!({ "prompt": after.prompt, "repo": repo, "worktree": wt });
                            if !after.model.is_empty() {
                                // A concrete pick (not the head "" default) is an
                                // explicit dialog choice: pin it so the worker
                                // runs it exactly, no active-provider re-head, no
                                // fallback.
                                params["model_pinned"] = serde_json::Value::Bool(true);
                                params["model"] = serde_json::Value::String(after.model);
                            }
                            let enq = RpcCall { method: "enqueue".into(), params };
                            rpc_once(&sock, &enq, 5_000)
                                .await
                                .err()
                                .map(|e| format!("enqueue in {name}: {e}"))
                        }
                        None => Some(format!("enqueue in {name}: worktree has no path")),
                    };
                    if status.is_some() {
                        let _ = tx.send(Event::ActionResult { status, invalidate_defs_for: None });
                    }
                    return;
                }
                // Create-only path: auto-open a tmux window in the new worktree.
                if std::env::var_os("TMUX").is_none() {
                    return; // outside tmux: same silent gate as the menu action
                }
                let Some(path) = path else {
                    return; // old daemon replied `true` — no path to open
                };
                if let Some(status) = open_tmux_window(&path).await {
                    let _ = tx.send(Event::ActionResult {
                        status: Some(status),
                        invalidate_defs_for: None,
                    });
                }
            });
        }
        Cmd::Goto { path, cmd } => {
            tokio::spawn(run_goto(goto_split_plan(&path, &cmd), tx));
        }
        Cmd::SaveLayout { path, json } => {
            tokio::spawn(async move {
                // Best-effort write-through: create the state dir if needed, then
                // overwrite the file. Any failure is dropped (no UI surface).
                if let Some(dir) = path.parent() {
                    let _ = tokio::fs::create_dir_all(dir).await;
                }
                let _ = tokio::fs::write(&path, json).await;
            });
        }
        Cmd::CopyClipboard { text } => {
            // OSC 52 is written on the calling (UI) thread, not a spawned task, so
            // it cannot interleave with crossterm's draw bytes: the event loop runs
            // execute() for each cmd BEFORE the frame redraw, so this lands cleanly
            // and the next draw fully repaints regardless. `c` = the clipboard
            // selection; base64 payload per the OSC 52 spec.
            let encoded = base64_encode(text.as_bytes());
            {
                use std::io::Write;
                let mut out = std::io::stdout().lock();
                let _ = write!(out, "\x1b]52;c;{encoded}\x07");
                let _ = out.flush();
            }
            // Belt-and-suspenders macOS fallback: pipe the raw text to pbcopy
            // off-thread (covers terminals with OSC 52 disabled).
            tokio::spawn(async move {
                use tokio::io::AsyncWriteExt;
                if let Ok(mut child) = tokio::process::Command::new("pbcopy")
                    .stdin(std::process::Stdio::piped())
                    .spawn()
                {
                    if let Some(mut stdin) = child.stdin.take() {
                        let _ = stdin.write_all(text.as_bytes()).await;
                        let _ = stdin.shutdown().await;
                    }
                    let _ = child.wait().await;
                }
            });
        }
        Cmd::ExpireSelection { epoch, delay_ms } => {
            // The 1s post-copy fade: sleep off-thread, then hand the epoch back
            // through the event channel. `update` drops it as stale if a newer
            // selection started in the meantime.
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                let _ = tx.send(Event::SelectionExpired { epoch });
            });
        }
        Cmd::Heal => {
            tokio::spawn(async move {
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
            });
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
    fn base64_matches_known_vectors() {
        // RFC 4648 test vectors — exercises 0/1/2 trailing bytes (padding cases).
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn base64_encodes_multibyte_utf8() {
        // "中" is 3 UTF-8 bytes (E4 B8 AD) → one full quantum, no padding.
        // Ground truth: `printf '中' | base64` → 5Lit.
        assert_eq!(base64_encode("中".as_bytes()), "5Lit");
    }

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

    #[test]
    fn empty_array_status_surfaces_the_message_on_an_empty_array() {
        let v = serde_json::json!([]);
        assert_eq!(
            empty_array_status(&v, Some("pr-review: nothing ran — deduped")),
            Some("pr-review: nothing ran — deduped".to_string())
        );
    }

    #[test]
    fn empty_array_status_stays_none_on_a_non_empty_array() {
        let v = serde_json::json!([{"id": "t1"}]);
        assert_eq!(empty_array_status(&v, Some("deduped")), None);
    }

    #[test]
    fn empty_array_status_stays_none_when_the_caller_did_not_ask() {
        // Every Cmd::Rpc other than run_definition_cmd sends `report_empty_as:
        // None` — an empty-array reply from e.g. `archive`/`set_cron_enabled`
        // must not spuriously report anything.
        let v = serde_json::json!([]);
        assert_eq!(empty_array_status(&v, None), None);
    }

    #[test]
    fn empty_array_status_stays_none_on_a_non_array_response() {
        // `archive`/`set_cron_enabled`/etc. return non-array shapes (or the
        // response is simply irrelevant to them, since they send `None`) —
        // guard against misreading an object/scalar as "empty".
        let v = serde_json::json!({"ok": true});
        assert_eq!(empty_array_status(&v, Some("should not fire")), None);
    }
}

#[cfg(test)]
mod goto_plan_tests {
    use super::{goto_sends_keys, goto_split_plan, GotoPlan, SessionChoice};

    #[test]
    fn non_empty_cmd_is_split_that_sends_keys() {
        // Fresh interactive (worktree) or resume (queue): right pane gets the
        // command. Plan is first-class Split — never CreateAndSend / init-tab.
        let plan = goto_split_plan("/wt/a", "claude");
        assert_eq!(
            plan,
            GotoPlan::Split { path: "/wt/a".into(), cmd: "claude".into() }
        );
        assert!(goto_sends_keys(&plan));
        let debug = format!("{plan:?}");
        assert!(!debug.contains("init-tab"), "plan must not mention init-tab: {debug}");
        assert!(!debug.contains("CreateAndSend"), "CreateAndSend retired: {debug}");
    }

    #[test]
    fn empty_cmd_is_split_with_no_send_keys() {
        // Empty cmd → both panes bare shells (new-window + split only).
        let plan = goto_split_plan("/wt/a", "");
        assert_eq!(
            plan,
            GotoPlan::Split { path: "/wt/a".into(), cmd: String::new() }
        );
        assert!(!goto_sends_keys(&plan));
    }

    #[test]
    fn resume_cmd_carries_provider_bin_and_session() {
        // Queue goto builds `{bin} --resume {session_id}` before the plan —
        // the plan itself is path+cmd only (provider resolution lives in actions).
        let plan = goto_split_plan("/wt/a", "grok --resume sess1");
        assert_eq!(
            plan,
            GotoPlan::Split {
                path: "/wt/a".into(),
                cmd: "grok --resume sess1".into(),
            }
        );
        assert!(goto_sends_keys(&plan));
    }

    #[test]
    fn session_choice_provider_present_and_absent() {
        // listSessions may include optional `provider` (Task 1). Present when
        // known from model ref / lineage; omitted → None via field default.
        let with: SessionChoice = serde_json::from_str(
            r#"{"session_id":"s1","label":"fix login","mtime_ms":1000,
                "model":"claude/opus","provider":"claude"}"#,
        )
        .unwrap();
        assert_eq!(with.provider.as_deref(), Some("claude"));
        assert_eq!(with.model.as_deref(), Some("claude/opus"));
        let without: SessionChoice = serde_json::from_str(
            r#"{"session_id":"s2","label":"old sess","mtime_ms":0}"#,
        )
        .unwrap();
        assert_eq!(without.provider, None);
        assert_eq!(without.model, None);
    }
}
