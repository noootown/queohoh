use super::*;
use crate::ipc::types::{ArgSpec, DefinitionSummary, Project, StateSnapshot, WorktreeInfo};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashMap;

fn key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn arg(name: &str) -> ArgSpec {
    ArgSpec { name: name.into(), default: None, options: None, description: None }
}

fn dsum(repo: &str, name: &str, scope: &str, args: Vec<ArgSpec>) -> DefinitionSummary {
    DefinitionSummary { repo: repo.into(), name: name.into(), scope: scope.into(), args, has_discovery: false, cron: None, description: None, model: None }
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

fn fixture_app_with_defs(repo: &str, defs: Vec<DefinitionSummary>) -> App {
    let mut app = fixture_app_one_project(repo);
    app.defs_by_project.insert(repo.into(), defs);
    app
}

fn fixture_app_with_defs_and_worktree(
    repo: &str,
    defs: Vec<DefinitionSummary>,
    (wt_name, branch): (&str, &str),
) -> App {
    let mut app = fixture_app_one_project(repo);
    let mut wts = HashMap::new();
    wts.insert(
        repo.to_string(),
        vec![WorktreeInfo {
            name: format!("{repo}.{wt_name}"),
            path: format!("/wt/{wt_name}"),
            branch: branch.into(),
            ..Default::default()
        }],
    );
    app.snapshot = Some(StateSnapshot {
        projects: vec![Project { name: repo.into(), github_id: None }],
        worktrees: wts,
        ..Default::default()
    });
    app.defs_by_project.insert(repo.into(), defs);
    app
}

fn fixture_def_pick_defs(defs: Vec<DefinitionSummary>, worktree: Option<String>, branch: Option<String>) -> App {
    let mut app = App::new("/tmp/runs".into(), "/tmp/daemon.sock".into());
    app.size = (120, 40);
    app.mode = Mode::DefPick { defs, index: 0, worktree, branch, query: String::new(), preview_scroll: 0 };
    app
}

fn fixture_def_pick(names: Vec<&str>, worktree: Option<&str>, branch: Option<&str>) -> App {
    let defs = names.iter().map(|n| dsum("platform", n, "project", vec![])).collect();
    fixture_def_pick_defs(defs, worktree.map(Into::into), branch.map(Into::into))
}

// --- Step 6: lazy fetch + in-flight dedup ---
#[test]
fn reconcile_defs_fetches_once_and_dedups() {
    let mut app = fixture_app_one_project("platform");
    let cmd = app.reconcile_defs();
    assert!(matches!(cmd, Some(Cmd::FetchDefinitions { ref repo }) if repo == "platform"));
    assert!(app.defs_inflight.contains("platform"));
    assert!(app.reconcile_defs().is_none());
    app.update(Event::Definitions { repo: "platform".into(), defs: vec![] });
    assert!(app.defs_by_project.contains_key("platform"));
    assert!(!app.defs_inflight.contains("platform"));
    assert!(app.reconcile_defs().is_none());
}

#[test]
fn definitions_event_keeps_only_the_fetched_repos_defs() {
    // The daemon's `definitions` call returns entries for EVERY project (a
    // global def like squash-merge appears once per project). Caching the
    // unfiltered list rendered N duplicate rows in the TASKS pane.
    let mut app = fixture_app_one_project("platform");
    app.update(Event::Definitions {
        repo: "platform".into(),
        defs: vec![
            dsum("platform", "squash-merge", "global", vec![]),
            dsum("web", "squash-merge", "global", vec![]),
            dsum("dotfiles", "squash-merge", "global", vec![]),
            dsum("platform", "pr-review", "project", vec![]),
        ],
    });
    let cached = &app.defs_by_project["platform"];
    assert_eq!(
        cached.iter().map(|d| (d.repo.as_str(), d.name.as_str())).collect::<Vec<_>>(),
        vec![("platform", "squash-merge"), ("platform", "pr-review")]
    );
}

#[test]
fn reconcile_full_def_fetches_the_selected_def_once_and_caches_on_reply() {
    // Tasks pane focused, cursor on the only def: the reconcile emits ONE
    // FetchDefinition, dedups while in flight, and stops once the reply
    // fills `full_defs` — the "(loading definition…)" fix.
    let mut app = fixture_app_with_defs("platform", vec![dsum("platform", "pr-ready", "project", vec![])]);
    let mut ui = TabUiState::default();
    ui.last_list_pane = ListPane::Tasks;
    app.ui_by_tab.insert("platform".into(), ui);
    let cmd = app.reconcile_full_def();
    assert!(
        matches!(cmd, Some(Cmd::FetchDefinition { ref repo, ref name }) if repo == "platform" && name == "pr-ready")
    );
    assert!(app.reconcile_full_def().is_none(), "in-flight fetch dedups");
    app.update(Event::Definition {
        repo: "platform".into(),
        name: "pr-ready".into(),
        def: Some(Box::new(TaskDefinition::default())),
    });
    assert!(app.full_defs.contains_key("platform/pr-ready"));
    assert!(app.reconcile_full_def().is_none(), "cached def is not refetched");
}

#[test]
fn reconcile_full_def_ignores_non_tasks_panes_and_failed_replies_dont_loop() {
    let mut app = fixture_app_with_defs("platform", vec![dsum("platform", "pr-ready", "project", vec![])]);
    // Default UI (last pane = Queue): no Definition context, no fetch.
    assert!(app.reconcile_full_def().is_none());
    let mut ui = TabUiState::default();
    ui.last_list_pane = ListPane::Tasks;
    app.ui_by_tab.insert("platform".into(), ui);
    assert!(app.reconcile_full_def().is_some());
    // Failed reply leaves the poison marker: no refetch loop.
    app.update(Event::Definition { repo: "platform".into(), name: "pr-ready".into(), def: None });
    assert!(app.reconcile_full_def().is_none(), "failed fetch must not refetch-loop");
    // Invalidation clears the poison so the next reconcile can retry.
    app.update(Event::ActionResult { status: None, invalidate_defs_for: Some("platform".into()) });
    app.update(Event::Definitions {
        repo: "platform".into(),
        defs: vec![dsum("platform", "pr-ready", "project", vec![])],
    });
    assert!(app.reconcile_full_def().is_some(), "invalidation re-arms the fetch");
}

#[test]
fn action_result_invalidation_marks_inflight_so_reconcile_dedups() {
    // The eager re-fetch on invalidation marks the repo in flight; the event
    // loop's follow-up reconcile must not emit a duplicate fetch.
    let mut app = fixture_app_with_defs("platform", vec![dsum("platform", "x", "project", vec![])]);
    let u = app.update(Event::ActionResult { status: None, invalidate_defs_for: Some("platform".into()) });
    assert!(u.cmds.iter().any(|c| matches!(c, Cmd::FetchDefinitions { repo } if repo == "platform")));
    assert!(app.defs_inflight.contains("platform"));
    assert!(app.reconcile_defs().is_none(), "reconcile must dedup against the eager re-fetch");
}

// --- tasks-pane `r` runs the highlighted def (dispatch shapes) ---
#[test]
fn tasks_pane_r_zero_arg_dispatches_immediately() {
    let mut app = fixture_app_with_defs("platform", vec![dsum("platform", "noargs", "project", vec![])]);
    app.set_focus(PaneId::Tasks);
    let update = app.update(key(KeyCode::Char('r')));
    assert!(
        update.cmds.iter().any(|c| matches!(c, Cmd::Rpc { call, invalidate_defs_for, timeout_is_ok, .. }
            if call.method == "runDefinition"
                && call.params["repo"] == "platform"
                && call.params["name"] == "noargs"
                && call.params["args"] == serde_json::json!([])
                && call.params["source"] == "tui"
                && call.params.get("worktree").is_none()
                && invalidate_defs_for.as_deref() == Some("platform")
                && *timeout_is_ok)),
        "expected an immediate runDefinition Rpc, got {:?}", update.cmds,
    );
    assert!(matches!(app.mode, Mode::List));
}

#[test]
fn tasks_pane_r_with_args_opens_run_form_with_ambient_overlay() {
    let mut app = fixture_app_with_defs_and_worktree(
        "platform",
        vec![dsum("platform", "deploy", "project", vec![arg("source")])],
        ("wt-a", "jus-9-x"),
    );
    app.set_focus(PaneId::Tasks);
    let update = app.update(key(KeyCode::Char('r')));
    match &app.mode {
        Mode::DefArgs { form } => {
            assert_eq!(form.args[0].options.as_deref(), Some(&["jus-9-x".to_string()][..]));
            assert_eq!(form.values[0], "jus-9-x");
            assert!(form.fixed.is_empty());
            assert_eq!(form.initial_worktree, None);
        }
        other => panic!("expected DefArgs, got {other:?}"),
    }
    // Opening the form fetches the prompt for the right panel.
    assert!(
        update.cmds.iter().any(|c| matches!(c, Cmd::FetchDefinition { repo, name }
            if repo == "platform" && name == "deploy")),
        "expected a FetchDefinition, got {:?}", update.cmds,
    );
}

// --- task menu (`t`) → Mode::DefPick, ordering, context, empty guard ---
#[test]
fn task_menu_opens_def_pick_in_server_order() {
    let mut app = fixture_app_with_defs("platform", vec![
        dsum("platform", "autotest", "project", vec![]),
        dsum("platform", "squash-merge", "global", vec![arg("source")]),
    ]);
    // `t` is a WORKTREES chip (keymap-gated there); this fixture seeds no
    // worktree rows, so the menu still opens without row context.
    app.set_focus(PaneId::Worktrees);
    app.update(key(KeyCode::Char('t')));
    match &app.mode {
        Mode::DefPick { defs, index, worktree, branch, query, preview_scroll } => {
            assert_eq!(defs.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), vec!["autotest", "squash-merge"]);
            assert_eq!(*index, 0);
            // Worktrees focused but no worktree rows → no worktree context.
            assert_eq!(worktree.as_deref(), None);
            assert_eq!(branch.as_deref(), None);
            assert!(query.is_empty());
            assert_eq!(*preview_scroll, 0);
        }
        other => panic!("expected DefPick, got {other:?}"),
    }
}

