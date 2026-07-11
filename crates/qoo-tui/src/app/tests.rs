use super::*;
use crate::ipc::types::{Project, TaskInstance, TaskTarget};
use crossterm::event::{KeyEvent, KeyModifiers};

fn app() -> App {
    App::new(PathBuf::from("/runs"), PathBuf::from("/sock"))
}

fn running_task(repo: &str) -> TaskInstance {
    TaskInstance {
        id: "t1".into(),
        status: TaskStatus::Running,
        target: TaskTarget {
            repo: repo.into(),
            git_ref: "temp".into(),
            worktree: Some("wt-a".into()),
        },
        ..Default::default()
    }
}

fn snapshot_with(projects: &[&str], tasks: Vec<TaskInstance>) -> StateSnapshot {
    StateSnapshot {
        projects: projects.iter().map(|n| Project { name: n.to_string(), github_id: None }).collect(),
        tasks,
        ..Default::default()
    }
}

#[test]
fn snapshot_event_commits_state_and_dirties() {
    let mut app = app();
    let u = app.update(Event::Snapshot(snapshot_with(&["platform"], vec![])));
    assert!(u.dirty);
    assert!(app.connected);
    assert_eq!(app.snapshot.as_ref().unwrap().projects.len(), 1);
}

#[test]
fn disconnected_dirties_only_on_transition_and_keeps_snapshot() {
    let mut app = app();
    app.update(Event::Snapshot(snapshot_with(&["platform"], vec![])));
    let u = app.update(Event::Disconnected);
    assert!(u.dirty);
    assert!(!app.connected);
    assert!(app.snapshot.is_some()); // last snapshot stays rendered
    // The 2s retry loop re-sends Disconnected while the daemon is down —
    // repeats must not repaint (zero idle renders).
    let again = app.update(Event::Disconnected);
    assert!(!again.dirty);
}

#[test]
fn resize_dirties() {
    let mut app = app();
    assert!(app.update(Event::Resize).dirty);
}

#[test]
fn q_in_list_mode_quits() {
    let mut app = app();
    let key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
    let u = app.update(Event::Key(key));
    assert_eq!(u.cmds, vec![Cmd::Quit]);
}

#[test]
fn tick_advances_clock_and_dirties() {
    let mut app = app();
    // Tick repaints (elapsed labels) only while the active project has a
    // running task — otherwise it is a zero-render no-op (see idle test).
    app.update(Event::Snapshot(snapshot_with(&["platform"], vec![running_task("platform")])));
    app.active_tab = 0;
    app.now_epoch_s = 0;
    let u = app.update(Event::Tick);
    assert!(u.dirty);
    assert!(app.now_epoch_s > 0);
}

#[test]
fn wants_tick_requires_running_task_in_active_project() {
    let mut app = app();
    assert!(!app.wants_tick()); // no snapshot yet
    app.update(Event::Snapshot(snapshot_with(
        &["platform", "web"],
        vec![running_task("web")],
    )));
    app.active_tab = 0; // platform — the running task is on web
    assert!(!app.wants_tick());
    app.active_tab = 1; // web
    assert!(app.wants_tick());
}

#[test]
fn detail_scroll_inverts_on_bottom_anchored_transcript() {
    // fixture_app: active detail context is the running task's transcript,
    // which is bottom-anchored (k = older, so a negative delta grows offset).
    let mut app = crate::test_fixtures::fixture_app();
    app.detail_max_scroll.set(10); // as if the last render had 10 lines of slack
    assert!(!app.detail_scroll(1)); // toward newest — already at tail, no-op
    assert!(app.detail_scroll(-1)); // toward older — offset grows
    assert_eq!(app.ui().scroll_offset, 1);
}

#[test]
fn detail_scroll_edge_jumps_head_and_tail() {
    let mut app = crate::test_fixtures::fixture_app();
    app.detail_max_scroll.set(42);
    assert!(app.detail_scroll_edge(-1)); // head/oldest → the render-fed max
    assert_eq!(app.ui().scroll_offset, 42);
    assert!(app.detail_scroll_edge(1)); // tail → 0
    assert_eq!(app.ui().scroll_offset, 0);
    assert!(!app.detail_scroll_edge(1)); // already at tail
}

#[test]
fn home_end_scroll_detail_only_never_the_list_cursor() {
    // Regression: Home/End used to share ScrollEdge with g/G, which — because
    // a list pane is always focused in Mode::List — jumped the LIST cursor.
    // DetailScrollEdge must take the detail path unconditionally: the queue
    // selection stays put while only the detail scroll moves.
    let mut app = crate::test_fixtures::fixture_app();
    // Queue is focused by default; pin the cursor to the first row so the old
    // End→last-row behavior would be observable if it regressed.
    assert_eq!(app.ui().focus, PaneId::Queue);
    app.ui().selections[ListPane::Queue.idx()] = Selection { cursor: 0, anchor: None };
    app.detail_max_scroll.set(10); // fixture detail is bottom-anchored

    // End (dir > 0). Bottom-anchored tail = 0, so no scroll change, but the
    // key must still route through the detail path (never the list arm).
    let up = app.apply_action(AppAction::DetailScrollEdge(1));
    assert!(!up.dirty, "already at tail → no-op");
    assert_eq!(app.ui().selections[ListPane::Queue.idx()].cursor, 0);

    // Home (dir < 0). Bottom-anchored head = max: the detail scrolls but the
    // queue cursor still must not move.
    let up = app.apply_action(AppAction::DetailScrollEdge(-1));
    assert!(up.dirty);
    assert_eq!(app.ui().scroll_offset, 10, "detail jumped to head");
    assert_eq!(
        app.ui().selections[ListPane::Queue.idx()].cursor,
        0,
        "list cursor untouched by Home/End"
    );
}

#[test]
fn detail_scroll_clamps_at_max_so_overscroll_banks_no_phantom_distance() {
    // Regression: over-scrolling past the head kept growing the stored
    // offset, so scrolling back required burning through phantom distance.
    let mut app = crate::test_fixtures::fixture_app();
    app.detail_max_scroll.set(3);
    for _ in 0..10 {
        app.detail_scroll(-1); // way past the head
    }
    assert_eq!(app.ui().scroll_offset, 3, "stored offset stops at the content max");
    // The very next scroll toward the tail must move the view immediately.
    assert!(app.detail_scroll(1));
    assert_eq!(app.ui().scroll_offset, 2);
}

#[test]
fn reset_scroll_returns_to_anchor() {
    let mut app = crate::test_fixtures::fixture_app();
    app.detail_max_scroll.set(10);
    app.detail_scroll(-5);
    assert_eq!(app.ui().scroll_offset, 5);
    app.reset_scroll();
    assert_eq!(app.ui().scroll_offset, 0);
}

#[test]
fn detail_scroll_does_not_invert_on_top_anchored() {
    let mut app = crate::test_fixtures::fixture_app();
    app.defs_by_project.insert(
        "acme".into(),
        vec![DefinitionSummary { repo: "acme".into(), name: "pr-ready".into(), ..Default::default() }],
    );
    let mut ui = TabUiState::default();
    ui.last_list_pane = ListPane::Tasks;
    app.ui_by_tab.insert("acme".into(), ui);
    // Definition context is head-anchored: positive delta grows offset directly.
    app.detail_max_scroll.set(10);
    assert!(app.detail_scroll(1));
    assert_eq!(app.ui().scroll_offset, 1);
    assert!(app.detail_scroll(-1));
    assert_eq!(app.ui().scroll_offset, 0);
}

