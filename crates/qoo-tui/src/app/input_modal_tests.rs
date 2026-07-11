use super::*;
use crate::hit::{ButtonKind, HitTarget};
use crate::ipc::types::{Project, StateSnapshot, WorktreeInfo};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use std::collections::HashMap;

fn key(c: char) -> Event {
    Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
}
fn enter() -> Event {
    Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
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
fn add_task_worktree_targeted_enqueue_carries_worktree() {
    let mut a = app();
    a.mode = Mode::AddTask {
        worktree: Some("platform.wt-a".into()),
        session: SessionMode::Fresh,
        input: tui_input::Input::default(),
    };
    type_str(&mut a, "do a thing");
    let u = a.update(enter());
    assert!(matches!(a.mode, Mode::List));
    let call = rpc_call(&u);
    assert_eq!(call.method, "enqueue");
    assert_eq!(
        call.params,
        serde_json::json!({
            "prompt": "do a thing", "repo": "platform", "worktree": "platform.wt-a", "session": "fresh"
        })
    );
}

#[test]
fn add_task_adhoc_omits_worktree() {
    let mut a = app();
    a.mode = Mode::AddTask {
        worktree: None,
        session: SessionMode::Fresh,
        input: tui_input::Input::default(),
    };
    type_str(&mut a, "run this now");
    let u = a.update(enter());
    let call = rpc_call(&u);
    assert_eq!(
        call.params,
        serde_json::json!({
            "prompt": "run this now", "repo": "platform", "session": "fresh"
        })
    );
}

#[test]
fn queue_c_opens_adhoc_add_task() {
    let mut a = app(); // queue focused by default
    a.update(key('c'));
    match &a.mode {
        Mode::AddTask { worktree, session, .. } => {
            assert!(worktree.is_none());
            assert!(matches!(session, SessionMode::Fresh));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn add_task_esc_cancels_without_cmd() {
    let mut a = app();
    a.mode = Mode::AddTask {
        worktree: None,
        session: SessionMode::Fresh,
        input: tui_input::Input::default(),
    };
    let u = a.update(esc());
    assert!(matches!(a.mode, Mode::List));
    assert!(u.cmds.is_empty());
}

#[test]
fn typing_q_inserts_literal_and_backspace_edits() {
    let mut a = app();
    a.mode = Mode::AddTask {
        worktree: None,
        session: SessionMode::Fresh,
        input: tui_input::Input::default(),
    };
    a.update(key('q'));
    match &a.mode {
        Mode::AddTask { input, .. } => assert_eq!(input.value(), "q"),
        _ => panic!(),
    }
    a.update(Event::Key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)));
    match &a.mode {
        Mode::AddTask { input, .. } => assert_eq!(input.value(), ""),
        _ => panic!(),
    }
}

#[test]
fn mouse_event_never_reaches_the_input() {
    let mut a = app();
    a.mode = Mode::AddTask {
        worktree: None,
        session: SessionMode::Fresh,
        input: tui_input::Input::default(),
    };
    type_str(&mut a, "hi");
    // A drag/motion mouse event over the field must not append glyphs.
    a.update(Event::Mouse(MouseEvent {
        kind: MouseEventKind::Moved,
        column: 10,
        row: 5,
        modifiers: KeyModifiers::NONE,
    }));
    match &a.mode {
        Mode::AddTask { input, .. } => assert_eq!(input.value(), "hi"),
        _ => panic!(),
    }
}

/// Render the current mode into `a.hit` (so mouse routing has real button
/// geometry), then return the scanned coordinates of a `Button` target.
fn render_and_find_button(a: &mut App, kind: ButtonKind) -> (u16, u16) {
    use ratatui::{Terminal, backend::TestBackend};
    let (w, h) = a.size;
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    let mut hits = crate::hit::HitMap::new();
    term.draw(|f| hits = crate::view::render(a, f)).unwrap();
    a.hit = hits;
    for y in 0..h {
        for x in 0..w {
            if a.hit.hit(x, y) == Some(&HitTarget::Button(kind)) {
                return (x, y);
            }
        }
    }
    panic!("Button({kind:?}) region not found after render");
}

fn click(a: &mut App, x: u16, y: u16) -> Update {
    a.update(Event::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: x,
        row: y,
        modifiers: KeyModifiers::NONE,
    }))
}

#[test]
fn click_confirm_equals_enter_and_cancel_equals_esc() {
    // Click Confirm ≡ Enter: dispatches enqueue and closes to List.
    let mut a = app();
    a.mode = Mode::AddTask {
        worktree: None,
        session: SessionMode::Fresh,
        input: tui_input::Input::default(),
    };
    type_str(&mut a, "run this now");
    let (cx, cy) = render_and_find_button(&mut a, ButtonKind::Confirm);
    let u = click(&mut a, cx, cy);
    assert!(matches!(a.mode, Mode::List));
    let call = rpc_call(&u);
    assert_eq!(call.method, "enqueue");
    assert_eq!(
        call.params,
        serde_json::json!({ "prompt": "run this now", "repo": "platform", "session": "fresh" })
    );

    // Click Cancel ≡ Esc: closes to List with no cmd.
    let mut a = app();
    a.mode = Mode::AddTask {
        worktree: None,
        session: SessionMode::Fresh,
        input: tui_input::Input::default(),
    };
    let (cx, cy) = render_and_find_button(&mut a, ButtonKind::Cancel);
    let u = click(&mut a, cx, cy);
    assert!(matches!(a.mode, Mode::List));
    assert!(u.cmds.is_empty());
}
