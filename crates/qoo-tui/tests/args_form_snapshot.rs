use qoo_tui::app::{App, Mode};
use qoo_tui::ipc::types::{ArgSpec, TaskDefinition};
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

// The full two-panel run form: a multiline free-text value (soft-wrapped, with
// the caret) on the left, the def's cached prompt scrollable on the right.
#[test]
fn args_form_multiline_and_prompt_snapshot() {
    let mut app = form_app();
    app.size = (100, 28);
    app.full_defs.insert(
        "platform/pr-ready".into(),
        TaskDefinition {
            name: "pr-ready".into(),
            repo: "platform".into(),
            prompt: "Flip the PR from WIP to ready-for-review.\n\nRun a self review sized to the diff, sync the description, then assign the four standing reviewers.".into(),
            ..Default::default()
        },
    );
    if let Mode::DefArgs { form } = &mut app.mode {
        // Focus the free-text "pr" row and type a multiline bug-report-style value.
        form.focus_field(0);
        form.insert_str("first line of the report\nsecond line that is long enough to soft-wrap across the field");
    }
    let mut term = Terminal::new(TestBackend::new(100, 28)).unwrap();
    term.draw(|f| { qoo_tui::view::render(&app, f); }).unwrap();
    insta::assert_snapshot!(term.backend());
}
