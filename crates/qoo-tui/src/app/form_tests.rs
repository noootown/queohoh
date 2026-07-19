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
        vec!["claude-fable-5".into(), "claude-opus-4.8".into(), "claude-sonnet-5".into(), "claude-haiku-4.5".into()],
        "claude-opus-4.8",
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
    // Focus starts on the model dropdown. Down opens it (highlighting "claude-opus-4.8").
    app.update(key(KeyCode::Down));
    match &app.mode {
        Mode::Form { state, .. } => {
            assert!(state.dropdown_open);
            assert_eq!(state.dropdown_index, 1); // claude-opus-4.8
        }
        other => panic!("expected Form, got {other:?}"),
    }
    // Down again moves the open highlight; Enter commits it.
    app.update(key(KeyCode::Down));
    app.update(key(KeyCode::Enter));
    assert_eq!(field_value(&app, 0), "claude-sonnet-5");
    match &app.mode {
        Mode::Form { state, .. } => assert!(!state.dropdown_open),
        other => panic!("expected Form, got {other:?}"),
    }
}

fn dropdown_index(app: &App) -> usize {
    match &app.mode {
        Mode::Form { state, .. } => state.dropdown_index,
        other => panic!("expected Form, got {other:?}"),
    }
}

// Options: claude-fable-5(0) claude-opus-4.8(1) claude-sonnet-5(2) claude-haiku-4.5(3),
// default "claude-opus-4.8" so open → idx 1.
#[test]
fn dropdown_down_wraps_from_last_to_first() {
    let mut app = form_app();
    app.update(key(KeyCode::Down)); // open → claude-opus-4.8 (1)
    app.update(key(KeyCode::Down)); // → claude-sonnet-5 (2)
    app.update(key(KeyCode::Down)); // → claude-haiku-4.5 (3), the last option
    assert_eq!(dropdown_index(&app), 3);
    app.update(key(KeyCode::Down)); // Down on last wraps to first
    assert_eq!(dropdown_index(&app), 0);
}

#[test]
fn dropdown_up_wraps_from_first_to_last() {
    let mut app = form_app();
    app.update(key(KeyCode::Down)); // open → claude-opus-4.8 (1)
    app.update(key(KeyCode::Up)); // → claude-fable-5 (0), the first option
    assert_eq!(dropdown_index(&app), 0);
    app.update(key(KeyCode::Up)); // Up on first wraps to last
    assert_eq!(dropdown_index(&app), 3);
}

#[test]
fn single_option_dropdown_stays_put_on_up_and_down() {
    let mut app = form_app();
    app.mode = Mode::Form {
        state: FormState::new(
            "New session",
            "Enqueue",
            vec![Field::dropdown("model", vec!["only".into()], "only"), Field::textarea("prompt", "", true)],
        ),
        action: FormAction::NewSession {
            repo: "platform".into(),
            worktree: "platform.wt-a".into(),
            resume_session_id: None,
        },
    };
    app.update(key(KeyCode::Down)); // open → only (0)
    assert_eq!(dropdown_index(&app), 0);
    app.update(key(KeyCode::Down)); // wrap-to-self, no move, no crash
    assert_eq!(dropdown_index(&app), 0);
    app.update(key(KeyCode::Up)); // wrap-to-self, no move, no crash
    assert_eq!(dropdown_index(&app), 0);
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
    app.form_click(&HitTarget::DropdownItem(3)); // claude-haiku-4.5
    assert_eq!(field_value(&app, 0), "claude-haiku-4.5");
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
            SessionChoice { session_id: "s1".into(), label: "fix login".into(), mtime_ms: 0, model: None, provider: None },
            SessionChoice { session_id: "s2".into(), label: "add tests".into(), mtime_ms: 0, model: None, provider: None },
        ],
        loading: false,
        index: 0,
        query: String::new(),
        focus: ButtonKind::Confirm,
        ret: SessionPickReturn::Launch,
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
            // The ad-hoc preselect is the head option (value "" = leave model
            // unset → the daemon resolves the chain), not a concrete model.
            assert_eq!(state.fields[0].value, "");
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
fn launcher_new_session_form_enqueues_picked_model_ref_and_prompt() {
    let mut app = launcher_app();
    app.update(enter()); // → form (focus on model dropdown, head "" preselected)
    // Pick a concrete catalog model: Down opens the list (highlight = head idx 0),
    // two more Downs reach `claude/claude-opus-4.8` (built-in fallback order: head,
    // fable, opus, …), Enter commits its VALUE `claude/claude-opus-4.8`
    // (display `claude-opus-4.8 (claude)`).
    app.update(key(KeyCode::Down)); // open
    app.update(key(KeyCode::Down)); // → claude/claude-fable-5
    app.update(key(KeyCode::Down)); // → claude/claude-opus-4.8
    app.update(enter()); // pick
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
    assert_eq!(params["model"], "claude/claude-opus-4.8");
    // A concrete pick is an explicit dialog choice: pinned so the worker runs
    // it exactly (no active-provider re-head, no fallback).
    assert_eq!(params["model_pinned"], true);
    assert!(params.get("resume_session_id").is_none());
}