#[test]
fn task_menu_from_worktrees_pane_carries_worktree_and_branch() {
    let mut app = fixture_app_with_defs_and_worktree(
        "platform",
        vec![dsum("platform", "autotest", "project", vec![])],
        ("wt-a", "jus-4-x"),
    );
    app.set_focus(PaneId::Worktrees);
    app.update(key(KeyCode::Char('t')));
    match &app.mode {
        Mode::DefPick { worktree, branch, .. } => {
            assert_eq!(worktree.as_deref(), Some("platform.wt-a"));
            assert_eq!(branch.as_deref(), Some("jus-4-x"));
        }
        other => panic!("expected DefPick, got {other:?}"),
    }
}

#[test]
fn task_menu_with_no_defs_sets_status_line() {
    let mut app = fixture_app_with_defs("platform", vec![]);
    app.set_focus(PaneId::Worktrees); // `t` is a WORKTREES chip
    let update = app.update(key(KeyCode::Char('t')));
    assert_eq!(app.status_line.as_deref(), Some("no task definitions found"));
    assert!(update.cmds.is_empty());
    assert!(matches!(app.mode, Mode::List));
}

#[test]
fn task_menu_prefetches_highlighted_def_prompt_once() {
    let mut app = fixture_app_with_defs("platform", vec![dsum("platform", "autotest", "project", vec![])]);
    app.set_focus(PaneId::Worktrees); // `t` is a WORKTREES chip
    // Opening the menu emits a FetchDefinition for the highlighted def.
    let u = app.update(key(KeyCode::Char('t')));
    assert!(
        u.cmds.iter().any(|c| matches!(c, Cmd::FetchDefinition { repo, name }
            if repo == "platform" && name == "autotest")),
        "expected a FetchDefinition, got {:?}", u.cmds,
    );
    assert!(app.full_defs_inflight.contains("platform/autotest"));
    // The reply populates full_defs and clears the in-flight flag.
    app.update(Event::Definition {
        repo: "platform".into(),
        name: "autotest".into(),
        def: Some(Box::new(crate::ipc::types::TaskDefinition { prompt: "hi".into(), ..Default::default() })),
    });
    assert!(app.full_defs.contains_key("platform/autotest"));
    assert!(!app.full_defs_inflight.contains("platform/autotest"));
    // A second highlight of the (now cached) def emits no fetch.
    let u2 = app.def_pick_move(0, 1, 1);
    assert!(!u2.cmds.iter().any(|c| matches!(c, Cmd::FetchDefinition { .. })));
}

