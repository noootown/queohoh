//! `Mode::Form` key + mouse handling (Task 4.3). Pins the shared form kit's
//! interaction wiring: Tab focus incl. the button row, text editing, the inline
//! dropdown, Shift+Enter newline, explicit-commit Primary/Cancel, Esc, and click
//! routing. The Primary action firing (enqueue / create+enqueue) is Phase 5.

use super::*;
use crate::event::SessionChoice;
use crate::hit::{ButtonKind, HitTarget};
use crate::ipc::types::{Project, StateSnapshot};
use crate::view::form::{Field, FocusKind, FormState};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}
fn ch(c: char) -> Event {
    Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
}
fn shift(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::SHIFT))
}

fn model_dropdown() -> Field {
    Field::dropdown(
        "model",
        vec!["fable".into(), "opus".into(), "sonnet".into(), "haiku".into()],
        "opus",
    )
}

/// App parked in `Mode::Form` with a `[model dropdown, prompt textarea(required)]`
/// form and the `CreateWorktree` action stub (Task 4.3 doesn't fire it).
fn form_app() -> App {
    let mut app = App::new("/tmp/runs".into(), "/tmp/daemon.sock".into());
    app.size = (120, 40);
    app.snapshot = Some(StateSnapshot {
        projects: vec![Project { name: "platform".into(), github_id: None }],
        ..Default::default()
    });
    app.connected = true;
    app.mode = Mode::Form {
        state: FormState::new(
            "New session",
            "Enqueue",
            vec![model_dropdown(), Field::textarea("prompt", "", true)],
        ),
        action: FormAction::NewSession {
            repo: "platform".into(),
            worktree: "platform.wt-a".into(),
            resume_session_id: None,
        },
    };
    app
}

fn focus_kind(app: &App) -> FocusKind {
    match &app.mode {
        Mode::Form { state, .. } => state.focus_kind(),
        other => panic!("expected Form, got {other:?}"),
    }
}
fn field_value(app: &App, i: usize) -> String {
    match &app.mode {
        Mode::Form { state, .. } => state.fields[i].value.clone(),
        other => panic!("expected Form, got {other:?}"),
    }
}

#[test]
fn tab_cycles_focus_over_fields_then_buttons() {
    let mut app = form_app();
    assert_eq!(focus_kind(&app), FocusKind::Field(0));
    app.update(key(KeyCode::Tab));
    assert_eq!(focus_kind(&app), FocusKind::Field(1));
    app.update(key(KeyCode::Tab));
    assert_eq!(focus_kind(&app), FocusKind::Primary);
    app.update(key(KeyCode::Tab));
    assert_eq!(focus_kind(&app), FocusKind::Cancel);
    app.update(key(KeyCode::Tab));
    assert_eq!(focus_kind(&app), FocusKind::Field(0)); // wraps
    app.update(shift(KeyCode::BackTab));
    assert_eq!(focus_kind(&app), FocusKind::Cancel); // wraps back
}

#[test]
fn arrow_keys_never_change_field_focus() {
    // App-wide standard: only Tab/Shift-Tab move focus. Up/Down on a focused
    // textarea navigate its lines (never stepping to the next/prev field or the
    // buttons), so a multiline prompt stays reviewable.
    let mut app = form_app();
    app.update(key(KeyCode::Tab)); // → prompt textarea (field 1)
    assert_eq!(focus_kind(&app), FocusKind::Field(1));
    // Down / Up from the textarea keep focus put (no jump to Primary / dropdown).
    app.update(key(KeyCode::Down));
    assert_eq!(focus_kind(&app), FocusKind::Field(1), "Down must not step focus");
    app.update(key(KeyCode::Up));
    assert_eq!(focus_kind(&app), FocusKind::Field(1), "Up must not step focus");
}

#[test]
fn up_down_navigate_textarea_lines_without_moving_focus() {
    let mut app = form_app();
    app.update(key(KeyCode::Tab)); // → prompt textarea
    for c in "ab".chars() { app.update(ch(c)); }
    app.update(shift(KeyCode::Enter)); // newline
    app.update(ch('c')); // value "ab\nc", caret after 'c' (line 1, col 1)
    // Up moves the caret to line 0 (col 1), so typing lands inside "ab".
    app.update(key(KeyCode::Up));
    app.update(ch('X'));
    assert_eq!(field_value(&app, 1), "aXb\nc");
    assert_eq!(focus_kind(&app), FocusKind::Field(1), "line nav keeps focus on the textarea");
}