#[test]
fn launcher_new_session_head_option_omits_the_model_param() {
    // Leaving the model dropdown on its head option (value "") must send NO
    // `model` param — the daemon then resolves the default chain. No pin
    // either: an unset model has nothing to pin.
    let mut app = launcher_app();
    app.update(enter()); // → form; head "" preselected, untouched
    app.update(key(KeyCode::Tab)); // → prompt
    for c in "leave it".chars() {
        app.update(ch(c));
    }
    app.update(key(KeyCode::Tab)); // → Primary
    let up = app.update(enter());
    let params = enqueue_params(&up);
    assert_eq!(params["prompt"], "leave it");
    assert!(params.get("model").is_none(), "head option leaves model unset");
    assert!(
        params.get("model_pinned").is_none(),
        "head option sends no model_pinned either"
    );
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
            // Model first (initial focus), then the branch name + prompt.
            assert_eq!(state.fields[0].label, "model");
            assert_eq!(state.fields[1].label, "branch / worktree name");
            assert!(state.fields[1].required);
            assert_eq!(state.fields[2].label, "prompt");
            assert!(state.fields[2].required);
            assert!(matches!(action, FormAction::CreateWorktree { repo } if repo == "platform"));
        }
        other => panic!("expected Form, got {other:?}"),
    }
}

fn open_create_worktree_form(app: &mut App) {
    app.update(key(KeyCode::Down)); // → Create Worktree row
    app.update(enter()); // → form (focus on the leading model dropdown)
}

#[test]
fn create_worktree_invalid_branch_keeps_form_open_with_name_error() {
    let mut app = launcher_app();
    open_create_worktree_form(&mut app);
    app.update(key(KeyCode::Tab)); // model → branch name (field 1)
    for c in "bad name".chars() {
        app.update(ch(c)); // a space is an invalid branch char
    }
    // Fill the prompt so the only failure is the branch syntax.
    app.update(key(KeyCode::Tab)); // → prompt
    app.update(ch('p'));
    app.update(key(KeyCode::Tab)); // → Primary
    let up = app.update(enter());
    assert!(up.cmds.is_empty());
    match &app.mode {
        Mode::Form { state, action: FormAction::CreateWorktree { .. } } => {
            assert_eq!(state.error, Some(1)); // name field flagged
            assert_eq!(state.focus_kind(), FocusKind::Field(1));
        }
        other => panic!("expected Form still open, got {other:?}"),
    }
}

#[test]
fn create_worktree_valid_fires_create_then_enqueue() {
    use crate::event::EnqueueAfter;
    let mut app = launcher_app();
    open_create_worktree_form(&mut app);
    app.update(key(KeyCode::Tab)); // model (head "" default — left as-is) → name
    for c in "feat-x".chars() {
        app.update(ch(c)); // valid branch name
    }
    app.update(key(KeyCode::Tab)); // → prompt
    for c in "build it".chars() {
        app.update(ch(c));
    }
    app.update(key(KeyCode::Tab)); // → Primary
    let up = app.update(enter());
    assert!(matches!(app.mode, Mode::List));
    match &up.cmds[..] {
        [Cmd::CreateWorktree { repo, name, enqueue: Some(EnqueueAfter { prompt, model }) }] => {
            assert_eq!(repo, "platform");
            assert_eq!(name, "feat-x");
            assert_eq!(prompt, "build it");
            // Head option left untouched → empty model (the enqueue-after path
            // drops it, so the daemon resolves the default chain).
            assert_eq!(model, "");
        }
        other => panic!("expected CreateWorktree+enqueue, got {other:?}"),
    }
}

#[test]
fn paste_into_input_field_collapses_control_chars() {
    // Single-line input: a multiline paste can't smuggle a newline in.
    let mut app = launcher_app();
    open_create_worktree_form(&mut app); // focus on the model dropdown (0)
    app.update(key(KeyCode::Tab)); // → name Input field (1)
    app.update(Event::Paste("do a\nthen b".into()));
    assert_eq!(field_value(&app, 1), "do a then b");
}

