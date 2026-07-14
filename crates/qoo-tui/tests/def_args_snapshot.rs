//! App-level two-panel def-args render through `view::render`. Exercises the
//! `Mode::DefArgs` dispatch + right-panel prompt resolution from `full_defs`
//! that the `view::def_args` unit test does not — the shared `FormState` engine
//! drawn in the picker shell (bordered fields left, the def's prompt right).

use qoo_tui::app::{App, Mode};
use qoo_tui::ipc::types::{ArgSpec, TaskDefinition};
use qoo_tui::view::form::{Field, FormState};
use ratatui::{Terminal, backend::TestBackend};

fn arg(name: &str) -> ArgSpec {
    ArgSpec { name: name.into(), r#type: None, default: None, options: None, description: None }
}

/// An app parked on `Mode::DefArgs` for the `pr-ready` def: `pr` is a free-text
/// textarea, `mode` an enum dropdown, `source` a fixed (read-only) field — the
/// same field mapping `App::form_from_args` produces. The def's cached prompt
/// feeds the right panel.
fn form_app() -> App {
    let mut app = App::new("/tmp/runs".into(), "/tmp/d.sock".into());
    app.size = (100, 28);
    let args = vec![
        arg("pr"),
        ArgSpec { options: Some(vec!["ready".into(), "create".into()]), default: Some("ready".into()), ..arg("mode") },
        arg("source"),
    ];
    let state = FormState::new(
        "pr-ready",
        "Run",
        vec![
            Field::textarea("pr", "", true),
            Field::dropdown("mode", vec!["ready".into(), "create".into()], "ready"),
            Field::readonly("source", "wt-a"),
        ],
    );
    app.mode = Mode::DefArgs {
        state,
        repo: "platform".into(),
        def_name: "pr-ready".into(),
        args,
        initial_worktree: None,
        preview_scroll: 0,
    };
    app.full_defs.insert(
        "platform/pr-ready".into(),
        TaskDefinition {
            name: "pr-ready".into(),
            repo: "platform".into(),
            prompt: "Flip the PR from WIP to ready-for-review.\n\nRun a self review sized to the diff, sync the description, then assign the four standing reviewers.".into(),
            ..Default::default()
        },
    );
    app
}

fn text(term: &Terminal<TestBackend>, cols: u16, rows: u16) -> String {
    let buf = term.backend().buffer().clone();
    let mut s = String::new();
    for y in 0..rows {
        for x in 0..cols {
            s.push_str(buf[(x, y)].symbol());
        }
        s.push('\n');
    }
    s
}

#[test]
fn def_args_two_panel_app_render() {
    let mut app = form_app();
    let mut term = Terminal::new(TestBackend::new(100, 28)).unwrap();
    term.draw(|f| {
        qoo_tui::view::render(&mut app, f);
    })
    .unwrap();
    insta::assert_snapshot!(term.backend());
}

#[test]
fn def_args_open_dropdown_app_render() {
    let mut app = form_app();
    if let Mode::DefArgs { state, .. } = &mut app.mode {
        state.focus_field(1); // the `mode` enum
        state.open_dropdown();
    }
    let mut term = Terminal::new(TestBackend::new(100, 28)).unwrap();
    term.draw(|f| {
        qoo_tui::view::render(&mut app, f);
    })
    .unwrap();
    let s = text(&term, 100, 28);
    assert!(s.contains("ready"), "open dropdown lists its options: {s}");
    assert!(s.contains("create"), "open dropdown lists every option");
}

#[test]
fn def_args_multiline_value_app_render() {
    let mut app = form_app();
    if let Mode::DefArgs { state, .. } = &mut app.mode {
        state.focus_field(0); // the free-text `pr` textarea
        state.insert_str("first line of the report\nsecond line here");
    }
    let mut term = Terminal::new(TestBackend::new(100, 28)).unwrap();
    term.draw(|f| {
        qoo_tui::view::render(&mut app, f);
    })
    .unwrap();
    let s = text(&term, 100, 28);
    assert!(s.contains("first line of the report"), "multiline value's first line renders: {s}");
    assert!(s.contains("second line here"), "multiline value's second line renders");
    assert!(s.contains("Flip the PR"), "the def's prompt renders on the right");
}