#[test]
fn wants_tick_false_with_only_queued_tasks() {
    let mut app = app();
    let mut task = running_task("platform");
    task.status = TaskStatus::Queued;
    app.update(Event::Snapshot(snapshot_with(&["platform"], vec![task])));
    app.active_tab = 0;
    assert!(!app.wants_tick());
}

// -- Task 10: run-file wiring --------------------------------------------
use crate::runfiles::RunFiles;

fn run_files_fixture() -> RunFiles {
    RunFiles {
        transcript_tail: (0..5).map(|i| format!("line {i}")).collect(),
        report: vec!["# ok".to_string()],
        ..Default::default()
    }
}

#[test]
fn snapshot_event_schedules_debounced_read_for_selected_run() {
    let mut app = crate::test_fixtures::fixture_app();
    // last_list_pane defaults to Queue, cursor 0 → task 01RUN (a Run context).
    let up = app.update(Event::Snapshot(crate::test_fixtures::fixture_snapshot()));
    assert!(up.cmds.iter().any(|c| matches!(
        c,
        Cmd::ReadRunFiles { task_id, delay_ms: 120, .. } if task_id == "01RUN"
    )));
}

#[test]
fn stale_run_files_event_is_discarded() {
    let mut app = crate::test_fixtures::fixture_app();
    let up = app.update(Event::RunFiles {
        task_id: "01SOMEONE_ELSE".into(),
        files: run_files_fixture(),
    });
    assert!(!up.dirty);
    assert!(app.run_files.is_none());
}

#[test]
fn identical_run_files_do_not_dirty_but_still_repoll() {
    let mut app = crate::test_fixtures::fixture_app();
    app.run_files = Some(("01RUN".to_string(), run_files_fixture()));
    let up = app.update(Event::RunFiles {
        task_id: "01RUN".into(),
        files: run_files_fixture(),
    });
    assert!(!up.dirty, "content-identical read must not trigger a render");
    // 01RUN is running in the fixture → 1s follow-up poll is scheduled.
    assert!(up.cmds.iter().any(|c| matches!(
        c,
        Cmd::ReadRunFiles { task_id, delay_ms: 1000, .. } if task_id == "01RUN"
    )));
}

#[test]
fn changed_run_files_dirty_and_commit() {
    let mut app = crate::test_fixtures::fixture_app();
    app.run_files = Some(("01RUN".to_string(), RunFiles::default()));
    let up = app.update(Event::RunFiles {
        task_id: "01RUN".into(),
        files: run_files_fixture(),
    });
    assert!(up.dirty);
    assert_eq!(app.run_files.as_ref().unwrap().1, run_files_fixture());
}

#[test]
fn no_repoll_when_selected_task_not_running() {
    let mut app = crate::test_fixtures::fixture_app();
    // Point the queue cursor at 01QUE (index 1, a queued task).
    app.ui().selections[0].cursor = 1;
    let up = app.update(Event::RunFiles {
        task_id: "01QUE".into(),
        files: run_files_fixture(),
    });
    assert!(up.dirty);
    assert!(up.cmds.is_empty(), "non-running task must not start the 1s poll loop");
}

// -- Task 11: key dispatch through update() -------------------------------
use crate::app::{ListPane, PaneId};

fn press(app: &mut App, code: KeyCode) -> Update {
    app.update(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)))
}

/// A shifted keypress — the left-pane extend-selection modifier (shift+arrows).
fn press_shift(app: &mut App, code: KeyCode) -> Update {
    app.update(Event::Key(KeyEvent::new(code, KeyModifiers::SHIFT)))
}

#[test]
fn cycle_pane_wraps_queue_tasks_worktrees() {
    let mut app = crate::test_fixtures::fixture_app();
    // Detail is display-only and never enters the focus cycle.
    let order = [PaneId::Tasks, PaneId::Worktrees, PaneId::Queue];
    for expected in order {
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.ui().focus, expected);
    }
}

#[test]
fn hl_cycle_detail_subtabs_without_moving_focus() {
    let mut app = crate::test_fixtures::fixture_app();
    press(&mut app, KeyCode::Tab); // → tasks
    // h/l cycle the detail sub-tab; they never move the left-pane focus.
    press(&mut app, KeyCode::Char('l'));
    assert_eq!(app.ui().focus, PaneId::Tasks);
    press(&mut app, KeyCode::Char('h'));
    assert_eq!(app.ui().focus, PaneId::Tasks);
}

#[test]
fn move_cursor_wraps_circularly_and_extend_stays_clamped() {
    let mut app = crate::test_fixtures::fixture_app();
    // 4 queue rows (3 live + 1 archived). The ARROW keys move the left cursor
    // (j/k now address the detail pane). Navigation is circular: 10 ↓ presses
    // from row 0 land on 10 % 4 = row 2.
    for _ in 0..10 {
        press(&mut app, KeyCode::Down);
    }
    assert_eq!(app.ui().selections[0].cursor, 2);
    press(&mut app, KeyCode::Down); // → 3 (last)
    press(&mut app, KeyCode::Down); // wraps → 0
    assert_eq!(app.ui().selections[0].cursor, 0);
    press(&mut app, KeyCode::Up); // wraps back → 3
    assert_eq!(app.ui().selections[0].cursor, 3);
    // Extend-selection (shift+arrows) does NOT wrap (a wrapping range would be
    // ambiguous).
    press_shift(&mut app, KeyCode::Down); // can't extend past end → anchor stays None
    assert_eq!(app.ui().selections[0].anchor, None);
    press_shift(&mut app, KeyCode::Up);
    assert_eq!(app.ui().selections[0], Selection { cursor: 2, anchor: Some(3) });
}

#[test]
fn jk_move_the_worktree_detail_row_cursor_and_reset_on_selection_change() {
    let mut app = crate::test_fixtures::fixture_app();
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Tab); // focus worktrees; cursor 0 = acme.feature lane
    assert_eq!(app.ui().last_list_pane, ListPane::Worktrees);
    assert_eq!(app.ui().detail_row, 0);
    // acme.feature's lane has two tasks (running + queued): j advances the detail
    // row cursor and clamps at the last row.
    press(&mut app, KeyCode::Char('j'));
    assert_eq!(app.ui().detail_row, 1);
    press(&mut app, KeyCode::Char('j')); // clamp (2 tasks → last index 1)
    assert_eq!(app.ui().detail_row, 1);
    press(&mut app, KeyCode::Char('k'));
    assert_eq!(app.ui().detail_row, 0);
    // Move it back down, then change the WORKTREES selection → the detail row
    // cursor resets (the lane-task list changed out from under it).
    press(&mut app, KeyCode::Char('j'));
    assert_eq!(app.ui().detail_row, 1);
    press(&mut app, KeyCode::Down); // worktree cursor 0 → 1
    assert_eq!(app.ui().detail_row, 0);
}

