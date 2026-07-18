//! The unified adhoc-create form (`c` / Create) — target combobox, session
//! picker, model dropdown, prompt textarea — reached from any list pane. Drives
//! the real `Mode::Form { AdhocTask }` flow through `App::update`.

use super::*;
use crate::app::mode::adhoc_field;
use crate::event::SessionChoice;
use crate::ipc::types::{Project, StateSnapshot, TaskInstance, TaskStatus, WorktreeInfo};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashMap;

fn ch(c: char) -> Event {
    Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
}
fn keyc(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}
fn enter() -> Event {
    keyc(KeyCode::Enter)
}
fn esc() -> Event {
    keyc(KeyCode::Esc)
}
fn shift_enter() -> Event {
    Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT))
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
    a.connected = true;
    a
}

fn type_str(a: &mut App, s: &str) {
    for c in s.chars() {
        a.update(ch(c));
    }
}

fn focus_field(a: &mut App, i: usize) {
    if let Mode::Form { state, .. } = &mut a.mode {
        state.focus_field(i);
    }
}

fn enqueue_params(up: &Update) -> serde_json::Value {
    up.cmds
        .iter()
        .find_map(|c| match c {
            Cmd::Rpc { call, .. } if call.method == "enqueue" => Some(call.params.clone()),
            _ => None,
        })
        .expect("expected an enqueue Rpc cmd")
}

/// Type `prompt` into the prompt textarea, then fire the Primary button.
fn fill_prompt_and_submit(a: &mut App, prompt: &str) -> Update {
    focus_field(a, adhoc_field::PROMPT);
    type_str(a, prompt);
    if let Mode::Form { state, .. } = &mut a.mode {
        state.focus = state.fields.len(); // Primary
    }
    a.update(enter())
}

#[test]
fn queue_c_opens_adhoc_form_with_four_fields() {
    let mut a = app(); // queue focused by default
    a.update(ch('c'));
    match &a.mode {
        Mode::Form { state, action } => {
            assert!(matches!(action, FormAction::AdhocTask { .. }));
            assert_eq!(state.fields.len(), 4);
            assert_eq!(state.fields[adhoc_field::TARGET].label, "worktree / PR / ticket");
            assert!(state.fields[adhoc_field::TARGET].value.is_empty());
            assert_eq!(state.fields[adhoc_field::SESSION].value, "New session");
            assert_eq!(state.fields[adhoc_field::MODEL].label, "model");
            assert_eq!(state.fields[adhoc_field::PROMPT].label, "prompt");
            assert!(state.fields[adhoc_field::PROMPT].required);
            // Focus starts on the model dropdown (field 0, the first
            // non-readonly field under the app-wide model-first ordering).
            assert_eq!(state.focus, adhoc_field::MODEL);
        }
        other => panic!("expected adhoc Form, got {other:?}"),
    }
}

#[test]
fn adhoc_empty_target_enqueues_temp_no_ref() {
    let mut a = app();
    a.update(ch('c'));
    let up = fill_prompt_and_submit(&mut a, "run this now");
    assert!(matches!(a.mode, Mode::List));
    let p = enqueue_params(&up);
    assert_eq!(p["prompt"], "run this now");
    assert_eq!(p["repo"], "platform");
    assert!(p.get("ref").is_none(), "empty target sends no ref → daemon temp");
    assert!(p.get("worktree").is_none());
}

#[test]
fn adhoc_existing_worktree_target_sends_worktree_ref() {
    let mut a = app();
    a.update(ch('c'));
    focus_field(&mut a, adhoc_field::TARGET);
    type_str(&mut a, "platform.wt-a");
    let up = fill_prompt_and_submit(&mut a, "do it");
    let p = enqueue_params(&up);
    // Mirrors runDefinition: canonical ref, never a bare `worktree` param.
    assert_eq!(p["ref"], "worktree:platform.wt-a");
    assert!(p.get("worktree").is_none());
    assert_eq!(p["prompt"], "do it");
    // The model dropdown is left on its head option (value "" = leave unset), so
    // no `model` param is sent — the daemon resolves the default chain.
    assert!(p.get("model").is_none());
    assert!(p.get("model_pinned").is_none(), "unset model has nothing to pin");
}