// --- Mode::DefPick navigation + close ---
#[test]
fn def_pick_moves_circularly_and_closes_on_esc() {
    let mut app = fixture_def_pick(vec!["a", "b"], Some("platform.wt"), Some("jus-1-x"));
    app.update(key(KeyCode::Down)); // 0 -> 1
    assert!(matches!(app.mode, Mode::DefPick { index: 1, .. }));
    app.update(key(KeyCode::Down)); // wraps -> 0
    assert!(matches!(app.mode, Mode::DefPick { index: 0, .. }));
    app.update(key(KeyCode::Up)); // wraps back -> 1
    assert!(matches!(app.mode, Mode::DefPick { index: 1, .. }));
    app.update(key(KeyCode::Esc));
    assert!(matches!(app.mode, Mode::List));
}

#[test]
fn def_pick_nav_and_query_edits_reset_preview_scroll() {
    // ctrl+d/u paging was removed (wheel-only scroll); pin that nav and
    // query edits still reset a scrolled preview.
    fn set_scroll(app: &mut App, v: usize) {
        if let Mode::DefPick { preview_scroll, .. } = &mut app.mode {
            *preview_scroll = v;
        } else {
            panic!("expected DefPick");
        }
    }
    let mut app = fixture_def_pick(vec!["a", "b"], None, None);
    set_scroll(&mut app, 4);
    app.update(key(KeyCode::Down)); // highlight change resets the preview
    assert!(matches!(app.mode, Mode::DefPick { preview_scroll: 0, .. }));
    set_scroll(&mut app, 4);
    app.update(key(KeyCode::Char('a'))); // query edit resets too
    assert!(matches!(app.mode, Mode::DefPick { preview_scroll: 0, .. }));
}

