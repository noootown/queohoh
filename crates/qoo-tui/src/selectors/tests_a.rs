    use super::*;
    use crate::ipc::types::{
        CatalogEntry, DefaultModels, ModelRef, Project, SessionEntry, TaskTarget, WorktreeInfo,
    };

    // ---- fixtures (mirror __tests__/helpers.ts makeTask/makeSnapshot/makeSession) ----

    /// Empty resolve ctx — model column blanks (used by layout tests that don't
    /// care about the Model column).
    fn empty_model_owned() -> ModelResolveOwned {
        ModelResolveOwned::default()
    }

    /// Catalog + enabled providers matching the core BUILTIN_CATALOG subset the
    /// form picker ships (claude + grok). `active` drives re-head / group-head.
    fn resolve_owned(active: &str) -> ModelResolveOwned {
        let e = |provider: &str, id: &str, label: &str| CatalogEntry {
            provider: provider.into(),
            id: id.into(),
            label: label.into(),
            hidden: false,
        };
        ModelResolveOwned {
            catalog: vec![
                e("claude", "claude-fable-5", "claude-fable-5"),
                e("claude", "claude-opus-4-8", "claude-opus-4.8"),
                e("claude", "claude-sonnet-5", "claude-sonnet-5"),
                e("claude", "claude-haiku-4-5", "claude-haiku-4.5"),
                e("grok", "grok-4.5", "grok-4.5"),
                e("grok", "grok-composer-2.5-fast", "grok-composer-2.5-fast"),
            ],
            enabled_providers: vec!["claude".into(), "grok".into()],
            default_models: DefaultModels::default(),
            active_provider: active.into(),
        }
    }

    fn make_task(status: TaskStatus) -> TaskInstance {
        TaskInstance {
            id: "01TUI000000000000000000001".into(),
            status,
            definition: None,
            item: None,
            item_key: None,
            target: TaskTarget {
                repo: "platform".into(),
                git_ref: "temp".into(),
                worktree: Some("wt-a".into()),
            },
            priority: "normal".into(),
            created: "2026-07-08T10:00:00.000Z".into(),
            started_at: None,
            finished_at: None,
            source: "tui".into(),
            ephemeral_worktree: false,
            error: None,
            session: "fresh".into(),
            resume_session_id: None,
            model: None,
            prompt: "fix the flaky test\nmore context\n".into(),
            verify: None,
            verified: None,
            verify_exit_code: None,
            verify_output: None,
            lane: None,
            not_before: None,
        }
    }

    fn task_on(status: TaskStatus, id: &str, repo: &str, worktree: Option<&str>) -> TaskInstance {
        let mut t = make_task(status);
        t.id = id.into();
        t.target.repo = repo.into();
        t.target.worktree = worktree.map(str::to_string);
        t
    }

    fn make_session(cwd: &str, kind: &str) -> SessionEntry {
        SessionEntry {
            kind: kind.into(),
            key: format!("sess-{cwd}"),
            lane: None,
            cwd: Some(cwd.into()),
            pid: Some(4242),
            started_at: "2026-07-08T09:00:00.000Z".into(),
            heartbeat_at: "2026-07-08T10:00:00.000Z".into(),
        }
    }

    fn wt(name: &str, path: &str, branch: &str) -> WorktreeInfo {
        WorktreeInfo {
            name: name.into(),
            path: path.into(),
            branch: branch.into(),
            ..Default::default()
        }
    }

    fn platform_worktrees() -> HashMap<String, Vec<WorktreeInfo>> {
        HashMap::from([(
            "platform".to_string(),
            vec![
                wt("wt-a", "/wt/wt-a", "feat/a"),
                wt("wt-b", "/wt/wt-b", "feat/b"),
                wt("wt-c", "/wt/wt-c", "feat/c"),
            ],
        )])
    }

    fn snap(tasks: Vec<TaskInstance>, archived: Vec<TaskInstance>) -> StateSnapshot {
        StateSnapshot { tasks, archived_recent: archived, ..Default::default() }
    }

    fn projects(names: &[&str]) -> Vec<Project> {
        names.iter().map(|n| Project { name: n.to_string(), github_id: None }).collect()
    }

    /// NOW from the TS suites: Date.parse("2026-07-08T10:03:12.000Z")
    fn now() -> u64 {
        parse_iso_epoch_s("2026-07-08T10:03:12.000Z")
    }

    // ---- parse_iso_epoch_s ----

    #[test]
    fn parse_iso_epoch_anchors_and_deltas() {
        assert_eq!(parse_iso_epoch_s("1970-01-01T00:00:00.000Z"), 0);
        assert_eq!(parse_iso_epoch_s("1970-01-02T00:00:00.000Z"), 86_400);
        let a = parse_iso_epoch_s("2026-07-08T10:00:00.000Z");
        let b = parse_iso_epoch_s("2026-07-08T10:03:12.000Z");
        assert_eq!(b - a, 192);
        // leap-year sanity: 2024-02-29 is exactly one day before 2024-03-01
        assert_eq!(
            parse_iso_epoch_s("2024-03-01T00:00:00.000Z")
                - parse_iso_epoch_s("2024-02-29T00:00:00.000Z"),
            86_400
        );
    }

    // ---- build_tabs (mirrors buildProjectTabs) ----

    #[test]
    fn tabs_list_config_projects_in_order_without_synthetic() {
        let mut s = snap(vec![task_on(TaskStatus::Running, "t1", "platform", Some("wt-a"))], vec![]);
        s.projects = projects(&["platform", "web"]);
        assert_eq!(
            build_tabs(&s),
            vec![
                TabInfo { name: "platform".into(), synthetic: false },
                TabInfo { name: "web".into(), synthetic: false },
            ]
        );
    }

    #[test]
    fn tabs_append_synthetic_orphan_repos_sorted_alphabetically() {
        let mut s = snap(
            vec![
                task_on(TaskStatus::Running, "t1", "zeta", Some("wt-a")),
                task_on(TaskStatus::Queued, "t2", "platform", Some("wt-a")),
            ],
            vec![task_on(TaskStatus::Done, "t3", "alpha", Some("wt-a"))],
        );
        s.projects = projects(&["platform"]);
        assert_eq!(
            build_tabs(&s),
            vec![
                TabInfo { name: "platform".into(), synthetic: false },
                TabInfo { name: "alpha".into(), synthetic: true },
                TabInfo { name: "zeta".into(), synthetic: true },
            ]
        );
    }

    #[test]
    fn tabs_keep_config_projects_with_no_tasks() {
        let mut s = snap(vec![], vec![]);
        s.projects = projects(&["platform"]);
        assert_eq!(build_tabs(&s), vec![TabInfo { name: "platform".into(), synthetic: false }]);
    }

    // ---- queue_rows (mirrors queueRowsForProject + buildQueueRows) ----

    #[test]
    fn queue_rows_exclude_other_projects_live_and_archived() {
        let s = snap(
            vec![
                task_on(TaskStatus::Running, "01TASKAAA000000000000000000", "platform", Some("wt-a")),
                task_on(TaskStatus::Running, "01TASKBBB000000000000000000", "web", Some("wt-b")),
            ],
            vec![
                task_on(TaskStatus::Done, "01TASKCCC000000000000000000", "platform", Some("wt-a")),
                task_on(TaskStatus::Done, "01TASKDDD000000000000000000", "web", Some("wt-b")),
            ],
        );
        let rows = queue_rows(&s, "platform");
        assert_eq!(
            rows.iter().map(|r| r.task_id.as_str()).collect::<Vec<_>>(),
            vec!["01TASKAAA000000000000000000", "01TASKCCC000000000000000000"]
        );
        assert_eq!(rows.iter().map(|r| r.archived).collect::<Vec<_>>(), vec![false, true]);
    }

    #[test]
    fn running_count_for_is_scoped_to_the_project() {
        // Three running tasks spread across three projects: each project has
        // exactly one running task, so none is near a per-project cap of e.g. 10.
        // The tabbar must not read this as a saturated cap (the old "3/3" bug).
        let s = StateSnapshot {
            tasks: vec![
                task_on(TaskStatus::Running, "r-plat", "platform", Some("wt-a")),
                task_on(TaskStatus::Running, "r-web", "web", Some("wt-b")),
                task_on(TaskStatus::Running, "r-docs", "docs", Some("wt-c")),
                // Queued on platform → not running → must not count.
                task_on(TaskStatus::Queued, "q-plat", "platform", Some("wt-a")),
            ],
            running: vec!["r-plat".into(), "r-web".into(), "r-docs".into()],
            ..Default::default()
        };
        assert_eq!(running_count_for(&s, "platform"), 1);
        assert_eq!(running_count_for(&s, "web"), 1);
        assert_eq!(running_count_for(&s, "docs"), 1);
        assert_eq!(running_count_for(&s, "absent"), 0);
        // Per-project counts partition the global running total exactly.
        let total: usize =
            ["platform", "web", "docs"].iter().map(|p| running_count_for(&s, p)).sum();
        assert_eq!(total, s.running.len());
    }

    #[test]
    fn active_count_for_counts_queued_plus_running_per_project() {
        // The tabbar chip suffix `(n)`: scheduled (queued) + running tasks for
        // that project. Terminal and needs-input rows never count.
        let s = StateSnapshot {
            tasks: vec![
                task_on(TaskStatus::Running, "r-plat", "platform", Some("wt-a")),
                task_on(TaskStatus::Queued, "q-plat", "platform", Some("wt-a")),
                task_on(TaskStatus::Queued, "q2-plat", "platform", None),
                task_on(TaskStatus::NeedsInput, "n-plat", "platform", None),
                task_on(TaskStatus::Done, "d-plat", "platform", Some("wt-a")),
                task_on(TaskStatus::Queued, "q-web", "web", None),
            ],
            running: vec!["r-plat".into()],
            ..Default::default()
        };
        assert_eq!(active_count_for(&s, "platform"), 3);
        assert_eq!(active_count_for(&s, "web"), 1);
        assert_eq!(active_count_for(&s, "absent"), 0);
    }

    #[test]
    fn queue_rows_detail_running_elapsed_queued_position_failed_error() {
        let running = task_on(TaskStatus::Running, "t1", "platform", Some("wt-a"));
        let q1 = task_on(TaskStatus::Queued, "t2", "platform", Some("wt-a"));
        let q2 = task_on(TaskStatus::Queued, "t3", "platform", Some("wt-a"));
        let mut failed = task_on(TaskStatus::Failed, "t4", "platform", Some("wt-a"));
        failed.error = Some("tree left dirty".into());
        let done = task_on(TaskStatus::Done, "t5", "platform", Some("wt-a"));
        let rows = queue_rows(&snap(vec![running, q1, q2, failed, done], vec![]), "platform");
        // ACTIVE section (running → queued) then FINISHED section (done/failed,
        // ordered by completion; here both lack finishedAt so id-desc wins →
        // t5(done) before t4(failed)). The `#N in lane` position is still computed
        // in creation order, so q1/q2 keep #1/#2 regardless of the display sort.
        // Running stores a start epoch (pane formats the timer at paint); Queued
        // bakes `#N in lane`; Failed/Done are empty.
        assert_eq!(rows[0].detail, ""); // running: timer not baked
        assert_eq!(
            rows[0].running_elapsed,
            Some(parse_iso_epoch_s("2026-07-08T10:00:00.000Z"))
        );
        assert_eq!(
            elapsed_label(rows[0].running_elapsed.unwrap(), now()),
            "⏱ 3m12s"
        );
        assert_eq!(rows[1].detail, "#1 in lane"); // q1
        assert_eq!(rows[1].running_elapsed, None);
        assert_eq!(rows[2].detail, "#2 in lane"); // q2
        assert_eq!(rows[3].detail, ""); // done: no trailing "done"
        assert_eq!(rows[4].detail, ""); // failed: no trailing error word
        assert_eq!(rows[0].worktree, "wt-a");
        assert_eq!(rows[0].created_epoch_s, parse_iso_epoch_s("2026-07-08T10:00:00.000Z"));
        assert!(rows[0].running && !rows[1].running);
        assert_eq!(
            rows.iter().map(|r| r.glyph).collect::<Vec<_>>(),
            vec!['▶', '○', '○', '●', '✗']
        );
    }

    #[test]
    fn queue_rows_lane_override_shares_position_across_worktrees() {
        // Three self-review-e2e-style tasks: same definition lane override
        // (`testing1-stack`), different worktrees. Scheduler serializes them on
        // one key; the Live column must share one #N counter (not #1 per worktree).
        let mut running = task_on(TaskStatus::Running, "t1", "platform", Some("JUS-1927"));
        running.lane = Some("testing1-stack".into());
        let mut q1 = task_on(TaskStatus::Queued, "t2", "platform", Some("qoo-small"));
        q1.lane = Some("testing1-stack".into());
        let mut q2 = task_on(TaskStatus::Queued, "t3", "platform", Some("SEC-37"));
        q2.lane = Some("testing1-stack".into());
        // Control: plain autofix-style task on yet another worktree — own lane.
        let plain = task_on(TaskStatus::Queued, "t4", "platform", Some("other-wt"));

        let rows = queue_rows(&snap(vec![running, q1, q2, plain], vec![]), "platform");
        let by_id = |id: &str| {
            rows.iter()
                .find(|r| r.task_id == id)
                .map(|r| r.detail.as_str())
                .unwrap_or("")
        };
        assert_eq!(by_id("t1"), ""); // running: timer, not #N
        assert_eq!(by_id("t2"), "#1 in lane");
        assert_eq!(by_id("t3"), "#2 in lane");
        assert_eq!(by_id("t4"), "#1 in lane"); // different scheduler key
    }

    #[test]
    fn scheduler_lane_key_prefers_override_over_worktree() {
        let mut t = task_on(TaskStatus::Queued, "t", "platform", Some("JUS-1"));
        assert_eq!(scheduler_lane_key(&t), "platform:JUS-1");
        t.lane = Some("testing1-stack".into());
        assert_eq!(scheduler_lane_key(&t), "platform:testing1-stack");
    }

    #[test]
    fn queue_rows_sub_classifies_failed_by_error_reason() {
        // `worker.ts` stamps the exact strings "timed out" / "session limit" /
        // "out of budget" / "provider unavailable" into `task.error` for those
        // failure modes; any other reason (or none) stays the generic `✗`. All
        // special cases are still red (see `theme::glyph_style`) — only the glyph
        // differs, so they read apart at a glance without a color that could be
        // confused for a different severity.
        let mut timed_out = task_on(TaskStatus::Failed, "t1", "platform", Some("wt-a"));
        timed_out.error = Some("timed out".into());
        let mut session_limit = task_on(TaskStatus::Failed, "t2", "platform", Some("wt-a"));
        session_limit.error = Some("session limit".into());
        let mut generic = task_on(TaskStatus::Failed, "t3", "platform", Some("wt-a"));
        generic.error = Some("exit code 1".into());
        let mut no_reason = task_on(TaskStatus::Failed, "t4", "platform", Some("wt-a"));
        no_reason.error = None;
        let mut out_of_budget = task_on(TaskStatus::Failed, "t5", "platform", Some("wt-a"));
        out_of_budget.error = Some("out of budget".into());
        let mut provider_unavailable = task_on(TaskStatus::Failed, "t6", "platform", Some("wt-a"));
        provider_unavailable.error = Some("provider unavailable".into());
        let rows = queue_rows(
            &snap(
                vec![timed_out, session_limit, generic, no_reason, out_of_budget, provider_unavailable],
                vec![],
            ),
            "platform"
        );
        let glyph_for = |id: &str| rows.iter().find(|r| r.task_id == id).unwrap().glyph;
        assert_eq!(glyph_for("t1"), '⧗');
        assert_eq!(glyph_for("t2"), '$'); // session-limit shares the $ limit glyph
        assert_eq!(glyph_for("t3"), '✗');
        assert_eq!(glyph_for("t4"), '✗');
        assert_eq!(glyph_for("t5"), '$');
        assert_eq!(glyph_for("t6"), '⊟');
    }

    #[test]
    fn queue_rows_needs_input_has_no_detail_text() {
        let ni = task_on(TaskStatus::NeedsInput, "t1", "platform", Some("wt-a"));
        let rows = queue_rows(&snap(vec![ni], vec![]), "platform");
        assert_eq!(rows[0].detail, ""); // the ‼ glyph carries the state
        assert_eq!(rows[0].glyph, '‼');
    }

    #[test]
    fn queue_rows_use_ref_as_lane_when_worktree_unresolved_and_append_archived() {
        let mut pending = task_on(TaskStatus::Queued, "t1", "platform", None);
        pending.target.git_ref = "pr:257".into();
        let old = task_on(TaskStatus::Done, "t0", "platform", Some("wt-a"));
        let rows = queue_rows(&snap(vec![pending], vec![old]), "platform");
        assert_eq!(rows[0].worktree, "pr:257");
        assert!(rows[1].archived);
        assert_eq!(rows[1].detail, ""); // archived rows carry no detail text
    }

    #[test]
    fn queue_rows_shows_all_archived_for_project() {
        // Full archive history for the project tab — no last-N tail.
        let archived: Vec<TaskInstance> = (0..15)
            .map(|i| task_on(TaskStatus::Done, &format!("t{i:02}"), "platform", Some("wt-a")))
            .collect();
        let rows = queue_rows(&snap(vec![], archived), "platform");
        assert_eq!(rows.len(), 15);
        // With no finishedAt the FINISHED section falls back to id-desc, so
        // the newest (t14) leads and the oldest (t00) is last.
        assert_eq!(rows[0].task_id, "t14");
        assert_eq!(rows[14].task_id, "t00");
        assert!(rows.iter().all(|r| r.archived));
    }

    #[test]
    fn queue_rows_hide_archived_whose_worktree_was_deleted() {
        // The repo HAS a worktree listing; wt-a exists, wt-gone doesn't. The
        // archived row on the deleted worktree is hidden outright (not dimmed);
        // the one whose worktree survives keeps the dimmed display, and the
        // `@repo` sentinel (nothing to delete) is untouched.
        let mut s = snap(
            vec![],
            vec![
                task_on(TaskStatus::Done, "t1", "platform", Some("wt-a")),
                task_on(TaskStatus::Failed, "t2", "platform", Some("wt-gone")),
                task_on(TaskStatus::Done, "t3", "platform", Some(REPO_SENTINEL)),
            ],
        );
        s.worktrees = platform_worktrees();
        let rows = queue_rows(&s, "platform");
        let ids: Vec<&str> = rows.iter().map(|r| r.task_id.as_str()).collect();
        assert!(!ids.contains(&"t2"), "deleted-worktree archived row is hidden: {ids:?}");
        assert!(ids.contains(&"t1") && ids.contains(&"t3"), "survivors keep the dimmed display");
    }

    #[test]
    fn queue_rows_keep_archived_when_repo_has_no_worktree_listing() {
        // No `worktrees` entry for the repo (old daemon / cache not populated
        // yet) → hide nothing, mirroring the daemon sweep's cold-cache guard.
        let rows = queue_rows(
            &snap(vec![], vec![task_on(TaskStatus::Done, "t1", "platform", Some("wt-gone"))]),
            "platform",
        );
        assert_eq!(rows.len(), 1);
        assert!(rows[0].archived);
    }

    #[test]
    fn queue_rows_deleted_worktree_filter_never_touches_live_tasks() {
        // A LIVE task on a missing worktree stays visible — archiving on
        // deletion is the daemon's call (engine sweep), never a display trick;
        // a running task on a vanished worktree is a bug to surface.
        let mut s = snap(
            vec![task_on(TaskStatus::Running, "t1", "platform", Some("wt-gone"))],
            vec![],
        );
        s.worktrees = platform_worktrees();
        let rows = queue_rows(&s, "platform");
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].archived);
    }

    #[test]
    fn queue_rows_hidden_deleted_worktree_archived_do_not_block_survivors() {
        // 5 archived on a deleted worktree + 12 on a live one: deleted-lane
        // noise is filtered out, and every survivor still shows (no display cap).
        let mut archived: Vec<TaskInstance> = (0..5)
            .map(|i| task_on(TaskStatus::Done, &format!("gone{i:02}"), "platform", Some("wt-gone")))
            .collect();
        archived.extend(
            (0..12).map(|i| task_on(TaskStatus::Done, &format!("kept{i:02}"), "platform", Some("wt-a"))),
        );
        let mut s = snap(vec![], archived);
        s.worktrees = platform_worktrees();
        let rows = queue_rows(&s, "platform");
        assert_eq!(rows.len(), 12);
        assert!(rows.iter().all(|r| r.task_id.starts_with("kept")), "only survivors shown");
    }

    #[test]
    fn queue_rows_strip_repo_prefix_in_lane() {
        let running = task_on(
            TaskStatus::Running,
            "t1",
            "platform",
            Some("platform.dedup-dependabot-run"),
        );
        let rows = queue_rows(&snap(vec![running], vec![]), "platform");
        assert_eq!(rows[0].worktree, "dedup-dependabot-run");
    }

    #[test]
    fn queue_rows_display_repo_sentinel_as_repo_name() {
        let live = task_on(TaskStatus::Running, "t1", "platform", Some(REPO_SENTINEL));
        let archived = task_on(TaskStatus::Done, "t2", "platform", Some(REPO_SENTINEL));
        let rows = queue_rows(&snap(vec![live], vec![archived]), "platform");
        assert_eq!(rows[0].worktree, "platform");
        assert_eq!(rows[1].worktree, "platform");
    }

    #[test]
    fn queue_rows_carry_def_name_and_created_epoch() {
        let mut t = task_on(TaskStatus::Running, "t1", "platform", Some("wt-a"));
        t.definition = Some("squash-merge".into());
        t.created = "2026-07-08T10:00:00.000Z".into();
        let adhoc = task_on(TaskStatus::Queued, "t2", "platform", Some("wt-a"));
        let rows = queue_rows(&snap(vec![t, adhoc], vec![]), "platform");
        assert_eq!(rows[0].def_name, Some("squash-merge".into()));
        assert_eq!(rows[1].def_name, None);
        assert_eq!(rows[0].created_epoch_s, parse_iso_epoch_s("2026-07-08T10:00:00.000Z"));
    }

    #[test]
    fn queue_rows_strip_repo_qualifier_from_def_name() {
        // The daemon qualifies project-scoped defs as `repo/name`; the scope is
        // meaningless in the queue display.
        let mut t = task_on(TaskStatus::Running, "t1", "platform", Some("wt-a"));
        t.definition = Some("platform/pr-ready".into());
        let rows = queue_rows(&snap(vec![t], vec![]), "platform");
        assert_eq!(rows[0].def_name, Some("pr-ready".into()));
        assert_eq!(def_display_name("pr-ready"), "pr-ready");
        assert_eq!(def_display_name("platform/pr-ready"), "pr-ready");
    }

    #[test]
    fn queue_rows_not_before_only_for_queued_or_running() {
        // Queued deferred → Live can paint the schedule stamp.
        let mut queued = task_on(TaskStatus::Queued, "01Q", "platform", Some("wt-a"));
        queued.not_before = Some("2099-01-01T00:00:00.000Z".into());
        // Running mid-defer-stop may also carry a stamp (paint prefers the timer).
        let mut running = task_on(TaskStatus::Running, "01R", "platform", Some("wt-a"));
        running.not_before = Some("2099-01-01T00:00:00.000Z".into());
        // Cancelled with a STALE stamp must never expose not_before_epoch_s —
        // defense in depth when the daemon left not_before on disk.
        let mut cancelled = task_on(TaskStatus::Cancelled, "01C", "platform", Some("wt-a"));
        cancelled.not_before = Some("2099-01-01T00:00:00.000Z".into());
        cancelled.finished_at = Some("2026-07-08T10:05:00.000Z".into());
        let mut failed = task_on(TaskStatus::Failed, "01F", "platform", Some("wt-a"));
        failed.not_before = Some("2099-01-01T00:00:00.000Z".into());
        failed.finished_at = Some("2026-07-08T10:05:00.000Z".into());

        let rows = queue_rows(
            &snap(vec![queued, running, cancelled, failed], vec![]),
            "platform",
        );
        let by_id: HashMap<_, _> =
            rows.into_iter().map(|r| (r.task_id.clone(), r)).collect();
        let expected = parse_iso_epoch_s("2099-01-01T00:00:00.000Z");
        assert_eq!(by_id["01Q"].not_before_epoch_s, Some(expected));
        assert_eq!(by_id["01R"].not_before_epoch_s, Some(expected));
        assert_eq!(by_id["01C"].not_before_epoch_s, None);
        assert_eq!(by_id["01F"].not_before_epoch_s, None);
    }

    // ---- queue section ordering + divider ----

    /// task_on with a priority + optional finishedAt override.
    fn qtask(
        status: TaskStatus,
        id: &str,
        priority: &str,
        finished: Option<&str>,
    ) -> TaskInstance {
        let mut t = task_on(status, id, "platform", Some("wt-a"));
        t.priority = priority.into();
        t.finished_at = finished.map(str::to_string);
        t
    }

    #[test]
    fn queue_active_section_orders_running_ready_lane_then_deferred() {
        // ACTIVE bands: running (newest start first) → needs-input → ready
        // `#N in lane` (group by lane ASC, # ASC) → deferred (notBefore ASC).
        // Priority is no longer a sort key. Deferred sit on their own lane so
        // they do not steal #N from the ready-lane assertions.
        let mut run_old = qtask(TaskStatus::Running, "01RUN_OLD", "high", None);
        run_old.started_at = Some("2026-07-09T10:00:00.000Z".into());
        let mut run_new = qtask(TaskStatus::Running, "01RUN_NEW", "low", None);
        run_new.started_at = Some("2026-07-09T12:00:00.000Z".into());
        // Snapshot order of ready queued sets #N: A1 then A2 on wt-a; B alone.
        // Display groups lane key ASC → platform:wt-a before platform:wt-b.
        let ready_a1 = task_on(TaskStatus::Queued, "01READY_A1", "platform", Some("wt-a"));
        let ready_a2 = task_on(TaskStatus::Queued, "01READY_A2", "platform", Some("wt-a"));
        let ready_b = task_on(TaskStatus::Queued, "01READY_B", "platform", Some("wt-b"));
        let mut def_late = task_on(TaskStatus::Queued, "01DEF_LATE", "platform", Some("wt-d"));
        def_late.not_before = Some("2099-06-01T00:00:00.000Z".into());
        def_late.priority = "high".into();
        let mut def_early = task_on(TaskStatus::Queued, "01DEF_EARLY", "platform", Some("wt-d"));
        def_early.not_before = Some("2099-01-01T00:00:00.000Z".into());
        def_early.priority = "low".into();

        let rows = queue_rows(
            &snap(
                vec![
                    ready_b, // scrambled input order
                    def_late,
                    run_old,
                    ready_a2,
                    qtask(TaskStatus::NeedsInput, "01NEEDS", "normal", None),
                    def_early,
                    ready_a1,
                    run_new,
                ],
                vec![],
            ),
            "platform",
        );
        // Ready #N follows snapshot order among Queued: first ready_b in the
        // vec is not first overall — positions are assigned in iteration order
        // of ALL queued (ready+deferred). Walk: ready_b → #1 wt-b; def_late →
        // #1 wt-d; ready_a2 → #1 wt-a; def_early → #2 wt-d; ready_a1 → #2 wt-a.
        // So ready display by (lane, #): wt-a #1=A2, #2=A1; wt-b #1=B.
        assert_eq!(
            rows.iter().map(|r| r.task_id.as_str()).collect::<Vec<_>>(),
            vec![
                "01RUN_NEW",   // running, newer start
                "01RUN_OLD",   // running, older start
                "01NEEDS",     // needs-input
                "01READY_A2",  // lane wt-a #1 (seen before A1 in snapshot)
                "01READY_A1",  // lane wt-a #2
                "01READY_B",   // lane wt-b #1
                "01DEF_EARLY", // deferred, earlier wake
                "01DEF_LATE",  // deferred, later wake
            ]
        );
        assert_eq!(queue_divider_after(&rows), None);
    }

    #[test]
    fn queue_finished_section_orders_by_completion_desc_then_id_fallback() {
        // All finished, no active. Rows WITH finishedAt sort newest-completion
        // first; the row WITHOUT it falls back to id and sinks below them.
        let rows = queue_rows(
            &snap(
                vec![
                    qtask(TaskStatus::Done, "01D_1000", "normal", Some("2026-07-09T10:00:00.000Z")),
                    qtask(TaskStatus::Failed, "01F_1200", "normal", Some("2026-07-09T12:00:00.000Z")),
                    qtask(TaskStatus::Done, "01D_NONE", "normal", None),
                    qtask(TaskStatus::Failed, "01F_1100", "normal", Some("2026-07-09T11:00:00.000Z")),
                ],
                vec![],
            ),
            "platform",
        );
        assert_eq!(
            rows.iter().map(|r| r.task_id.as_str()).collect::<Vec<_>>(),
            vec!["01F_1200", "01F_1100", "01D_1000", "01D_NONE"]
        );
        // Every row is finished → still no divider (needs both sections).
        assert_eq!(queue_divider_after(&rows), None);
    }

    #[test]
    fn queue_divider_sits_after_the_last_active_row() {
        // Two active + two finished → the divider is drawn after real index 1
        // (the last active row), i.e. immediately before the first finished row.
        let rows = queue_rows(
            &snap(
                vec![
                    qtask(TaskStatus::Failed, "01FAIL", "normal", Some("2026-07-09T11:00:00.000Z")),
                    qtask(TaskStatus::Running, "01RUN", "normal", None),
                    qtask(TaskStatus::Queued, "01QUE", "normal", None),
                    qtask(TaskStatus::Done, "01DONE", "normal", Some("2026-07-09T10:00:00.000Z")),
                ],
                vec![],
            ),
            "platform",
        );
        // Sorted: [running, queued | failed, done].
        assert_eq!(
            rows.iter().map(|r| r.task_id.as_str()).collect::<Vec<_>>(),
            vec!["01RUN", "01QUE", "01FAIL", "01DONE"]
        );
        assert!(!queue_row_finished(&rows[1]) && queue_row_finished(&rows[2]));
        assert_eq!(queue_divider_after(&rows), Some(1));
    }

    #[test]
    fn queue_cancelled_and_skipped_join_the_finished_section() {
        // Regression: cancelled/skipped are TERMINAL — they belong BELOW the
        // divider, not in the ACTIVE section (the bug: they fell through
        // `queue_row_finished` to `false`). With one active row present, the
        // divider sits after it and both terminal rows sort into FINISHED by
        // completion desc (cancelled 12:00 before skipped 11:00).
        let rows = queue_rows(
            &snap(
                vec![
                    qtask(TaskStatus::Running, "01RUN", "normal", None),
                    qtask(TaskStatus::Cancelled, "01CAN", "normal", Some("2026-07-09T12:00:00.000Z")),
                    qtask(TaskStatus::Skipped, "01SKP", "normal", Some("2026-07-09T11:00:00.000Z")),
                ],
                vec![],
            ),
            "platform",
        );
        assert_eq!(
            rows.iter().map(|r| r.task_id.as_str()).collect::<Vec<_>>(),
            vec!["01RUN", "01CAN", "01SKP"]
        );
        assert!(!queue_row_finished(&rows[0]), "running stays active");
        assert!(queue_row_finished(&rows[1]), "cancelled is finished");
        assert!(queue_row_finished(&rows[2]), "skipped is finished");
        assert_eq!(queue_divider_after(&rows), Some(0));
    }

    #[test]
    fn queue_divider_none_for_single_section_or_empty() {
        // Empty list, all-active, and all-finished each yield no divider.
        assert_eq!(queue_divider_after(&[]), None);
        let active = queue_rows(
            &snap(vec![qtask(TaskStatus::Running, "01R", "normal", None)], vec![]),
            "platform",
        );
        assert_eq!(queue_divider_after(&active), None);
        let finished = queue_rows(
            &snap(vec![qtask(TaskStatus::Done, "01D", "normal", None)], vec![]),
            "platform",
        );
        assert_eq!(queue_divider_after(&finished), None);
    }

    #[test]
    fn queue_archived_rows_sink_below_live_finished_rows() {
        // A live failed task (finished 12:00) and an archived done task (finished
        // 13:00) both land in the FINISHED section, but archived rows sink to
        // the BOTTOM (dismissed clutter never interleaves with finished tasks
        // that still want a look) — even though the archived row finished later.
        let rows = queue_rows(
            &snap(
                vec![
                    qtask(TaskStatus::Running, "01RUN", "normal", None),
                    qtask(TaskStatus::Failed, "01FAIL", "normal", Some("2026-07-09T12:00:00.000Z")),
                ],
                vec![qtask(TaskStatus::Done, "01ARCH", "normal", Some("2026-07-09T13:00:00.000Z"))],
            ),
            "platform",
        );
        assert_eq!(
            rows.iter().map(|r| r.task_id.as_str()).collect::<Vec<_>>(),
            vec!["01RUN", "01FAIL", "01ARCH"]
        );
        assert!(!rows[1].archived && rows[2].archived);
        assert_eq!(queue_divider_after(&rows), Some(0)); // after the single active row
    }

    #[test]
    fn queue_archived_tail_keeps_completion_order_within_itself() {
        // Within the archived tail the existing completion-desc order holds.
        let rows = queue_rows(
            &snap(
                vec![],
                vec![
                    qtask(TaskStatus::Done, "01OLD", "normal", Some("2026-07-09T11:00:00.000Z")),
                    qtask(TaskStatus::Done, "01NEW", "normal", Some("2026-07-09T13:00:00.000Z")),
                ],
            ),
            "platform",
        );
        assert_eq!(
            rows.iter().map(|r| r.task_id.as_str()).collect::<Vec<_>>(),
            vec!["01NEW", "01OLD"]
        );
    }

    // ---- lane_task_live / lane_task_cols (worktree detail lane list) ----

    #[test]
    fn lane_task_live_running_queued_and_terminal() {
        // Running → elapsed against `now` (created default is 3m12s before now).
        let running = task_on(TaskStatus::Running, "t1", "platform", Some("wt-a"));
        assert_eq!(lane_task_live(&running, now(), 0, 0), "⏱ 3m12s");
        // A re-run stamps `started_at` LATER than `created`: the timer anchors on
        // the re-run (47s ago), not the original creation — so it never inherits
        // the phantom elapsed that would race it to the 3h ceiling.
        let mut rerun = running.clone();
        rerun.started_at = Some("2026-07-08T10:02:25.000Z".into()); // now - 47s
        assert_eq!(lane_task_live(&rerun, now(), 0, 0), "⏱ 47s");
        // Queued → `#N in lane` using the caller-supplied 1-based position (the
        // elapsed clock is never consulted for a queued row).
        let queued = task_on(TaskStatus::Queued, "t2", "platform", Some("wt-a"));
        assert_eq!(lane_task_live(&queued, now(), 1, 0), "#1 in lane");
        assert_eq!(lane_task_live(&queued, now(), 3, 0), "#3 in lane");
        // Every terminal / non-live status → empty (the glyph carries the state).
        for status in
            [TaskStatus::Done, TaskStatus::Failed, TaskStatus::Cancelled, TaskStatus::NeedsInput]
        {
            let t = task_on(status, "t3", "platform", Some("wt-a"));
            assert_eq!(lane_task_live(&t, now(), 0, 0), "", "{status:?} has no live text");
        }
    }

    #[test]
    fn lane_task_cols_full_width_and_narrow_degradation() {
        // Wide: all trailing columns kept at their fixed widths, name is the flex
        // remainder = width − 2 (glyph) − 3×(COL_GAP + col).
        let wide = lane_task_cols(60);
        assert_eq!(wide.created_w, TIMESTAMP_W);
        assert_eq!(wide.age_w, AGE_W);
        assert_eq!(wide.live_w, QUEUE_LIVE_W);
        let trailing = 3 * COL_GAP + TIMESTAMP_W + AGE_W + QUEUE_LIVE_W;
        assert_eq!(wide.name_w, 60 - 2 - trailing);
        // Degradation order is Live first, then Created, then Age — each drops only
        // once the name floor no longer fits.
        let drop_live = lane_task_cols(2 + LANE_NAME_MIN + (COL_GAP + TIMESTAMP_W) + (COL_GAP + AGE_W));
        assert_eq!(drop_live.live_w, 0);
        assert_eq!(drop_live.created_w, TIMESTAMP_W);
        assert_eq!(drop_live.age_w, AGE_W);
        assert!(drop_live.name_w >= LANE_NAME_MIN);
        // Very narrow: created also drops, age is the last trailing column kept.
        let narrow = lane_task_cols(2 + LANE_NAME_MIN + (COL_GAP + AGE_W));
        assert_eq!(narrow.live_w, 0);
        assert_eq!(narrow.created_w, 0);
        assert_eq!(narrow.age_w, AGE_W);
    }

    // ---- elapsed_label / prompt_summary / strip_repo_prefix / lane_key / arg_summary ----

    #[test]
    fn elapsed_label_formats_seconds_minutes_hours() {
        assert_eq!(elapsed_label(0, 47), "⏱ 47s");
        assert_eq!(elapsed_label(0, 192), "⏱ 3m12s");
        assert_eq!(elapsed_label(0, 303), "⏱ 5m03s"); // zero-padded seconds
        assert_eq!(elapsed_label(0, 3840), "⏱ 1h04m"); // zero-padded minutes
        assert_eq!(elapsed_label(100, 50), "⏱ 0s"); // clock skew clamps to 0
    }

    #[test]
    fn remaining_label_formats_countdown() {
        assert_eq!(remaining_label(47, 0), "⧗ 47s");
        assert_eq!(remaining_label(12 * 60, 0), "⧗ 12m");
        assert_eq!(remaining_label(4 * 3600 + 32 * 60, 0), "⧗ 4h32m");
        assert_eq!(remaining_label(5 * 3600, 0), "⧗ 5h00m");
        assert_eq!(remaining_label(50, 100), "⧗ 0s"); // past/equal clamps
    }

    #[test]
    fn lane_task_live_shows_countdown_when_deferred() {
        let mut t = make_task(TaskStatus::Queued);
        t.not_before = Some("1970-01-01T05:00:00.000Z".into()); // epoch 18000
        assert_eq!(lane_task_live(&t, 0, 1, 0), "⧗ 5h00m");
        // Past notBefore falls back to #N in lane.
        assert_eq!(lane_task_live(&t, 20_000, 2, 0), "#2 in lane");
    }

    #[test]
    fn lane_task_live_ignores_stale_not_before_on_cancelled() {
        // Status-gated: cancelled never paints a schedule stamp, even with a
        // future not_before left on the wire after a buggy cancel path.
        let mut t = make_task(TaskStatus::Cancelled);
        t.not_before = Some("2099-01-01T00:00:00.000Z".into());
        assert_eq!(lane_task_live(&t, 0, 0, 0), "");
    }

    #[test]
    fn absolute_local_label_offsets_and_day_boundary() {
        let noon = parse_iso_epoch_s("2026-07-09T12:00:00.000Z");
        assert_eq!(absolute_local_label(noon, 0), "07/09 12:00");
        assert_eq!(absolute_local_label(noon, 3600), "07/09 13:00");
        // +1h pushes a 23:30Z time into the next local day.
        let late = parse_iso_epoch_s("2026-07-09T23:30:00.000Z");
        assert_eq!(absolute_local_label(late, 3600), "07/10 00:30");
    }

    #[test]
    fn relative_age_label_buckets_and_skew() {
        assert_eq!(relative_age_label(100, 100), "just now");
        assert_eq!(relative_age_label(100, 159), "just now"); // <60s
        assert_eq!(relative_age_label(0, 300), "5m ago");
        assert_eq!(relative_age_label(0, 3600), "1h ago");
        assert_eq!(relative_age_label(0, 172_800), "2d ago");
        assert_eq!(relative_age_label(200, 100), "just now"); // created > now
    }

    #[test]
    fn prompt_summary_first_non_blank_line_clipped_at_240() {
        assert_eq!(prompt_summary("\n\nfix the thing\nrest"), "fix the thing");
        assert_eq!(prompt_summary(""), "");
        let long = "a".repeat(250);
        let expected = format!("{}…", "a".repeat(239));
        assert_eq!(prompt_summary(&long), expected);
        assert_eq!(prompt_summary(&"a".repeat(240)), "a".repeat(240)); // exactly 240 fits
    }

    #[test]
    fn strip_repo_prefix_cases() {
        assert_eq!(strip_repo_prefix("platform.dedup-dependabot-run", "platform"), "dedup-dependabot-run");
        assert_eq!(strip_repo_prefix("platform", "platform"), "platform"); // bare repo kept
        assert_eq!(strip_repo_prefix("wt-a", "platform"), "wt-a"); // unprefixed kept
        assert_eq!(strip_repo_prefix("@repo", "platform"), "platform"); // sentinel → repo name
    }

    #[test]
    fn lane_key_joins_repo_and_worktree() {
        assert_eq!(lane_key("platform", "wt-a"), "platform:wt-a");
    }

    #[test]
    fn arg_summary_names_and_defaults() {
        let args = vec![
            ArgSpec { name: "pr".into(), r#type: None, default: None, options: None, description: None },
            ArgSpec { name: "mode".into(), r#type: None, default: Some("ready".into()), options: None, description: None },
            ArgSpec { name: "review".into(), r#type: None, default: Some("auto".into()), options: None, description: None },
        ];
        assert_eq!(arg_summary(&args), "pr, mode=ready, review=auto");
        assert_eq!(arg_summary(&[]), "");
    }

    // ---- worktree_rows (mirrors buildWorktreeRows) ----

    #[test]
    fn worktree_busy_when_running_task_shares_lane() {
        let mut s = snap(vec![task_on(TaskStatus::Running, "t1", "platform", Some("wt-a"))], vec![]);
        s.worktrees = platform_worktrees();
        let rows = worktree_rows(&s, "platform");
        assert_eq!(rows.iter().find(|r| r.name == "wt-a").unwrap().state, WtState::Busy);
    }

    #[test]
    fn worktree_failed_when_newest_lane_task_failed() {
        let mut s = snap(
            vec![
                task_on(TaskStatus::Done, "01TASKB00000000000000000001", "platform", Some("wt-b")),
                task_on(TaskStatus::Failed, "01TASKB00000000000000000002", "platform", Some("wt-b")),
            ],
            vec![],
        );
        s.worktrees = platform_worktrees();
        let rows = worktree_rows(&s, "platform");
        assert_eq!(rows.iter().find(|r| r.name == "wt-b").unwrap().state, WtState::Failed);
    }

    #[test]
    fn worktree_free_when_newest_lane_task_not_failed() {
        let mut s = snap(
            vec![
                task_on(TaskStatus::Failed, "01TASKB00000000000000000001", "platform", Some("wt-c")),
                task_on(TaskStatus::Done, "01TASKB00000000000000000002", "platform", Some("wt-c")),
            ],
            vec![],
        );
        s.worktrees = platform_worktrees();
        let rows = worktree_rows(&s, "platform");
        assert_eq!(rows.iter().find(|r| r.name == "wt-c").unwrap().state, WtState::Free);
    }

    #[test]
    fn worktree_running_beats_newer_failed_task() {
        let mut s = snap(
            vec![
                task_on(TaskStatus::Running, "01TASKB00000000000000000001", "platform", Some("wt-a")),
                task_on(TaskStatus::Failed, "01TASKB00000000000000000009", "platform", Some("wt-a")),
            ],
            vec![],
        );
        s.worktrees = platform_worktrees();
        let rows = worktree_rows(&s, "platform");
        assert_eq!(rows.iter().find(|r| r.name == "wt-a").unwrap().state, WtState::Busy);
    }

    #[test]
    fn worktree_rows_emitted_in_order_with_full_fields() {
        let mut s = snap(vec![], vec![]);
        s.worktrees = platform_worktrees();
        let rows = worktree_rows(&s, "platform");
        assert_eq!(
            rows,
            vec![
                WorktreeRow {
                    name: "wt-a".into(), raw_name: "wt-a".into(), path: "/wt/wt-a".into(),
                    branch: "feat/a".into(), state: WtState::Free,
                    queued: 0, is_session: false, ..Default::default()
                },
                WorktreeRow {
                    name: "wt-b".into(), raw_name: "wt-b".into(), path: "/wt/wt-b".into(),
                    branch: "feat/b".into(), state: WtState::Free,
                    queued: 0, is_session: false, ..Default::default()
                },
                WorktreeRow {
                    name: "wt-c".into(), raw_name: "wt-c".into(), path: "/wt/wt-c".into(),
                    branch: "feat/c".into(), state: WtState::Free,
                    queued: 0, is_session: false, ..Default::default()
                },
            ]
        );
    }

    #[test]
    fn worktree_rows_order_by_activity_recency_tiers() {
        // Four lanes exercising the last-run tier: two running (newest START
        // first — running_elapsed is the running task's created epoch), one with a
        // finished task (its CREATION epoch), one with only a recent commit
        // (last-run 0 → commit fallback keeps it last). No github_id here, so the
        // mine tier is a no-op. Session rows are appended after and never enter
        // this ordering.
        let mut running_old = task_on(TaskStatus::Running, "01R_OLD", "platform", Some("wt-a"));
        running_old.created = "2026-07-09T12:00:00.000Z".into();
        let mut running_new = task_on(TaskStatus::Running, "01R_NEW", "platform", Some("wt-b"));
        running_new.created = "2026-07-09T12:05:00.000Z".into();
        // The new sort keys last-run off the finished task's CREATION epoch (the
        // `last` tuple), not `finished_at`; set created explicitly.
        let mut finished = task_on(TaskStatus::Done, "01DONE", "platform", Some("wt-c"));
        finished.created = "2026-07-09T11:00:00.000Z".into();

        let mut s = snap(vec![running_old, running_new, finished], vec![]);
        s.worktrees = HashMap::from([(
            "platform".to_string(),
            vec![
                wt("wt-a", "/wt/wt-a", "a"), // busy, older start
                wt("wt-b", "/wt/wt-b", "b"), // busy, newer start
                wt("wt-c", "/wt/wt-c", "c"), // recent finished task
                {
                    let mut w = wt("wt-d", "/wt/wt-d", "d"); // only a recent commit
                    w.last_commit_epoch = Some(1_752_000_000);
                    w
                },
            ],
        )]);
        s.sessions = vec![make_session("/wt/wt-a", "interactive")];
        let rows = worktree_rows(&s, "platform");
        assert_eq!(
            rows.iter().filter(|r| !r.is_session).map(|r| r.name.as_str()).collect::<Vec<_>>(),
            vec!["wt-b", "wt-a", "wt-c", "wt-d"]
        );
        // The session row is still last, after every real worktree.
        assert!(rows.last().unwrap().is_session);
    }

    /// A bare worktree row for the pure sort-key tests (only the fields the
    /// comparator reads matter; the rest default).
    fn wtrow(name: &str) -> WorktreeRow {
        WorktreeRow { name: name.into(), raw_name: name.into(), ..Default::default() }
    }

    #[test]
    fn worktree_is_mine_email_and_name_substring_case_insensitive() {
        // Match on EMAIL substring, case-insensitive (the GitHub noreply form).
        let mut by_email = wtrow("a");
        by_email.last_commit_author_email =
            Some("12345+NOOOTOWN@users.noreply.github.com".into());
        assert!(worktree_is_mine(&by_email, Some("noootown")));

        // Match on author NAME substring (email absent), case-insensitive.
        let mut by_name = wtrow("b");
        by_name.last_commit_author = Some("Ian Chiu".into());
        assert!(worktree_is_mine(&by_name, Some("ian")));
        assert!(worktree_is_mine(&by_name, Some("chiu")));

        // No substring match anywhere → not mine.
        assert!(!worktree_is_mine(&by_email, Some("someoneelse")));

        // No github_id, or an empty one, disables the tier entirely (no-op).
        assert!(!worktree_is_mine(&by_email, None));
        assert!(!worktree_is_mine(&by_email, Some("")));
    }

    #[test]
    fn worktree_last_run_epoch_takes_max_of_running_and_finished() {
        let mut both = wtrow("a");
        both.running_elapsed = Some(100);
        both.last = Some(('✓', "t".into(), 50, false));
        assert_eq!(worktree_last_run_epoch(&both), 100); // running newer

        let mut fin_newer = wtrow("b");
        fin_newer.running_elapsed = Some(30);
        fin_newer.last = Some(('✓', "t".into(), 80, false));
        assert_eq!(worktree_last_run_epoch(&fin_newer), 80); // finished newer

        let mut fin_only = wtrow("c");
        fin_only.last = Some(('✓', "t".into(), 50, false));
        assert_eq!(worktree_last_run_epoch(&fin_only), 50); // no running task

        assert_eq!(worktree_last_run_epoch(&wtrow("d")), 0); // no activity
    }

    #[test]
    fn cmp_worktree_rows_mine_first_beats_fresher_non_mine() {
        // A mine row with STALE activity still outranks a non-mine row with the
        // freshest possible activity — the mine tier dominates.
        let mut mine = wtrow("mine");
        mine.last_commit_author_email = Some("me@example.com".into());
        mine.running_elapsed = Some(10);
        let mut other = wtrow("other");
        other.running_elapsed = Some(9_999);
        assert_eq!(cmp_worktree_rows(&mine, &other, Some("me")), Ordering::Less);
        assert_eq!(cmp_worktree_rows(&other, &mine, Some("me")), Ordering::Greater);
        // Without a github_id the mine tier is inert, so freshness decides.
        assert_eq!(cmp_worktree_rows(&mine, &other, None), Ordering::Greater);
    }

    #[test]
    fn cmp_worktree_rows_running_outranks_finished_only_then_commit_fallback() {
        // Tier 2: a running lane (last-run 100) beats a finished-only lane (50).
        let mut running = wtrow("run");
        running.running_elapsed = Some(100);
        let mut finished = wtrow("fin");
        finished.last = Some(('✓', "t".into(), 50, false));
        assert_eq!(cmp_worktree_rows(&running, &finished, None), Ordering::Less);

        // Tier 3: two idle lanes (last-run 0) fall to last-commit desc.
        let mut newer_commit = wtrow("newer");
        newer_commit.last_commit_epoch = Some(200);
        let mut older_commit = wtrow("older");
        older_commit.last_commit_epoch = Some(100);
        assert_eq!(cmp_worktree_rows(&newer_commit, &older_commit, None), Ordering::Less);
    }

    #[test]
    fn cmp_worktree_rows_stable_tiebreak_keeps_input_order() {
        // Rows tied on every key compare Equal, so a STABLE sort preserves their
        // original order (the documented tiebreak).
        let a = wtrow("a");
        let b = wtrow("b");
        assert_eq!(cmp_worktree_rows(&a, &b, None), Ordering::Equal);
        let mut rows = [wtrow("first"), wtrow("second"), wtrow("third")];
        rows.sort_by(|x, y| cmp_worktree_rows(x, y, None));
        assert_eq!(
            rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            vec!["first", "second", "third"],
        );
    }

    #[test]
    fn worktree_rows_sorts_mine_first_end_to_end() {
        // End-to-end through worktree_rows: the project's github_id is read from
        // the snapshot and both mine rows (email- and name-matched) precede the
        // fresher non-mine row; mine rows keep their input order (stable).
        let mut s = snap(vec![], vec![]);
        s.projects = vec![Project { name: "platform".into(), github_id: Some("noootown".into()) }];
        s.worktrees = HashMap::from([(
            "platform".to_string(),
            vec![
                {
                    let mut w = wt("fresh", "/wt/fresh", "f"); // not mine, fresh commit
                    w.last_commit_epoch = Some(9_000);
                    w.last_commit_author_email = Some("someone@else.com".into());
                    w
                },
                {
                    let mut w = wt("mine-email", "/wt/mine-email", "m1"); // mine by email
                    w.last_commit_author_email =
                        Some("12345+NOOOTOWN@users.noreply.github.com".into());
                    w
                },
                {
                    let mut w = wt("mine-name", "/wt/mine-name", "m2"); // mine by name
                    w.last_commit_author = Some("noootown dev".into());
                    w
                },
            ],
        )]);
        let names: Vec<String> =
            worktree_rows(&s, "platform").into_iter().map(|r| r.name).collect();
        assert_eq!(names, vec!["mine-email", "mine-name", "fresh"]);
    }

    #[test]
    fn session_row_appended_for_interactive_cwd_inside_worktree() {
        let mut s = snap(vec![], vec![]);
        s.worktrees = platform_worktrees();
        s.sessions = vec![
            make_session("/wt/wt-b/packages/tui", "interactive"),
            make_session("/elsewhere/repo", "interactive"),
            make_session("/wt/wt-a", "worker"),
        ];
        let rows = worktree_rows(&s, "platform");
        let sessions: Vec<&WorktreeRow> = rows.iter().filter(|r| r.is_session).collect();
        assert_eq!(sessions.len(), 1);
        let row = sessions[0];
        assert_eq!(row.name, "tui");
        assert_eq!(row.raw_name, "tui");
        assert_eq!(row.path, "/wt/wt-b/packages/tui");
        assert_eq!(row.branch, "");
        assert_eq!(row.state, WtState::You);
        assert_eq!(row.queued, 0);
    }

    #[test]
    fn session_row_matches_exact_cwd_but_not_sibling_prefix() {
        let mut s = snap(vec![], vec![]);
        s.worktrees = platform_worktrees();
        s.sessions = vec![make_session("/wt/wt-a", "interactive")];
        let rows = worktree_rows(&s, "platform");
        assert_eq!(rows.iter().filter(|r| r.is_session).count(), 1);

        s.sessions = vec![make_session("/wt/wt-a-sibling", "interactive")];
        let rows = worktree_rows(&s, "platform");
        assert_eq!(rows.iter().filter(|r| r.is_session).count(), 0);
    }

    #[test]
    fn no_rows_for_project_without_worktrees() {
        let mut s = snap(vec![], vec![]);
        s.worktrees = platform_worktrees();
        assert_eq!(worktree_rows(&s, "web"), vec![]);
    }

    #[test]
    fn worktree_rows_strip_repo_prefix_but_keep_raw_name() {
        let mut s = snap(vec![], vec![]);
        s.worktrees = HashMap::from([(
            "platform".to_string(),
            vec![
                wt("platform", "/wt/platform", "main"),
                wt("platform.dedup-dependabot-run", "/wt/platform.dedup-dependabot-run", "dedup-dependabot-run"),
            ],
        )]);
        let rows = worktree_rows(&s, "platform");
        // Idle worktrees (no tasks, no git enrichment) tie on every ordering tier,
        // so the STABLE sort keeps their daemon-emitted input order: platform,
        // then dedup-dependabot-run (as inserted above).
        assert_eq!(
            rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            vec!["platform", "dedup-dependabot-run"]
        );
        // raw_name retains the full `<repo>.` prefix for daemon dispatch.
        let dedup = rows.iter().find(|r| r.name == "dedup-dependabot-run").unwrap();
        assert_eq!(dedup.raw_name, "platform.dedup-dependabot-run");
        assert_eq!(dedup.path, "/wt/platform.dedup-dependabot-run");
        let plain = rows.iter().find(|r| r.name == "platform").unwrap();
        assert_eq!(plain.raw_name, "platform");
    }

    #[test]
    fn session_row_display_name_strips_repo_prefix() {
        let mut s = snap(vec![], vec![]);
        s.worktrees = HashMap::from([(
            "platform".to_string(),
            vec![wt("platform.feat-x", "/wt/platform.feat-x", "feat-x")],
        )]);
        s.sessions = vec![make_session("/wt/platform.feat-x", "interactive")];
        let rows = worktree_rows(&s, "platform");
        let session = rows.iter().find(|r| r.is_session).unwrap();
        assert_eq!(session.name, "feat-x");
        assert_eq!(session.raw_name, "feat-x"); // mirrors display name — never dispatched
    }

    #[test]
    fn worktree_counts_queued_tasks_per_lane() {
        let mut s = snap(
            vec![
                task_on(TaskStatus::Queued, "01TASKQ00000000000000000001", "platform", Some("wt-a")),
                task_on(TaskStatus::Queued, "01TASKQ00000000000000000002", "platform", Some("wt-a")),
                task_on(TaskStatus::Running, "01TASKQ00000000000000000003", "platform", Some("wt-b")),
            ],
            vec![],
        );
        s.worktrees = platform_worktrees();
        let rows = worktree_rows(&s, "platform");
        assert_eq!(rows.iter().find(|r| r.name == "wt-a").unwrap().queued, 2);
        assert_eq!(rows.iter().find(|r| r.name == "wt-b").unwrap().queued, 0);
        assert_eq!(rows.iter().find(|r| r.name == "wt-c").unwrap().queued, 0);
    }

