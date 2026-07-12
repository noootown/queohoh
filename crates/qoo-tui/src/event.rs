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
    Definition { repo: String, name: String, def: Option<TaskDefinition> },
    /// Result of the on-demand `settings` RPC that backs the `s` overlay.
    /// `None` = the call failed or the daemon predates the RPC (stored as
    /// `Some(None)` in `App::settings` → overlay shows the "unavailable" line).
    Settings { payload: Option<SettingsPayload> },
    /// The post-copy selection fade fired. Stale (epoch < the current selection
    /// generation) expiries are ignored by `update`.
    SelectionExpired { epoch: u64 },
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
    /// One-shot fetch of the daemon's model-alias settings for the `s` overlay.
    /// Emitted once on first open (App::settings is None); the reply lands as
    /// [`Event::Settings`].
    FetchSettings,
    ReadRunFiles { task_id: String, tail_lines: usize, delay_ms: u64 },
    /// Create a worktree via the `createWorktree` RPC (10-minute budget —
    /// post-create `wt.toml` hooks routinely run for minutes), then, inside
    /// tmux, auto-open a tmux window in the returned path (user request: no
    /// manual open after a create). Outside tmux, or against an old daemon
    /// whose reply carries no path, the open is silently skipped.
    CreateWorktree { repo: String, name: String },
    OpenTmux { path: String },
    /// Resume a task's Claude session in a NEW tmux pane rooted at its worktree:
    /// `tmux split-window -c <path> 'claude --resume <session_id>'`. Fired by the
    /// queue "Resume" action; gated on being inside tmux + a known session/path.
    TmuxResume { path: String, session_id: String },
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
        Cmd::CreateWorktree { repo, name } => {
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
                if failed || std::env::var_os("TMUX").is_none() {
                    return; // outside tmux: same silent gate as the menu action
                }
                let Some(path) = result
                    .ok()
                    .and_then(|v| v.get("path").and_then(|p| p.as_str()).map(str::to_string))
                else {
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
        Cmd::OpenTmux { path } => {
            tokio::spawn(async move {
                if let Some(status) = open_tmux_window(&path).await {
                    let _ = tx.send(Event::ActionResult {
                        status: Some(status),
                        invalidate_defs_for: None,
                    });
                }
            });
        }
        Cmd::TmuxResume { path, session_id } => {
            tokio::spawn(async move {
                // A new pane split from the current window, rooted at the
                // worktree, running `claude --resume <session>` as its command.
                let result = tokio::process::Command::new("tmux")
                    .args([
                        "split-window",
                        "-c",
                        &path,
                        &format!("claude --resume {session_id}"),
                    ])
                    .output()
                    .await;
                let status = match result {
                    Ok(out) if out.status.success() => None,
                    Ok(out) => Some(format!("tmux: {}", String::from_utf8_lossy(&out.stderr).trim())),
                    Err(e) => Some(format!("tmux: {e}")),
                };
                if status.is_some() {
                    let _ = tx.send(Event::ActionResult { status, invalidate_defs_for: None });
                }
            });
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
}