#[test]
fn adhoc_picked_model_is_pinned() {
    // Picking a concrete catalog model (not the head "" default) is an
    // explicit dialog choice: it must be sent with `model_pinned: true` so
    // the worker runs it exactly — no active-provider re-head, no fallback.
    let mut a = app();
    a.update(ch('c'));
    focus_field(&mut a, adhoc_field::MODEL);
    // Same catalog order as the new-session picker: Down opens the list
    // (highlight = head idx 0), two more Downs reach `claude/claude-opus-4.8`.
    a.update(keyc(KeyCode::Down)); // open
    a.update(keyc(KeyCode::Down)); // → claude/claude-fable-5
    a.update(keyc(KeyCode::Down)); // → claude/claude-opus-4.8
    a.update(enter()); // pick
    let up = fill_prompt_and_submit(&mut a, "run it");
    let p = enqueue_params(&up);
    assert_eq!(p["model"], "claude/claude-opus-4.8");
    assert_eq!(p["model_pinned"], true);
}

#[test]
fn adhoc_pr_number_target_sends_pr_ref() {
    let mut a = app();
    a.update(ch('c'));
    focus_field(&mut a, adhoc_field::TARGET);
    type_str(&mut a, "45");
    let up = fill_prompt_and_submit(&mut a, "fix pr");
    let p = enqueue_params(&up);
    assert_eq!(p["ref"], "pr:45");
}

#[test]
fn adhoc_c_on_queue_pane_opens_blank_ignoring_selected_task() {
    // A new adhoc task has nothing to do with which past task is selected, so
    // the QUEUE `c` never prefills the target from the cursor row's worktree.
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
    let mut task = TaskInstance::default();
    task.id = "t1".into();
    task.status = TaskStatus::Failed;
    task.target.repo = "platform".into();
    task.target.worktree = Some("platform.wt-a".into());
    a.update(Event::Snapshot(StateSnapshot {
        projects: vec![Project { name: "platform".into(), github_id: None }],
        worktrees: wts,
        tasks: vec![task],
        ..Default::default()
    }));
    a.connected = true;
    a.update(ch('c')); // queue focused by default, task row under the cursor
    match &a.mode {
        Mode::Form { state, .. } => {
            assert!(state.fields[adhoc_field::TARGET].value.is_empty(), "queue create opens blank");
        }
        other => panic!("expected adhoc Form, got {other:?}"),
    }
}

#[test]
fn adhoc_c_on_worktrees_pane_prefills_target() {
    let mut a = app();
    // Tab Queue → Tasks → Worktrees.
    a.update(keyc(KeyCode::Tab));
    a.update(keyc(KeyCode::Tab));
    a.update(ch('c'));
    match &a.mode {
        Mode::Form { state, .. } => {
            assert_eq!(state.fields[adhoc_field::TARGET].value, "platform.wt-a");
        }
        other => panic!("expected adhoc Form, got {other:?}"),
    }
}

#[test]
fn adhoc_esc_cancels_without_cmd() {
    let mut a = app();
    a.update(ch('c'));
    let u = a.update(esc());
    assert!(matches!(a.mode, Mode::List));
    assert!(u.cmds.is_empty());
}