#[test]
fn enter_on_lane_task_jumps_to_its_queue_detail() {
    let mut app = crate::test_fixtures::fixture_app();
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Tab); // focus worktrees (acme.feature lane)
    // detail_row 0 = the running task (running sorts first in the lane list).
    let u = press(&mut app, KeyCode::Enter);
    assert!(u.dirty);
    // Focus + selection jumped to the queue; the detail is now the Run context on
    // its transcript sub-tab (mirrors clicking that queue row).
    assert_eq!(app.ui().focus, PaneId::Queue);
    assert_eq!(app.ui().last_list_pane, ListPane::Queue);
    assert_eq!(app.ui().sub_tab[DetailKind::Run as usize], 0);
    let c = crate::view::compute(&app);
    assert_eq!(c.queue[app.ui().selections[0].cursor].task_id, "01RUN");
}

#[test]
fn queue_nav_crosses_divider_onto_a_real_finished_row() {
    // Real render so app.hit carries the true divider geometry.
    let mut app = app_rendered(80, 24);
    // The ACTIVE/FINISHED divider is inert: exactly 4 queue rows are
    // clickable (2 active + 2 finished), and none of them is the divider.
    let queue_row_hits = app
        .hit
        .iter()
        .filter(|(_, t)| matches!(t, HitTarget::Row(ListPane::Queue, _)))
        .count();
    assert_eq!(queue_row_hits, 4, "the divider adds no Row hit target");
    // From the last ACTIVE row, j crosses the divider onto the first FINISHED
    // row (index 2) — the cursor never stalls on the divider line.
    press(&mut app, KeyCode::Down); // 0 → 1 (last active)
    press(&mut app, KeyCode::Down); // 1 → 2 (first finished, across divider)
    assert_eq!(app.ui().selections[0].cursor, 2);
    // Opening the menu targets that real finished task — the cursor index maps
    // 1:1 to a real row, so the divider never shifts the row lookup.
    press(&mut app, KeyCode::Char('a'));
    match &app.mode {
        Mode::ActionMenu { title, .. } => assert_eq!(title, "flaky migration"),
        other => panic!("expected ActionMenu on the failed task, got {other:?}"),
    }
}

#[test]
fn queue_range_selection_spans_the_section_divider() {
    let mut app = app_rendered(80, 24);
    press(&mut app, KeyCode::Down); // cursor 0 → 1 (last active row)
    press_shift(&mut app, KeyCode::Down); // extend into row 2 (first finished)
    assert_eq!(app.ui().selections[0], Selection { cursor: 2, anchor: Some(1) });
    // The range covers a real row on EACH side of the divider (active 1 +
    // finished 2) — selections/cursor operate purely in real-row space.
    assert_eq!(crate::view::selection_range(&app.ui().selections[0]), (1, 2));
}

#[test]
fn g_and_shift_g_jump_list_cursor_to_edges() {
    let mut app = crate::test_fixtures::fixture_app();
    app.update(Event::Key(KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT)));
    assert_eq!(app.ui().selections[0].cursor, 3);
    press(&mut app, KeyCode::Char('g'));
    assert_eq!(app.ui().selections[0].cursor, 0);
}

#[test]
fn staged_esc_range_then_filter_then_noop() {
    let mut app = crate::test_fixtures::fixture_app();
    // Build a range and a filter. "cache" matches 01RUN + 01QUE summaries
    // (the fixture has no "line" row, so a filter must keep ≥2 rows to extend).
    app.ui().search[0] = "cache".into();
    press_shift(&mut app, KeyCode::Down); // range 0..1
    assert!(app.ui().selections[0].anchor.is_some());
    press(&mut app, KeyCode::Esc); // 1: clears range
    assert_eq!(app.ui().selections[0].anchor, None);
    assert_eq!(app.ui().search[0], "cache");
    press(&mut app, KeyCode::Esc); // 2: clears filter
    assert_eq!(app.ui().search[0], "");
    let up = press(&mut app, KeyCode::Esc); // 3: noop
    assert!(!up.dirty);
}

#[test]
fn search_mode_types_filters_and_encloses() {
    let mut app = crate::test_fixtures::fixture_app();
    press(&mut app, KeyCode::Char('/'));
    assert!(matches!(app.mode, Mode::Search { pane: ListPane::Queue }));
    press(&mut app, KeyCode::Char('d'));
    press(&mut app, KeyCode::Char('o'));
    assert_eq!(app.ui().search[0], "do");
    // "do" (in "docs") matches only 01QUE's summary → 1 visible row, cursor reset.
    assert_eq!(app.visible_len(ListPane::Queue), 1);
    assert_eq!(app.ui().selections[0], Selection { cursor: 0, anchor: None });
    press(&mut app, KeyCode::Backspace);
    assert_eq!(app.ui().search[0], "d");
    press(&mut app, KeyCode::Enter); // apply: keep filter, back to list
    assert!(matches!(app.mode, Mode::List));
    assert_eq!(app.ui().search[0], "d");
    // esc inside search clears + closes.
    press(&mut app, KeyCode::Char('/'));
    press(&mut app, KeyCode::Char('x'));
    press(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, Mode::List));
    assert_eq!(app.ui().search[0], "");
}

#[test]
fn search_typing_schedules_run_read() {
    // A search keystroke resets the effective selection to cursor 0 of the
    // filtered list, so it must schedule the debounced run-file read (every
    // other selection-changing path does).
    let mut app = crate::test_fixtures::fixture_app();
    press(&mut app, KeyCode::Char('/')); // search over the queue pane
    // 'c' (in "cache") keeps 01RUN at cursor 0 of the filtered queue.
    let up = press(&mut app, KeyCode::Char('c'));
    assert!(up.cmds.iter().any(|c| matches!(
        c,
        Cmd::ReadRunFiles { task_id, delay_ms: 120, .. } if task_id == "01RUN"
    )), "typing must schedule the 120ms run-file read");
    // Backspace re-scopes the selection → schedules again.
    let up = press(&mut app, KeyCode::Backspace);
    assert!(up.cmds.iter().any(|c| matches!(
        c,
        Cmd::ReadRunFiles { task_id, delay_ms: 120, .. } if task_id == "01RUN"
    )), "backspace must schedule the 120ms run-file read");
}

#[test]
fn search_enter_apply_schedules_run_read() {
    let mut app = crate::test_fixtures::fixture_app();
    press(&mut app, KeyCode::Char('/'));
    press(&mut app, KeyCode::Char('c')); // filter, keeping 01RUN at cursor 0
    let up = press(&mut app, KeyCode::Enter); // apply
    assert!(matches!(app.mode, Mode::List));
    assert!(up.cmds.iter().any(|c| matches!(
        c,
        Cmd::ReadRunFiles { task_id, delay_ms: 120, .. } if task_id == "01RUN"
    )), "Enter-apply must schedule the 120ms run-file read");
}

#[test]
fn search_esc_clear_schedules_run_read() {
    let mut app = crate::test_fixtures::fixture_app();
    press(&mut app, KeyCode::Char('/'));
    press(&mut app, KeyCode::Char('c')); // filter, keeping 01RUN at cursor 0
    let up = press(&mut app, KeyCode::Esc); // clear filter + close
    assert!(matches!(app.mode, Mode::List));
    assert_eq!(app.ui().search[0], "");
    assert!(up.cmds.iter().any(|c| matches!(
        c,
        Cmd::ReadRunFiles { task_id, delay_ms: 120, .. } if task_id == "01RUN"
    )), "Esc-clear must schedule the 120ms run-file read");
}

