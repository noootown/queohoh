//! The unified adhoc-create form (`s` / Schedule on QUEUE) — target combobox, session
//! picker, model dropdown, prompt textarea — reached from the QUEUE `[s]chedule`
//! chip. Drives the real `Mode::Form { AdhocTask }` flow through `App::update`.

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
    let _ = type_str_cmds(a, s);
}

/// Like [`type_str`], but returns every `Cmd` produced while typing (so callers
/// can assert a side-effect like the target-change sessions prefetch).
fn type_str_cmds(a: &mut App, s: &str) -> Vec<Cmd> {
    let mut cmds = Vec::new();
    for c in s.chars() {
        cmds.extend(a.update(ch(c)).cmds);
    }
    cmds
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
fn queue_s_opens_adhoc_form_with_four_fields() {
    let mut a = app(); // queue focused by default
    a.update(ch('s'));
    match &a.mode {
        Mode::Form { state, action } => {
            assert!(matches!(action, FormAction::AdhocTask { .. }));
            assert_eq!(state.fields.len(), 4);
            // Order: target → session → model → prompt (model under session so
            // a chosen session can filter models by provider).
            assert_eq!(state.fields[adhoc_field::TARGET].label, "worktree / PR / ticket");
            assert!(state.fields[adhoc_field::TARGET].value.is_empty());
            assert_eq!(state.fields[adhoc_field::SESSION].label, "session");
            assert_eq!(state.fields[adhoc_field::SESSION].value, "New session");
            assert_eq!(state.fields[adhoc_field::MODEL].label, "model");
            assert_eq!(state.fields[adhoc_field::PROMPT].label, "prompt");
            assert!(state.fields[adhoc_field::PROMPT].required);
            // Focus starts on the target combobox (field 0).
            assert_eq!(state.focus, adhoc_field::TARGET);
        }
        other => panic!("expected adhoc Form, got {other:?}"),
    }
}

#[test]
fn adhoc_empty_target_enqueues_temp_no_ref() {
    let mut a = app();
    a.update(ch('s'));
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
    a.update(ch('s'));
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
    a.update(ch('s'));
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
    a.update(ch('s'));
    focus_field(&mut a, adhoc_field::TARGET);
    type_str(&mut a, "45");
    let up = fill_prompt_and_submit(&mut a, "fix pr");
    let p = enqueue_params(&up);
    assert_eq!(p["ref"], "pr:45");
}

#[test]
fn adhoc_s_on_queue_pane_opens_blank_ignoring_selected_task() {
    // A new adhoc task has nothing to do with which past task is selected, so
    // the QUEUE `s` never prefills the target from the cursor row's worktree.
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
    a.update(ch('s')); // queue focused by default, task row under the cursor
    match &a.mode {
        Mode::Form { state, .. } => {
            assert!(state.fields[adhoc_field::TARGET].value.is_empty(), "queue schedule opens blank");
        }
        other => panic!("expected adhoc Form, got {other:?}"),
    }
}

#[test]
fn schedule_and_create_inert_on_worktrees_and_tasks() {
    // Create/schedule chip lives on QUEUE only — `s`/`c` must not open the form
    // from TASKS or WORKTREES (and `c` is cron on TASKS, not create).
    let mut a = app();
    // Tab Queue → Tasks.
    a.update(keyc(KeyCode::Tab));
    a.update(ch('s'));
    assert!(matches!(a.mode, Mode::List), "s inert on TASKS");
    // Tab Tasks → Worktrees.
    a.update(keyc(KeyCode::Tab));
    a.update(ch('s'));
    assert!(matches!(a.mode, Mode::List), "s inert on WORKTREES");
    a.update(ch('c'));
    assert!(matches!(a.mode, Mode::List), "c inert on WORKTREES (no create chip)");
}

#[test]
fn adhoc_esc_cancels_without_cmd() {
    let mut a = app();
    a.update(ch('s'));
    let u = a.update(esc());
    assert!(matches!(a.mode, Mode::List));
    assert!(u.cmds.is_empty());
}

#[test]
fn shift_enter_inserts_newline_in_prompt() {
    let mut a = app();
    a.update(ch('s'));
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
fn session_field_opens_inline_dropdown_for_existing_worktree() {
    // Adhoc session pick is an INLINE dropdown on the form (not Mode::SessionPick).
    // Typing an existing worktree into the target already kicks `listSessions`
    // (via `adhoc_on_target_changed`); Enter on session just opens the list.
    let mut a = app();
    a.update(ch('s'));
    focus_field(&mut a, adhoc_field::TARGET);
    let typed = type_str_cmds(&mut a, "platform.wt-a");
    assert!(
        typed.iter().any(|c| matches!(c, Cmd::FetchSessions { .. })),
        "typing an existing worktree target prefetches sessions"
    );
    focus_field(&mut a, adhoc_field::SESSION);
    a.update(enter());
    match &a.mode {
        Mode::Form { state, .. } => {
            assert!(state.dropdown_open, "session field opens inline list");
            assert_eq!(state.focus, adhoc_field::SESSION);
            assert_eq!(state.sessions_for.as_deref(), Some("platform.wt-a"));
        }
        other => panic!("expected Form with open session dropdown, got {other:?}"),
    }
}

#[test]
fn session_field_is_noop_without_an_existing_worktree_target() {
    let mut a = app();
    a.update(ch('s')); // empty target
    focus_field(&mut a, adhoc_field::SESSION);
    a.update(enter());
    assert!(matches!(a.mode, Mode::Form { .. }), "stays on the form");
    assert!(a.status_line.is_some(), "explains why sessions aren't offered");
}

#[test]
fn session_dropdown_resume_pins_and_preserves_prompt() {
    let mut a = app();
    a.update(ch('s'));
    focus_field(&mut a, adhoc_field::TARGET);
    type_str(&mut a, "platform.wt-a");
    focus_field(&mut a, adhoc_field::PROMPT);
    type_str(&mut a, "continue please");
    focus_field(&mut a, adhoc_field::SESSION);
    a.update(enter()); // open inline session dropdown (loading)
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
    // Dropdown rows: New session (0), then sessions (1..). One Down → sess-1.
    a.update(keyc(KeyCode::Down));
    a.update(enter()); // pick → pin on the form
    match &a.mode {
        Mode::Form { state, action } => {
            assert!(!state.dropdown_open);
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
        other => panic!("expected the adhoc Form after pick, got {other:?}"),
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
fn session_pick_scopes_model_options_to_provider() {
    // Claude session → only claude models; New session → full catalog again.
    let mut a = app();
    a.update(ch('s'));
    focus_field(&mut a, adhoc_field::TARGET);
    type_str(&mut a, "platform.wt-a");
    focus_field(&mut a, adhoc_field::SESSION);
    a.update(enter());
    a.update(Event::SessionsLoaded {
        worktree: "platform.wt-a".into(),
        result: Ok(vec![
            SessionChoice {
                session_id: "c1".into(),
                label: "Claude work".into(),
                mtime_ms: 3_000,
                model: Some("claude/claude-sonnet-5".into()),
                provider: Some("claude".into()),
            },
            SessionChoice {
                session_id: "g1".into(),
                label: "Grok work".into(),
                mtime_ms: 2_000,
                model: Some("grok/grok-4.5".into()),
                provider: Some("grok".into()),
            },
        ]),
    });
    // Pick claude session (row 1).
    a.update(keyc(KeyCode::Down));
    a.update(enter());
    match &a.mode {
        Mode::Form { state, .. } => {
            assert_eq!(state.fields[adhoc_field::SESSION].value, "↻ Claude work");
            let opts = match &state.fields[adhoc_field::MODEL].kind {
                crate::view::form::FieldKind::Dropdown { options } => options,
                other => panic!("expected model Dropdown, got {other:?}"),
            };
            assert!(
                opts.iter().all(|o| o.value.is_empty() || o.value.starts_with("claude/")),
                "claude session must only offer claude models: {opts:?}"
            );
            assert!(
                opts.iter().any(|o| o.value == "claude/claude-sonnet-5"),
                "session model should be in the scoped list"
            );
            assert_eq!(
                state.fields[adhoc_field::MODEL].value,
                "claude/claude-sonnet-5",
                "preselect the session's model"
            );
            assert!(
                !opts.iter().any(|o| o.value.starts_with("grok/")),
                "grok models must not appear under a claude session"
            );
        }
        other => panic!("{other:?}"),
    }
    // Pick New session → full catalog restored.
    focus_field(&mut a, adhoc_field::SESSION);
    a.update(enter());
    a.update(enter()); // row 0 = New session
    match &a.mode {
        Mode::Form { state, .. } => {
            assert_eq!(state.fields[adhoc_field::SESSION].value, "New session");
            let opts = match &state.fields[adhoc_field::MODEL].kind {
                crate::view::form::FieldKind::Dropdown { options } => options,
                other => panic!("{other:?}"),
            };
            assert!(
                opts.iter().any(|o| o.value.starts_with("claude/")),
                "new session offers claude"
            );
            assert!(
                opts.iter().any(|o| o.value.starts_with("grok/")),
                "new session offers grok"
            );
        }
        other => panic!("{other:?}"),
    }
    // Grok session → only grok.
    focus_field(&mut a, adhoc_field::SESSION);
    a.update(enter());
    a.update(keyc(KeyCode::Down));
    a.update(keyc(KeyCode::Down)); // row 2 = grok session
    a.update(enter());
    match &a.mode {
        Mode::Form { state, .. } => {
            let opts = match &state.fields[adhoc_field::MODEL].kind {
                crate::view::form::FieldKind::Dropdown { options } => options,
                other => panic!("{other:?}"),
            };
            assert!(
                opts.iter().all(|o| o.value.starts_with("grok/")),
                "grok session must only offer grok models: {opts:?}"
            );
            assert_eq!(state.fields[adhoc_field::MODEL].value, "grok/grok-4.5");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn session_dropdown_esc_closes_list_leaving_form() {
    let mut a = app();
    a.update(ch('s'));
    focus_field(&mut a, adhoc_field::TARGET);
    type_str(&mut a, "platform.wt-a");
    focus_field(&mut a, adhoc_field::PROMPT);
    type_str(&mut a, "keep this");
    focus_field(&mut a, adhoc_field::SESSION);
    a.update(enter()); // open inline session dropdown
    a.update(esc()); // closes the list only (form stays open)
    match &a.mode {
        Mode::Form { state, .. } => {
            assert!(!state.dropdown_open);
            assert_eq!(state.fields[adhoc_field::PROMPT].value, "keep this");
            assert_eq!(state.fields[adhoc_field::SESSION].value, "New session");
        }
        other => panic!("expected Form after Esc-close of session list, got {other:?}"),
    }
}

#[test]
fn editing_the_target_clears_a_stale_session_pin() {
    let mut a = app();
    a.update(ch('s'));
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