#[test]
fn paste_into_textarea_preserves_newlines() {
    let mut app = form_app(); // [model dropdown, prompt textarea]
    app.update(key(KeyCode::Tab)); // → prompt textarea
    app.update(Event::Paste("line1\nline2".into()));
    assert_eq!(field_value(&app, 1), "line1\nline2");
}

// --- Task 7: catalog-driven model dropdown ---------------------------------

use crate::ipc::types::{
    CatalogEntry, DefaultModels, DefaultModelsProject, SettingsPayload, SettingsProvider,
};

/// A settings payload with a two-provider catalog (claude + grok), grok's second
/// entry hidden, codex disabled, and per-repo default_models — the fixture the
/// catalog-driven dropdown tests share.
fn catalog_settings() -> SettingsPayload {
    let e = |provider: &str, id: &str, label: &str, hidden: bool| CatalogEntry {
        provider: provider.into(),
        id: id.into(),
        label: label.into(),
        hidden,
    };
    SettingsPayload {
        catalog: vec![
            e("claude", "claude-opus-4-8", "claude-opus-4.8", false),
            e("claude", "claude-sonnet-5", "claude-sonnet-5", false),
            e("grok", "grok-4.5", "grok-4.5", false),
            e("grok", "grok-legacy", "legacy", true), // hidden → filtered
            e("codex", "gpt-5.6-sol", "gpt-5.6-sol", false), // codex disabled → filtered
        ],
        active_provider: "claude".into(),
        default_models: DefaultModels {
            global: vec!["claude/claude-opus-4.8".into(), "grok/grok-4.5".into()],
            projects: vec![DefaultModelsProject {
                name: "platform".into(),
                default_models: vec!["grok/grok-4.5".into()],
                source: "/repos/platform/vars.yaml".into(),
            }],
        },
        providers: vec![
            SettingsProvider { name: "claude".into(), enabled: true, bin: None },
            SettingsProvider { name: "grok".into(), enabled: true, bin: None },
            SettingsProvider { name: "codex".into(), enabled: false, bin: None },
        ],
    }
}

/// The model dropdown's option VALUES (head "" first), read straight off the
/// built `model` field so the test pins the actual dropdown, not a helper.
fn model_option_values(app: &App, repo: &str) -> Vec<String> {
    match &app.model_field(repo).kind {
        crate::view::form::FieldKind::Dropdown { options } => {
            options.iter().map(|o| o.value.clone()).collect()
        }
        other => panic!("expected a Dropdown, got {other:?}"),
    }
}

#[test]
fn model_option_values_fall_back_to_builtin_catalog_when_settings_absent() {
    // Stale/absent settings → the built-in mirror (claude + grok groups; codex is
    // omitted from the mirror). Head "" first, then one `provider/label` per
    // VISIBLE entry — grok/grok-composer-2.5-fast is hidden, so the grok group shows only
    // grok-4.5.
    let app = launcher_app();
    assert_eq!(app.settings, None);
    assert_eq!(
        model_option_values(&app, "platform"),
        vec![
            "",
            "claude/claude-fable-5",
            "claude/claude-opus-4.8",
            "claude/claude-sonnet-5",
            "claude/claude-haiku-4.5",
            "grok/grok-4.5",
        ]
    );
}

#[test]
fn model_option_values_use_payload_catalog_in_order_filtering_hidden_and_disabled() {
    let mut app = launcher_app();
    app.settings = Some(Some(catalog_settings()));
    // Head "" + visible entries in catalog order; the hidden grok/legacy and the
    // disabled-provider codex/gpt-5.6-sol are both filtered out.
    let values = model_option_values(&app, "platform");
    assert_eq!(values, vec!["", "claude/claude-opus-4.8", "claude/claude-sonnet-5", "grok/grok-4.5"]);
    assert!(!values.iter().any(|v| v == "grok/legacy"), "hidden entry filtered");
    assert!(!values.iter().any(|v| v == "codex/gpt-5.6-sol"), "disabled provider filtered");
}