#[test]
fn help_opens_and_any_key_closes() {
    let mut app = crate::test_fixtures::fixture_app();
    press(&mut app, KeyCode::Char('?'));
    assert!(matches!(app.mode, Mode::Help));
    let up = press(&mut app, KeyCode::Char('z'));
    assert!(matches!(app.mode, Mode::List));
    assert!(up.dirty);
}

#[test]
fn settings_opens_fetches_once_and_any_key_closes() {
    let mut app = crate::test_fixtures::fixture_app();
    // First open: enters the overlay AND emits exactly one FetchSettings
    // (settings is None → never fetched).
    let up = press(&mut app, KeyCode::Char('s'));
    assert!(matches!(app.mode, Mode::Settings));
    assert_eq!(
        up.cmds.iter().filter(|c| matches!(c, Cmd::FetchSettings)).count(),
        1,
        "first open fetches settings"
    );
    // Any key closes.
    let up = press(&mut app, KeyCode::Char('z'));
    assert!(matches!(app.mode, Mode::List));
    assert!(up.dirty);
    // The reply lands and is cached.
    app.update(Event::Settings { payload: Some(SettingsPayload::default()) });
    assert!(matches!(app.settings, Some(Some(_))));
    // Second open: cached → NO re-fetch.
    let up = press(&mut app, KeyCode::Char('s'));
    assert!(matches!(app.mode, Mode::Settings));
    assert!(
        !up.cmds.iter().any(|c| matches!(c, Cmd::FetchSettings)),
        "cached settings must not re-fetch on re-open"
    );
}

#[test]
fn settings_failed_fetch_caches_none_and_does_not_refetch() {
    let mut app = crate::test_fixtures::fixture_app();
    press(&mut app, KeyCode::Char('s'));
    // A failed/unsupported fetch caches Some(None) (the "unavailable" state).
    app.update(Event::Settings { payload: None });
    assert!(matches!(app.settings, Some(None)));
    press(&mut app, KeyCode::Char('z')); // close
    // Re-open must NOT re-fetch — Some(None) is a cached outcome, not "never
    // fetched".
    let up = press(&mut app, KeyCode::Char('s'));
    assert!(
        !up.cmds.iter().any(|c| matches!(c, Cmd::FetchSettings)),
        "cached failure must not re-fetch"
    );
}

#[test]
fn status_line_clears_on_list_mode_keypress() {
    let mut app = crate::test_fixtures::fixture_app();
    app.status_line = Some("boom".into());
    let up = press(&mut app, KeyCode::Char('z')); // even an unbound key
    assert_eq!(app.status_line, None);
    assert!(up.dirty);
}

#[test]
fn cycle_sub_tab_wraps_within_kind() {
    let mut app = crate::test_fixtures::fixture_app();
    // Run context (queue cursor 0 → 01RUN): 3 sub-tabs. ctrl+x = next (global,
    // no detail focus needed), ctrl+z = previous.
    let ctrl = |c| Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
    app.update(ctrl('x'));
    assert_eq!(app.ui().sub_tab[DetailKind::Run as usize], 1);
    app.update(ctrl('x'));
    app.update(ctrl('x'));
    assert_eq!(app.ui().sub_tab[DetailKind::Run as usize], 0, "wraps past the end");
    app.update(ctrl('z'));
    assert_eq!(app.ui().sub_tab[DetailKind::Run as usize], 2, "wraps below zero");
}

// -- Task 12: mouse routing ----------------------------------------------------
use crate::hit::HitTarget;
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

fn mouse(kind: MouseEventKind, col: u16, row: u16) -> Event {
    Event::Mouse(MouseEvent { kind, column: col, row, modifiers: KeyModifiers::NONE })
}

/// Fixture app with a synthetic hit map (no render needed): a tab, three queue
/// rows at y=2..5, the queue body, and a queue scrollbar track x=30 y=2 h=10.
fn app_with_hits() -> App {
    let mut app = crate::test_fixtures::fixture_app();
    let mut hits = crate::hit::HitMap::new();
    hits.push(Rect { x: 0, y: 0, width: 10, height: 1 }, HitTarget::Tab(0));
    hits.push(Rect { x: 1, y: 2, width: 28, height: 8 }, HitTarget::PaneBody(PaneId::Queue));
    for i in 0..4usize {
        hits.push(
            Rect { x: 1, y: 2 + i as u16, width: 28, height: 1 },
            HitTarget::Row(ListPane::Queue, i),
        );
    }
    hits.push(Rect { x: 30, y: 2, width: 1, height: 10 }, HitTarget::ScrollbarTrack(PaneId::Queue));
    app.hit = hits;
    app
}

#[test]
fn click_row_focuses_and_selects_without_opening_menu() {
    let mut app = app_with_hits();
    app.set_focus(PaneId::Tasks); // start on another list pane
    let up = app.update(mouse(MouseEventKind::Down(MouseButton::Left), 5, 4)); // row 2
    assert!(up.dirty);
    assert_eq!(app.ui().focus, PaneId::Queue);
    assert_eq!(app.ui().selections[0], Selection { cursor: 2, anchor: None });
    assert!(matches!(app.mode, Mode::List), "single click selects only");
}

#[test]
fn double_click_same_row_within_window_opens_menu() {
    let mut app = app_with_hits();
    app.now_ms = 1_000;
    app.update(mouse(MouseEventKind::Down(MouseButton::Left), 5, 3)); // click row 1
    app.status_line = None;
    app.now_ms = 1_200; // 200ms later (< 400ms) → double-click
    app.update(mouse(MouseEventKind::Down(MouseButton::Left), 5, 3));
    match &app.mode {
        // Row 1 is the queued fixture task "write docs for the cache". The queue
        // menu is now a single Resume row.
        Mode::ActionMenu { title, items, index, .. } => {
            assert_eq!(title, "write docs for the cache");
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].label, "Resume");
            assert_eq!(*index, 0);
        }
        other => panic!("expected ActionMenu, got {other:?}"),
    }
}

#[test]
fn slow_second_click_only_reselects() {
    let mut app = app_with_hits();
    app.now_ms = 1_000;
    app.update(mouse(MouseEventKind::Down(MouseButton::Left), 5, 3)); // click row 1
    app.now_ms = 1_500; // 500ms later (> 400ms) → NOT a double-click
    app.update(mouse(MouseEventKind::Down(MouseButton::Left), 5, 3));
    assert!(matches!(app.mode, Mode::List), "slow second click must not open the menu");
    assert_eq!(app.ui().selections[0], Selection { cursor: 1, anchor: None });
}

#[test]
fn resort_between_clicks_keys_on_identity_not_index() {
    // Click row 0 (arms on the row's task id), then a new snapshot resorts a
    // DIFFERENT task into index 0. A second click at index 0 within the window
    // must NOT open the menu — the identity changed — it only re-selects.
    let mut app = app_with_hits();
    app.now_ms = 1_000;
    app.update(mouse(MouseEventKind::Down(MouseButton::Left), 5, 2)); // click row 0
    // Snapshot with a single running task whose id differs from the fixture's
    // row-0 task → index 0 now resolves to a different identity.
    app.update(Event::Snapshot(snapshot_with(&["acme"], vec![running_task("acme")])));
    app.now_ms = 1_200; // within 400ms
    app.update(mouse(MouseEventKind::Down(MouseButton::Left), 5, 2)); // click index 0 again
    assert!(
        matches!(app.mode, Mode::List),
        "a resort into the clicked slot must not fire the menu on the wrong row"
    );
    assert_eq!(app.ui().selections[0], Selection { cursor: 0, anchor: None });
}

