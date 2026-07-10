use std::io::{self, Stdout};

use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

/// RAII guard: enters raw mode + alt-screen on construction and, via `Drop`,
/// restores the terminal on every normal exit path (return, `?`, unwind).
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

/// Chains a panic hook that restores the terminal before the default hook
/// prints the panic, so a panic exit path leaves the terminal usable too
/// (the `Drop` guard does not run when the process aborts on panic).
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
    app.load_layout(); // per-project pane layout from <state_dir>/tui-layout.json
    let size = terminal.size()?;
    app.size = (size.width, size.height);

    qoo_tui::event::run_event_loop(&mut terminal, &mut app).await
}
