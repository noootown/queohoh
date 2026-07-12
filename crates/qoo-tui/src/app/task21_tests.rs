use super::*;
use crate::ipc::types::{Project, StateSnapshot, TaskInstance, TaskStatus, WorktreeInfo};
use crate::keymap::AppAction;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashMap;

fn key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn fixture_app_one_project(name: &str) -> App {
    let mut app = App::new("/tmp/runs".into(), "/tmp/daemon.sock".into());
    app.size = (120, 40);
    app.snapshot = Some(StateSnapshot {
        projects: vec![Project { name: name.into(), github_id: None }],
        ..Default::default()
    });
    app.connected = true;
    app
}

fn fixture_create_worktree(repo: &str) -> App {
    let mut app = fixture_app_one_project(repo);
    app.mode = Mode::CreateWorktree { input: tui_input::Input::default(), error: None };
    app
}

/// One project, worktrees pane focused (so `c` opens the create modal).
fn fixture_app_worktrees_focused(repo: &str) -> App {
    let mut app = fixture_app_one_project(repo);
    app.set_focus(PaneId::Worktrees);
    app
}

/// One project with a single worktree that has a running task on its lane
/// (→ `WtState::Busy`), worktrees pane focused with the busy row selected.
fn fixture_app_busy_worktree(repo: &str, wt: &str) -> App {
    let mut app = fixture_app_one_project(repo);
    let raw = format!("{repo}.{wt}");
    let mut worktrees = HashMap::new();
    worktrees.insert(
        repo.to_string(),
        vec![WorktreeInfo {
            name: raw.clone(),
            path: format!("/wt/{wt}"),
            branch: "feat-x".into(),
            ..Default::default()
        }],
    );
    let mut running = TaskInstance::default();
    running.id = "01RUN".into();
    running.status = TaskStatus::Running;
    running.target.repo = repo.into();
    running.target.worktree = Some(raw);
    app.snapshot = Some(StateSnapshot {
        projects: vec![Project { name: repo.into(), github_id: None }],
        worktrees,
        tasks: vec![running],
        ..Default::default()
    });
    app.set_focus(PaneId::Worktrees);
    app
}

// --- create-worktree flow ---
#[test]
fn create_worktree_entered_by_c_in_worktrees_pane() {
    let mut app = fixture_app_worktrees_focused("platform");
    app.update(key(KeyCode::Char('c')));
    assert!(matches!(app.mode, Mode::CreateWorktree { .. }));
}

#[test]
fn create_worktree_invalid_stays_open_with_error() {
    let mut app = fixture_create_worktree("platform");
    for c in "bad name".chars() {
        app.update(key(KeyCode::Char(c)));
    }
    let update = app.update(key(KeyCode::Enter));
    assert!(update.cmds.is_empty());
    match &app.mode {
        Mode::CreateWorktree { error, input } => {
            assert!(error.as_deref().unwrap().contains("whitespace"));
            assert_eq!(input.value(), "bad name"); // input preserved
        }
        other => panic!("expected CreateWorktree, got {other:?}"),
    }
}

#[test]
fn create_worktree_valid_dispatches_and_closes_immediately() {
    let mut app = fixture_create_worktree("platform");
    for c in "feature-x".chars() {
        app.update(key(KeyCode::Char(c)));
    }
    let update = app.update(key(KeyCode::Enter));
    assert!(matches!(app.mode, Mode::List)); // closes immediately (fires async)
    assert_eq!(app.status_line.as_deref(), Some("creating worktree feature-x…"));
    match &update.cmds[0] {
        Cmd::CreateWorktree { repo, name } => {
            assert_eq!(repo, "platform");
            assert_eq!(name, "feature-x");
        }
        other => panic!("expected CreateWorktree, got {other:?}"),
    }
}

#[test]
fn create_worktree_esc_cancels() {
    let mut app = fixture_create_worktree("platform");
    let update = app.update(key(KeyCode::Esc));
    assert!(update.cmds.is_empty());
    assert!(matches!(app.mode, Mode::List));
}

#[test]
fn create_worktree_outside_click_cancels_and_keys_stay_out_of_field() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    use ratatui::{Terminal, backend::TestBackend};
    let mut app = fixture_create_worktree("platform");
    let (w, h) = app.size;
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| app.hit = crate::view::render(&app, f)).unwrap();
    // Click the top-left corner — outside the centered modal → cancels.
    let ev = Event::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    let update = app.update(ev);
    assert!(update.cmds.is_empty());
    assert!(matches!(app.mode, Mode::List));
}

// --- busy worktree menu eligibility (regression) ---
#[test]
fn busy_worktree_remove_menu_row_is_disabled() {
    let mut app = fixture_app_busy_worktree("platform", "wt-a");
    app.apply_action(AppAction::OpenActionMenu); // select busy worktree, open menu
    let items = match &app.mode {
        Mode::ActionMenu { items, .. } => items.clone(),
        other => panic!("expected ActionMenu, got {other:?}"),
    };
    let remove = items
        .iter()
        .find(|it| it.label.starts_with("Remove worktree"))
        .expect("remove row present");
    assert_eq!(remove.disabled.as_deref(), Some("a task is running here"));
}