/// App with a DETAIL `PaneBody` hit rect + published selection geometry over a
/// small content area (x=1,y=1,20×4) and three known wrapped lines — ready to
/// drive a text-selection drag without rendering.
fn app_with_detail() -> App {
    let mut app = crate::test_fixtures::fixture_app();
    let area = Rect { x: 1, y: 1, width: 20, height: 4 };
    let mut hits = crate::hit::HitMap::new();
    hits.push(area, HitTarget::PaneBody(PaneId::Detail));
    app.hit = hits;
    *app.detail_geom.borrow_mut() = DetailGeom {
        area,
        window_start: 0,
        lines: vec![
            "hello world".to_string(),
            "second line".to_string(),
            "third".to_string(),
        ],
    };
    app
}

#[test]
fn detail_drag_selects_and_copies_on_release() {
    let mut app = app_with_detail();
    // Press at cell 0 of line 0 (col == area.x == 1, row == area.y == 1).
    app.update(mouse(MouseEventKind::Down(MouseButton::Left), 1, 1));
    assert!(matches!(app.drag, Some(DragKind::DetailSelect)));
    assert!(app.detail_selection.is_some());
    // Drag to cell 4 (col 5) on the same line → "hello".
    app.update(mouse(MouseEventKind::Drag(MouseButton::Left), 5, 1));
    let up = app.update(mouse(MouseEventKind::Up(MouseButton::Left), 5, 1));
    assert!(app.drag.is_none(), "drag ends on release");
    assert_eq!(app.status_line.as_deref(), Some("copied 5 chars"));
    assert!(
        up.cmds.iter().any(|c| matches!(c, Cmd::CopyClipboard { text } if text == "hello")),
        "release emits a clipboard copy of the selected text"
    );
    assert!(app.detail_selection.is_some(), "selection persists (highlighted) after release");
}

#[test]
fn release_arms_the_fade_and_matching_expiry_clears_but_stale_does_not() {
    let mut app = app_with_detail();
    app.update(mouse(MouseEventKind::Down(MouseButton::Left), 1, 1));
    app.update(mouse(MouseEventKind::Drag(MouseButton::Left), 5, 1));
    let up = app.update(mouse(MouseEventKind::Up(MouseButton::Left), 5, 1));
    let armed = up.cmds.iter().find_map(|c| match c {
        Cmd::ExpireSelection { epoch, delay_ms } => Some((*epoch, *delay_ms)),
        _ => None,
    });
    assert_eq!(armed, Some((app.selection_epoch, 1000)), "release arms a 1s fade");
    // A stale expiry (older generation) is a no-op.
    let stale = app.update(Event::SelectionExpired { epoch: app.selection_epoch - 1 });
    assert!(!stale.dirty);
    assert!(app.detail_selection.is_some(), "stale expiry must not clear a live selection");
    // The matching expiry clears the highlight.
    let hit = app.update(Event::SelectionExpired { epoch: app.selection_epoch });
    assert!(hit.dirty);
    assert!(app.detail_selection.is_none(), "matching expiry fades the highlight");
}

#[test]
fn new_selection_within_the_fade_window_survives_the_old_timer() {
    let mut app = app_with_detail();
    app.update(mouse(MouseEventKind::Down(MouseButton::Left), 1, 1));
    app.update(mouse(MouseEventKind::Drag(MouseButton::Left), 5, 1));
    app.update(mouse(MouseEventKind::Up(MouseButton::Left), 5, 1));
    let old_epoch = app.selection_epoch;
    // A new selection starts before the old fade fires...
    app.update(mouse(MouseEventKind::Down(MouseButton::Left), 1, 2));
    app.update(mouse(MouseEventKind::Drag(MouseButton::Left), 4, 2));
    // ...so the old timer's expiry is stale and leaves it alone.
    app.update(Event::SelectionExpired { epoch: old_epoch });
    assert!(app.detail_selection.is_some(), "old fade must not kill the new selection");
}

#[test]
fn detail_drag_across_lines_copies_with_newline() {
    let mut app = app_with_detail();
    app.update(mouse(MouseEventKind::Down(MouseButton::Left), 7, 1)); // line 0, cell 6 = "world"
    app.update(mouse(MouseEventKind::Drag(MouseButton::Left), 3, 2)); // line 1, cell 2 = "sec"
    let up = app.update(mouse(MouseEventKind::Up(MouseButton::Left), 3, 2));
    let copied = up.cmds.iter().find_map(|c| match c {
        Cmd::CopyClipboard { text } => Some(text.clone()),
        _ => None,
    });
    assert_eq!(copied.as_deref(), Some("world\nsec"));
    assert_eq!(app.status_line.as_deref(), Some("copied 2 lines"));
}

#[test]
fn detail_plain_click_clears_selection_and_copies_nothing() {
    let mut app = app_with_detail();
    // Seed a prior selection; a plain click (no drag) must clear it, no copy.
    app.detail_selection = Some(DetailSelection {
        anchor: DetailPoint { line: 0, cell: 0 },
        cursor: DetailPoint { line: 1, cell: 3 },
    });
    app.update(mouse(MouseEventKind::Down(MouseButton::Left), 3, 2));
    let up = app.update(mouse(MouseEventKind::Up(MouseButton::Left), 3, 2));
    assert!(app.detail_selection.is_none(), "plain click clears the selection");
    assert!(
        !up.cmds.iter().any(|c| matches!(c, Cmd::CopyClipboard { .. })),
        "no copy on plain click"
    );
    assert!(app.status_line.is_none());
}

#[test]
fn detail_selection_cleared_on_content_change() {
    let mut app = app_with_detail();
    app.detail_selection = Some(DetailSelection {
        anchor: DetailPoint { line: 0, cell: 0 },
        cursor: DetailPoint { line: 0, cell: 3 },
    });
    app.reset_scroll(); // sub-tab / selection changes route through here
    assert!(app.detail_selection.is_none());
}

fn ctrl_s(app: &mut App) -> Update {
    app.update(Event::Key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL)))
}

#[test]
fn ctrl_s_prefix_then_n_p_cycles_project_tabs() {
    let mut app = app();
    app.update(Event::Snapshot(snapshot_with(&["a", "b", "c"], vec![])));
    app.active_tab = 0;
    assert!(ctrl_s(&mut app).dirty);
    assert!(app.prefix_armed, "ctrl+s arms the prefix");
    press(&mut app, KeyCode::Char('n')); // next tab, disarms
    assert!(!app.prefix_armed);
    assert_eq!(app.active_tab, 1);
    ctrl_s(&mut app);
    press(&mut app, KeyCode::Char('p')); // previous tab (wraps)
    assert_eq!(app.active_tab, 0);
}