#[test]
fn typing_edits_focused_textarea_and_shift_enter_inserts_newline() {
    let mut app = form_app();
    app.update(key(KeyCode::Tab)); // → prompt textarea
    for c in "do".chars() {
        app.update(ch(c));
    }
    app.update(shift(KeyCode::Enter));
    app.update(ch('x'));
    assert_eq!(field_value(&app, 1), "do\nx");
    app.update(key(KeyCode::Backspace));
    assert_eq!(field_value(&app, 1), "do\n");
}

#[test]
fn plain_enter_in_textarea_inserts_newline_and_never_submits() {
    // Enter in a textarea adds a newline; ONLY the Primary button submits. A
    // stray Enter after typing must never fire the action (explicit-commit rule).
    let mut app = form_app();
    app.update(key(KeyCode::Tab)); // → prompt textarea (field 1)
    app.update(ch('a'));
    app.update(key(KeyCode::Enter)); // plain Enter = newline here
    app.update(ch('b'));
    assert_eq!(field_value(&app, 1), "a\nb");
    assert!(matches!(app.mode, Mode::Form { .. }), "plain Enter in a textarea must not submit");
}

#[test]
fn plain_enter_in_single_line_input_advances_focus_and_never_submits() {
    let mut app = form_app();
    // A form whose first field is a single-line Input, all required fields filled
    // so that a (wrong) submit would visibly close the form.
    app.mode = Mode::Form {
        state: FormState::new(
            "Create Worktree",
            "Create",
            vec![
                Field::input("branch", "feature/x", true),
                model_dropdown(),
                Field::textarea("prompt", "p", true),
            ],
        ),
        action: FormAction::CreateWorktree { repo: "platform".into() },
    };
    assert_eq!(focus_kind(&app), FocusKind::Field(0)); // the input
    app.update(key(KeyCode::Enter));
    assert!(matches!(app.mode, Mode::Form { .. }), "plain Enter in an input must not submit");
    assert_eq!(focus_kind(&app), FocusKind::Field(1)); // advanced to the model dropdown
}

#[test]
fn dropdown_down_opens_then_moves_and_enter_picks() {
    let mut app = form_app();
    // Focus starts on the model dropdown. Down opens it (highlighting "opus").
    app.update(key(KeyCode::Down));
    match &app.mode {
        Mode::Form { state, .. } => {
            assert!(state.dropdown_open);
            assert_eq!(state.dropdown_index, 1); // opus
        }
        other => panic!("expected Form, got {other:?}"),
    }
    // Down again moves the open highlight; Enter commits it.
    app.update(key(KeyCode::Down));
    app.update(key(KeyCode::Enter));
    assert_eq!(field_value(&app, 0), "sonnet");
    match &app.mode {
        Mode::Form { state, .. } => assert!(!state.dropdown_open),
        other => panic!("expected Form, got {other:?}"),
    }
}

#[test]
fn enter_on_primary_with_valid_fields_returns_to_list() {
    let mut app = form_app();
    app.update(key(KeyCode::Tab)); // prompt
    for c in "hi".chars() {
        app.update(ch(c));
    }
    app.update(key(KeyCode::Tab)); // → Primary
    assert_eq!(focus_kind(&app), FocusKind::Primary);
    app.update(key(KeyCode::Enter));
    // Task 4.3 stub: valid submit closes the form (Phase 5 attaches the cmds).
    assert!(matches!(app.mode, Mode::List));
}

#[test]
fn enter_on_primary_with_empty_required_field_keeps_form_open_with_error() {
    let mut app = form_app();
    app.update(key(KeyCode::Tab)); // prompt (left empty)
    app.update(key(KeyCode::Tab)); // → Primary
    let up = app.update(key(KeyCode::Enter));
    assert!(up.cmds.is_empty());
    match &app.mode {
        Mode::Form { state, .. } => {
            assert_eq!(state.error, Some(1)); // the required prompt
            assert_eq!(state.focus_kind(), FocusKind::Field(1)); // focus moved to it
        }
        other => panic!("expected Form, got {other:?}"),
    }
}

#[test]
fn enter_on_cancel_and_esc_close_the_form() {
    let mut app = form_app();
    app.update(key(KeyCode::Tab)); // field1
    app.update(key(KeyCode::Tab)); // Primary
    app.update(key(KeyCode::Tab)); // Cancel
    app.update(key(KeyCode::Enter));
    assert!(matches!(app.mode, Mode::List));

    let mut app = form_app();
    app.update(key(KeyCode::Esc));
    assert!(matches!(app.mode, Mode::List));
}

#[test]
fn click_form_field_focuses_and_dropdown_field_opens() {
    let mut app = form_app();
    app.form_click(&HitTarget::FormField(0)); // the model dropdown
    match &app.mode {
        Mode::Form { state, .. } => {
            assert_eq!(state.focus_kind(), FocusKind::Field(0));
            assert!(state.dropdown_open, "clicking a dropdown field opens it");
        }
        other => panic!("expected Form, got {other:?}"),
    }
}

