    // ---- build_lane_index (single-pass oracle vs the legacy O(W×T) per-lane
    // helpers it replaces) ----

    #[test]
    fn lane_index_matches_legacy_per_lane_helpers() {
        // wt-a: BUSY — a running task wins even with a Failed sibling present
        // (proves Busy short-circuits the newest-by-id Failed/Free branch).
        let tasks_a = vec![
            task_on(TaskStatus::Failed, "01A00000000000000000000001", "platform", Some("wt-a")),
            task_on(TaskStatus::Running, "01A00000000000000000000002", "platform", Some("wt-a")),
        ];

        // wt-b: queued-only, 2 queued tasks. The FIRST one in snapshot/vec order
        // (deliberately the HIGHER id, i.e. NOT the newest-by-id) must win — this
        // catches an index that picks by id instead of preserving `.find()`'s
        // first-in-vec-order semantics.
        let mut q_head =
            task_on(TaskStatus::Queued, "01B00000000000000000000009", "platform", Some("wt-b"));
        q_head.definition = Some("beta-def".into());
        let mut q_tail =
            task_on(TaskStatus::Queued, "01B00000000000000000000001", "platform", Some("wt-b"));
        q_tail.definition = Some("alpha-def".into());
        let tasks_b = vec![q_head, q_tail];

        // wt-c: FAILED — newest-by-id live task is Failed.
        let tasks_c = vec![
            task_on(TaskStatus::Done, "01C00000000000000000000001", "platform", Some("wt-c")),
            task_on(TaskStatus::Failed, "01C00000000000000000000002", "platform", Some("wt-c")),
        ];

        // wt-d: FREE — newest-by-id live task is Done despite an older Failed
        // sibling (mirror of wt-c, opposite outcome).
        let tasks_d = vec![
            task_on(TaskStatus::Failed, "01D00000000000000000000001", "platform", Some("wt-d")),
            task_on(TaskStatus::Done, "01D00000000000000000000002", "platform", Some("wt-d")),
        ];

        // wt-g: live/archived merge. The only LIVE task is OLDER by id than the
        // ARCHIVED task, so `last_finished` (live+archived) must surface the
        // archived one while `worktree_state` (live-only) still reads the live
        // Done task as Free — the two aggregates diverge from the same lane.
        let tasks_g =
            vec![task_on(TaskStatus::Done, "01G00000000000000000000001", "platform", Some("wt-g"))];
        let archived_g =
            task_on(TaskStatus::Failed, "01G00000000000000000000005", "platform", Some("wt-g"));

        // wt-e: archived-only — no live task on the lane at all.
        let archived_e =
            task_on(TaskStatus::Done, "01E00000000000000000000001", "platform", Some("wt-e"));

        let mut tasks = Vec::new();
        tasks.extend(tasks_a);
        tasks.extend(tasks_b);
        tasks.extend(tasks_c);
        tasks.extend(tasks_d);
        tasks.extend(tasks_g);
        let archived = vec![archived_e, archived_g];
        let s = snap(tasks, archived);

        // wt-f: deliberately has NO task anywhere (live or archived) — exercises
        // the "lane absent from the index" miss path, which must agree with what
        // the legacy helpers return when nothing matches (all defaults).
        let lanes: Vec<String> =
            ["wt-a", "wt-b", "wt-c", "wt-d", "wt-e", "wt-f", "wt-g"]
                .iter()
                .map(|w| lane_key("platform", w))
                .collect();

        let index = build_lane_index(&s);
        for lane in &lanes {
            let agg = index.get(lane);
            assert_eq!(lane_state(agg), worktree_state(&s, lane), "state mismatch for {lane}");
            assert_eq!(lane_queued(agg), queued_on_lane(&s, lane), "queued mismatch for {lane}");
            assert_eq!(
                lane_running_elapsed(agg),
                running_elapsed_on_lane(&s, lane),
                "running_elapsed mismatch for {lane}"
            );
            assert_eq!(
                lane_next_queued(agg),
                next_queued_name_on_lane(&s, lane),
                "next_queued mismatch for {lane}"
            );
            assert_eq!(
                lane_last_finished(agg),
                last_finished_on_lane(&s, lane),
                "last_finished mismatch for {lane}"
            );
        }

        // Sanity: the loop above only proves agreement, not that the fixture
        // actually exercised the interesting branches — pin the non-trivial
        // outcomes directly so a vacuously-passing fixture can't hide a bug.
        assert_eq!(worktree_state(&s, &lane_key("platform", "wt-a")), WtState::Busy);
        assert_eq!(worktree_state(&s, &lane_key("platform", "wt-c")), WtState::Failed);
        assert_eq!(worktree_state(&s, &lane_key("platform", "wt-d")), WtState::Free);
        assert_eq!(worktree_state(&s, &lane_key("platform", "wt-g")), WtState::Free);
        assert_eq!(queued_on_lane(&s, &lane_key("platform", "wt-b")), 2);
        assert_eq!(
            next_queued_name_on_lane(&s, &lane_key("platform", "wt-b")),
            Some(("beta-def".to_string(), true))
        );
        // wt-g's last-finished must come from the ARCHIVED Failed task (id …005),
        // not the live Done task (id …001) — the live+archived merge in action.
        assert_eq!(
            last_finished_on_lane(&s, &lane_key("platform", "wt-g")).map(|(g, _, _, _)| g),
            Some('✗')
        );
        // wt-e's last-finished must come from the archive alone.
        assert_eq!(
            last_finished_on_lane(&s, &lane_key("platform", "wt-e")).map(|(g, _, _, _)| g),
            Some('●')
        );
    }

    // ---- pane_layout (mirrors computePaneLayout) ----

    const NONE_COLLAPSED: [bool; 3] = [false, false, false];

    #[test]
    fn pane_layout_sums_exactly_to_body_height() {
        for body in [13u16, 20, 38, 50, 77] {
            let l = pane_layout(body, None, None, NONE_COLLAPSED);
            assert_eq!(l.queue_h + l.tasks_h + l.worktrees_h, body, "body={body}");
        }
    }

    #[test]
    fn pane_layout_gives_queue_half_and_lists_quarter_each() {
        let l = pane_layout(38, None, None, NONE_COLLAPSED);
        assert_eq!(l.tasks_h, 9);
        assert_eq!(l.worktrees_h, 9);
        assert_eq!(l.queue_h, 20);
    }

    #[test]
    fn pane_layout_keeps_minimums_for_tiny_body() {
        let l = pane_layout(1, None, None, NONE_COLLAPSED);
        assert!(l.tasks_h >= 4);
        assert!(l.worktrees_h >= 4);
        assert!(l.queue_h >= 4);
    }

    #[test]
    fn pane_layout_collapsed_panes_pinned_expanded_keep_floor_sum_exact() {
        // Each single-collapsed case: the collapsed pane is exactly COLLAPSED_H,
        // the two expanded panes each keep PANE_MIN_H, and the sum is exact.
        for (collapsed, idx) in [
            ([true, false, false], 0usize),
            ([false, true, false], 1),
            ([false, false, true], 2),
        ] {
            let l = pane_layout(40, None, None, collapsed);
            let heights = [l.queue_h, l.tasks_h, l.worktrees_h];
            assert_eq!(heights[idx], COLLAPSED_H, "collapsed pane pinned, case {idx}");
            for (i, &h) in heights.iter().enumerate() {
                if i != idx {
                    assert!(h >= PANE_MIN_H, "expanded pane {i} keeps floor, case {idx}");
                }
            }
            assert_eq!(heights.iter().sum::<u16>(), 40, "sum exact, case {idx}");
        }
    }

    #[test]
    fn pane_layout_two_collapsed_gives_remainder_to_single_expanded() {
        // Queue + tasks collapsed → worktrees is the lone expanded pane and takes
        // everything left over.
        let l = pane_layout(40, None, None, [true, true, false]);
        assert_eq!(l.queue_h, COLLAPSED_H);
        assert_eq!(l.tasks_h, COLLAPSED_H);
        assert_eq!(l.worktrees_h, 40 - 2 * COLLAPSED_H);
        assert_eq!(l.queue_h + l.tasks_h + l.worktrees_h, 40);
    }

    #[test]
    fn pane_layout_all_collapsed_folds_filler_into_last_pane() {
        // All three collapsed: the two upper panes pin to COLLAPSED_H, the last
        // pane's region absorbs the blank filler; the sum stays exact.
        let l = pane_layout(40, None, None, [true, true, true]);
        assert_eq!(l.queue_h, COLLAPSED_H);
        assert_eq!(l.tasks_h, COLLAPSED_H);
        assert_eq!(l.worktrees_h, 40 - 2 * COLLAPSED_H);
        assert_eq!(l.queue_h + l.tasks_h + l.worktrees_h, 40);
    }

    #[test]
    fn pane_layout_honors_override_on_upper_expanded_pane_with_one_collapsed() {
        // Worktrees collapsed → queue + tasks split the rest; the queue override
        // is honored, tasks absorbs the remainder, sum exact.
        let l = pane_layout(40, Some(24), None, [false, false, true]);
        assert_eq!(l.worktrees_h, COLLAPSED_H);
        assert_eq!(l.queue_h, 24);
        assert_eq!(l.tasks_h, 40 - COLLAPSED_H - 24);
        assert_eq!(l.queue_h + l.tasks_h + l.worktrees_h, 40);
        assert!(l.tasks_h >= PANE_MIN_H);
    }

    #[test]
    fn pane_layout_default_matches_legacy_formula() {
        // The no-override path must reproduce the old 2:1:1 formula byte-for-byte
        // so default snapshots never move.
        for body in [13u16, 22, 30, 38, 50, 64, 77, 120] {
            let list_h = std::cmp::max(4, body / 4);
            let queue_h = std::cmp::max(4, body.saturating_sub(2 * list_h));
            let l = pane_layout(body, None, None, NONE_COLLAPSED);
            assert_eq!(
                (l.queue_h, l.tasks_h, l.worktrees_h),
                (queue_h, list_h, list_h),
                "body={body}"
            );
        }
    }

    #[test]
    fn pane_layout_applies_overrides_and_sums_exactly() {
        // Grow queue, shrink tasks; worktrees absorbs the remainder.
        let l = pane_layout(40, Some(24), Some(6), NONE_COLLAPSED);
        assert_eq!(l.queue_h, 24);
        assert_eq!(l.tasks_h, 6);
        assert_eq!(l.worktrees_h, 10);
        assert_eq!(l.queue_h + l.tasks_h + l.worktrees_h, 40);
    }

    #[test]
    fn pane_layout_clamps_overrides_to_minimums() {
        // A queue override that would starve tasks+worktrees is capped so each
        // still keeps PANE_MIN_H, and the sum stays exact.
        let l = pane_layout(40, Some(1000), None, NONE_COLLAPSED);
        assert_eq!(l.queue_h, 40 - 2 * PANE_MIN_H); // body − two floors
        assert!(l.tasks_h >= PANE_MIN_H);
        assert!(l.worktrees_h >= PANE_MIN_H);
        assert_eq!(l.queue_h + l.tasks_h + l.worktrees_h, 40);

        // A tasks override big enough to squeeze worktrees below the floor is
        // capped to leave worktrees exactly PANE_MIN_H.
        let l = pane_layout(30, Some(10), Some(1000), NONE_COLLAPSED);
        assert_eq!(l.worktrees_h, PANE_MIN_H);
        assert_eq!(l.queue_h, 10);
        assert_eq!(l.queue_h + l.tasks_h + l.worktrees_h, 30);

        // A too-small override is raised to the floor.
        let l = pane_layout(40, Some(0), Some(0), NONE_COLLAPSED);
        assert_eq!(l.queue_h, PANE_MIN_H);
        assert_eq!(l.tasks_h, PANE_MIN_H);
        assert_eq!(l.queue_h + l.tasks_h + l.worktrees_h, 40);
    }

    #[test]
    fn clamp_left_cols_keeps_both_sides_usable() {
        // Wide terminal: honored within range.
        assert_eq!(clamp_left_cols(120, 50), 50);
        // Too wide → DETAIL keeps its floor.
        assert_eq!(clamp_left_cols(120, 200), 120 - 30);
        // Too narrow → left keeps its floor.
        assert_eq!(clamp_left_cols(120, 5), 24);
        // 60-col minimum: range [24, 30] stays non-empty (clamp never panics).
        assert_eq!(clamp_left_cols(60, 40), 30); // hi = max(60−30, 24) = 30
        assert_eq!(clamp_left_cols(60, 100), 30);
        assert_eq!(clamp_left_cols(60, 10), 24);
    }

    // ---- window_rows (mirrors windowRows) ----

    #[test]
    fn window_rows_edges_and_centering() {
        assert_eq!(window_rows(10, 3, 20), (0, 10)); // all rows fit
        assert_eq!(window_rows(10, 0, 4), (0, 4)); // top edge
        assert_eq!(window_rows(10, 5, 4), (3, 7)); // centered
        assert_eq!(window_rows(10, 9, 4), (6, 10)); // bottom edge
        assert_eq!(window_rows(10, 3, 0), (0, 0)); // non-positive capacity
        assert_eq!(window_rows(0, 0, 4), (0, 0)); // empty list
        assert_eq!(window_rows(10, 99, 4), (6, 10)); // out-of-range cursor clamps
    }

    // ---- pane_title (mirrors paneTitle incl. selection count) ----

    #[test]
    fn pane_title_plain_when_not_bulk() {
        assert_eq!(pane_title("QUEUE", 1, false), "QUEUE");
    }

    #[test]
    fn pane_title_selection_count() {
        assert_eq!(pane_title("WORKTREES", 3, true), "WORKTREES · 3 selected");
        assert_eq!(pane_title("WORKTREES", 2, true), "WORKTREES · 2 selected");
        // A single MARKED row is bulk — the title must say so, or the pane would
        // read "WORKTREES" while a row sits highlighted.
        assert_eq!(pane_title("WORKTREES", 1, true), "WORKTREES · 1 selected");
    }

    #[test]
    fn pane_title_bulk_but_zero_resolved_hides_the_ghost_count() {
        // `bulk` can be true (a mark is present) while it resolves to NO visible
        // row — e.g. the marked row was filtered out by search. "· 0 selected"
        // is nonsensical, so the suffix must not appear in that case.
        assert_eq!(pane_title("WORKTREES", 0, true), "WORKTREES");
    }

    // ---- filter_rows (mirrors matchesFilter) ----

    #[test]
    fn filter_rows_empty_query_matches_everything_else_ci_substring() {
        let rows = vec!["Fix-TUI-Bug".to_string(), "other".to_string(), "fix-tui-bug".to_string()];
        assert_eq!(filter_rows(&rows, "", |r| r.clone()), vec![0, 1, 2]);
        assert_eq!(filter_rows(&rows, "tui", |r| r.clone()), vec![0, 2]);
        assert_eq!(filter_rows(&rows, "TUI", |r| r.clone()), vec![0, 2]);
        assert_eq!(filter_rows(&rows, "xyz", |r| r.clone()), Vec::<usize>::new());
    }

    #[test]
    fn queue_search_text_matches_def_name_worktree_and_summary() {
        let row = QueueRow {
            task_id: "t1".into(),
            glyph: '○',
            running: false,
            worktree: "JUS-1966".into(),
            def_name: Some("intake".into()),
            summary: "blank page after undo".into(),
            detail: String::new(),
            running_elapsed: None,
            not_before_epoch_s: None,
            lane_key: String::new(),
            lane_position: None,
            created_epoch_s: 0,
            archived: false,
            status: TaskStatus::Queued,
            priority: "normal".into(),
            finished_epoch_s: None,
        };
        let hay = queue_search_text(&row);
        assert!(hay.contains("intake"));
        assert!(hay.contains("JUS-1966"));
        assert!(hay.contains("blank page after undo"));
        // Filter by task (def) name
        assert_eq!(filter_rows(&[row.clone()], "intake", queue_search_text), vec![0]);
        assert_eq!(
            filter_rows(&[row.clone()], "pr-ready", queue_search_text),
            Vec::<usize>::new()
        );
        // Filter by worktree / ticket-like name
        assert_eq!(filter_rows(&[row.clone()], "1966", queue_search_text), vec![0]);
        // Still matches prompt summary
        assert_eq!(filter_rows(&[row], "undo", queue_search_text), vec![0]);
    }

    // ---- column layout helpers ----

    #[test]
    fn clip_and_pad_clip_are_char_safe() {
        assert_eq!(clip("hello", 10), "hello"); // fits, unchanged
        assert_eq!(clip("hello world", 5), "hell…"); // 4 chars + …
        assert_eq!(clip("hi", 1), "…"); // width 1 → ellipsis only
        assert_eq!(clip("hi", 0), ""); // width 0 → empty
        // Multi-byte chars must not panic on truncation (é is 2 bytes, 1 char).
        assert_eq!(clip("café-latte", 5), "café…");
        // pad_clip always returns exactly `width` chars.
        assert_eq!(pad_clip("ab", 5), "ab   ");
        assert_eq!(pad_clip("abcdef", 4), "abc…");
        assert_eq!(pad_clip("", 3), "   ");
    }

    fn qrow(worktree: &str, def: Option<&str>, summary: &str) -> QueueRow {
        QueueRow {
            task_id: "t".into(),
            glyph: '○',
            running: false,
            worktree: worktree.into(),
            def_name: def.map(str::to_string),
            summary: summary.into(),
            detail: String::new(),
            running_elapsed: None,
            not_before_epoch_s: None,
            lane_key: String::new(),
            lane_position: None,
            created_epoch_s: 0,
            archived: false,
            status: TaskStatus::Queued,
            priority: "normal".into(),
            finished_epoch_s: None,
        }
    }

    #[test]
    fn queue_col_layout_wide_shows_all_columns_sized_to_visible_max() {
        let rows = vec![
            qrow("feature", Some("squash-merge"), "implement the widget cache"),
            qrow("main", None, "flaky migration"),
        ];
        // Wide pane: every column present, summary flexes to fill the slack.
        let l = queue_col_layout(&rows, 100, 0);
        assert_eq!(l.worktree_w, 7); // "feature"
        assert_eq!(l.def_w, 12); // "squash-merge"
        assert!(l.show_timestamp);
        assert!(l.age_w > 0);
        assert!(l.summary_w >= SUMMARY_MIN);
    }

    #[test]
    fn queue_col_layout_narrow_degrades_trailing_then_def_keeping_summary_floor() {
        let rows = vec![
            qrow("feature", Some("squash-merge"), "implement the widget cache"),
            qrow("main", None, "flaky migration"),
        ];
        // 23 inner cells (the 80x24 default left pane): timestamp/age/detail drop,
        // then def drops. With the main-session chain column gone the prefix is 2
        // cells lighter, so "feature" keeps its full 7 and still holds the floor.
        let l = queue_col_layout(&rows, 23, 0);
        assert!(!l.show_timestamp);
        assert_eq!(l.age_w, 0);
        assert_eq!(l.live_w, 0);
        assert_eq!(l.def_w, 0);
        assert_eq!(l.worktree_w, 7);
        assert!(l.summary_w >= SUMMARY_MIN, "summary keeps its floor (got {})", l.summary_w);
    }

    #[test]
    fn queue_col_layout_caps_worktree_and_def() {
        let rows = vec![qrow(
            "a-very-long-worktree-name-indeed",
            Some("a-very-long-definition-name"),
            "s",
        )];
        let l = queue_col_layout(&rows, 200, 0);
        assert_eq!(l.worktree_w, WORKTREE_CAP);
        assert_eq!(l.def_w, DEF_CAP);
    }

    #[test]
    fn def_and_wt_col_layout_size_name_column() {
        let defs = vec![
            DefinitionSummary { name: "squash-merge".into(), ..Default::default() },
            DefinitionSummary { name: "pr".into(), ..Default::default() },
        ];
        let m = empty_model_owned();
        let l = def_col_layout(&defs, 80, &m.ctx());
        assert_eq!(l.name_w, 12); // "squash-merge"
        assert_eq!(l.sched_w, 0); // no def carries a cron
        assert_eq!(l.desc_w, 0); // no def carries a description

        let mut s = snap(vec![], vec![]);
        s.worktrees = platform_worktrees();
        let rows = worktree_rows(&s, "platform");
        let l = wt_col_layout(&rows, 80);
        assert_eq!(l.name_w, 4); // "wt-a"/"wt-b"/"wt-c"
    }

    #[test]
    fn worktree_row_derives_running_next_and_last() {
        let running = {
            let mut t = task_on(TaskStatus::Running, "01R00000000000000000000001", "platform", Some("wt-a"));
            t.created = "2026-07-08T10:00:00.000Z".into();
            t
        };
        let queued = {
            let mut t = task_on(TaskStatus::Queued, "01Q00000000000000000000001", "platform", Some("wt-a"));
            t.definition = Some("platform/pr-ready".into());
            t
        };
        // A live FAILED task and an archived DONE task on the same lane; the
        // newest by id wins regardless of which list it lives in.
        let done_new = {
            let mut t = task_on(TaskStatus::Failed, "01D00000000000000000000009", "platform", Some("wt-a"));
            t.definition = Some("platform/deploy".into());
            t.created = "2026-07-08T09:30:00.000Z".into();
            t
        };
        let done_old = {
            let mut t = task_on(TaskStatus::Done, "01D00000000000000000000001", "platform", Some("wt-a"));
            t.definition = Some("platform/squash-merge".into());
            t.created = "2026-07-08T09:00:00.000Z".into();
            t
        };
        let mut s = snap(vec![running, queued, done_new], vec![done_old]);
        s.worktrees = platform_worktrees();
        let rows = worktree_rows(&s, "platform");
        let a = rows.iter().find(|r| r.name == "wt-a").unwrap();
        assert_eq!(a.state, WtState::Busy);
        assert_eq!(a.running_elapsed, Some(parse_iso_epoch_s("2026-07-08T10:00:00.000Z")));
        assert_eq!(a.next_name.as_deref(), Some("pr-ready"));
        assert!(a.next_is_def); // "pr-ready" came from a definition
        assert_eq!(
            a.last,
            Some(('✗', "deploy".into(), parse_iso_epoch_s("2026-07-08T09:30:00.000Z"), true))
        );
        // A worktree with no lane tasks carries none of the derived fields.
        let b = rows.iter().find(|r| r.name == "wt-b").unwrap();
        assert_eq!((b.running_elapsed, b.next_name.as_deref(), b.last.as_ref()), (None, None, None));
    }

    #[test]
    fn worktree_next_name_falls_back_to_clipped_prompt() {
        // No definition → the head-of-lane name is the prompt clipped to
        // NEXT_NAME_CAP.
        let mut queued = task_on(TaskStatus::Queued, "01Q00000000000000000000001", "platform", Some("wt-a"));
        queued.definition = None;
        queued.prompt = "rewrite the whole scheduler from scratch".into();
        let mut s = snap(vec![queued], vec![]);
        s.worktrees = platform_worktrees();
        let rows = worktree_rows(&s, "platform");
        let a = rows.iter().find(|r| r.name == "wt-a").unwrap();
        assert_eq!(a.next_name.as_deref(), Some(&*clip("rewrite the whole scheduler from scratch", NEXT_NAME_CAP)));
        assert_eq!(a.next_name.as_deref().map(|s| s.chars().count()), Some(NEXT_NAME_CAP));
        assert!(!a.next_is_def); // prompt fallback → not a def name
    }

    #[test]
    fn worktree_last_task_name_is_not_preclipped() {
        // The last-finished cell is the pane's FILL column: the render clips to
        // the row's actual width, so the derived name must NOT be pre-clipped to
        // NEXT_NAME_CAP (a 16-char pre-clip left the wide fill rendering blank).
        let long = "Continue from the approved design. The spec is committed at docs";
        let mut done = task_on(TaskStatus::Failed, "01D00000000000000000000001", "platform", Some("wt-a"));
        done.definition = None;
        done.prompt = long.into();
        let mut s = snap(vec![done], vec![]);
        s.worktrees = platform_worktrees();
        let rows = worktree_rows(&s, "platform");
        let a = rows.iter().find(|r| r.name == "wt-a").unwrap();
        let (glyph, name, _, is_def) = a.last.as_ref().unwrap();
        assert_eq!(*glyph, '✗');
        assert_eq!(name, long, "full prompt summary reaches the fill column");
        assert!(name.chars().count() > NEXT_NAME_CAP);
        assert!(!is_def);
    }

    #[test]
    fn worktree_rows_carry_protected_flag() {
        let mut wts = HashMap::new();
        wts.insert(
            "platform".to_string(),
            vec![
                WorktreeInfo {
                    name: "legal-lake".into(),
                    path: "/repos/platform.legal-lake".into(),
                    branch: "legal-lake".into(),
                    protected: true,
                    ..Default::default()
                },
                wt("JUS-1", "/repos/platform.JUS-1", "JUS-1"),
            ],
        );
        let s = StateSnapshot {
            projects: projects(&["platform"]),
            worktrees: wts,
            ..Default::default()
        };
        let rows = worktree_rows(&s, "platform");
        let by: HashMap<_, _> =
            rows.iter().map(|r| (r.raw_name.clone(), r.protected)).collect();
        assert!(by["legal-lake"]);
        assert!(!by["JUS-1"]);
    }

    #[test]
    fn worktree_last_task_sub_classifies_failed_by_error_reason() {
        // The WORKTREES pane's "Last Task" column derives from the same
        // `status_glyph` classification as the QUEUE pane — a timed-out,
        // session-limited, or out-of-budget run must show its distinct glyph here
        // too, not just in the queue.
        let mut timed_out = task_on(TaskStatus::Failed, "01D00000000000000000000001", "platform", Some("wt-a"));
        timed_out.error = Some("timed out".into());
        let mut s = snap(vec![timed_out], vec![]);
        s.worktrees = platform_worktrees();
        let rows = worktree_rows(&s, "platform");
        let a = rows.iter().find(|r| r.name == "wt-a").unwrap();
        assert_eq!(a.last.as_ref().unwrap().0, '⧗');

        let mut session_limit =
            task_on(TaskStatus::Failed, "01D00000000000000000000001", "platform", Some("wt-b"));
        session_limit.error = Some("session limit".into());
        let mut s = snap(vec![session_limit], vec![]);
        s.worktrees = platform_worktrees();
        let rows = worktree_rows(&s, "platform");
        let b = rows.iter().find(|r| r.name == "wt-b").unwrap();
        assert_eq!(b.last.as_ref().unwrap().0, '$'); // session-limit → $ limit glyph

        let mut out_of_budget =
            task_on(TaskStatus::Failed, "01D00000000000000000000001", "platform", Some("wt-a"));
        out_of_budget.error = Some("out of budget".into());
        let mut s = snap(vec![out_of_budget], vec![]);
        s.worktrees = platform_worktrees();
        let rows = worktree_rows(&s, "platform");
        let c = rows.iter().find(|r| r.name == "wt-a").unwrap();
        assert_eq!(c.last.as_ref().unwrap().0, '$');

        let mut provider_unavailable =
            task_on(TaskStatus::Failed, "01D00000000000000000000001", "platform", Some("wt-c"));
        provider_unavailable.error = Some("provider unavailable".into());
        let mut s = snap(vec![provider_unavailable], vec![]);
        s.worktrees = platform_worktrees();
        let rows = worktree_rows(&s, "platform");
        let d = rows.iter().find(|r| r.name == "wt-c").unwrap();
        assert_eq!(d.last.as_ref().unwrap().0, '⊟');
    }

    #[test]
    fn worktree_row_passes_through_git_enrichment() {
        let mut s = snap(vec![], vec![]);
        let mut wts = platform_worktrees();
        let list = wts.get_mut("platform").unwrap();
        list[0].dirty = Some(true);
        list[0].last_commit_epoch = Some(1_752_000_000);
        list[0].last_commit_author = Some("koshea".into());
        s.worktrees = wts;
        let rows = worktree_rows(&s, "platform");
        let a = rows.iter().find(|r| r.name == "wt-a").unwrap();
        assert_eq!(
            (a.dirty, a.last_commit_epoch, a.last_commit_author.as_deref()),
            (Some(true), Some(1_752_000_000), Some("koshea"))
        );
        let b = rows.iter().find(|r| r.name == "wt-b").unwrap();
        assert_eq!((b.dirty, b.last_commit_epoch, b.last_commit_author.as_deref()), (None, None, None));
    }

    #[test]
    fn wt_col_layout_degrades_columns_from_the_right() {
        // One fully-loaded row: every optional column populated.
        // Fixed widths: dirty=1, protected=1, merged=1, last-min=12,
        // author=AUTHOR_W(14), commit=8, activity=20; anchor = `● ± ⛨ ↣ name` =
        // 2+2+2+2+9 = 17. Full reserved = anchor(17) + author(2+14) +
        // commit(2+8) + last-min(2+12) + activity(2+20) = 79.
        let row = WorktreeRow {
            name: "feature-x".into(), // 9 cells
            raw_name: "feature-x".into(),
            state: WtState::Busy,
            queued: 2,
            running_elapsed: Some(now() - 192),     // "⏱ 3m12s"
            next_name: Some("pr-review".into()),
            next_is_def: false,
            last: Some(('✓', "pr-ready".into(), now() - 7200, true)), // "✓ pr-ready 2h ago" = 17
            dirty: Some(true),                       // ± = 1
            merged: Some(true),                      // ↣ = 1
            last_commit_author: Some("koshea".into()), // author column fixed AUTHOR_W
            last_commit_epoch: Some(now() - 3 * 86_400), // commit-age fixed COMMIT_AGE_W
            ..Default::default()
        };
        let rows = [row];
        // (dirty, protected, merged, activity, last, author, commit) presence.
        let present = |a: usize| {
            let l = wt_col_layout(&rows, a);
            (
                l.dirty_w > 0,
                l.protected_w > 0,
                l.merged_w > 0,
                l.activity_w > 0,
                l.last_w > 0,
                l.author_w > 0,
                l.commit_age_w > 0,
            )
        };
        // Wide: everything shown, name at full width. All reserved widths sum to
        // 79; at 120 the last-task FILL absorbs the slack.
        assert_eq!(present(79), (true, true, true, true, true, true, true));
        assert_eq!(wt_col_layout(&rows, 120).name_w, 9);
        assert_eq!(wt_col_layout(&rows, 120).last_w, 53, "last-task fill absorbs the slack");
        assert_eq!(wt_col_layout(&rows, 120).activity_w, 20, "activity is a fixed 20-cell slot");
        // Drop in ladder order: commit → author → merged → protected →
        // dirty → last → activity. Thresholds from used() after each drop:
        // full 79, -commit 69, -author 53, -merged 51, -prot 49, -dirty 47,
        // -last 33, -activity 11.
        assert_eq!(present(78), (true, true, true, true, true, true, false)); // commit
        assert_eq!(present(68), (true, true, true, true, true, false, false)); // author
        assert_eq!(present(51), (true, true, false, true, true, false, false)); // merged
        assert_eq!(present(49), (true, false, false, true, true, false, false)); // protected
        assert_eq!(present(47), (false, false, false, true, true, false, false)); // dirty
        assert_eq!(present(33), (false, false, false, true, false, false, false)); // last
        assert_eq!(wt_col_layout(&rows, 33).activity_w, 20);
        // Activity is the last optional to go.
        assert_eq!(present(32), (false, false, false, false, false, false, false));
        assert_eq!(wt_col_layout(&rows, 20).name_w, 9);
        // Below that, the name column shrinks (anchor `● name` = 2 + name_w).
        assert_eq!(wt_col_layout(&rows, 10).name_w, 8);
    }

    #[test]
    fn wt_col_layout_pr_column_gated_and_outlives_author_commit() {
        let row = WorktreeRow {
            name: "feature-x".into(),
            raw_name: "feature-x".into(),
            state: WtState::Busy,
            last: Some(('✓', "pr-ready".into(), now() - 7200, true)),
            dirty: Some(true),
            last_commit_author: Some("koshea".into()),
            last_commit_epoch: Some(now() - 3 * 86_400),
            pr_number: Some(42),
            pr_url: Some("https://github.com/o/r/pull/42".into()),
            ..Default::default()
        };
        let rows = [row.clone()];
        // Wide: the PR column is present at its fixed width.
        assert_eq!(wt_col_layout(&rows, 130).pr_w, PR_W);
        // Pane-gated: a row with no PR number reserves no PR column.
        let no_pr = WorktreeRow { pr_number: None, pr_url: None, ..row };
        assert_eq!(wt_col_layout(&[no_pr], 130).pr_w, 0);
        // Drop order: commit → author → PR. Scanning widths from wide to narrow,
        // the first width at which PR drops must already have dropped author and
        // commit-age (PR outlives them). While PR is present its cell offset stays
        // inside the pane, matching the render's fixed-width arithmetic.
        let mut pr_dropped = false;
        for a in (10..=130).rev() {
            let l = wt_col_layout(&rows, a);
            if l.pr_w == 0 && !pr_dropped {
                pr_dropped = true;
                assert_eq!(l.author_w, 0, "author drops before PR");
                assert_eq!(l.commit_age_w, 0, "commit-age drops before PR");
            }
            if l.pr_w > 0 {
                assert!(l.pr_col_x() < a, "PR cell starts within the pane width");
            }
        }
        assert!(pr_dropped, "PR eventually drops under width pressure");
    }

    #[test]
    fn fixed_column_widths_fit_representative_labels() {
        // Each fixed reserved width must hold its realistic max label under `cw`.
        // Timer: a 2-digit-hour elapsed ("⏱ 99h59m").
        let two_digit_hour = elapsed_label(0, 99 * 3600 + 59 * 60);
        assert_eq!(two_digit_hour, "⏱ 99h59m");
        assert!(cw(&two_digit_hour) <= TIMER_W);
        assert!(cw(&two_digit_hour) <= QUEUE_LIVE_W);
        // Deferred countdown shares the hourglass prefix and must fit Live.
        assert!(cw(&remaining_label(5 * 3600, 0)) <= QUEUE_LIVE_W);
        // Relative-age buckets, including the widest ("just now").
        for (c, n) in [(100u64, 100u64), (0, 300), (0, 3600), (0, 172_800)] {
            let label = relative_age_label(c, n);
            assert!(cw(&label) <= AGE_W, "age {label:?} fits AGE_W");
            assert!(cw(&label) <= COMMIT_AGE_W, "age {label:?} fits COMMIT_AGE_W");
        }
        assert_eq!(relative_age_label(100, 100), "just now");
        // Queue live position text.
        assert!(cw("#9 in lane") <= QUEUE_LIVE_W);
        // A representative author name fits AUTHOR_W; a longer name clips in the
        // renderer (pad_clip), so the column width itself is the invariant.
        let author_row = WorktreeRow { last_commit_author: Some("koshea".into()), ..Default::default() };
        assert!(cw(&wt_author_text(&author_row).unwrap()) <= AUTHOR_W);
        // Absolute timestamp.
        assert!(cw(&absolute_local_label(now(), 0)) <= TIMESTAMP_W);
    }

    #[test]
    fn wt_author_text_prefers_pr_author_over_last_commit_author() {
        // The PR author is the person who opened the PR; the local last-commit
        // author on a squash-merged branch is an automation merge commit
        // ("Ian Chiu"), so `pr_author` must win the Author column.
        let both = WorktreeRow {
            last_commit_author: Some("Ian Chiu".into()),
            pr_author: Some("Tim Kuminecz".into()),
            ..Default::default()
        };
        assert_eq!(wt_author_text(&both).as_deref(), Some("Tim Kuminecz"));

        // No PR author (old daemon / no PR) → fall back to the last-commit author.
        let only_commit = WorktreeRow {
            last_commit_author: Some("Ian Chiu".into()),
            pr_author: None,
            ..Default::default()
        };
        assert_eq!(wt_author_text(&only_commit).as_deref(), Some("Ian Chiu"));

        // Neither present → None (the whole Author column is omitted pane-wide).
        let neither = WorktreeRow::default();
        assert_eq!(wt_author_text(&neither), None);
    }

    #[test]
    fn wt_merge_marker_merged_wins_over_approved() {
        // Merged takes precedence: an approved-then-merged PR shows the merged
        // glyph, not the approved one — the merged fact subsumes the approval.
        let both = WorktreeRow {
            merged: Some(true),
            approved: Some(true),
            ..Default::default()
        };
        assert_eq!(wt_merge_marker(&both), Some(WtMergeMarker::Merged));

        // Merged with no/unknown approval still shows merged.
        let merged_only = WorktreeRow { merged: Some(true), ..Default::default() };
        assert_eq!(wt_merge_marker(&merged_only), Some(WtMergeMarker::Merged));
    }

    #[test]
    fn wt_merge_marker_approved_alone_shows_approved() {
        // Approved but not merged → the green approved marker.
        let approved = WorktreeRow {
            merged: Some(false),
            approved: Some(true),
            ..Default::default()
        };
        assert_eq!(wt_merge_marker(&approved), Some(WtMergeMarker::Approved));

        // Approved with merged unknown (old-daemon-style) still shows approved.
        let approved_merge_unknown = WorktreeRow { approved: Some(true), ..Default::default() };
        assert_eq!(wt_merge_marker(&approved_merge_unknown), Some(WtMergeMarker::Approved));
    }

    #[test]
    fn wt_merge_marker_neither_shows_nothing() {
        // A PR that exists but isn't approved/merged and has no label markers → blank.
        let not_approved = WorktreeRow {
            merged: Some(false),
            approved: Some(false),
            ready_for_review: Some(false),
            wip: Some(false),
            ..Default::default()
        };
        assert_eq!(wt_merge_marker(&not_approved), None);

        // Everything unknown (old daemon / no PR) → blank slot.
        let unknown = WorktreeRow::default();
        assert_eq!(wt_merge_marker(&unknown), None);
    }

    #[test]
    fn wt_merge_marker_ready_for_review_and_wip() {
        // Ready-for-review alone → ◎.
        let ready = WorktreeRow {
            merged: Some(false),
            approved: Some(false),
            ready_for_review: Some(true),
            wip: Some(false),
            ..Default::default()
        };
        assert_eq!(wt_merge_marker(&ready), Some(WtMergeMarker::ReadyForReview));

        // WIP alone → ✎.
        let wip = WorktreeRow {
            merged: Some(false),
            approved: Some(false),
            ready_for_review: Some(false),
            wip: Some(true),
            ..Default::default()
        };
        assert_eq!(wt_merge_marker(&wip), Some(WtMergeMarker::Wip));

        // Both labels → ready-for-review wins over WIP.
        let both_labels = WorktreeRow {
            ready_for_review: Some(true),
            wip: Some(true),
            ..Default::default()
        };
        assert_eq!(
            wt_merge_marker(&both_labels),
            Some(WtMergeMarker::ReadyForReview)
        );

        // Approve still beats ready-for-review.
        let approved_and_ready = WorktreeRow {
            approved: Some(true),
            ready_for_review: Some(true),
            wip: Some(true),
            ..Default::default()
        };
        assert_eq!(
            wt_merge_marker(&approved_and_ready),
            Some(WtMergeMarker::Approved)
        );

        // Merge still beats everything, including labels.
        let merged_and_labels = WorktreeRow {
            merged: Some(true),
            ready_for_review: Some(true),
            wip: Some(true),
            ..Default::default()
        };
        assert_eq!(
            wt_merge_marker(&merged_and_labels),
            Some(WtMergeMarker::Merged)
        );
    }

    #[test]
    fn queue_col_layout_stable_when_a_row_gains_a_timer() {
        // Two row sets identical except one row goes finished (no start epoch) →
        // running (start epoch set; timer formatted at paint). The live/age/
        // timestamp columns are FIXED reserved widths (never data-sized), so the
        // layout is byte-identical.
        let finished = |running: bool, glyph: char| QueueRow {
            task_id: "t".into(),
            glyph,
            running,
            worktree: "feature".into(),
            def_name: Some("squash-merge".into()),
            summary: "implement the widget cache".into(),
            detail: String::new(),
            running_elapsed: if running { Some(now() - 303) } else { None },
            not_before_epoch_s: None,
            lane_key: String::new(),
            lane_position: None,
            created_epoch_s: 0,
            archived: false,
            status: if running { TaskStatus::Running } else { TaskStatus::Done },
            priority: "normal".into(),
            finished_epoch_s: None,
        };
        let before = vec![finished(false, '✓'), qrow("main", None, "flaky migration")];
        let after = vec![finished(true, '▶'), qrow("main", None, "flaky migration")];
        assert_eq!(queue_col_layout(&before, 100, 0), queue_col_layout(&after, 100, 0));
    }

    #[test]
    fn wt_col_layout_stable_when_a_row_gains_a_timer_or_queued() {
        let base = WorktreeRow {
            name: "feature".into(),
            raw_name: "feature".into(),
            state: WtState::Free,
            ..Default::default()
        };
        // Pair 1: activity is statically reserved (fixed WT_ACTIVITY_W) for any
        // non-empty pane. Gaining a timer or a queued task does not re-size.
        let with_timer = WorktreeRow {
            running_elapsed: Some(now() - 100),
            ..base.clone()
        };
        assert_eq!(
            wt_col_layout(std::slice::from_ref(&base), 120).activity_w,
            20,
            "idle pane still reserves the Live column"
        );
        assert_eq!(
            wt_col_layout(std::slice::from_ref(&base), 120),
            wt_col_layout(std::slice::from_ref(&with_timer), 120)
        );
        // Pair 2: another row gaining a queued task changes nothing either.
        let other_queued = WorktreeRow {
            name: "other".into(),
            raw_name: "other".into(),
            queued: 1,
            next_name: Some("pr-review".into()),
            next_is_def: true,
            ..base.clone()
        };
        let base_gains_queued = WorktreeRow {
            queued: 2,
            next_name: Some("squash-merge".into()),
            next_is_def: true,
            ..base.clone()
        };
        assert_eq!(
            wt_col_layout(&[base.clone(), other_queued.clone()], 120),
            wt_col_layout(&[base_gains_queued, other_queued], 120)
        );
        // Pair 3: one row gains its FIRST finished task. The last-task cell is
        // the always-reserved FILL, so the layout is byte-identical too.
        let with_last = WorktreeRow {
            last: Some(('✓', "pr-ready".into(), now() - 7200, true)),
            ..base.clone()
        };
        assert_eq!(
            wt_col_layout(std::slice::from_ref(&base), 120),
            wt_col_layout(&[with_last], 120)
        );
    }

    #[test]
    fn cron_human_tier_table() {
        let cases = [
            // daily, with and without a nonzero minute
            ("30 13 * * *", Some("Everyday 1:30pm")),
            ("0 9 * * *", Some("Everyday 9am")),
            ("0 0 * * *", Some("Everyday 12am")),
            ("0 12 * * *", Some("Everyday 12pm")),
            // sub-hourly / hourly / multi-hour frequencies
            ("*/15 * * * *", Some("Every 15m")),
            ("0 * * * *", Some("Hourly")),
            ("0 */2 * * *", Some("Every 2h")),
            // weekday selectors
            ("30 13 * * 1", Some("Mon 1:30pm")),
            ("0 9 * * 0", Some("Sun 9am")),
            ("0 9 * * 7", Some("Sun 9am")), // 7 is also Sunday
            ("30 13 * * 1-5", Some("Weekdays 1:30pm")),
            ("0 9 * * 0,6", Some("Weekends 9am")),
            ("0 9 * * 6,0", Some("Weekends 9am")),
            // monthly (day-of-month) with ordinal
            ("0 9 1 * *", Some("Monthly 1st 9am")),
            ("30 8 22 * *", Some("Monthly 22nd 8:30am")),
            ("0 0 3 * *", Some("Monthly 3rd 12am")),
        ];
        for (expr, want) in cases {
            assert_eq!(cron_human(expr).as_deref(), want, "cron {expr:?}");
        }
    }

    #[test]
    fn cron_human_valid_shape_falls_back_to_raw() {
        // A well-formed 5-field cron we don't confidently phrase (specific dom +
        // month + dow) is shown verbatim rather than dropped.
        assert_eq!(cron_human("15 10 5 6 2").as_deref(), Some("15 10 5 6 2"));
        // Every-minute is valid but unphrased → raw.
        assert_eq!(cron_human("* * * * *").as_deref(), Some("* * * * *"));
        // Extra internal whitespace collapses in parsing but the raw fallback
        // preserves the trimmed original text.
        assert_eq!(cron_human("  15 10 5 6 2  ").as_deref(), Some("15 10 5 6 2"));
    }

    #[test]
    fn cron_human_garbage_returns_none() {
        for expr in ["", "   ", "not a cron", "@daily", "* * *", "30 13 * * * *", "a b c d e"] {
            assert_eq!(cron_human(expr), None, "garbage {expr:?}");
        }
    }

    #[test]
    fn def_sched_text_is_cron_only() {
        let mut d = crate::ipc::types::DefinitionSummary::default();
        // neither → empty
        assert_eq!(def_sched_text(&d), "");
        // discovery only → still empty (the marker lives in the front slot now)
        d.has_discovery = true;
        assert_eq!(def_sched_text(&d), "");
        // cron only → humanized cron, regardless of discovery
        d.has_discovery = false;
        d.cron = Some("30 15 * * *".into());
        assert_eq!(def_sched_text(&d), "Everyday 3:30pm");
        // both → cron only, no marker appended
        d.has_discovery = true;
        assert_eq!(def_sched_text(&d), "Everyday 3:30pm");
    }

    #[test]
    fn def_marker_slot_reserved_when_any_def_has_discovery() {
        let mut plain = crate::ipc::types::DefinitionSummary::default();
        plain.name = "aa".into();
        let mut disc = crate::ipc::types::DefinitionSummary::default();
        disc.name = "bb".into();
        disc.has_discovery = true;
        let m = empty_model_owned();
        // No discovery anywhere → no slot.
        assert_eq!(def_col_layout(&[plain.clone()], 80, &m.ctx()).marker_w, 0);
        // Any discovery def visible → 2-cell slot (glyph + separator), pane-wide.
        let l = def_col_layout(&[plain, disc], 80, &m.ctx()).marker_w;
        assert_eq!(l, 2);
    }

    #[test]
    fn def_col_layout_sizes_and_caps_schedule_column() {
        let m = empty_model_owned();
        let defs = vec![
            DefinitionSummary {
                name: "pr-review".into(),
                cron: Some("30 13 * * *".into()), // "Everyday 1:30pm" == 15 cells
                has_discovery: true,
                ..Default::default()
            },
            DefinitionSummary {
                name: "lint".into(),
                cron: Some("0 9 * * *".into()), // "Everyday 9am" == 12 cells
                ..Default::default()
            },
            DefinitionSummary { name: "deploy".into(), ..Default::default() }, // no cron
        ];
        let l = def_col_layout(&defs, 120, &m.ctx());
        assert_eq!(l.sched_w, 15); // "Everyday 1:30pm" (cron-only; marker lives in the front slot)
        assert_eq!(l.marker_w, 2, "pr-review has_discovery reserves the pane-wide front slot");

        // A raw-cron fallback longer than SCHED_CAP is clamped to the cap.
        let long = vec![DefinitionSummary {
            name: "x".into(),
            cron: Some("15 10 5 6 2".into()), // unphrased → 11-char raw... still ≤ cap
            ..Default::default()
        }];
        assert_eq!(def_col_layout(&long, 120, &m.ctx()).sched_w, 11);
        let huge = vec![DefinitionSummary {
            name: "x".into(),
            cron: Some("1,2,3,4,5,6,7,8 10 5 6 2".into()), // long raw fallback
            ..Default::default()
        }];
        assert_eq!(def_col_layout(&huge, 120, &m.ctx()).sched_w, SCHED_CAP);
    }

    #[test]
    fn def_col_layout_description_fills_then_degrades() {
        // name="pr-review"(9), cron→"Everyday 1:30pm"(15 cells), plus a 2-cell
        // front discovery-marker slot (pr-review has_discovery), description
        // present. Schedule footprint = 15; marker footprint = 2 (no extra
        // COL_GAP of its own — it embeds its own separator space), so the total
        // fixed-column budget is unchanged from when the marker lived inside
        // `sched_w`.
        let m = empty_model_owned();
        let defs = vec![
            DefinitionSummary {
                name: "pr-review".into(),
                cron: Some("30 13 * * *".into()),
                has_discovery: true,
                description: Some("Review an open PR end to end.".into()),
                ..Default::default()
            },
            DefinitionSummary { name: "lint".into(), ..Default::default() },
        ];
        // Wide: marker(2) + name(9) + (2+15 sched) = 28; desc = 120 - 28 - 2 = 90.
        let wide = def_col_layout(&defs, 120, &m.ctx());
        assert_eq!((wide.name_w, wide.sched_w, wide.marker_w), (9, 15, 2));
        assert_eq!(wide.desc_w, 90, "description is the fill remainder");
        // Tighter: the desc fill shrinks toward 0 first (name/schedule/marker kept).
        let mid = def_col_layout(&defs, 40, &m.ctx());
        assert_eq!((mid.name_w, mid.sched_w, mid.marker_w), (9, 15, 2));
        assert_eq!(mid.desc_w, 10, "fill absorbs only what's left: 40 - 28 - 2");
        // Very narrow: name shrinks next (28 > 20 → shrink by 8 → name_w 1), but
        // schedule and marker stay. 9 - (28 - 20) = 1.
        let tiny = def_col_layout(&defs, 20, &m.ctx());
        assert_eq!(tiny.desc_w, 0);
        assert_eq!(tiny.name_w, 1, "name shrinks before schedule/marker, which are kept");
        assert_eq!(tiny.sched_w, 15, "schedule is the trailing kept column");
        assert_eq!(tiny.marker_w, 2, "marker slot is kept");
        // No description anywhere → no fill; schedule keeps its today-position.
        let no_desc = vec![DefinitionSummary {
            name: "lint".into(),
            cron: Some("0 9 * * *".into()),
            ..Default::default()
        }];
        assert_eq!(def_col_layout(&no_desc, 120, &m.ctx()).desc_w, 0);
    }

    #[test]
    fn def_model_text_shows_effective_head_under_active_provider() {
        // Def authored `claude/claude-opus-4.8`, active grok, catalog with both
        // groups → effective head is the re-headed grok group head only
        // (`grok-4.5`), not the full `grok-4.5 → claude-opus-4.8` chain (that
        // stays on the detail config pane).
        let m = resolve_owned("grok");
        let one = DefinitionSummary {
            model: Some(ModelRef::One("claude/claude-opus-4.8".into())),
            ..Default::default()
        };
        assert_eq!(def_model_text(&one, &m.ctx()), "grok-4.5");

        // List that already includes the active provider: stable-partition puts
        // the grok entry first; the column still shows only that head.
        let list = DefinitionSummary {
            model: Some(ModelRef::Many(vec!["claude/claude-opus-4.8".into(), "grok/grok-4.5".into()])),
            ..Default::default()
        };
        assert_eq!(def_model_text(&list, &m.ctx()), "grok-4.5");

        // Same authored ref under active claude → single versioned label.
        let m_claude = resolve_owned("claude");
        assert_eq!(def_model_text(&one, &m_claude.ctx()), "claude-opus-4.8");

        // Absent model + empty defaults + no active → blank (pane-gate).
        let empty = empty_model_owned();
        assert_eq!(def_model_text(&DefinitionSummary::default(), &empty.ctx()), "");

        // Absent model + empty defaults + active grok enabled → group-head prepend.
        assert_eq!(
            def_model_text(&DefinitionSummary::default(), &m.ctx()),
            "grok-4.5"
        );
    }

    #[test]
    fn def_col_layout_model_sizes_and_degrades_before_name() {
        // active=grok: both defs resolve to head `grok-4.5` (8 cells) — the
        // full fallback chain is not shown in the TASKS column.
        // cron→"Everyday 1:30pm"(15 cells), plus a 2-cell front discovery-marker
        // slot (pr-review has_discovery), description present. Schedule
        // footprint = 15; marker footprint = 2.
        let m = resolve_owned("grok");
        let defs = vec![
            DefinitionSummary {
                name: "pr-review".into(),
                model: Some(ModelRef::One("claude/claude-opus-4.8".into())),
                cron: Some("30 13 * * *".into()),
                has_discovery: true,
                description: Some("Review an open PR end to end.".into()),
                ..Default::default()
            },
            DefinitionSummary {
                name: "lint".into(),
                model: Some(ModelRef::One("grok/grok-4.5".into())),
                ..Default::default()
            },
        ];
        let head_w = cw("grok-4.5"); // 8 display cells
        // Wide: model sized to the widest head (8), desc is the fill remainder.
        // used_wo_desc = marker(2) + name(9) + (2+8) + (2+15) = 38; desc = 120 - 38 - 2 = 80.
        let wide = def_col_layout(&defs, 120, &m.ctx());
        assert_eq!((wide.name_w, wide.model_w, wide.sched_w, wide.marker_w), (9, head_w, 15, 2));
        assert_eq!(wide.desc_w, 80, "description is the fill remainder after the model column");
        // Kept: name+model+schedule+marker (38) still fit in 50; the fill takes the rest.
        let kept = def_col_layout(&defs, 50, &m.ctx());
        assert_eq!(kept.model_w, head_w, "model kept while the fixed columns fit");
        assert_eq!(kept.desc_w, 10, "fill absorbs only what's left: 50 - 38 - 2");
        // Tighter (35): the model column drops (before the name shrinks) — 38 > 35.
        // used_wo_desc without model = marker(2) + name(9) + (2+15) = 28.
        let narrow = def_col_layout(&defs, 35, &m.ctx());
        assert_eq!(narrow.model_w, 0);
        assert_eq!(narrow.name_w, 9, "name still fits; schedule/marker kept");
        assert_eq!(narrow.desc_w, 5, "fill absorbs the rest: 35 - 28 - 2");
        assert_eq!(narrow.marker_w, 2, "marker slot survives model drop");
        // Narrowest (25): model gone AND the name shrinks (28 - 25 = 3 → 9-3=6).
        let tightest = def_col_layout(&defs, 25, &m.ctx());
        assert_eq!((tightest.model_w, tightest.name_w), (0, 6));
        // No model anywhere AND empty defaults under active="" → model column omitted.
        let empty = empty_model_owned();
        let no_model = vec![DefinitionSummary { name: "lint".into(), ..Default::default() }];
        assert_eq!(def_col_layout(&no_model, 120, &empty.ctx()).model_w, 0);
    }

    #[test]
    fn def_model_text_and_layout_resolve_short_family_token_refs() {
        // Real-world regression: workspace defs author `claude/sonnet` /
        // `claude/opus` (pre-versioned labels). Without short-form find_model
        // fallback every cell blanks → model_w=0 and the Model column vanishes
        // even on a wide pane. After the fix the column shows the effective
        // head's versioned label (`claude-sonnet-5`) and model_w is non-zero.
        let m = resolve_owned("claude");
        let defs = vec![
            DefinitionSummary {
                name: "sanitize-project".into(),
                model: Some(ModelRef::Many(vec![
                    "claude/sonnet".into(),
                    "grok/grok-4.5".into(),
                ])),
                description: Some("Remove stale worktrees".into()),
                ..Default::default()
            },
            DefinitionSummary {
                name: "squash-merge".into(),
                model: Some(ModelRef::Many(vec![
                    "claude/sonnet".into(),
                    "grok/grok-4.5".into(),
                ])),
                description: Some("Squash a branch".into()),
                ..Default::default()
            },
        ];
        let head = "claude-sonnet-5";
        assert_eq!(def_model_text(&defs[0], &m.ctx()), head);
        assert_eq!(def_model_text(&defs[1], &m.ctx()), head);
        let wide = def_col_layout(&defs, 120, &m.ctx());
        assert_eq!(wide.model_w, cw(head));
        assert!(wide.model_w > 0, "Model column must stay pane-gated on when short refs resolve");

        // Stale default_models alone (no def model:) also keep the column —
        // effective default head under active claude.
        let mut m_defaults = resolve_owned("claude");
        m_defaults.default_models = DefaultModels {
            global: vec!["claude/opus".into(), "grok/grok-4.5".into()],
            projects: vec![],
        };
        let bare = DefinitionSummary {
            name: "adhoc-shaped".into(),
            model: None,
            ..Default::default()
        };
        let defaults_head = "claude-opus-4.8";
        assert_eq!(def_model_text(&bare, &m_defaults.ctx()), defaults_head);
        assert_eq!(
            def_col_layout(&[bare], 80, &m_defaults.ctx()).model_w,
            cw(defaults_head)
        );
    }
