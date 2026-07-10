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