#[test]
fn def_pick_q_types_into_filter_instead_of_closing() {
    let mut app = fixture_def_pick(vec!["alpha", "beta"], None, None);
    app.update(key(KeyCode::Char('q'))); // no longer closes — types 'q'
    match &app.mode {
        Mode::DefPick { query, index, .. } => {
            assert_eq!(query, "q");
            assert_eq!(*index, 0);
        }
        other => panic!("expected DefPick still open, got {other:?}"),
    }
}

#[test]
fn def_pick_typing_filters_and_enter_activates_match() {
    // Filter to "beta" then Enter dispatches its zero-arg run.
    let mut app = fixture_def_pick_defs(
        vec![dsum("platform", "alpha", "project", vec![]), dsum("platform", "beta", "project", vec![])],
        None,
        None,
    );
    app.update(key(KeyCode::Char('b'))); // query "b" → only "beta"
    let u = app.update(key(KeyCode::Enter));
    assert!(matches!(app.mode, Mode::List));
    assert!(
        u.cmds.iter().any(|c| matches!(c, Cmd::Rpc { call, .. }
            if call.method == "runDefinition" && call.params["name"] == "beta")),
        "expected runDefinition for beta, got {:?}", u.cmds,
    );
}

#[test]
fn def_pick_enter_zero_arg_dispatches_with_worktree() {
    let mut app = fixture_def_pick_defs(
        vec![dsum("platform", "autotest", "project", vec![])],
        Some("platform.wt-a".into()),
        Some("jus-1-x".into()),
    );
    let update = app.update(key(KeyCode::Enter));
    match &update.cmds[0] {
        Cmd::Rpc { call, invalidate_defs_for, .. } => {
            assert_eq!(call.method, "runDefinition");
            assert_eq!(call.params["worktree"], "platform.wt-a");
            assert_eq!(call.params["args"], serde_json::json!([]));
            assert_eq!(invalidate_defs_for.as_deref(), Some("platform"));
        }
        other => panic!("expected runDefinition, got {other:?}"),
    }
    assert!(matches!(app.mode, Mode::List));
}

