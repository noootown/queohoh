use super::*;
use crate::ipc::types::{Project, StateSnapshot, WorktreeInfo};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use std::collections::HashMap;

fn key(c: char) -> Event {
    Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
}
fn enter() -> Event {
    Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
}
fn shift_enter() -> Event {
    Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT))
}
fn esc() -> Event {
    Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
}

fn app() -> App {
    let mut a = App::new("/tmp/runs".into(), "/tmp/s.sock".into());
    a.size = (120, 40);
    let mut wts = HashMap::new();
    wts.insert(
        "platform".into(),
        vec![WorktreeInfo {
            name: "platform.wt-a".into(),
            path: "/wt/wt-a".into(),
            branch: "jus-42".into(),
            ..Default::default()
        }],
    );
    a.update(Event::Snapshot(StateSnapshot {
        projects: vec![Project { name: "platform".into(), github_id: None }],
        worktrees: wts,
        ..Default::default()
    }));
    a
}

fn fresh_add_task(worktree: Option<String>) -> Mode {
    Mode::AddTask {
        worktree,
        resume_session_id: None,
        resume_label: None,
        editor: crate::view::multiline_input::MultilineInput::default(),
    }
}

fn type_str(a: &mut App, s: &str) {
    for c in s.chars() {
        a.update(key(c));
    }
}

fn rpc_call(u: &Update) -> &crate::event::RpcCall {
    u.cmds
        .iter()
        .find_map(|c| if let Cmd::Rpc { call, .. } = c { Some(call) } else { None })
        .expect("expected an Rpc cmd")
}

#[test]
fn add_task_enter_submits_prompt_without_session_field() {
    let mut a = app();
    a.mode = fresh_add_task(Some("platform.wt-a".into()));
    type_str(&mut a, "do it");
    let up = a.update(enter());
    assert!(matches!(a.mode, Mode::List));
    let call = rpc_call(&up);
    assert_eq!(call.method, "enqueue");
    assert_eq!(
        call.params,
        serde_json::json!({ "prompt": "do it", "repo": "platform", "worktree": "platform.wt-a" })
    );
}

#[test]
fn add_task_adhoc_omits_worktree() {
    let mut a = app();
    a.mode = fresh_add_task(None);
    type_str(&mut a, "run this now");
    let u = a.update(enter());
    let call = rpc_call(&u);
    assert_eq!(call.params, serde_json::json!({ "prompt": "run this now", "repo": "platform" }));
}

#[test]
fn add_task_with_pin_sends_resume_session_id() {
    let mut a = app();
    a.mode = Mode::AddTask {
        worktree: Some("platform.wt-a".into()),
        resume_session_id: Some("sess-1".into()),
        resume_label: Some("Fix the parser".into()),
        editor: crate::view::multiline_input::MultilineInput::default(),
    };
    a.update(key('x'));
    let up = a.update(enter());
    let call = rpc_call(&up);
    assert_eq!(call.params["resume_session_id"], serde_json::json!("sess-1"));
    // A pin still carries the worktree + prompt; it never carries "session".
    assert_eq!(call.params["worktree"], serde_json::json!("platform.wt-a"));
    assert!(call.params.get("session").is_none());
}

#[test]
fn shift_enter_inserts_newline_instead_of_submitting() {
    let mut a = app();
    a.mode = fresh_add_task(None);
    a.update(key('a'));
    a.update(shift_enter());
    a.update(key('b'));
    // Still open (did not submit), and the newline landed between the chars.
    match &a.mode {
        Mode::AddTask { editor, .. } => assert_eq!(editor.text, "a\nb"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn queue_c_opens_adhoc_add_task() {
    let mut a = app(); // queue focused by default
    a.update(key('c'));
    match &a.mode {
        Mode::AddTask { worktree, resume_session_id, resume_label, .. } => {
            assert!(worktree.is_none());
            assert!(resume_session_id.is_none());
            assert!(resume_label.is_none());
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn add_task_esc_cancels_without_cmd() {
    let mut a = app();
    a.mode = fresh_add_task(None);
    let u = a.update(esc());
    assert!(matches!(a.mode, Mode::List));
    assert!(u.cmds.is_empty());
}

#[test]
fn typing_q_inserts_literal_and_backspace_edits() {
    let mut a = app();
    a.mode = fresh_add_task(None);
    a.update(key('q'));
    match &a.mode {
        Mode::AddTask { editor, .. } => assert_eq!(editor.text, "q"),
        _ => panic!(),
    }
    a.update(Event::Key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)));
    match &a.mode {
        Mode::AddTask { editor, .. } => assert_eq!(editor.text, ""),
        _ => panic!(),
    }
}

#[test]
fn mouse_event_never_reaches_the_input() {
    let mut a = app();
    a.mode = fresh_add_task(None);
    type_str(&mut a, "hi");
    // A drag/motion mouse event over the field must not append glyphs.
    a.update(Event::Mouse(MouseEvent {
        kind: MouseEventKind::Moved,
        column: 10,
        row: 5,
        modifiers: KeyModifiers::NONE,
    }));
    match &a.mode {
        Mode::AddTask { editor, .. } => assert_eq!(editor.text, "hi"),
        _ => panic!(),
    }
}