#[test]
fn click_dropdown_item_picks_it() {
    let mut app = form_app();
    app.form_click(&HitTarget::FormField(0)); // open the dropdown
    app.form_click(&HitTarget::DropdownItem(3)); // haiku
    assert_eq!(field_value(&app, 0), "haiku");
}

#[test]
fn click_cancel_button_closes_form() {
    let mut app = form_app();
    app.form_click(&HitTarget::Button(ButtonKind::Cancel));
    assert!(matches!(app.mode, Mode::List));
}

// --- Task 5.1: launcher New-session / resume → form → enqueue(model) ---

fn enter() -> Event {
    key(KeyCode::Enter)
}

/// App parked in the launcher (`Mode::SessionPick`) for a worktree, with two
/// loaded resumable sessions and focus on Next.
fn launcher_app() -> App {
    let mut app = App::new("/tmp/runs".into(), "/tmp/daemon.sock".into());
    app.size = (120, 40);
    app.snapshot = Some(StateSnapshot {
        projects: vec![Project { name: "platform".into(), github_id: None }],
        ..Default::default()
    });
    app.connected = true;
    app.mode = Mode::SessionPick {
        repo: "platform".into(),
        worktree: "platform.wt-a".into(),
        items: vec![
            SessionChoice { session_id: "s1".into(), label: "fix login".into(), mtime_ms: 0, model: None },
            SessionChoice { session_id: "s2".into(), label: "add tests".into(), mtime_ms: 0, model: None },
        ],
        loading: false,
        index: 0,
        query: String::new(),
        focus: ButtonKind::Confirm,
    };
    app
}

fn enqueue_params(up: &Update) -> serde_json::Value {
    for c in &up.cmds {
        if let Cmd::Rpc { call, .. } = c
            && call.method == "enqueue"
        {
            return call.params.clone();
        }
    }
    panic!("no enqueue Cmd::Rpc in {:?}", up.cmds);
}

#[test]
fn launcher_new_session_opens_form_with_model_and_prompt() {
    let mut app = launcher_app();
    app.update(enter()); // Next on New session (index 0)
    match &app.mode {
        Mode::Form { state, action } => {
            assert_eq!(state.fields.len(), 2);
            assert_eq!(state.fields[0].label, "model");
            assert_eq!(state.fields[0].value, "opus"); // resolved default (fallback)
            assert_eq!(state.fields[1].label, "prompt");
            assert!(state.fields[1].required);
            match action {
                FormAction::NewSession { repo, worktree, resume_session_id } => {
                    assert_eq!(repo, "platform");
                    assert_eq!(worktree, "platform.wt-a");
                    assert!(resume_session_id.is_none());
                }
                other => panic!("expected NewSession, got {other:?}"),
            }
        }
        other => panic!("expected Form, got {other:?}"),
    }
}

#[test]
fn launcher_new_session_form_enqueues_with_model_and_prompt() {
    let mut app = launcher_app();
    app.update(enter()); // → form (model=opus, prompt empty)
    app.update(key(KeyCode::Tab)); // → prompt textarea
    for c in "do the thing".chars() {
        app.update(ch(c));
    }
    app.update(key(KeyCode::Tab)); // → Primary
    let up = app.update(enter());
    assert!(matches!(app.mode, Mode::List));
    let params = enqueue_params(&up);
    assert_eq!(params["prompt"], "do the thing");
    assert_eq!(params["repo"], "platform");
    assert_eq!(params["worktree"], "platform.wt-a");
    assert_eq!(params["model"], "opus");
    assert!(params.get("resume_session_id").is_none());
}

#[test]
fn launcher_resume_row_carries_session_id_into_enqueue() {
    let mut app = launcher_app();
    app.update(key(KeyCode::Down)); // New(0) → Create(1)
    app.update(key(KeyCode::Down)); // → first session (view row 2 = s1)
    app.update(enter()); // → form, resume s1
    match &app.mode {
        Mode::Form { action: FormAction::NewSession { resume_session_id, .. }, .. } => {
            assert_eq!(resume_session_id.as_deref(), Some("s1"));
        }
        other => panic!("expected NewSession resume form, got {other:?}"),
    }
    app.update(key(KeyCode::Tab)); // prompt
    for c in "keep going".chars() {
        app.update(ch(c));
    }
    app.update(key(KeyCode::Tab)); // Primary
    let up = app.update(enter());
    let params = enqueue_params(&up);
    assert_eq!(params["resume_session_id"], "s1");
    assert_eq!(params["prompt"], "keep going");
}