#[test]
fn model_display_is_label_paren_provider_and_head_label_shows_refs() {
    // model_display: `label (provider)`.
    let opus = CatalogEntry { provider: "claude".into(), id: "claude-opus-4-8".into(), label: "claude-opus-4.8".into(), hidden: false };
    assert_eq!(opus.model_display(), "claude-opus-4.8 (claude)");
    // Head label: repo default_models (no marker), a def's list (a `def:` marker),
    // and the bare `default` when there are no refs.
    let refs = vec!["claude/claude-opus-4.8".to_string(), "grok/grok-4.5".to_string()];
    assert_eq!(
        crate::app::form::default_head_label(&refs, false),
        "default (claude/claude-opus-4.8 → grok/grok-4.5)"
    );
    assert_eq!(
        crate::app::form::default_head_label(&refs, true),
        "default (def: claude/claude-opus-4.8 → grok/grok-4.5)"
    );
    assert_eq!(crate::app::form::default_head_label(&[], false), "default");
}

#[test]
fn model_field_head_option_labels_from_repo_default_models() {
    // The ad-hoc model field: head option value "" (leave unset), display from
    // the repo's default_models (project override wins over global).
    let mut app = launcher_app();
    app.settings = Some(Some(catalog_settings()));
    let field = app.model_field("platform");
    assert_eq!(field.value, "", "head option preselected");
    match &field.kind {
        crate::view::form::FieldKind::Dropdown { options } => {
            // Head first: value "", label = the single model the platform default
            // (grok/grok-4.5) RESOLVES to under the active provider (claude) —
            // the claude group head is prepended, so it resolves to claude/claude-opus-4.8.
            assert_eq!(options[0].value, "");
            assert_eq!(options[0].label, "default (claude-opus-4.8)");
            // Then the provider-first catalog (active=claude leads): value
            // `provider/label`, display `label (provider)`.
            assert_eq!(options[1].value, "claude/claude-opus-4.8");
            assert_eq!(options[1].label, "claude-opus-4.8 (claude)");
            assert_eq!(options[3].value, "grok/grok-4.5");
            assert_eq!(options[3].label, "grok-4.5 (grok)");
        }
        other => panic!("expected a labeled Dropdown, got {other:?}"),
    }
    // A repo with no project override falls back to the global chain — the head
    // shows only the resolved head, not the whole `→` chain.
    let global = app.model_field("other");
    match &global.kind {
        crate::view::form::FieldKind::Dropdown { options } => {
            assert_eq!(options[0].label, "default (claude-opus-4.8)");
        }
        other => panic!("expected a labeled Dropdown, got {other:?}"),
    }
}

#[test]
fn model_field_defaulting_selects_preferred_ref_or_falls_back_to_head() {
    // The resume path: a session's `provider/label` ref preselects when it names a
    // real visible option, else the head ("" = leave unset).
    let mut app = launcher_app();
    app.settings = Some(Some(catalog_settings()));
    let picked = app.model_field_defaulting("platform", Some("grok/grok-4.5"));
    assert_eq!(picked.value, "grok/grok-4.5");
    // A hidden ref is not a visible option → falls back to the head.
    let hidden = app.model_field_defaulting("platform", Some("grok/legacy"));
    assert_eq!(hidden.value, "");
    // A stale/foreign ref → head, never a phantom selection.
    let stale = app.model_field_defaulting("platform", Some("nonexistent/tier"));
    assert_eq!(stale.value, "");
}

