use qoo_tui::app::{App, Mode};
use qoo_tui::ipc::types::{ArgSpec, DefinitionSummary};
use ratatui::{Terminal, backend::TestBackend};

#[test]
fn def_pick_popup_snapshot() {
    let mut app = App::new("/tmp/runs".into(), "/tmp/d.sock".into());
    app.size = (80, 24);
    app.mode = Mode::DefPick {
        defs: vec![
            DefinitionSummary {
                repo: "platform".into(),
                name: "autotest".into(),
                scope: "project".into(),
                args: vec![],
                has_discovery: true,
                cron: None,
                description: None,
                model: None,
                worktree: None,
            },
            DefinitionSummary {
                repo: "platform".into(),
                name: "squash-merge".into(),
                scope: "global".into(),
                args: vec![
                    ArgSpec { name: "source".into(), r#type: None, default: None, options: None, description: None },
                    ArgSpec { name: "target".into(), r#type: None, default: Some("main".into()), options: None, description: None },
                ],
                has_discovery: false,
                cron: None,
                description: None,
                model: None,
                worktree: Some("repo".into()),
            },
        ],
        index: 1,
        worktree: Some("platform.wt-a".into()),
        branch: Some("jus-1-x".into()),
        query: String::new(),
        preview_scroll: 0,
    };
    let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
    term.draw(|f| {
        qoo_tui::view::render(&mut app, f);
    })
    .unwrap();
    insta::assert_snapshot!(term.backend());
}