// --- Task 5.2: launcher Create Worktree → form → create + enqueue ---

#[test]
fn launcher_create_worktree_opens_three_field_form() {
    let mut app = launcher_app();
    app.update(key(KeyCode::Down)); // New(0) → Create Worktree(1)
    app.update(enter());
    match &app.mode {
        Mode::Form { state, action } => {
            assert_eq!(state.fields.len(), 3);
            assert_eq!(state.fields[0].label, "branch / worktree name");
            assert!(state.fields[0].required);
            assert_eq!(state.fields[1].label, "model");
            assert_eq!(state.fields[2].label, "prompt");
            assert!(state.fields[2].required);
            assert!(matches!(action, FormAction::CreateWorktree { repo } if repo == "platform"));
        }
        other => panic!("expected Form, got {other:?}"),
    }
}

fn open_create_worktree_form(app: &mut App) {
    app.update(key(KeyCode::Down)); // → Create Worktree row
    app.update(enter()); // → form (focus on name input)
}

#[test]
fn create_worktree_invalid_branch_keeps_form_open_with_name_error() {
    let mut app = launcher_app();
    open_create_worktree_form(&mut app);
    for c in "bad name".chars() {
        app.update(ch(c)); // a space is an invalid branch char
    }
    // Fill the prompt so the only failure is the branch syntax.
    app.update(key(KeyCode::Tab)); // model
    app.update(key(KeyCode::Tab)); // prompt
    app.update(ch('p'));
    app.update(key(KeyCode::Tab)); // Primary
    let up = app.update(enter());
    assert!(up.cmds.is_empty());
    match &app.mode {
        Mode::Form { state, action: FormAction::CreateWorktree { .. } } => {
            assert_eq!(state.error, Some(0)); // name field flagged
            assert_eq!(state.focus_kind(), FocusKind::Field(0));
        }
        other => panic!("expected Form still open, got {other:?}"),
    }
}

#[test]
fn create_worktree_valid_fires_create_then_enqueue() {
    use crate::event::EnqueueAfter;
    let mut app = launcher_app();
    open_create_worktree_form(&mut app);
    for c in "feat-x".chars() {
        app.update(ch(c)); // valid branch name
    }
    app.update(key(KeyCode::Tab)); // model (opus default)
    app.update(key(KeyCode::Tab)); // prompt
    for c in "build it".chars() {
        app.update(ch(c));
    }
    app.update(key(KeyCode::Tab)); // Primary
    let up = app.update(enter());
    assert!(matches!(app.mode, Mode::List));
    match &up.cmds[..] {
        [Cmd::CreateWorktree { repo, name, enqueue: Some(EnqueueAfter { prompt, model }) }] => {
            assert_eq!(repo, "platform");
            assert_eq!(name, "feat-x");
            assert_eq!(prompt, "build it");
            assert_eq!(model, "opus");
        }
        other => panic!("expected CreateWorktree+enqueue, got {other:?}"),
    }
}

#[test]
fn paste_into_input_field_collapses_control_chars() {
    // Single-line input: a multiline paste can't smuggle a newline in.
    let mut app = launcher_app();
    open_create_worktree_form(&mut app); // focus on the name Input field (0)
    app.update(Event::Paste("do a\nthen b".into()));
    assert_eq!(field_value(&app, 0), "do a then b");
}

#[test]
fn paste_into_textarea_preserves_newlines() {
    let mut app = form_app(); // [model dropdown, prompt textarea]
    app.update(key(KeyCode::Tab)); // → prompt textarea
    app.update(Event::Paste("line1\nline2".into()));
    assert_eq!(field_value(&app, 1), "line1\nline2");
}

#[test]
fn resolve_default_model_prefers_project_override_then_global_then_opus() {
    use crate::ipc::types::{SettingsModels, SettingsPayload, SettingsProjectLayer};
    let mut app = launcher_app();
    // No settings fetched → fallback opus.
    assert_eq!(app.resolve_default_model("platform"), "opus");
    // Global default only.
    app.settings = Some(Some(SettingsPayload {
        models: SettingsModels { default_model: "sonnet".into(), ..Default::default() },
    }));
    assert_eq!(app.resolve_default_model("platform"), "sonnet");
    // Project override wins over global.
    app.settings = Some(Some(SettingsPayload {
        models: SettingsModels {
            default_model: "sonnet".into(),
            projects: vec![SettingsProjectLayer {
                repo: "platform".into(),
                default_model: "haiku".into(),
                ..Default::default()
            }],
            ..Default::default()
        },
    }));
    assert_eq!(app.resolve_default_model("platform"), "haiku");
}
