use qoo_tui::app::{App, Mode};
use qoo_tui::ipc::types::{Project, StateSnapshot};
use ratatui::{Terminal, backend::TestBackend};

#[test]
fn create_worktree_modal_with_error_snapshot() {
    let mut app = App::new("/tmp/runs".into(), "/tmp/d.sock".into());
    app.size = (80, 24);
    // One project so `active_repo()` yields "platform" for the modal title.
    app.snapshot = Some(StateSnapshot {
        projects: vec![Project { name: "platform".into(), github_id: None }],
        ..Default::default()
    });
    let input = tui_input::Input::new("bad name".into());
    app.mode = Mode::CreateWorktree { input, error: Some("no whitespace allowed".into()) };
    let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
    term.draw(|f| {
        qoo_tui::view::render(&app, f);
    })
    .unwrap();
    insta::assert_snapshot!(term.backend());
}