#[test]
fn model_field_for_session_filters_by_provider() {
    // Schedule-form session→model coupling: pinned provider hides foreign models.
    let mut app = launcher_app();
    app.settings = Some(Some(catalog_settings()));
    let claude = app.model_field_for_session(
        "platform",
        Some("claude"),
        Some("claude/claude-sonnet-5"),
    );
    match &claude.kind {
        crate::view::form::FieldKind::Dropdown { options } => {
            assert!(options.iter().all(|o| o.value.starts_with("claude/")));
            assert!(!options.iter().any(|o| o.value.starts_with("grok/")));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(claude.value, "claude/claude-sonnet-5");
    // New session / unknown provider keeps the full catalog + empty head.
    let any = app.model_field_for_session("platform", None, None);
    match &any.kind {
        crate::view::form::FieldKind::Dropdown { options } => {
            assert_eq!(options[0].value, "");
            assert!(options.iter().any(|o| o.value.starts_with("claude/")));
            assert!(options.iter().any(|o| o.value.starts_with("grok/")));
        }
        other => panic!("{other:?}"),
    }
}

// --- Task 4: def-run model picker = effective chain -------------------------

/// Settings + snapshot with active_provider = grok (snapshot wins over settings
/// for `App::active_provider`). Shares the catalog_settings catalog/providers.
fn app_with_active_grok() -> App {
    let mut app = launcher_app();
    let mut settings = catalog_settings();
    settings.active_provider = "grok".into();
    app.settings = Some(Some(settings));
    if let Some(snap) = app.snapshot.as_mut() {
        snap.active_provider = Some("grok".into());
    }
    app
}

/// Option VALUES of a def-run model field (leads with the empty "" head).
fn def_model_option_values(field: &Field) -> Vec<String> {
    match &field.kind {
        crate::view::form::FieldKind::Dropdown { options } => {
            options.iter().map(|o| o.value.clone()).collect()
        }
        other => panic!("expected a Dropdown, got {other:?}"),
    }
}

#[test]
fn def_model_field_leads_with_default_head_then_full_catalog() {
    // Def `model: [claude/claude-opus-4.8, grok/grok-4.5]`, active=grok → the picker leads
    // with an EMPTY "" head labeled with the resolved head (grok/grok-4.5,
    // label-only) so a plain Run leaves the def's authored chain to the daemon;
    // the FULL visible catalog follows in provider-first order.
    let app = app_with_active_grok();
    let spec = crate::ipc::types::ModelRef::Many(vec![
        "claude/claude-opus-4.8".into(),
        "grok/grok-4.5".into(),
    ]);
    let field = app.def_model_field("platform", Some(&spec));
    assert_eq!(
        def_model_option_values(&field),
        vec!["", "grok/grok-4.5", "claude/claude-opus-4.8", "claude/claude-sonnet-5"]
    );
    assert_eq!(field.value, "", "the default head is preselected → unpinned");
    match &field.kind {
        crate::view::form::FieldKind::Dropdown { options } => {
            assert_eq!(options[0].label, "default (grok-4.5)");
            assert_eq!(options[1].label, "grok-4.5 (grok)");
            assert_eq!(options[2].label, "claude-opus-4.8 (claude)");
            assert_eq!(options[3].label, "claude-sonnet-5 (claude)");
        }
        other => panic!("expected labeled Dropdown, got {other:?}"),
    }
}

#[test]
fn def_model_field_head_labels_the_resolved_single_spec() {
    // Def `model: claude/claude-opus-4.8` only, active=grok → resolve_model_chain prepends
    // the grok group head, so the resolved head is grok/grok-4.5. The empty head
    // is labeled with it (label-only) and preselected (unpinned).
    let app = app_with_active_grok();
    let spec = crate::ipc::types::ModelRef::One("claude/claude-opus-4.8".into());
    let field = app.def_model_field("platform", Some(&spec));
    assert_eq!(field.value, "");
    match &field.kind {
        crate::view::form::FieldKind::Dropdown { options } => {
            assert_eq!(options[0].value, "");
            assert_eq!(options[0].label, "default (grok-4.5)");
        }
        other => panic!("expected labeled Dropdown, got {other:?}"),
    }
}

#[test]
fn adhoc_model_field_still_has_default_head_and_catalog() {
    // Ad-hoc create keeps the empty "" head + full visible catalog; the head
    // label now shows only the resolved default head under the active provider.
    let mut app = launcher_app();
    app.settings = Some(Some(catalog_settings()));
    let field = app.model_field("platform");
    assert_eq!(field.value, "");
    let values = model_option_values(&app, "platform");
    assert_eq!(values[0], "");
    assert!(values.contains(&"claude/claude-opus-4.8".into()));
    assert!(values.contains(&"grok/grok-4.5".into()));
    match &field.kind {
        crate::view::form::FieldKind::Dropdown { options } => {
            assert_eq!(options[0].label, "default (claude-opus-4.8)");
        }
        other => panic!("expected labeled Dropdown, got {other:?}"),
    }
}

#[test]
fn model_field_floats_active_provider_group_to_top_under_grok() {
    // New-session picker with grok active (the team's screenshot case): head =
    // the resolved default head, rendered label-only as `default (grok-4.5)`
    // (platform default is grok/grok-4.5, no re-head needed), then the grok
    // group floats above the claude group.
    let app = app_with_active_grok();
    let field = app.model_field("platform");
    assert_eq!(field.value, "");
    assert_eq!(
        model_option_values(&app, "platform"),
        vec!["", "grok/grok-4.5", "claude/claude-opus-4.8", "claude/claude-sonnet-5"]
    );
    match &field.kind {
        crate::view::form::FieldKind::Dropdown { options } => {
            assert_eq!(options[0].label, "default (grok-4.5)");
            assert_eq!(options[1].label, "grok-4.5 (grok)");
            assert_eq!(options[2].label, "claude-opus-4.8 (claude)");
        }
        other => panic!("expected labeled Dropdown, got {other:?}"),
    }
}