#[test]
fn ctrl_s_prefix_swallows_other_keys_and_disarms() {
    let mut app = app();
    app.update(Event::Snapshot(snapshot_with(&["a", "b"], vec![])));
    app.active_tab = 0;
    ctrl_s(&mut app);
    let before = app.collapsed;
    press(&mut app, KeyCode::Char('z')); // would collapse — swallowed by the prefix
    assert!(!app.prefix_armed, "any other key disarms");
    assert_eq!(app.active_tab, 0, "tab unchanged");
    assert_eq!(app.collapsed, before, "swallowed key had no effect");
}

#[test]
fn pane_button_create_click_focuses_pane_then_acts() {
    let mut app = app_with_hits();
    app.set_focus(PaneId::Queue);
    let mut hits = app.hit.clone();
    hits.push(
        Rect { x: 20, y: 0, width: 4, height: 1 },
        HitTarget::PaneButton(PaneId::Worktrees, crate::hit::PaneButton::Create),
    );
    app.hit = hits;
    app.update(mouse(MouseEventKind::Down(MouseButton::Left), 21, 0));
    // Focus moved to worktrees first, then `c` opened the create-worktree modal.
    assert_eq!(app.active_ui().last_list_pane, ListPane::Worktrees);
    assert!(matches!(app.mode, Mode::CreateWorktree { .. }));
}

#[test]
fn pane_button_tasks_click_focuses_pane_then_opens_task_menu() {
    let mut app = app_with_hits();
    app.set_focus(PaneId::Queue);
    // Seed a def so the click lands in DefPick rather than the empty-defs
    // status line.
    app.defs_by_project.insert("acme".into(), vec![{
        let mut d = crate::ipc::types::DefinitionSummary::default();
        d.repo = "acme".into();
        d.name = "autotest".into();
        d
    }]);
    let mut hits = app.hit.clone();
    hits.push(
        Rect { x: 20, y: 0, width: 4, height: 1 },
        HitTarget::PaneButton(PaneId::Worktrees, crate::hit::PaneButton::Tasks),
    );
    app.hit = hits;
    app.update(mouse(MouseEventKind::Down(MouseButton::Left), 21, 0));
    // Focus moved to worktrees first, then the task menu opened carrying the
    // selected worktree row's context.
    assert_eq!(app.active_ui().last_list_pane, ListPane::Worktrees);
    match &app.mode {
        Mode::DefPick { worktree, .. } => assert!(worktree.is_some()),
        other => panic!("expected DefPick, got {other:?}"),
    }
}

#[test]
fn pane_button_collapse_click_toggles_without_moving_focus() {
    let mut app = app_with_hits();
    app.set_focus(PaneId::Queue);
    let before = app.collapsed[ListPane::Tasks.idx()];
    let mut hits = app.hit.clone();
    hits.push(
        Rect { x: 20, y: 0, width: 4, height: 1 },
        HitTarget::PaneButton(PaneId::Tasks, crate::hit::PaneButton::Collapse),
    );
    app.hit = hits;
    app.update(mouse(MouseEventKind::Down(MouseButton::Left), 21, 0));
    assert_ne!(app.collapsed[ListPane::Tasks.idx()], before, "collapse toggled");
    assert_eq!(app.ui().focus, PaneId::Queue, "collapse button leaves focus put");
}

/// The right-aligned `PaneButton` rect for `(pane, btn)` from a real render.
fn pane_button_rect(app: &App, pane: PaneId, btn: crate::hit::PaneButton) -> Rect {
    app.hit
        .iter()
        .find_map(|(r, t)| (*t == HitTarget::PaneButton(pane, btn)).then_some(*r))
        .unwrap_or_else(|| panic!("pane button {pane:?}/{btn:?} registered"))
}

#[test]
fn real_render_worktrees_create_chip_click_opens_modal() {
    // Worktrees' top border is the lower row of divider band 1; the chip must
    // still win the click (PaneButton registered after PaneDividerH). Rendered
    // wide: `create` is a pane-scoped chip (drops before the row-scoped verbs
    // on a narrow pane), so it needs a wide worktrees title bar to survive.
    let mut app = app_rendered(120, 24);
    app.set_focus(PaneId::Queue);
    let r = pane_button_rect(&app, PaneId::Worktrees, crate::hit::PaneButton::Create);
    app.update(mouse(
        MouseEventKind::Down(MouseButton::Left),
        r.x + r.width / 2,
        r.y,
    ));
    assert_eq!(app.active_ui().last_list_pane, ListPane::Worktrees, "focus moved");
    assert!(matches!(app.mode, Mode::CreateWorktree { .. }), "create modal opened");
}

#[test]
fn real_render_tasks_collapse_chip_click_toggles_over_divider() {
    // Tasks' top border is the lower row of divider band 0; the collapse chip
    // must win over the divider and leave focus unchanged.
    let mut app = app_rendered(80, 24);
    app.set_focus(PaneId::Queue);
    let before = app.collapsed[ListPane::Tasks.idx()];
    let r = pane_button_rect(&app, PaneId::Tasks, crate::hit::PaneButton::Collapse);
    assert_eq!(app.drag, None);
    app.update(mouse(
        MouseEventKind::Down(MouseButton::Left),
        r.x + r.width / 2,
        r.y,
    ));
    assert_ne!(app.collapsed[ListPane::Tasks.idx()], before, "collapse toggled");
    assert_eq!(app.ui().focus, PaneId::Queue, "collapse leaves focus put");
    assert_eq!(app.drag, None, "chip click did not start a divider drag");
}

#[test]
fn detail_body_click_does_not_steal_focus() {
    let mut app = app_with_hits();
    app.set_focus(PaneId::Queue);
    let mut hits = app.hit.clone();
    hits.push(
        Rect { x: 40, y: 2, width: 20, height: 10 },
        HitTarget::PaneBody(PaneId::Detail),
    );
    app.hit = hits;
    let up = app.update(mouse(MouseEventKind::Down(MouseButton::Left), 45, 5));
    assert!(!up.dirty, "detail body click is inert");
    assert_eq!(app.ui().focus, PaneId::Queue);
}

#[test]
fn shift_click_extends_selection() {
    let mut app = app_with_hits();
    // Plain click must land on an UNSELECTED row (the default cursor is 0, so
    // clicking row 0 would open its action menu). Row 1 becomes the anchor.
    app.update(mouse(MouseEventKind::Down(MouseButton::Left), 5, 3)); // row 1
    let ev = Event::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 5,
        row: 5, // row 3
        modifiers: KeyModifiers::SHIFT,
    });
    app.update(ev);
    assert_eq!(app.ui().selections[0], Selection { cursor: 3, anchor: Some(1) });
}

#[test]
fn wheel_moves_pane_under_cursor_without_focus_change() {
    let mut app = app_with_hits();
    app.set_focus(PaneId::Tasks);
    let up = app.update(mouse(MouseEventKind::ScrollDown, 5, 3)); // over queue body
    assert!(up.dirty);
    assert_eq!(app.ui().focus, PaneId::Tasks, "wheel must not steal focus");
    assert_eq!(app.ui().selections[0].cursor, 1);
    app.update(mouse(MouseEventKind::ScrollUp, 5, 3));
    assert_eq!(app.ui().selections[0].cursor, 0);
}