#[test]
fn def_pick_enter_with_args_opens_def_args_with_fixed_context() {
    let mut app = fixture_def_pick_defs(
        vec![dsum("platform", "deploy", "project", vec![
            arg("source"),
            ArgSpec { default: Some("main".into()), ..arg("target") },
        ])],
        Some("platform.wt-a".into()),
        Some("jus-9-x".into()),
    );
    app.update(key(KeyCode::Enter));
    match &app.mode {
        Mode::DefArgs { form } => {
            assert_eq!(form.fixed.get("source").map(String::as_str), Some("jus-9-x"));
            assert_eq!(form.fixed.get("ticket").map(String::as_str), Some("JUS-9"));
            assert_eq!(form.values[0], "jus-9-x"); // source row prefilled from fixed
            assert_eq!(form.values[1], "main");     // target from default (editable)
            assert_eq!(form.initial_worktree.as_deref(), Some("platform.wt-a"));
        }
        other => panic!("expected DefArgs, got {other:?}"),
    }
}

// --- Task 20: Mode::DefArgs key + mouse handling ---
fn shift(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::SHIFT))
}
fn def_args_app(args: Vec<ArgSpec>, fixed: HashMap<String, String>, worktree: Option<String>) -> App {
    let mut app = fixture_app_one_project("platform");
    app.mode = Mode::DefArgs { form: crate::view::args_form::ArgsForm::new("platform".into(), "pr-ready".into(), args, fixed, HashMap::new(), worktree) };
    app
}

#[test]
fn def_args_fill_text_and_submit_positional_with_fixed_and_worktree() {
    use crossterm::event::KeyCode::*;
    let mut app = def_args_app(
        vec![
            ArgSpec { name: "source".into(), default: None, options: None, description: None },
            ArgSpec { name: "target".into(), default: Some("main".into()), options: None, description: None },
        ],
        HashMap::from([("source".into(), "wt-a".into())]),
        Some("platform.wt-a".into()),
    );
    // Focus starts on target (source fixed). Clear "main", type "dev".
    for _ in 0..4 { app.update(key(Backspace)); }
    for c in "dev".chars() { app.update(key(Char(c))); }
    let update = app.update(key(Enter));
    match &update.cmds[0] {
        Cmd::Rpc { call, invalidate_defs_for, .. } => {
            assert_eq!(call.method, "runDefinition");
            assert_eq!(call.params["args"], serde_json::json!(["wt-a", "dev"]));
            assert_eq!(call.params["worktree"], "platform.wt-a");
            assert_eq!(invalidate_defs_for.as_deref(), Some("platform"));
        }
        other => panic!("expected runDefinition, got {other:?}"),
    }
    assert!(matches!(app.mode, Mode::List));
}

#[test]
fn def_args_required_empty_blocks_submit() {
    use crossterm::event::KeyCode::*;
    let mut app = def_args_app(vec![ArgSpec { name: "pr".into(), default: None, options: None, description: None }], HashMap::new(), None);
    let update = app.update(key(Enter)); // required + empty
    assert!(update.cmds.is_empty());
    assert!(matches!(app.mode, Mode::DefArgs { .. }));
    if let Mode::DefArgs { form } = &app.mode { assert_eq!(form.error, Some(0)); }
}

