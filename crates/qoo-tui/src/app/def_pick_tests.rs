use super::*;
use crate::ipc::types::{ArgSpec, DefinitionSummary, Project, StateSnapshot, WorktreeInfo};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashMap;

fn key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn arg(name: &str) -> ArgSpec {
    ArgSpec { name: name.into(), r#type: None, default: None, options: None, description: None }
}

fn dsum(repo: &str, name: &str, scope: &str, args: Vec<ArgSpec>) -> DefinitionSummary {
    DefinitionSummary { repo: repo.into(), name: name.into(), scope: scope.into(), args, has_discovery: false, cron: None, description: None, model: None, worktree: None }
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
        Mode::DefArgs { state, args, initial_worktree, .. } => {
            // The ambient overlay injects the worktree branch as the source arg's
            // only option → field 0 is a seeded Dropdown, prefilled from the row.
            assert_eq!(args[0].options.as_deref(), Some(&["jus-9-x".to_string()][..]));
            assert!(matches!(&state.fields[0].kind, crate::view::form::FieldKind::Dropdown { .. }));
            assert_eq!(state.fields[0].value, "jus-9-x");
            assert_eq!(*initial_worktree, None);
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
fn def_uses_worktree_context_predicate() {
    let wt = |d: DefinitionSummary, w: &str| DefinitionSummary { worktree: Some(w.into()), ..d };
    // `worktree: repo` + no context-fillable args → agnostic.
    assert!(!def_uses_worktree_context(&wt(dsum("p", "sanitize-project", "global", vec![]), "repo")));
    // Any non-repo worktree setting consumes the context (target override applies).
    assert!(def_uses_worktree_context(&wt(dsum("p", "autofix", "project", vec![arg("situation")]), "auto")));
    assert!(def_uses_worktree_context(&wt(dsum("p", "adhoc", "project", vec![]), "temp")));
    // `worktree: repo` but a context-fillable arg (source/branch/ticket) → kept.
    assert!(def_uses_worktree_context(&wt(dsum("p", "squash-merge", "global", vec![arg("source")]), "repo")));
    // A worktree-TYPED arg → kept regardless of the worktree setting.
    let target = ArgSpec { r#type: Some("worktree".into()), ..arg("target") };
    assert!(def_uses_worktree_context(&wt(dsum("p", "pr-review", "project", vec![target]), "repo")));
    // Old daemon (no worktree field on the summary) → never hide on missing data.
    assert!(def_uses_worktree_context(&dsum("p", "unknown", "project", vec![])));
}

#[test]
fn task_menu_on_a_worktree_hides_worktree_agnostic_defs() {
    // With a worktree context, repo-pinned no-arg defs are filtered out; defs
    // that consume the context (non-repo worktree, or a source arg) remain.
    let repo_noargs =
        DefinitionSummary { worktree: Some("repo".into()), ..dsum("platform", "sanitize-project", "global", vec![]) };
    let auto = DefinitionSummary { worktree: Some("auto".into()), ..dsum("platform", "autofix", "project", vec![arg("situation")]) };
    let repo_source =
        DefinitionSummary { worktree: Some("repo".into()), ..dsum("platform", "squash-merge", "global", vec![arg("source")]) };
    let mut app = fixture_app_with_defs_and_worktree(
        "platform",
        vec![repo_noargs.clone(), auto, repo_source],
        ("wt-a", "jus-4-x"),
    );
    app.set_focus(PaneId::Worktrees);
    app.update(key(KeyCode::Char('t')));
    match &app.mode {
        Mode::DefPick { defs, worktree, .. } => {
            assert!(worktree.is_some());
            assert_eq!(
                defs.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(),
                vec!["autofix", "squash-merge"],
                "repo-pinned no-arg def is hidden on a worktree-scoped menu"
            );
        }
        other => panic!("expected DefPick, got {other:?}"),
    }
    // Contextless open (no worktree rows) keeps the full list, agnostic included.
    let mut app = fixture_app_with_defs("platform", vec![repo_noargs]);
    app.set_focus(PaneId::Worktrees);
    app.update(key(KeyCode::Char('t')));
    match &app.mode {
        Mode::DefPick { defs, worktree, .. } => {
            assert_eq!(worktree.as_deref(), None);
            assert_eq!(defs.len(), 1, "contextless menu keeps worktree-agnostic defs");
        }
        other => panic!("expected DefPick, got {other:?}"),
    }
}

#[test]
fn task_menu_on_a_worktree_with_only_agnostic_defs_refuses() {
    let repo_noargs =
        DefinitionSummary { worktree: Some("repo".into()), ..dsum("platform", "seed-data-sync", "project", vec![]) };
    let mut app = fixture_app_with_defs_and_worktree("platform", vec![repo_noargs], ("wt-a", "jus-4-x"));
    app.set_focus(PaneId::Worktrees);
    let update = app.update(key(KeyCode::Char('t')));
    assert!(matches!(app.mode, Mode::List));
    assert!(update.cmds.is_empty());
    assert_eq!(app.status_line.as_deref(), Some("no worktree-scoped task definitions"));
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
        Mode::DefArgs { state, initial_worktree, .. } => {
            // `source` is fixed from the worktree branch → a read-only field
            // prefilled with the branch; `target` is editable from its default.
            assert!(state.fields[0].readonly);
            assert_eq!(state.fields[0].value, "jus-9-x");
            assert!(!state.fields[1].readonly);
            assert_eq!(state.fields[1].value, "main"); // target from default (editable)
            assert_eq!(state.focus, 1); // focus starts past the read-only source
            assert_eq!(initial_worktree.as_deref(), Some("platform.wt-a"));
        }
        other => panic!("expected DefArgs, got {other:?}"),
    }
}

// --- Task 6: form_from_args builds the FormState fields ---
#[test]
fn open_def_args_builds_formstate_fields() {
    let mut app = fixture_app_one_project("platform");
    app.open_def_args(
        "platform".into(),
        "pr-ready".into(),
        vec![
            ArgSpec { options: Some(vec!["full-review".into(), "bypass-review".into()]), ..arg("review") },
            arg("pr"),
        ],
        HashMap::new(),
        HashMap::new(),
        None,
        Vec::new(),
        Vec::new(),
    );
    match &app.mode {
        Mode::DefArgs { state, .. } => {
            assert!(matches!(state.fields[0].kind, crate::view::form::FieldKind::Dropdown { .. }));
            assert_eq!(state.focus, 0); // first (non-readonly) field
        }
        other => panic!("expected DefArgs, got {other:?}"),
    }
}

// --- Task 14: worktree-typed arg seeds a combobox / locks readonly ----------
fn wt_arg(name: &str) -> ArgSpec {
    ArgSpec { name: name.into(), r#type: Some("worktree".into()), default: None, options: None, description: None }
}

#[test]
fn form_from_args_worktree_arg_is_seeded_combobox() {
    let app = fixture_app_one_project("platform");
    let wts = vec!["platform.wt-a".to_string(), "platform.wt-b".to_string()];
    let state = app.form_from_args("pr-review", &[wt_arg("target")], &HashMap::new(), &HashMap::new(), &wts, &[], None);
    match &state.fields[0].kind {
        crate::view::form::FieldKind::Combobox { options } => assert_eq!(options, &wts),
        other => panic!("expected Combobox seeded with worktrees, got {other:?}"),
    }
    assert!(
        state.fields[0].required,
        "the task-pane worktree combobox must be required so an empty submit is blocked inline"
    );
}

#[test]
fn form_from_args_worktree_arg_locks_readonly_from_launch() {
    let app = fixture_app_one_project("platform");
    let wts = vec!["platform.wt-a".to_string()];
    let state = app.form_from_args(
        "pr-review", &[wt_arg("target")], &HashMap::new(), &HashMap::new(), &wts, &[], Some("platform.wt-a"),
    );
    assert!(state.fields[0].readonly, "launch-from-worktree locks the field readonly");
    assert_eq!(state.fields[0].value, "platform.wt-a");
}

// --- field-kind mapping: plain input, type:text, type:branch ---------------
#[test]
fn form_from_args_plain_arg_is_single_line_input() {
    // A plain free-text arg (no type, no options) renders as a single-line
    // Input, not a 3-row Textarea — e.g. squash-merge's `target`. (Regression.)
    let app = fixture_app_one_project("platform");
    let target = ArgSpec { default: Some("main".into()), ..arg("target") };
    let state = app.form_from_args(
        "squash-merge", &[target], &HashMap::new(), &HashMap::new(), &[], &[], None,
    );
    assert!(matches!(state.fields[0].kind, crate::view::form::FieldKind::Input));
    assert_eq!(state.fields[0].value, "main");
    assert!(!state.fields[0].required, "an arg with a default is not required");
}

#[test]
fn form_from_args_type_text_is_textarea() {
    // `type: text` opts back into the multiline auto-growing textarea.
    let app = fixture_app_one_project("platform");
    let situation = ArgSpec { r#type: Some("text".into()), ..arg("situation") };
    let state = app.form_from_args(
        "autofix", &[situation], &HashMap::new(), &HashMap::new(), &[], &[], None,
    );
    assert!(matches!(state.fields[0].kind, crate::view::form::FieldKind::Textarea));
    assert!(state.fields[0].required, "a free-text arg with no default is required");
}

#[test]
fn form_from_args_type_branch_is_dropdown_seeded_with_branches_incl_default() {
    // `type: branch` → dropdown seeded with the repo's branches; the default
    // value is prepended so it stays selectable even without a local worktree.
    let app = fixture_app_one_project("platform");
    let target = ArgSpec { r#type: Some("branch".into()), default: Some("main".into()), ..arg("target") };
    let branches = vec!["improvement".to_string(), "wt-a".to_string()]; // no "main"
    let state = app.form_from_args(
        "squash-merge", &[target], &HashMap::new(), &HashMap::new(), &[], &branches, None,
    );
    match &state.fields[0].kind {
        crate::view::form::FieldKind::Dropdown { options } => {
            assert_eq!(options.first().map(|o| o.value.as_str()), Some("main"), "default prepended");
            assert!(options.iter().any(|o| o.value == "improvement"));
        }
        other => panic!("expected Dropdown seeded with branches, got {other:?}"),
    }
    assert_eq!(state.fields[0].value, "main");
}

// --- Task 7/9: Mode::DefArgs key + mouse handling on the shared FormState ---
fn def_args_app(args: Vec<ArgSpec>, fixed: HashMap<String, String>, worktree: Option<String>) -> App {
    let mut app = fixture_app_one_project("platform");
    let state = app.form_from_args("pr-ready", &args, &fixed, &HashMap::new(), &[], &[], worktree.as_deref());
    app.mode = Mode::DefArgs {
        state,
        repo: "platform".into(),
        def_name: "pr-ready".into(),
        args,
        initial_worktree: worktree,
        preview_scroll: 0,
    };
    app
}

#[test]
fn def_args_fill_text_and_submit_positional_with_fixed_and_worktree() {
    use crossterm::event::KeyCode::*;
    let mut app = def_args_app(
        vec![
            ArgSpec { name: "source".into(), r#type: None, default: None, options: None, description: None },
            ArgSpec { name: "target".into(), r#type: None, default: Some("main".into()), options: None, description: None },
        ],
        HashMap::from([("source".into(), "wt-a".into())]),
        Some("platform.wt-a".into()),
    );
    // Focus starts on target (source read-only). Clear "main", type "dev".
    for _ in 0..4 { app.update(key(Backspace)); }
    for c in "dev".chars() { app.update(key(Char(c))); }
    // Only the Primary button submits: Tab from the target field onto Run, Enter.
    app.update(key(Tab));
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
    let mut app = def_args_app(vec![ArgSpec { name: "pr".into(), r#type: None, default: None, options: None, description: None }], HashMap::new(), None);
    // Tab onto the Primary button, then Enter: validation flags the empty field.
    app.update(key(Tab));
    let update = app.update(key(Enter));
    assert!(update.cmds.is_empty());
    assert!(matches!(app.mode, Mode::DefArgs { .. }));
    if let Mode::DefArgs { state, .. } = &app.mode { assert_eq!(state.error, Some(0)); }
}

#[test]
fn def_args_enter_never_submits_from_a_text_field() {
    // App-wide standard: plain Enter on a focused free-text field inserts a
    // newline (textarea) and never submits — only the Primary button does.
    use crossterm::event::KeyCode::*;
    let mut app = def_args_app(vec![ArgSpec { name: "pr".into(), r#type: Some("text".into()), default: None, options: None, description: None }], HashMap::new(), None);
    let update = app.update(key(Enter));
    assert!(update.cmds.is_empty(), "Enter on a field must not submit");
    match &app.mode {
        Mode::DefArgs { state, .. } => assert_eq!(state.fields[0].value, "\n"),
        other => panic!("expected DefArgs still open, got {other:?}"),
    }
}

#[test]
fn def_args_enter_on_input_advances_focus_without_submitting() {
    // A single-line Input (a plain arg, the new default): plain Enter advances
    // focus off the field (toward the Run button) and never submits or inserts
    // a newline — the app-wide explicit-commit rule.
    use crossterm::event::KeyCode::*;
    let mut app = def_args_app(
        vec![ArgSpec { name: "pr".into(), r#type: None, default: None, options: None, description: None }],
        HashMap::new(), None,
    );
    let update = app.update(key(Enter));
    assert!(update.cmds.is_empty(), "Enter on an input must not submit");
    match &app.mode {
        Mode::DefArgs { state, .. } => {
            assert_eq!(state.fields[0].value, "", "no newline inserted into an input");
            assert_ne!(state.focus, 0, "focus advanced off the input");
        }
        other => panic!("expected DefArgs still open, got {other:?}"),
    }
}

#[test]
fn def_args_enter_on_enum_opens_dropdown_then_pick() {
    use crossterm::event::KeyCode::*;
    let mut app = def_args_app(
        vec![ArgSpec { name: "mode".into(), r#type: None, default: Some("ready".into()), options: Some(vec!["ready".into(), "create".into()]), description: None }],
        HashMap::new(), None,
    );
    app.update(key(Enter)); // dropdown focus -> opens dropdown, highlight = current
    if let Mode::DefArgs { state, .. } = &app.mode { assert!(state.dropdown_open); assert_eq!(state.dropdown_index, 0); }
    app.update(key(Down));  // highlight create
    app.update(key(Enter)); // pick
    if let Mode::DefArgs { state, .. } = &app.mode { assert_eq!(state.fields[0].value, "create"); assert!(!state.dropdown_open); }
}

#[test]
fn def_args_esc_closes_dropdown_then_cancels() {
    use crossterm::event::KeyCode::*;
    let mut app = def_args_app(
        vec![ArgSpec { name: "mode".into(), r#type: None, default: Some("ready".into()), options: Some(vec!["ready".into(), "create".into()]), description: None }],
        HashMap::new(), None,
    );
    app.update(key(Enter)); // open dropdown
    app.update(key(Esc));   // closes dropdown only
    assert!(matches!(app.mode, Mode::DefArgs { .. }));
    app.update(key(Esc));   // cancels form
    assert!(matches!(app.mode, Mode::List));
}

// --- Task 12: Combobox key handling on Mode::DefArgs -------------------------
/// A Mode::DefArgs parked on a single Combobox field seeded with `options`.
fn def_args_combobox_app(options: Vec<String>) -> App {
    use crate::view::form::{Field, FormState};
    let mut app = fixture_app_one_project("platform");
    let state = FormState::new("pr-ready", "Run", vec![Field::combobox("target", options, "")]);
    app.mode = Mode::DefArgs {
        state,
        repo: "platform".into(),
        def_name: "pr-ready".into(),
        args: vec![ArgSpec { name: "target".into(), r#type: Some("worktree".into()), default: None, options: None, description: None }],
        initial_worktree: None,
        preview_scroll: 0,
    };
    app
}

#[test]
fn def_args_combobox_type_opens_and_filters() {
    use crossterm::event::KeyCode::*;
    let mut app = def_args_combobox_app(vec!["acme".into(), "beta".into()]);
    for c in "ac".chars() { app.update(key(Char(c))); }
    match &app.mode {
        Mode::DefArgs { state, .. } => {
            assert_eq!(state.fields[0].value, "ac");
            assert!(state.dropdown_open, "typing (re)opens the filtered list");
            let view = state.combobox_filtered();
            assert!(view.iter().any(|(_, s)| s == "acme"));
            assert!(!view.iter().any(|(_, s)| s == "beta"), "beta filtered out");
        }
        other => panic!("expected DefArgs, got {other:?}"),
    }
}

#[test]
fn def_args_combobox_picks_typed_pr_ref() {
    use crossterm::event::KeyCode::*;
    let mut app = def_args_combobox_app(vec!["acme".into()]);
    for c in "45".chars() { app.update(key(Char(c))); }
    // The only filtered row is the synthetic `pr:45`; Down highlights it, Enter picks.
    app.update(key(Down));
    app.update(key(Enter));
    match &app.mode {
        Mode::DefArgs { state, .. } => {
            assert_eq!(state.fields[0].value, "pr:45");
            assert!(!state.dropdown_open, "pick closes the list");
        }
        other => panic!("expected DefArgs, got {other:?}"),
    }
}

#[test]
fn def_args_combobox_esc_closes_list_then_cancels() {
    use crossterm::event::KeyCode::*;
    let mut app = def_args_combobox_app(vec!["acme".into()]);
    app.update(key(Char('a'))); // opens the list
    app.update(key(Esc));       // closes the list only
    match &app.mode {
        Mode::DefArgs { state, .. } => assert!(!state.dropdown_open),
        other => panic!("expected DefArgs still open, got {other:?}"),
    }
    app.update(key(Esc)); // cancels the form
    assert!(matches!(app.mode, Mode::List));
}

// --- Task 15: submit resolves the worktree combobox to a ref ----------------
/// A Mode::DefArgs on a single worktree-typed combobox arg seeded with
/// `options`, over a snapshot carrying `worktrees` (raw names for exact-match).
fn def_args_worktree_submit_app(options: Vec<String>, worktrees: Vec<&str>) -> App {
    use crate::view::form::{Field, FormState};
    let mut app = fixture_app_one_project("platform");
    if !worktrees.is_empty() {
        let mut wts = HashMap::new();
        wts.insert(
            "platform".to_string(),
            worktrees
                .iter()
                .map(|w| WorktreeInfo { name: w.to_string(), path: format!("/wt/{w}"), branch: w.to_string(), ..Default::default() })
                .collect(),
        );
        app.snapshot = Some(StateSnapshot {
            projects: vec![Project { name: "platform".into(), github_id: None }],
            worktrees: wts,
            ..Default::default()
        });
    }
    let state = FormState::new("pr-review", "Run", vec![Field::combobox("target", options, "")]);
    app.mode = Mode::DefArgs {
        state,
        repo: "platform".into(),
        def_name: "pr-review".into(),
        args: vec![wt_arg("target")],
        initial_worktree: None,
        preview_scroll: 0,
    };
    app
}

#[test]
fn def_args_combobox_submits_pr_ref() {
    use crossterm::event::KeyCode::*;
    let mut app = def_args_worktree_submit_app(vec!["acme".into()], vec![]);
    for c in "45".chars() { app.update(key(Char(c))); }
    app.update(key(Tab)); // combobox → Run (closes the list)
    let update = app.update(key(Enter));
    match &update.cmds[0] {
        Cmd::Rpc { call, .. } => {
            assert_eq!(call.method, "runDefinition");
            assert_eq!(call.params["ref"], "pr:45");
            assert!(call.params.get("worktree").is_none(), "a ref replaces the worktree param");
        }
        other => panic!("expected runDefinition, got {other:?}"),
    }
    assert!(matches!(app.mode, Mode::List));
}

#[test]
fn def_args_combobox_submits_already_canonical_pr_ref() {
    use crossterm::event::KeyCode::*;
    // Typing the canonical `pr:1925` (or picking the combobox's own synthetic
    // "use PR" row, whose value IS `pr:1925`) must submit `ref == "pr:1925"`, not
    // the double-wrapped `worktree:pr:1925` the daemon can't resolve — the submit
    // path classifies a second time and must be idempotent on canonical refs.
    let mut app = def_args_worktree_submit_app(vec!["acme".into()], vec![]);
    for c in "pr:1925".chars() { app.update(key(Char(c))); }
    app.update(key(Tab)); // combobox → Run (closes the list)
    let update = app.update(key(Enter));
    match &update.cmds[0] {
        Cmd::Rpc { call, .. } => {
            assert_eq!(call.method, "runDefinition");
            assert_eq!(call.params["ref"], "pr:1925");
            assert!(call.params.get("worktree").is_none(), "a ref replaces the worktree param");
        }
        other => panic!("expected runDefinition, got {other:?}"),
    }
    assert!(matches!(app.mode, Mode::List));
}

#[test]
fn def_args_worktree_combobox_empty_blocks_submit() {
    use crossterm::event::KeyCode::*;
    // Built through `form_from_args` (the real task-pane launch path, no
    // `initial_worktree`) rather than the raw-FormState submit-app helper, so
    // the combobox picks up the production `required = true` wiring. Leaving
    // it empty and submitting must NOT resolve to the malformed ref
    // `"worktree:"` — `validate()` flags it and keeps the form open
    // (final-review finding M1).
    let mut app = fixture_app_one_project("platform");
    let wts = vec!["acme".to_string()];
    let state = app.form_from_args("pr-review", &[wt_arg("target")], &HashMap::new(), &HashMap::new(), &wts, &[], None);
    app.mode = Mode::DefArgs {
        state,
        repo: "platform".into(),
        def_name: "pr-review".into(),
        args: vec![wt_arg("target")],
        initial_worktree: None,
        preview_scroll: 0,
    };
    app.update(key(Tab)); // combobox → Run, value left empty
    let update = app.update(key(Enter));
    assert!(update.cmds.is_empty(), "an empty required worktree field must not emit a run command");
    assert!(matches!(app.mode, Mode::DefArgs { .. }), "the form must stay open on the blocked submit");
    if let Mode::DefArgs { state, .. } = &app.mode {
        assert_eq!(state.error, Some(0));
    }
}

#[test]
fn def_args_combobox_submits_worktree_ref_for_existing_name() {
    use crossterm::event::KeyCode::*;
    // Typed "JUS-1756" matches an existing worktree → worktree:<name> wins over
    // the ticket classifier.
    let mut app = def_args_worktree_submit_app(vec!["JUS-1756".into()], vec!["JUS-1756"]);
    for c in "JUS-1756".chars() { app.update(key(Char(c))); }
    app.update(key(Tab));
    let update = app.update(key(Enter));
    match &update.cmds[0] {
        Cmd::Rpc { call, .. } => {
            assert_eq!(call.params["ref"], "worktree:JUS-1756");
            assert!(call.params.get("worktree").is_none());
        }
        other => panic!("expected runDefinition, got {other:?}"),
    }
}

#[test]
fn def_args_locked_worktree_submits_ref_and_omits_worktree_param() {
    use crossterm::event::KeyCode::*;
    // Launched FROM worktree "platform.wt-a": the target arg is readonly-locked;
    // submit still resolves it to a ref (and never sends the legacy worktree param).
    let mut app = fixture_app_one_project("platform");
    let mut wts = HashMap::new();
    wts.insert(
        "platform".to_string(),
        vec![WorktreeInfo { name: "platform.wt-a".into(), path: "/wt/wt-a".into(), branch: "wt-a".into(), ..Default::default() }],
    );
    app.snapshot = Some(StateSnapshot {
        projects: vec![Project { name: "platform".into(), github_id: None }],
        worktrees: wts,
        ..Default::default()
    });
    let wt_names = app.active_worktree_names();
    let state = app.form_from_args(
        "pr-review", &[wt_arg("target")], &HashMap::new(), &HashMap::new(), &wt_names, &[], Some("platform.wt-a"),
    );
    app.mode = Mode::DefArgs {
        state,
        repo: "platform".into(),
        def_name: "pr-review".into(),
        args: vec![wt_arg("target")],
        initial_worktree: Some("platform.wt-a".into()),
        preview_scroll: 0,
    };
    app.update(key(Tab)); // skip the readonly field → Run
    let update = app.update(key(Enter));
    match &update.cmds[0] {
        Cmd::Rpc { call, .. } => {
            assert_eq!(call.params["ref"], "worktree:platform.wt-a");
            assert!(call.params.get("worktree").is_none(), "a ref replaces the worktree param");
        }
        other => panic!("expected runDefinition, got {other:?}"),
    }
}

#[test]
fn def_args_tab_moves_focus_and_dropdown_pick_sets_value() {
    use crossterm::event::KeyCode::*;
    let mut app = def_args_app(
        vec![
            ArgSpec { name: "mode".into(), r#type: None, default: Some("ready".into()), options: Some(vec!["ready".into(), "create".into()]), description: None },
            ArgSpec { name: "pr".into(), r#type: None, default: None, options: None, description: None },
        ],
        HashMap::new(), None,
    );
    // The dropdown value changes by open + move + pick (arrows never cycle it).
    app.update(key(Enter)); // open
    app.update(key(Down));  // highlight create
    app.update(key(Enter)); // pick
    if let Mode::DefArgs { state, .. } = &app.mode { assert_eq!(state.fields[0].value, "create"); }
    app.update(key(Tab));   // focus pr
    if let Mode::DefArgs { state, .. } = &app.mode { assert_eq!(state.focus, 1); }
    app.update(key(BackTab)); // back to mode
    if let Mode::DefArgs { state, .. } = &app.mode { assert_eq!(state.focus, 0); }
}

#[test]
fn def_args_click_focuses_field_and_run_submits() {
    use crate::hit::{ButtonKind, HitTarget};
    let mut app = def_args_app(vec![ArgSpec { name: "pr".into(), r#type: None, default: None, options: None, description: None }], HashMap::new(), None);
    app.def_args_click(&HitTarget::FormField(0));
    if let Mode::DefArgs { state, .. } = &app.mode { assert_eq!(state.focus, 0); }
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
    let mut app = def_args_app(vec![ArgSpec { name: "pr".into(), r#type: None, default: None, options: None, description: None }], HashMap::new(), None);
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
    let mut app = def_args_app(vec![ArgSpec { name: "pr".into(), r#type: None, default: None, options: None, description: None }], HashMap::new(), None);
    let (w, h) = app.size;
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| { app.hit = crate::view::render(&mut app, f); }).unwrap();
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
        vec![ArgSpec { name: "desc".into(), r#type: Some("text".into()), default: None, options: None, description: None }],
        HashMap::new(),
        None,
    );
    let u = app.update(Event::Paste("line one\nline two".into()));
    assert!(u.dirty);
    assert!(u.cmds.is_empty()); // no submit — the paste just fills the field
    match &app.mode {
        Mode::DefArgs { state, .. } => assert_eq!(state.fields[0].value, "line one\nline two"),
        other => panic!("expected DefArgs still open, got {other:?}"),
    }
}

// Paste into the adhoc-create form's prompt textarea keeps newlines verbatim
// (the prompt is a multiline body).
#[test]
fn paste_into_adhoc_prompt_keeps_newlines() {
    use crate::app::mode::adhoc_field;
    let mut app = fixture_app_one_project("platform");
    app.open_adhoc_create("platform".into(), None);
    if let Mode::Form { state, .. } = &mut app.mode {
        state.focus_field(adhoc_field::PROMPT);
    }
    app.update(Event::Paste("do a\nthen b".into()));
    match &app.mode {
        Mode::Form { state, .. } => {
            assert_eq!(state.fields[adhoc_field::PROMPT].value, "do a\nthen b");
        }
        other => panic!("expected Form, got {other:?}"),
    }
}

// alt+enter is not a submit chord: like plain Enter on a focused textarea it
// inserts a newline (only the Primary button submits), so it never dispatches.
#[test]
fn alt_enter_does_not_submit_in_def_args_form() {
    let mut app = def_args_app(
        vec![ArgSpec { name: "pr".into(), r#type: Some("text".into()), default: None, options: None, description: None }],
        HashMap::new(),
        None,
    );
    let update = app.update(Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT)));
    // No submit; form stays open on the focused text field.
    assert!(update.cmds.is_empty());
    match &app.mode {
        Mode::DefArgs { state, .. } => {
            assert_eq!(state.focus, 0, "alt+enter did not submit — focus stays on the field");
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