#[test]
fn scrollbar_drag_math_maps_proportionally() {
    let mut app = app_with_hits();
    // Track: y=2, h=10. Queue has 4 rows → scrollable = 3.
    app.update(mouse(MouseEventKind::Down(MouseButton::Left), 30, 2)); // top
    assert!(app.drag == Some(DragKind::Scrollbar(PaneId::Queue)));
    assert_eq!(app.ui().selections[0].cursor, 0); // (2−2)*3/10 = 0
    app.update(mouse(MouseEventKind::Drag(MouseButton::Left), 30, 11)); // near bottom
    assert_eq!(app.ui().selections[0].cursor, 2); // (11−2)*3/10 = 2
    app.update(mouse(MouseEventKind::Drag(MouseButton::Left), 30, 40)); // past end → clamp
    assert_eq!(app.ui().selections[0].cursor, 3);
    app.update(mouse(MouseEventKind::Up(MouseButton::Left), 30, 40));
    assert_eq!(app.drag, None);
}

/// Render a real fixture app to a `TestBackend` so `app.hit` carries the true
/// divider geometry (mirrors the view tests' `render_at`).
fn app_rendered(w: u16, h: u16) -> App {
    use ratatui::{Terminal, backend::TestBackend};
    let mut app = crate::test_fixtures::fixture_app();
    app.size = (w, h);
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    let mut hits = crate::hit::HitMap::new();
    terminal.draw(|f| hits = crate::view::render(&app, f)).unwrap();
    app.hit = hits;
    app
}

fn divider_h_rect(app: &App, which: usize) -> Rect {
    app.hit
        .iter()
        .find_map(|(r, t)| (*t == HitTarget::PaneDividerH(which)).then_some(*r))
        .expect("horizontal divider registered")
}

fn divider_v_rect(app: &App) -> Rect {
    app.hit
        .iter()
        .find_map(|(r, t)| (*t == HitTarget::PaneDividerV).then_some(*r))
        .expect("vertical divider registered")
}

#[test]
fn drag_horizontal_divider_resizes_queue_and_up_ends_drag() {
    let mut app = app_rendered(80, 24);
    // Default at 80x24: body_h = 22 → queue 12, tasks 5, worktrees 5.
    assert_eq!(app.queue_h_override, None);
    let r = divider_h_rect(&app, 0); // queue/tasks boundary
    // Down on the divider records the drag kind.
    app.update(mouse(MouseEventKind::Down(MouseButton::Left), r.x + 2, r.y));
    assert_eq!(app.drag, Some(DragKind::DividerH(0)));
    // Drag the boundary down several rows → the queue pane grows.
    let u = app.update(mouse(MouseEventKind::Drag(MouseButton::Left), r.x + 2, r.y + 4));
    assert!(u.dirty);
    let q = app.queue_h_override.expect("queue override set by drag");
    assert!(q > 12, "dragging the boundary down grows the queue (q={q})");
    // Overrides never violate the minimum-height / exact-sum invariant.
    let l = crate::selectors::pane_layout(22, app.queue_h_override, app.tasks_h_override, app.collapsed);
    assert_eq!(l.queue_h + l.tasks_h + l.worktrees_h, 22);
    assert!(l.worktrees_h >= 4 && l.tasks_h >= 4);
    // Up ends the drag; the override persists.
    app.update(mouse(MouseEventKind::Up(MouseButton::Left), r.x + 2, r.y + 4));
    assert_eq!(app.drag, None);
    assert_eq!(app.queue_h_override, Some(q));
}

#[test]
fn drag_tasks_worktrees_divider_resizes_tasks() {
    let mut app = app_rendered(80, 24);
    let r = divider_h_rect(&app, 1); // tasks/worktrees boundary
    app.update(mouse(MouseEventKind::Down(MouseButton::Left), r.x + 2, r.y));
    assert_eq!(app.drag, Some(DragKind::DividerH(1)));
    // Drag the lower boundary down → the tasks pane grows past its default 5.
    app.update(mouse(MouseEventKind::Drag(MouseButton::Left), r.x + 2, r.y + 3));
    let t = app.tasks_h_override.expect("tasks override set");
    assert!(t > 5, "tasks pane grows (t={t})");
    let l = crate::selectors::pane_layout(22, app.queue_h_override, app.tasks_h_override, app.collapsed);
    assert_eq!(l.queue_h + l.tasks_h + l.worktrees_h, 22);
    assert!(l.worktrees_h >= 4);
}

#[test]
fn drag_vertical_divider_resizes_left_column() {
    let mut app = app_rendered(80, 24);
    assert_eq!(app.left_cols, None);
    let r = divider_v_rect(&app);
    // Down a few rows below the top corner (avoid the H-divider overlap).
    app.update(mouse(MouseEventKind::Down(MouseButton::Left), r.x, r.y + 3));
    assert_eq!(app.drag, Some(DragKind::DividerV));
    // Drag right → the left column widens to the clamped drop column.
    let target_col = r.x + 12;
    app.update(mouse(MouseEventKind::Drag(MouseButton::Left), target_col, r.y + 3));
    assert_eq!(app.left_cols, Some(crate::selectors::clamp_left_cols(80, target_col)));
    assert!(app.left_cols.unwrap() > r.x, "left column grew");
    app.update(mouse(MouseEventKind::Up(MouseButton::Left), target_col, r.y + 3));
    assert_eq!(app.drag, None);
}

// -- pane collapse + per-project layout persistence ----------------------

#[test]
fn key_z_toggles_focused_pane_collapse_and_emits_save() {
    let mut app = crate::test_fixtures::fixture_app();
    // Route the fixture through a snapshot so the active project's layout is
    // reconciled (applied_layout_repo becomes Some("acme")).
    app.update(Event::Snapshot(crate::test_fixtures::fixture_snapshot()));
    assert_eq!(app.collapsed, [false, false, false]);
    // Focus is Queue by default → `z` collapses the queue pane and persists.
    let up = press(&mut app, KeyCode::Char('z'));
    assert_eq!(app.collapsed, [true, false, false]);
    assert!(
        up.cmds.iter().any(|c| matches!(c, Cmd::SaveLayout { .. })),
        "collapse toggle emits a SaveLayout Cmd"
    );
    // Toggling again expands it, again persisting.
    let up = press(&mut app, KeyCode::Char('z'));
    assert_eq!(app.collapsed, [false, false, false]);
    assert!(up.cmds.iter().any(|c| matches!(c, Cmd::SaveLayout { .. })));
}

#[test]
fn key_z_on_detail_focus_is_noop() {
    let mut app = crate::test_fixtures::fixture_app();
    app.set_focus(PaneId::Detail);
    let up = press(&mut app, KeyCode::Char('z'));
    assert_eq!(app.collapsed, [false, false, false]);
    assert!(!up.cmds.iter().any(|c| matches!(c, Cmd::SaveLayout { .. })));
}

#[test]
fn click_on_bare_title_row_does_not_toggle_collapse() {
    // The whole-row collapse toggle was removed: it swallowed divider drags
    // starting on the shared border row. Collapse is the chip or `z` only.
    let mut app = app_rendered(80, 24);
    let r = divider_h_rect(&app, 0); // queue/tasks boundary = TASKS title row
    // Click a border cell away from any chip: starts a divider drag, never
    // a collapse.
    app.update(mouse(MouseEventKind::Down(MouseButton::Left), r.x + 2, r.y + 1));
    assert_eq!(app.collapsed, [false, false, false]);
    assert!(matches!(app.drag, Some(DragKind::DividerH(0))));
    app.update(mouse(MouseEventKind::Up(MouseButton::Left), r.x + 2, r.y + 1));
}