#[test]
fn shift_enter_inserts_newline_in_prompt() {
    let mut a = app();
    a.update(ch('c'));
    focus_field(&mut a, adhoc_field::PROMPT);
    a.update(ch('a'));
    a.update(shift_enter());
    a.update(ch('b'));
    match &a.mode {
        Mode::Form { state, .. } => assert_eq!(state.fields[adhoc_field::PROMPT].value, "a\nb"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn session_field_opens_session_pick_for_existing_worktree() {
    let mut a = app();
    a.update(ch('c'));
    focus_field(&mut a, adhoc_field::TARGET);
    type_str(&mut a, "platform.wt-a");
    focus_field(&mut a, adhoc_field::SESSION);
    let u = a.update(enter());
    match &a.mode {
        Mode::SessionPick { worktree, ret, .. } => {
            assert_eq!(worktree, "platform.wt-a");
            assert!(matches!(ret, SessionPickReturn::Adhoc { .. }));
        }
        other => panic!("expected SessionPick, got {other:?}"),
    }
    assert!(u.cmds.iter().any(|c| matches!(c, Cmd::FetchSessions { .. })));
}

#[test]
fn session_field_is_noop_without_an_existing_worktree_target() {
    let mut a = app();
    a.update(ch('c')); // empty target
    focus_field(&mut a, adhoc_field::SESSION);
    a.update(enter());
    assert!(matches!(a.mode, Mode::Form { .. }), "stays on the form");
    assert!(a.status_line.is_some(), "explains why sessions aren't offered");
}

#[test]
fn session_pick_resume_returns_pinned_and_preserves_prompt() {
    let mut a = app();
    a.update(ch('c'));
    focus_field(&mut a, adhoc_field::TARGET);
    type_str(&mut a, "platform.wt-a");
    focus_field(&mut a, adhoc_field::PROMPT);
    type_str(&mut a, "continue please");
    focus_field(&mut a, adhoc_field::SESSION);
    a.update(enter()); // → SessionPick (loading)
    a.update(Event::SessionsLoaded {
        worktree: "platform.wt-a".into(),
        result: Ok(vec![SessionChoice {
            session_id: "sess-1".into(),
            label: "Fix parser".into(),
            mtime_ms: 2_000,
            model: Some("sonnet".into()),
            provider: None,
        }]),
    });
    // View rows: New(0), Create Worktree(1), session(2).
    a.update(keyc(KeyCode::Down));
    a.update(keyc(KeyCode::Down));
    a.update(enter()); // confirm the session → back to the adhoc form
    match &a.mode {
        Mode::Form { state, action } => {
            assert_eq!(state.fields[adhoc_field::PROMPT].value, "continue please");
            assert_eq!(state.fields[adhoc_field::SESSION].value, "↻ Fix parser");
            match action {
                FormAction::AdhocTask { resume_session_id, resume_worktree, .. } => {
                    assert_eq!(resume_session_id.as_deref(), Some("sess-1"));
                    assert_eq!(resume_worktree.as_deref(), Some("platform.wt-a"));
                }
                other => panic!("{other:?}"),
            }
        }
        other => panic!("expected the restored adhoc Form, got {other:?}"),
    }
    // Submit carries both the ref and the pinned session.
    if let Mode::Form { state, .. } = &mut a.mode {
        state.focus = state.fields.len();
    }
    let up = a.update(enter());
    let p = enqueue_params(&up);
    assert_eq!(p["ref"], "worktree:platform.wt-a");
    assert_eq!(p["resume_session_id"], "sess-1");
}

#[test]
fn session_pick_cancel_restores_the_form_unchanged() {
    let mut a = app();
    a.update(ch('c'));
    focus_field(&mut a, adhoc_field::TARGET);
    type_str(&mut a, "platform.wt-a");
    focus_field(&mut a, adhoc_field::PROMPT);
    type_str(&mut a, "keep this");
    focus_field(&mut a, adhoc_field::SESSION);
    a.update(enter()); // → SessionPick
    a.update(esc()); // cancel
    match &a.mode {
        Mode::Form { state, .. } => {
            assert_eq!(state.fields[adhoc_field::PROMPT].value, "keep this");
            assert_eq!(state.fields[adhoc_field::SESSION].value, "New session");
        }
        other => panic!("expected the restored adhoc Form, got {other:?}"),
    }
}

#[test]
fn editing_the_target_clears_a_stale_session_pin() {
    let mut a = app();
    a.update(ch('c'));
    // Simulate a prior pick: a pin + updated session label.
    if let Mode::Form { state, action } = &mut a.mode {
        if let FormAction::AdhocTask { resume_session_id, resume_label, resume_worktree, .. } = action {
            *resume_session_id = Some("sess-1".into());
            *resume_label = Some("X".into());
            *resume_worktree = Some("platform.wt-a".into());
        }
        state.set_field_value(adhoc_field::SESSION, "↻ X");
        state.focus_field(adhoc_field::TARGET);
    }
    a.update(ch('z')); // any edit to the target invalidates the pin
    match &a.mode {
        Mode::Form { state, action } => {
            assert_eq!(state.fields[adhoc_field::SESSION].value, "New session");
            assert!(matches!(action, FormAction::AdhocTask { resume_session_id: None, .. }));
        }
        other => panic!("{other:?}"),
    }
}