#[test]
fn def_args_enter_on_enum_opens_dropdown_then_pick() {
    use crossterm::event::KeyCode::*;
    let mut app = def_args_app(
        vec![ArgSpec { name: "mode".into(), default: Some("ready".into()), options: Some(vec!["ready".into(), "create".into()]), description: None }],
        HashMap::new(), None,
    );
    app.update(key(Enter)); // enum focus -> opens dropdown
    if let Mode::DefArgs { form } = &app.mode { assert_eq!(form.dropdown, Some(0)); }
    app.update(key(Down));  // highlight create
    app.update(key(Enter)); // pick
    if let Mode::DefArgs { form } = &app.mode { assert_eq!(form.values[0], "create"); assert_eq!(form.dropdown, None); }
}

#[test]
fn def_args_esc_closes_dropdown_then_cancels() {
    use crossterm::event::KeyCode::*;
    let mut app = def_args_app(
        vec![ArgSpec { name: "mode".into(), default: Some("ready".into()), options: Some(vec!["ready".into(), "create".into()]), description: None }],
        HashMap::new(), None,
    );
    app.update(key(Enter)); // open dropdown
    app.update(key(Esc));   // closes dropdown only
    assert!(matches!(app.mode, Mode::DefArgs { .. }));
    app.update(key(Esc));   // cancels form
    assert!(matches!(app.mode, Mode::List));
}

#[test]
fn def_args_shift_tab_and_arrows_move_and_cycle() {
    use crossterm::event::KeyCode::*;
    let mut app = def_args_app(
        vec![
            ArgSpec { name: "mode".into(), default: Some("ready".into()), options: Some(vec!["ready".into(), "create".into()]), description: None },
            ArgSpec { name: "pr".into(), default: None, options: None, description: None },
        ],
        HashMap::new(), None,
    );
    app.update(key(Right)); // cycle mode -> create
    if let Mode::DefArgs { form } = &app.mode { assert_eq!(form.values[0], "create"); }
    app.update(key(Tab));   // focus pr
    if let Mode::DefArgs { form } = &app.mode { assert_eq!(form.focus, 1); }
    app.update(shift(BackTab)); // shift-tab back to mode
    if let Mode::DefArgs { form } = &app.mode { assert_eq!(form.focus, 0); }
}

#[test]
fn def_args_click_focuses_field_and_run_submits() {
    use crate::hit::{ButtonKind, HitTarget};
    let mut app = def_args_app(vec![ArgSpec { name: "pr".into(), default: None, options: None, description: None }], HashMap::new(), None);
    app.def_args_click(&HitTarget::FormField(0));
    if let Mode::DefArgs { form } = &app.mode { assert_eq!(form.focus, 0); }
    // fill then Run
    app.update(Event::Key(KeyEvent::new(KeyCode::Char('7'), KeyModifiers::NONE)));
    let update = app.def_args_click(&HitTarget::Button(ButtonKind::Confirm));
    assert!(matches!(update.cmds[0], Cmd::Rpc { .. }));
    assert!(matches!(app.mode, Mode::List));
}

// A click landing on a pane target behind the popup dismisses the form (same
// as esc / clicking empty space), matching route_def_pick_click and the menu
// router. The Modal body stays inert.
#[test]
fn def_args_click_on_pane_target_behind_form_cancels() {
    use crate::hit::HitTarget;
    let mut app = def_args_app(vec![ArgSpec { name: "pr".into(), default: None, options: None, description: None }], HashMap::new(), None);
    let update = app.def_args_click(&HitTarget::Row(ListPane::Queue, 0));
    assert!(update.dirty);
    assert!(matches!(app.mode, Mode::List));
}

