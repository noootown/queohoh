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
    /// Write-through of the per-project pane layout. Fire-and-forget off the UI
    /// thread; a failed write is silently tolerated (layout is a convenience).
    SaveLayout { path: PathBuf, json: String },
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