#[test]
fn divider_drag_up_emits_save() {
    let mut app = app_rendered(80, 24);
    let r = divider_h_rect(&app, 0);
    app.update(mouse(MouseEventKind::Down(MouseButton::Left), r.x + 2, r.y));
    app.update(mouse(MouseEventKind::Drag(MouseButton::Left), r.x + 2, r.y + 4));
    // No SaveLayout mid-drag; the write happens only on release.
    let up = app.update(mouse(MouseEventKind::Up(MouseButton::Left), r.x + 2, r.y + 4));
    assert_eq!(app.drag, None);
    assert!(
        up.cmds.iter().any(|c| matches!(c, Cmd::SaveLayout { .. })),
        "divider drag-end emits a SaveLayout Cmd"
    );
}

#[test]
fn scrollbar_drag_up_does_not_emit_save() {
    let mut app = app_with_hits();
    app.update(mouse(MouseEventKind::Down(MouseButton::Left), 30, 2)); // scrollbar track
    let up = app.update(mouse(MouseEventKind::Up(MouseButton::Left), 30, 5));
    assert_eq!(app.drag, None);
    assert!(!up.cmds.iter().any(|c| matches!(c, Cmd::SaveLayout { .. })));
}

#[test]
fn divider_drag_ignored_when_adjacent_pane_collapsed() {
    let mut app = app_rendered(80, 24);
    app.collapsed = [false, true, false]; // tasks collapsed → both H dividers pinned
    let before = app.queue_h_override;
    assert!(!app.drag_divider_h(0, 20), "queue/tasks boundary can't move");
    assert!(!app.drag_divider_h(1, 20), "tasks/worktrees boundary can't move");
    assert_eq!(app.queue_h_override, before);
}

#[test]
fn switching_projects_swaps_and_isolates_layout() {
    let mut app = app();
    app.update(Event::Snapshot(snapshot_with(&["platform", "web"], vec![])));
    // On project 0 (platform): collapse the queue pane.
    assert_eq!(app.active_tab, 0);
    app.collapsed = [true, false, false];
    // Switch to project 1 (web): platform's layout is stashed, web loads
    // defaults (nothing saved yet).
    press(&mut app, KeyCode::Char('2'));
    assert_eq!(app.active_tab, 1);
    assert_eq!(app.collapsed, [false, false, false], "web starts at defaults");
    // Give web a distinct layout, then switch back to platform.
    app.collapsed = [false, false, true];
    press(&mut app, KeyCode::Char('1'));
    assert_eq!(app.active_tab, 0);
    assert_eq!(app.collapsed, [true, false, false], "platform's layout restored");
    // And forward to web again → its own layout.
    press(&mut app, KeyCode::Char('2'));
    assert_eq!(app.collapsed, [false, false, true], "web's layout restored");
}

#[test]
fn loaded_layout_applies_to_active_project_on_first_snapshot() {
    let mut app = app();
    // Simulate a persisted layout for "platform" loaded at startup.
    app.layout_by_project.insert(
        "platform".to_string(),
        crate::layout::ProjectLayout {
            left_cols: Some(50),
            queue_h: None,
            tasks_h: None,
            collapsed: [false, true, false],
        },
    );
    app.update(Event::Snapshot(snapshot_with(&["platform"], vec![])));
    assert_eq!(app.collapsed, [false, true, false]);
    assert_eq!(app.left_cols, Some(50));
}

#[test]
fn click_tab_switches_and_click_nothing_closes_overlay() {
    let mut app = app_with_hits();
    app.mode = Mode::Help;
    // Click hitting no target while an overlay is open → ClearEsc semantics.
    let up = app.update(mouse(MouseEventKind::Down(MouseButton::Left), 70, 20));
    assert!(matches!(app.mode, Mode::List));
    assert!(up.dirty);
    // Tab click (single-project fixture: index 0 → no change, not dirty).
    let up = app.update(mouse(MouseEventKind::Down(MouseButton::Left), 3, 0));
    assert!(!up.dirty);
}

#[test]
fn modal_target_swallows_clicks() {
    let mut app = app_with_hits();
    app.hit.push(Rect { x: 0, y: 0, width: 80, height: 24 }, HitTarget::Modal);
    app.mode = Mode::Help;
    let up = app.update(mouse(MouseEventKind::Down(MouseButton::Left), 5, 4));
    assert!(matches!(app.mode, Mode::Help), "click inside a modal never leaks through");
    assert!(!up.dirty);
}

/// Confirm/Help overlays own every click: a click on a live pane widget
/// behind the popup dismisses it (same as esc, no dispatch); a click inside
/// the modal body is inert.
fn assert_overlay_owns_clicks(make_mode: impl Fn() -> Mode) {
    // Click on Row(Queue, 2) at (5, 4), behind the popup → dismiss, no cmd.
    let mut app = app_with_hits();
    app.mode = make_mode();
    let up = app.update(mouse(MouseEventKind::Down(MouseButton::Left), 5, 4));
    assert!(matches!(app.mode, Mode::List), "outside-click must dismiss");
    assert!(up.dirty);
    assert!(up.cmds.is_empty(), "outside-click dismiss dispatches nothing");

    // Click inside the modal body (Modal covers the screen) → inert.
    let mut app = app_with_hits();
    app.hit.push(Rect { x: 0, y: 0, width: 80, height: 24 }, HitTarget::Modal);
    app.mode = make_mode();
    let up = app.update(mouse(MouseEventKind::Down(MouseButton::Left), 5, 4));
    assert!(!matches!(app.mode, Mode::List), "click inside modal stays open");
    assert!(!up.dirty, "click inside modal is inert");
    assert!(up.cmds.is_empty());
}

#[test]
fn confirm_remove_overlay_owns_clicks() {
    assert_overlay_owns_clicks(|| Mode::ConfirmRemove {
        repo: "acme".into(),
        worktree: "acme.feature".into(),
        branch: "feature/x".into(),
    });
}

#[test]
fn confirm_bulk_remove_overlay_owns_clicks() {
    assert_overlay_owns_clicks(|| Mode::ConfirmBulkRemove {
        repo: "acme".into(),
        names: vec!["acme.feature".into(), "acme.hotfix".into()],
    });
}

#[test]
fn help_overlay_owns_clicks() {
    assert_overlay_owns_clicks(|| Mode::Help);
}

#[test]
fn settings_overlay_owns_clicks() {
    assert_overlay_owns_clicks(|| Mode::Settings);
}

#[test]
fn idle_tick_with_nothing_running_is_not_dirty() {
    let mut app = crate::test_fixtures::fixture_app();
    let mut snap = crate::test_fixtures::fixture_snapshot();
    snap.running.clear();
    for t in &mut snap.tasks {
        if matches!(t.status, crate::ipc::types::TaskStatus::Running) {
            t.status = crate::ipc::types::TaskStatus::Done;
        }
    }
    app.update(Event::Snapshot(snap));
    let up = app.update(Event::Tick);
    assert!(!up.dirty, "zero idle renders: Tick with nothing running must not dirty");
}
