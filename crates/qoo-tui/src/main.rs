use std::io::{self, Stdout};

use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

/// RAII guard: enters raw mode + alt-screen + bracketed paste on construction
/// and, via `Drop`, restores the terminal on every normal exit path (return,
/// `?`, unwind). Bracketed paste routes a multiline paste as one `Event::Paste`
/// (never a burst of Enter keypresses). When the terminal advertises the kitty
/// keyboard protocol (Ghostty, kitty, foot, WezTerm), we additionally push
/// `DISAMBIGUATE_ESCAPE_CODES` so `Shift+Enter` reaches the app as a distinct
/// chord (a hard-newline in text fields); `kbd_enhanced` records whether that
/// push happened so `Drop` pops exactly what it pushed. DISAMBIGUATE alone does
/// NOT enable key-release/repeat reporting, so no press/release regression on
/// plain terminals — and every key handler already filters `KeyEventKind::Press`
/// defensively regardless.
struct TerminalGuard {
    kbd_enhanced: bool,
}

impl TerminalGuard {
    fn new() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut out = io::stdout();
        execute!(out, EnterAlternateScreen, EnableMouseCapture, EnableBracketedPaste)?;
        // Best-effort: a terminal that doesn't support the protocol (or errors on
        // the query) simply keeps `Shift+Enter` indistinguishable from `Enter`
        // (Alt+Enter is the documented fallback newline chord there).
        let kbd_enhanced = supports_keyboard_enhancement().unwrap_or(false);
        if kbd_enhanced {
            let _ = execute!(
                out,
                PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
            );
        }
        Ok(Self { kbd_enhanced })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let mut out = io::stdout();
        if self.kbd_enhanced {
            let _ = execute!(out, PopKeyboardEnhancementFlags);
        }
        let _ = execute!(out, DisableBracketedPaste, DisableMouseCapture, LeaveAlternateScreen);
        let _ = disable_raw_mode();
    }
}

/// Chains a panic hook that restores the terminal before the default hook
/// prints the panic, so a panic exit path leaves the terminal usable too
/// (the `Drop` guard does not run when the process aborts on panic). Pops the
/// keyboard-enhancement flags unconditionally — a pop with an empty stack is a
/// harmless no-op on terminals that never had a push.
fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let mut out = io::stdout();
        let _ = execute!(
            out,
            PopKeyboardEnhancementFlags,
            DisableBracketedPaste,
            DisableMouseCapture,
            LeaveAlternateScreen
        );
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
    // Attach-only mode: never restart a daemon owned by another checkout. The
    // self-heal keys on THIS checkout's packages/daemon/dist fingerprint, so two
    // worktrees' TUIs would otherwise restart the shared daemon back and forth.
    if std::env::args().any(|a| a == "--no-heal") {
        app.heal_enabled = false;
    }
    app.load_layout(); // per-project pane layout from <state_dir>/tui-layout.json
    let size = terminal.size()?;
    app.size = (size.width, size.height);

    qoo_tui::event::run_event_loop(&mut terminal, &mut app).await
}