// Drive a click through the REAL rendered hit map (not a hand-built target):
// render the form, find the [ Run ] button rect, click its center, and assert
// the geometry routes to a submit.
#[test]
fn def_args_run_button_click_through_rendered_hitmap_submits() {
    use crate::hit::{ButtonKind, HitTarget};
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    use ratatui::{Terminal, backend::TestBackend};
    let mut app = def_args_app(vec![ArgSpec { name: "pr".into(), default: None, options: None, description: None }], HashMap::new(), None);
    let (w, h) = app.size;
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| { app.hit = crate::view::render(&app, f); }).unwrap();
    // Fill the required field, then click the real Run button rect.
    app.update(Event::Key(KeyEvent::new(KeyCode::Char('7'), KeyModifiers::NONE)));
    let run = app
        .hit
        .iter()
        .find(|(_, t)| matches!(t, HitTarget::Button(ButtonKind::Confirm)))
        .map(|(r, _)| *r)
        .expect("rendered form registers a Run button");
    let ev = Event::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: run.x + run.width / 2,
        row: run.y,
        modifiers: KeyModifiers::NONE,
    });
    let update = app.update(ev);
    assert!(matches!(update.cmds[0], Cmd::Rpc { .. }));
    assert!(matches!(app.mode, Mode::List));
}

// Bracketed paste into the run form's free-text field keeps newlines verbatim
// (the "paste a bug report" flow); a multiline blob must not submit mid-paste.
#[test]
fn def_args_paste_inserts_multiline_verbatim() {
    let mut app = def_args_app(
        vec![ArgSpec { name: "desc".into(), default: None, options: None, description: None }],
        HashMap::new(),
        None,
    );
    let u = app.update(Event::Paste("line one\nline two".into()));
    assert!(u.dirty);
    assert!(u.cmds.is_empty()); // no submit — the paste just fills the field
    match &app.mode {
        Mode::DefArgs { form } => assert_eq!(form.values[0], "line one\nline two"),
        other => panic!("expected DefArgs still open, got {other:?}"),
    }
}

// Paste into the multiline AddTask editor keeps newlines verbatim (the prompt
// is a multiline body now).
#[test]
fn paste_into_add_task_editor_keeps_newlines() {
    let mut app = fixture_app_one_project("platform");
    app.mode = Mode::AddTask {
        worktree: None,
        resume_session_id: None,
        resume_label: None,
        editor: crate::view::multiline_input::MultilineInput::default(),
    };
    app.update(Event::Paste("do a\nthen b".into()));
    match &app.mode {
        Mode::AddTask { editor, .. } => assert_eq!(editor.text, "do a\nthen b"),
        other => panic!("expected AddTask, got {other:?}"),
    }
}

// Paste into the single-line CreateWorktree input collapses newlines/tabs to
// spaces so a multiline paste can't smuggle a newline into a one-line field.
#[test]
fn paste_into_single_line_input_collapses_control_chars() {
    let mut app = fixture_app_one_project("platform");
    app.mode = Mode::CreateWorktree { input: tui_input::Input::default(), error: None };
    app.update(Event::Paste("do a\nthen b".into()));
    match &app.mode {
        Mode::CreateWorktree { input, .. } => assert_eq!(input.value(), "do a then b"),
        other => panic!("expected CreateWorktree, got {other:?}"),
    }
}

// alt+enter no longer inserts a newline in the DefArgs form (only shift+enter
// does). The alt+enter falls through to the plain-Enter arm, which on a
// required-but-empty free-text field flags the row and blocks submit — proving
// no '\n' was inserted and that alt+enter is not treated as a newline chord.
#[test]
fn alt_enter_does_not_insert_newline_in_def_args_form() {
    let mut app = def_args_app(
        vec![ArgSpec { name: "pr".into(), default: None, options: None, description: None }],
        HashMap::new(),
        None,
    );
    let update = app.update(Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT)));
    // No submit (required + empty), form stays open, value has no newline.
    assert!(update.cmds.is_empty());
    match &app.mode {
        Mode::DefArgs { form } => {
            assert_eq!(form.error, Some(0), "alt+enter attempted submit, flagging the empty row");
            assert!(!form.values[0].contains('\n'), "alt+enter must not insert a newline");
        }
        other => panic!("expected DefArgs still open, got {other:?}"),
    }
}

// Paste is inert in List mode (no text target).
#[test]
fn paste_in_list_mode_is_ignored() {
    let mut app = fixture_app_one_project("platform");
    let u = app.update(Event::Paste("noise".into()));
    assert!(!u.dirty);
    assert!(matches!(app.mode, Mode::List));
}
