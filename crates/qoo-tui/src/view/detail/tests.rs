    use super::*;
    use crate::app::{DetailKind, ListPane, PaneId, TabUiState};
    use crate::hit::HitTarget;
    use crate::runfiles::RunFiles;
    use crate::selectors::ModelResolveOwned;
    use crate::test_fixtures::fixture_app;
    use crate::view::render as render_frame;
    use ratatui::{Terminal, backend::TestBackend};

    /// A `ModelResolveOwned` for detail tests: providers derived from `catalog`
    /// (all enabled), empty default_models, the given active provider.
    fn owned_ctx(catalog: Vec<CatalogEntry>, active: &str) -> ModelResolveOwned {
        let mut enabled_providers: Vec<String> = Vec::new();
        for e in &catalog {
            if !enabled_providers.contains(&e.provider) {
                enabled_providers.push(e.provider.clone());
            }
        }
        ModelResolveOwned {
            catalog,
            enabled_providers,
            default_models: crate::ipc::types::DefaultModels::default(),
            active_provider: active.into(),
        }
    }

    /// An empty resolve context for detail tests that don't exercise the model row.
    fn empty_owned() -> ModelResolveOwned {
        owned_ctx(Vec::new(), "")
    }

    /// fixture_app focused on the detail pane over the queue selection, with a
    /// 40-line transcript loaded for the running task.
    fn detail_app(sub_tab_run: usize) -> App {
        detail_app_transcript((0..40).map(|i| format!("line {i}")).collect(), sub_tab_run)
    }

    /// Detail pane over the queue selection with a caller-supplied transcript —
    /// the single fixture-builder both the plain and fenced snapshot tests use.
    fn detail_app_transcript(transcript: Vec<String>, sub_tab_run: usize) -> App {
        let mut app = fixture_app();
        app.run_files = Some((
            "01RUN".to_string(),
            Box::new(RunFiles { transcript_tail: transcript, report: vec![], ..Default::default() }),
        ));
        let mut ui = TabUiState::default();
        ui.focus = PaneId::Detail;
        ui.last_list_pane = ListPane::Queue;
        ui.sub_tab[DetailKind::Run as usize] = sub_tab_run;
        app.ui_by_tab.insert("acme".to_string(), ui);
        app
    }

    fn render_at(app: &App, w: u16, h: u16) -> (Terminal<TestBackend>, HitMap) {
        let mut app = app.clone();
        app.size = (w, h);
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        let mut hits = HitMap::new();
        terminal.draw(|frame| hits = render_frame(&mut app, frame)).unwrap();
        (terminal, hits)
    }

    #[test]
    fn snapshot_detail_transcript() {
        // Transcript is now sub-tab index 1 (report is first).
        let (terminal, hits) = render_at(&detail_app(1), 80, 24);
        insta::assert_snapshot!("detail_transcript", terminal.backend());
        assert!(
            hits.iter().any(|(_, t)| *t == HitTarget::SubTab(1)),
            "transcript sub-tab chip is clickable"
        );
    }

    #[test]
    fn snapshot_detail_transcript_fenced() {
        // A ```bash and ```json block: opening fences render as labeled rules,
        // closing fences as plain rules, bodies get syntax accents — the literal
        // backticks never appear.
        let app = detail_app_transcript(
            [
                "Build steps:",
                "```bash",
                "cd ~/proj && make build",
                "cat log.txt | grep error",
                "```",
                "Config:",
                "```json",
                "{\"name\": \"qoo\", \"count\": 3, \"ok\": true}",
                "```",
                "done",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            1,
        );
        let (terminal, _hits) = render_at(&app, 80, 24);
        insta::assert_snapshot!("detail_transcript_fenced", terminal.backend());
    }

    #[test]
    fn snapshot_detail_transcript_wrapped_url() {
        // A long GitHub URL on the final (bottom-anchored) transcript line wraps
        // onto the next row instead of clipping at the pane edge. The preceding
        // short lines push the view past the viewport so this also exercises the
        // scrollbar-column two-pass and the bottom-anchored tail landing on the
        // last WRAPPED segment.
        let mut lines: Vec<String> = (0..24).map(|i| format!("line {i}")).collect();
        lines.push(
            "See https://github.com/justicebid/monorepo/pull/1234/files#diff-0a1b2c3d4e5f done"
                .to_string(),
        );
        let (terminal, _hits) = render_at(&detail_app_transcript(lines, 1), 80, 24);
        insta::assert_snapshot!("detail_transcript_wrapped_url", terminal.backend());
    }

    /// Detail pane focused on the definition config sub-tab: a def summary makes
    /// the Tasks pane selectable (→ Definition context) and a full def in
    /// `full_defs` supplies the config rows. Authored `claude/claude-opus-4.8`
    /// under active=grok exercises the re-headed full chain display, and the
    /// `discovery: —` row the dim placeholder.
    fn detail_def_config_app() -> App {
        use crate::ipc::types::{ArgSpec, DefinitionSummary};
        let mut app = fixture_app();
        app.defs_by_project.insert(
            "acme".to_string(),
            vec![DefinitionSummary {
                repo: "acme".to_string(),
                name: "pr-ready".to_string(),
                scope: "project".to_string(),
                ..Default::default()
            }],
        );
        app.full_defs.insert(
            "acme/pr-ready".to_string(),
            TaskDefinition {
                name: "pr-ready".to_string(),
                repo: "acme".to_string(),
                args: vec![ArgSpec { name: "situation".to_string(), ..Default::default() }],
                dedup: "none".to_string(),
                worktree: "auto".to_string(),
                model: Some("claude/claude-opus-4.8".into()),
                timeout_ms: 1_800_000,
                priority: "normal".to_string(),
                prompt: "do the thing".to_string(),
                ..Default::default()
            },
        );
        let mut ui = TabUiState::default();
        ui.focus = PaneId::Detail;
        ui.last_list_pane = ListPane::Tasks;
        ui.sub_tab[DetailKind::Definition as usize] = 1; // config sub-tab
        app.ui_by_tab.insert("acme".to_string(), ui);
        app
    }

    #[test]
    fn snapshot_detail_definition_config() {
        // The config tab renders aligned key/value rows: keys in accent, the
        // full re-headed model chain with dim arrows, and the empty
        // `discovery` value as a dim `—`.
        let (terminal, hits) = render_at(&detail_def_config_app(), 60, 16);
        insta::assert_snapshot!("detail_definition_config", terminal.backend());
        assert!(
            hits.iter().any(|(_, t)| *t == HitTarget::SubTab(1)),
            "config sub-tab chip is clickable"
        );
    }

    /// Detail pane over a WORKTREES selection: the info block (path/branch/
    /// commit/author/updated/pr as aligned key/value rows, no `state`) followed by
    /// the lane's tasks as queue-style rows — running first (mauve def name), then
    /// queued (fg prompt summary), each with a right-pinned relative age. The
    /// first row renders selected-style (the default detail row cursor).
    fn detail_worktree_app() -> App {
        let mut app = fixture_app();
        let now = app.now_epoch_s;
        if let Some(w) = app
            .snapshot
            .as_mut()
            .and_then(|snap| snap.worktrees.get_mut("acme"))
            .and_then(|wts| wts.iter_mut().find(|w| w.name == "acme.feature"))
        {
            w.last_commit_hash = Some("a1b2c3d".to_string());
            w.last_commit_author = Some("Ian Chiu".to_string());
            w.last_commit_epoch = Some(now - 3 * 86_400);
            w.pr_number = Some(42);
        }
        let mut ui = TabUiState::default();
        ui.focus = PaneId::Detail;
        ui.last_list_pane = ListPane::Worktrees;
        // acme.feature sorts first (it has live task activity), so cursor 0 selects
        // it; its lane carries the running + queued fixture tasks.
        ui.selections[ListPane::Worktrees as usize].cursor = 0;
        app.ui_by_tab.insert("acme".to_string(), ui);
        app
    }

    #[test]
    fn snapshot_detail_worktree_info() {
        let (terminal, _hits) = render_at(&detail_worktree_app(), 80, 24);
        let body = terminal.backend().to_string();
        // Info block keys present; `state` is gone; git facts surfaced.
        assert!(body.contains("commit"), "commit row present");
        assert!(body.contains("a1b2c3d"), "short hash shown");
        assert!(body.contains("#42"), "PR number shown");
        assert!(!body.contains("state"), "state row dropped");
        insta::assert_snapshot!("detail_worktree_info", terminal.backend());
    }

    /// Detail pane over the run `info` sub-tab: a single finished run (so the
    /// queue cursor deterministically selects it) with a fully-populated
    /// `data.json` meta including a def snapshot → all four sections (Run/Timing/
    /// Details/Config) render.
    fn detail_info_app() -> App {
        use crate::ipc::types::TaskStatus;
        let mut app = fixture_app();
        // Anchor `now` just after this run's timestamps so the Timing rows show
        // meaningful relative ages (the shared fixture's `now` predates them).
        app.now_epoch_s = crate::selectors::parse_iso_epoch_s("2026-07-09T12:05:03.000Z");
        if let Some(snap) = app.snapshot.as_mut() {
            let mut t = snap.tasks[0].clone(); // 01RUN base (worktree acme.feature, tui)
            t.status = TaskStatus::Done;
            t.definition = Some("squash-merge".to_string());
            t.created = "2026-07-09T12:00:00.000Z".to_string();
            t.finished_at = Some("2026-07-09T12:03:20.000Z".to_string());
            snap.tasks = vec![t];
            snap.archived_recent = vec![];
            snap.running = vec![];
        }
        app.run_files = Some((
            "01RUN".to_string(),
            Box::new(RunFiles {
                session_id: Some("sess-abc123".to_string()),
                worktree_path: Some("/repos/acme.feature".to_string()),
                meta: Some(RunMeta {
                    started_at: Some("2026-07-09T12:00:05.000Z".to_string()),
                    finished_at: Some("2026-07-09T12:03:20.000Z".to_string()),
                    outcome: Some("done".to_string()),
                    reason: None,
                    exit_code: Some(0),
                    timed_out: false,
                    session_id: Some("sess-abc123".to_string()),
                    model: Some("claude-opus-4-8".to_string()),
                    provider: None,
                    resolved_worktree: Some("/repos/acme.feature".to_string()),
                    resolved_worktree_path: Some("/repos/acme.feature".to_string()),
                    cost_usd: Some(0.42),
                    turns: Some(37),
                    duration_ms: Some(195_000),
                    input_tokens: Some(199_057),
                    output_tokens: Some(22_341),
                    definition: Some(TaskDefinition {
                        name: "squash-merge".to_string(),
                        repo: "acme".to_string(),
                        description: Some("Squash-merge the branch.".to_string()),
                        dedup: "none".to_string(),
                        worktree: "auto".to_string(),
                        model: Some("claude/claude-opus-4.8".into()),
                        timeout_ms: 1_800_000,
                        priority: "normal".to_string(),
                        cron: Some("30 13 * * *".to_string()),
                        ..Default::default()
                    }),
                }),
                ..Default::default()
            }),
        ));
        let mut ui = TabUiState::default();
        ui.focus = PaneId::Detail;
        ui.last_list_pane = ListPane::Queue;
        ui.sub_tab[DetailKind::Run as usize] = 3; // info sub-tab
        app.ui_by_tab.insert("acme".to_string(), ui);
        app
    }

    #[test]
    fn snapshot_detail_run_info() {
        // Taller viewport so all four sections fit on one screen (the info panel
        // runs ~26 lines).
        let (terminal, hits) = render_at(&detail_info_app(), 80, 34);
        let body = terminal.backend().to_string();
        // All four sections present; def name surfaced; info chip clickable.
        for header in ["Run", "Timing", "Details", "Config"] {
            assert!(body.contains(header), "{header} section header present");
        }
        assert!(body.contains("squash-merge"), "definition name shown");
        assert!(body.contains("$0.42"), "cost shown");
        assert!(body.contains("199k in / 22k out"), "tokens row shown next to cost");
        assert!(
            hits.iter().any(|(_, t)| *t == HitTarget::SubTab(3)),
            "info sub-tab chip is clickable"
        );
        insta::assert_snapshot!("detail_run_info", terminal.backend());
    }

    /// Detail pane over the run `report` sub-tab (index 0, the default) on a
    /// `failed` run whose result text hit Claude's session limit — the exact
    /// `report.md` shape `finishRun` writes for the bug report this feature was
    /// built for. Verifies the `## Stats` bullets render as aligned Config rows
    /// (model gold-colored) instead of a plain markdown bullet list.
    fn detail_report_app() -> App {
        use crate::ipc::types::TaskStatus;
        let mut app = fixture_app();
        app.now_epoch_s = crate::selectors::parse_iso_epoch_s("2026-07-12T23:30:29.000Z");
        if let Some(snap) = app.snapshot.as_mut() {
            let mut t = snap.tasks[0].clone(); // 01RUN base (worktree acme.feature, tui)
            t.status = TaskStatus::Failed;
            t.error = Some("session limit".to_string());
            t.created = "2026-07-12T23:11:31.000Z".to_string();
            t.finished_at = Some("2026-07-12T23:30:29.000Z".to_string());
            snap.tasks = vec![t];
            snap.archived_recent = vec![];
            snap.running = vec![];
        }
        app.run_files = Some((
            "01RUN".to_string(),
            Box::new(RunFiles {
                report: [
                    "# Result",
                    "",
                    "You've hit your session limit · resets 1pm (America/Chicago)",
                    "",
                    "## Stats",
                    "- outcome: failed (exit code 1)",
                    "- model: claude-fable-5",
                    "- cost: $31.07151099999998",
                    "- turns: 40",
                    "- duration: 1129s",
                    "",
                ]
                .iter()
                .map(|s| s.to_string())
                .collect(),
                meta: Some(RunMeta {
                    outcome: Some("failed".to_string()),
                    reason: Some("exit code 1".to_string()),
                    exit_code: Some(1),
                    model: Some("claude-fable-5".to_string()),
                    cost_usd: Some(31.07151099999998),
                    turns: Some(40),
                    duration_ms: Some(1_129_000),
                    ..Default::default()
                }),
                ..Default::default()
            }),
        ));
        let mut ui = TabUiState::default();
        ui.focus = PaneId::Detail;
        ui.last_list_pane = ListPane::Queue;
        ui.sub_tab[DetailKind::Run as usize] = 0; // report sub-tab (default)
        app.ui_by_tab.insert("acme".to_string(), ui);
        app
    }

    #[test]
    fn snapshot_detail_run_report_stats() {
        let (terminal, _hits) = render_at(&detail_report_app(), 80, 24);
        let body = terminal.backend().to_string();
        assert!(body.contains("claude-fable-5"), "model value shown");
        assert!(body.contains("session limit"), "the raw result text is untouched markdown");
        assert!(!body.contains("- outcome"), "literal bullet dash replaced");
        assert!(!body.contains("- model"), "literal bullet dash replaced");
        insta::assert_snapshot!("detail_run_report_stats", terminal.backend());
    }

    /// Minimal live task for the `run_info_lines` unit tests.
    fn info_task(status: TaskStatus) -> TaskInstance {
        TaskInstance {
            id: "01RUN".to_string(),
            status,
            definition: Some("squash-merge".to_string()),
            created: "2026-07-09T12:00:00.000Z".to_string(),
            ..Default::default()
        }
    }

    // 2026-07-09T12:05:03Z, matching fixture_app; tz is arbitrary for value checks.
    const INFO_NOW: u64 = 1_752_062_703;
    const INFO_TZ: i32 = -18_000;

    #[test]
    fn run_info_lines_empty_meta() {
        // No run record yet: sections still render, but unfinished fields dash out
        // and there is no Config section (no def snapshot) and no error/reason row.
        let task = info_task(TaskStatus::Queued);
        let (lines, ctxs) = run_info_lines(&task, &RunMeta::default(), &[], INFO_NOW, INFO_TZ);
        assert!(lines.iter().any(|l| l == "Run"));
        assert!(lines.iter().any(|l| l == "Timing"));
        assert!(lines.iter().any(|l| l == "Details"));
        assert!(!lines.iter().any(|l| l == "Config"), "no Config without a def snapshot");
        assert_eq!(ctxs.iter().filter(|c| matches!(c, LineCtx::Header)).count(), 3);
        assert!(lines.iter().any(|l| l.contains("01RUN")), "id row");
        for key in ["started", "finished", "duration", "exit code", "cost", "tokens", "turns"] {
            assert!(
                lines.iter().any(|l| l.trim_start().starts_with(key) && l.contains(EM_DASH)),
                "{key} dashes out"
            );
        }
        assert!(!lines.iter().any(|l| l.trim_start().starts_with("error")));
        assert!(!lines.iter().any(|l| l.trim_start().starts_with("reason")));
        assert!(!lines.iter().any(|l| l.trim_start().starts_with("timed out")));
    }

    #[test]
    fn run_info_lines_finished_run() {
        let task = info_task(TaskStatus::Done);
        let meta = RunMeta {
            started_at: Some("2026-07-09T12:00:05.000Z".to_string()),
            finished_at: Some("2026-07-09T12:03:20.000Z".to_string()),
            outcome: Some("done".to_string()),
            exit_code: Some(0),
            session_id: Some("sess-abc123".to_string()),
            model: Some("claude-opus-4-8".to_string()),
            resolved_worktree: Some("/repos/acme.feature".to_string()),
            resolved_worktree_path: Some("/repos/acme.feature".to_string()),
            cost_usd: Some(0.42),
            turns: Some(37),
            duration_ms: Some(195_000),
            input_tokens: Some(199_057),
            output_tokens: Some(22_341),
            definition: Some(TaskDefinition {
                worktree: "auto".to_string(),
                dedup: "none".to_string(),
                timeout_ms: 1_800_000,
                priority: "normal".to_string(),
                description: Some("Squash-merge the branch.".to_string()),
                cron: Some("30 13 * * *".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let (lines, _) = run_info_lines(&task, &meta, &[], INFO_NOW, INFO_TZ);
        assert!(lines.iter().any(|l| l == "Config"), "Config section present with a def");
        assert!(lines.iter().any(|l| l.contains("$0.42")), "cost shown");
        assert!(
            lines.iter().any(|l| l.trim_start().starts_with("tokens") && l.contains("199k in / 22k out")),
            "tokens row shown next to cost: {lines:?}"
        );
        assert!(lines.iter().any(|l| l.trim_start().starts_with("turns") && l.contains("37")));
        assert!(lines.iter().any(|l| l.trim_start().starts_with("duration") && l.contains("3m")));
        assert!(lines.iter().any(|l| l.contains("Squash-merge the branch.")), "description row");
        assert!(lines.iter().any(|l| l.trim_start().starts_with("cron") && l.contains("30 13")));
        assert!(!lines.iter().any(|l| l.trim_start().starts_with("timed out")), "false → no row");
    }

    #[test]
    fn run_info_lines_tokens_row_dashes_when_absent_and_handles_partial_presence() {
        let task = info_task(TaskStatus::Done);
        // Neither side reported (a claude/codex run with no usage object at
        // all, or an old run record predating this field) → the whole row
        // dashes out, same as `cost` does — not `"— in / — out"`.
        let no_usage = RunMeta::default();
        let (lines, _) = run_info_lines(&task, &no_usage, &[], INFO_NOW, INFO_TZ);
        let tokens_line = lines
            .iter()
            .find(|l| l.trim_start().starts_with("tokens"))
            .expect("tokens row present");
        assert!(tokens_line.trim_end().ends_with(EM_DASH), "bare dash: {tokens_line:?}");
        assert!(!tokens_line.contains("in /"), "bare dash, not a formatted pair: {tokens_line:?}");

        // Only one side reported: the present side still renders, the missing
        // side dashes out independently — the two counts don't gate each other.
        let input_only =
            RunMeta { input_tokens: Some(500), output_tokens: None, ..Default::default() };
        let (lines2, _) = run_info_lines(&task, &input_only, &[], INFO_NOW, INFO_TZ);
        assert!(
            lines2
                .iter()
                .any(|l| l.trim_start().starts_with("tokens") && l.contains(&format!("500 in / {EM_DASH} out"))),
            "input-only still renders, output side dashes: {lines2:?}"
        );
    }

    /// The `model` row shows `label (provider)` when the recorded id resolves
    /// against the catalog, and appends ` · <raw id>` only when raw id ≠ label
    /// (so pins that differ from the short/versioned label still surface the
    /// true CLI id). Falls back to the bare raw id alone when the id isn't in
    /// the catalog (unknown provider, or a stale/removed catalog entry — the
    /// recorded id is still ground truth either way).
    #[test]
    fn run_info_lines_model_row_resolves_via_catalog_or_falls_back_to_raw_id() {
        let task = info_task(TaskStatus::Done);
        // id ≠ label → keep the ` · <raw id>` disambiguator.
        let catalog = vec![
            CatalogEntry {
                provider: "claude".to_string(),
                id: "claude-opus-4-8".to_string(),
                label: "claude-opus-4.8".to_string(),
                hidden: false,
            },
            // id == label (versioned label) → omit the redundant suffix.
            CatalogEntry {
                provider: "claude".to_string(),
                id: "claude-sonnet-5".to_string(),
                label: "claude-sonnet-5".to_string(),
                hidden: false,
            },
            CatalogEntry {
                provider: "grok".to_string(),
                id: "grok-4.5".to_string(),
                label: "grok-4.5".to_string(),
                hidden: false,
            },
        ];
        let known = RunMeta { model: Some("claude-opus-4-8".to_string()), ..Default::default() };
        let (lines, _) = run_info_lines(&task, &known, &catalog, INFO_NOW, INFO_TZ);
        assert!(
            lines
                .iter()
                .any(|l| l.trim_start().starts_with("model")
                    && l.contains("claude-opus-4.8 (claude) · claude-opus-4-8")),
            "id ≠ label keeps label (provider) · raw id: {lines:?}"
        );

        // Versioned label equals the CLI id → just `label (provider)`.
        let equal = RunMeta { model: Some("claude-sonnet-5".to_string()), ..Default::default() };
        let (lines, _) = run_info_lines(&task, &equal, &catalog, INFO_NOW, INFO_TZ);
        assert!(
            lines.iter().any(|l| {
                let trimmed = l.trim_start();
                trimmed.starts_with("model")
                    && l.contains("claude-sonnet-5 (claude)")
                    && !l.contains('·')
            }),
            "id == label drops the · raw id suffix: {lines:?}"
        );
        let grok = RunMeta { model: Some("grok-4.5".to_string()), ..Default::default() };
        let (lines, _) = run_info_lines(&task, &grok, &catalog, INFO_NOW, INFO_TZ);
        assert!(
            lines.iter().any(|l| {
                let trimmed = l.trim_start();
                trimmed.starts_with("model") && l.contains("grok-4.5 (grok)") && !l.contains('·')
            }),
            "grok equal id/label also drops · raw id: {lines:?}"
        );

        // A stale/unknown id (not in the catalog) falls back to the bare id —
        // no `label (provider) · ` decoration.
        let unknown = RunMeta { model: Some("claude-legacy-1".to_string()), ..Default::default() };
        let (lines, _) = run_info_lines(&task, &unknown, &catalog, INFO_NOW, INFO_TZ);
        assert!(
            lines.iter().any(|l| {
                let trimmed = l.trim_start();
                trimmed.starts_with("model") && trimmed.trim_end().ends_with("claude-legacy-1") && !l.contains('(')
            }),
            "unknown id falls back to the raw id alone: {lines:?}"
        );
    }

    #[test]
    fn run_info_lines_failed_run_with_reason() {
        let mut task = info_task(TaskStatus::Failed);
        task.error = None; // no live error → falls back to the run record's reason
        let meta = RunMeta {
            outcome: Some("failed".to_string()),
            reason: Some("timed out waiting".to_string()),
            exit_code: Some(1),
            timed_out: true,
            ..Default::default()
        };
        let (lines, _) = run_info_lines(&task, &meta, &[], INFO_NOW, INFO_TZ);
        assert!(
            lines.iter().any(|l| l.trim_start().starts_with("reason") && l.contains("timed out waiting"))
        );
        assert!(!lines.iter().any(|l| l.trim_start().starts_with("error")), "no live error → reason used");
        assert!(lines.iter().any(|l| l.trim_start().starts_with("timed out") && l.contains("yes")));
        // Live error preempts the run record's reason.
        task.error = Some("boom".to_string());
        let (lines2, _) = run_info_lines(&task, &meta, &[], INFO_NOW, INFO_TZ);
        assert!(lines2.iter().any(|l| l.trim_start().starts_with("error") && l.contains("boom")));
        assert!(!lines2.iter().any(|l| l.trim_start().starts_with("reason")), "error preempts reason");
    }

    #[test]
    fn stats_rows_formats_outcome_reason_and_dashes_missing_fields() {
        let meta = RunMeta {
            outcome: Some("failed".to_string()),
            reason: Some("exit code 1".to_string()),
            model: Some("claude-fable-5".to_string()),
            cost_usd: Some(31.07151099999998),
            turns: Some(40),
            duration_ms: Some(1_129_000),
            ..Default::default()
        };
        let rows = stats_rows(&meta);
        assert_eq!(
            rows,
            vec![
                ("outcome", "failed (exit code 1)".to_string()),
                ("model", "claude-fable-5".to_string()),
                ("cost", "$31.07151099999998".to_string()),
                ("turns", "40".to_string()),
                ("duration", "18m".to_string()), // format_duration, not raw seconds
            ]
        );
    }

    #[test]
    fn stats_rows_dashes_out_an_empty_meta() {
        let rows = stats_rows(&RunMeta::default());
        assert!(rows.iter().all(|(_, v)| v == EM_DASH), "every field absent → dash: {rows:?}");
    }

    #[test]
    fn report_content_replaces_stats_bullets_with_aligned_config_rows() {
        // A real report.md shape from `run-store.ts`'s `finishRun`.
        let report: Vec<String> = [
            "# Result",
            "",
            "You've hit your session limit · resets 1pm (America/Chicago)",
            "",
            "## Stats",
            "- outcome: failed (exit code 1)",
            "- model: claude-fable-5",
            "- cost: $31.07151099999998",
            "- turns: 40",
            "- duration: 1129s",
            "",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let meta = RunMeta {
            outcome: Some("failed".to_string()),
            reason: Some("exit code 1".to_string()),
            model: Some("claude-fable-5".to_string()),
            cost_usd: Some(31.07151099999998),
            turns: Some(40),
            duration_ms: Some(1_129_000),
            ..Default::default()
        };
        let (lines, ctxs) = report_content(report, Some(&meta));
        // Everything above the heading is untouched markdown.
        assert_eq!(lines[0], "# Result");
        assert_eq!(lines[2], "You've hit your session limit · resets 1pm (America/Chicago)");
        assert_eq!(lines[4], "## Stats");
        assert_eq!(ctxs[4], LineCtx::Text, "heading stays plain markdown (is_heading bolds it)");
        // The 5 literal `- key: value` bullets collapse to 5 aligned Config rows —
        // no leftover `- ` bullet text, and each carries LineCtx::Config.
        let bullet_rows = &lines[5..10];
        assert!(bullet_rows.iter().all(|l| !l.starts_with("- ")), "bullets replaced: {bullet_rows:?}");
        assert!(bullet_rows[1].contains("claude-fable-5"), "model value present: {bullet_rows:?}");
        for ctx in &ctxs[5..10] {
            assert!(matches!(ctx, LineCtx::Config { .. }), "stats rows use Config styling: {ctx:?}");
        }
        // Trailing blank line after the (now shorter) stats block is preserved.
        assert_eq!(lines.last().map(String::as_str), Some(""));
    }

    #[test]
    fn report_content_falls_back_to_plain_markdown_without_meta_or_heading() {
        let report = vec!["# Result".to_string(), "".to_string(), "still running".to_string()];
        // No meta at all (adhoc run mid-flight, or an old daemon).
        let (lines, _) = report_content(report.clone(), None);
        assert_eq!(lines, report);
        // Meta present but report.md predates the `## Stats` heading (or the run
        // hasn't reached `finishRun` yet) — leave the text alone rather than
        // silently dropping content that doesn't match the expected shape.
        let (lines2, _) = report_content(report.clone(), Some(&RunMeta::default()));
        assert_eq!(lines2, report);
    }

    #[test]
    fn detail_worktree_pr_is_an_osc8_link_only_with_a_url() {
        // The base fixture sets pr_number but no pr_url → the `#42` value is plain
        // text: no OSC 8 opener anywhere in the rendered buffer.
        let (terminal, _hits) = render_at(&detail_worktree_app(), 80, 24);
        let buf = terminal.backend().buffer();
        let has_opener = |buf: &ratatui::buffer::Buffer| {
            (buf.area.y..buf.area.bottom()).any(|y| {
                (buf.area.x..buf.area.right()).any(|x| buf[(x, y)].symbol().contains("\x1b]8;;"))
            })
        };
        assert!(!has_opener(buf), "pr number without a url gets no OSC 8 link");

        // Add the url: the `#42` value is wrapped in an OSC 8 terminal hyperlink
        // carrying it (folded into the first glyph cell), and reads as a link
        // (underlined). The terminal — not the app — handles cmd+click.
        let mut app = detail_worktree_app();
        let url = "https://github.com/acme/acme/pull/42".to_string();
        if let Some(w) = app
            .snapshot
            .as_mut()
            .and_then(|snap| snap.worktrees.get_mut("acme"))
            .and_then(|wts| wts.iter_mut().find(|w| w.name == "acme.feature"))
        {
            w.pr_url = Some(url.clone());
        }
        let (terminal, _hits) = render_at(&app, 80, 24);
        let buf = terminal.backend().buffer();
        let opener = format!("\x1b]8;;{url}\x1b\\");
        let mut found: Option<(u16, u16)> = None;
        let mut count = 0usize;
        for y in buf.area.y..buf.area.bottom() {
            for x in buf.area.x..buf.area.right() {
                if buf[(x, y)].symbol().contains(&opener) {
                    count += 1;
                    found = Some((x, y));
                }
            }
        }
        assert_eq!(count, 1, "exactly one OSC 8 link cell");
        let (x, y) = found.expect("OSC 8 link cell present");
        let sym = buf[(x, y)].symbol();
        assert!(sym.contains("#42"), "the wrapped glyphs are #42: {sym:?}");
        assert!(sym.ends_with("\x1b]8;;\x1b\\"), "closer present: {sym:?}");
        assert!(
            buf[(x, y)].modifier.contains(Modifier::UNDERLINED),
            "the #42 link cell is underlined"
        );
    }

    #[test]
    fn compact_count_boundary_cases() {
        // Below 1000: the bare number, no suffix.
        assert_eq!(compact_count(0), "0");
        assert_eq!(compact_count(1), "1");
        assert_eq!(compact_count(999), "999");
        // [1000, 1_000_000): rounded to the nearest thousand, `k` suffix.
        assert_eq!(compact_count(1000), "1k");
        assert_eq!(compact_count(1500), "2k"); // round-half-up
        assert_eq!(compact_count(22_341), "22k");
        assert_eq!(compact_count(199_057), "199k");
        assert_eq!(compact_count(999_499), "999k");
        assert_eq!(compact_count(999_500), "1000k"); // rounds up, stays in k range
        // >= 1_000_000: one decimal place, `M` suffix.
        assert_eq!(compact_count(1_000_000), "1.0M");
        assert_eq!(compact_count(1_200_000), "1.2M");
        assert_eq!(compact_count(12_340_000), "12.3M");
    }

    #[test]
    fn format_duration_human_units() {
        assert_eq!(format_duration(0), "0s");
        assert_eq!(format_duration(30_000), "30s");
        assert_eq!(format_duration(59_000), "59s");
        // Whole minutes truncate seconds.
        assert_eq!(format_duration(90_000), "1m");
        assert_eq!(format_duration(1_800_000), "30m");
        assert_eq!(format_duration(2_700_000), "45m");
        // Hours, whole and mixed.
        assert_eq!(format_duration(3_600_000), "1h");
        assert_eq!(format_duration(5_400_000), "1h 30m");
        assert_eq!(format_duration(7_200_000), "2h");
    }

    #[test]
    fn config_view_aligns_keys_and_shows_model_refs() {
        use crate::ipc::types::{CatalogEntry, ModelRef};
        let entry = |provider: &str, id: &str, label: &str| CatalogEntry {
            provider: provider.into(),
            id: id.into(),
            label: label.into(),
            hidden: false,
        };
        // Unique labels → each ref renders label-only (provider prefix dropped).
        let owned = owned_ctx(
            vec![
                entry("claude", "claude-opus-4-8", "claude-opus-4.8"),
                entry("grok", "grok-4.5", "grok-4.5"),
            ],
            "claude",
        );
        let mut def = TaskDefinition {
            name: "pr-ready".to_string(),
            model: Some(ModelRef::Many(vec!["claude/claude-opus-4.8".into(), "grok/grok-4.5".into()])),
            timeout_ms: 1_800_000,
            worktree: "auto".to_string(),
            dedup: "none".to_string(),
            priority: "normal".to_string(),
            ..Default::default()
        };
        let (lines, key_col) = config_view(&def, &owned);
        // Longest key is "purge_after_days" (16) + CONFIG_KEY_GAP.
        assert_eq!(key_col, 16 + CONFIG_KEY_GAP);
        // Every line's key column is padded to the same width.
        for line in &lines {
            assert!(line.chars().count() >= key_col, "{line:?} shorter than key column");
        }
        // Name is the first attribute so the def identity is always visible.
        assert!(
            lines[0].starts_with("name") && lines[0].contains("pr-ready"),
            "name is the first config row: {:?}",
            lines[0]
        );
        // Full resolved chain (active=claude, both in the pool) joined by ` → `;
        // label-only refs, versioned catalog labels. Keys are left-padded.
        let model_line = lines.iter().find(|l| l.starts_with("model")).expect("model row");
        assert!(model_line.contains("claude-opus-4.8 → grok-4.5"), "{model_line}");
        assert!(lines.iter().any(|l| l.starts_with("timeout") && l.contains("30m")));
        assert!(lines.iter().any(|l| l.starts_with("discovery") && l.contains(EM_DASH)));
        assert!(lines.iter().any(|l| l.starts_with("on_done") && l.contains("stay")));
        assert!(lines.iter().any(|l| l.starts_with("purge_after_days") && l.contains(EM_DASH)));
        // A single-entry resolved chain is just that label (no arrow).
        def.model = Some(ModelRef::One("claude/claude-opus-4.8".into()));
        let (lines, _) = config_view(&def, &owned);
        let model_line = lines.iter().find(|l| l.starts_with("model")).expect("model row");
        assert!(
            model_line.contains("claude-opus-4.8") && !model_line.contains("→"),
            "{model_line}"
        );
        // No `model:` + catalog/active present → resolves defaults/group-head
        // under the active provider (claude group head here).
        def.model = None;
        let (lines, _) = config_view(&def, &owned);
        assert!(
            lines.iter().any(|l| l.starts_with("model") && l.contains("claude-opus-4.8")),
            "null model resolves under active provider: {lines:?}"
        );
        // No model + empty resolve context → dash.
        let (lines, _) = config_view(&def, &empty_owned());
        assert!(lines.iter().any(|l| l.starts_with("model") && l.contains(EM_DASH)));
    }

    #[test]
    fn config_view_shows_resolved_chain_reheaded_under_active_provider() {
        use crate::ipc::types::{CatalogEntry, ModelRef};
        let entry = |provider: &str, id: &str, label: &str| CatalogEntry {
            provider: provider.into(),
            id: id.into(),
            label: label.into(),
            hidden: false,
        };
        // Def authors claude/claude-opus-4.8, but the operator is on grok → the
        // full resolved chain re-heads onto grok: `grok-4.5 → claude-opus-4.8`.
        // Not an authored remap (`opus → grok-4.5`) and not head-only.
        let owned = owned_ctx(
            vec![
                entry("claude", "claude-opus-4-8", "claude-opus-4.8"),
                entry("grok", "grok-4.5", "grok-4.5"),
            ],
            "grok",
        );
        let def = TaskDefinition {
            model: Some(ModelRef::One("claude/claude-opus-4.8".into())),
            ..Default::default()
        };
        let (lines, _) = config_view(&def, &owned);
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("model") && l.contains("grok-4.5 → claude-opus-4.8")),
            "full re-headed chain: {lines:?}"
        );
    }

    #[test]
    fn definition_discovery_tab_shows_command_and_item_key() {
        let mut def = crate::ipc::types::TaskDefinition::default();
        def.discovery = Some(crate::ipc::types::Discovery {
            command: "gh pr list --json url\njq '.[]'".to_string(),
            item_key: "{{url}}".to_string(),
        });
        let ctx = DetailContext::Definition { repo: "p".into(), name: "pr-review".into() };
        let (lines, _, placeholder) = content_for(&ctx, 2, Some(&def), None, 0, &empty_owned(), 0, 0);
        assert_eq!(placeholder, "");
        assert!(lines.iter().any(|l| l == "gh pr list --json url"), "lines: {lines:?}");
        assert!(lines.iter().any(|l| l == "jq '.[]'"), "multi-line command preserved");
        assert!(lines.iter().any(|l| l == "item key: {{url}}"), "lines: {lines:?}");
    }

    #[test]
    fn definition_discovery_tab_placeholder_when_no_discovery() {
        let def = crate::ipc::types::TaskDefinition::default();
        let ctx = DetailContext::Definition { repo: "p".into(), name: "lint".into() };
        let (lines, _, placeholder) = content_for(&ctx, 2, Some(&def), None, 0, &empty_owned(), 0, 0);
        assert!(lines.is_empty());
        assert_eq!(placeholder, "(no discovery)");
    }

    #[test]
    fn transcript_arm_respects_starts_in_fence_flag() {
        // A tail window that opened mid-fence: with the flag set, a `### heading`
        // line inside the window styles as Fenced (plain), not as a markdown
        // Header/Text — the fix that keeps mid-fence tails from inverting.
        let files = RunFiles {
            transcript_tail: vec!["make test".into(), "### Tool: Bash".into()],
            transcript_starts_in_fence: true,
            ..Default::default()
        };
        let ctx = DetailContext::Run { task: crate::ipc::types::TaskInstance::default() };
        let (_lines, ctxs, _) = content_for(&ctx, 1, None, Some(&files), 0, &empty_owned(), 0, 0);
        assert_eq!(ctxs[0], LineCtx::Fenced { lang: String::new() });
        assert_eq!(ctxs[1], LineCtx::Fenced { lang: String::new() });

        // Same lines WITHOUT the flag: the `###` line is a plain Text line.
        let files_flat = RunFiles { transcript_starts_in_fence: false, ..files };
        let (_lines, ctxs_flat, _) = content_for(&ctx, 1, None, Some(&files_flat), 0, &empty_owned(), 0, 0);
        assert_eq!(ctxs_flat[0], LineCtx::Text);
        assert_eq!(ctxs_flat[1], LineCtx::Text);
    }

    #[test]
    fn wrap_for_viewport_reserves_scrollbar_gutter_and_track_on_overflow() {
        // Four 10-cell lines into a width-10, height-3 viewport fit at full width
        // (4 display lines) but 4 > 3 forces a scrollbar, so the second pass
        // re-wraps at width 8 (margin 1 + track 1) — each 10-cell line splits
        // into two segments (8+2) → 8 display lines.
        let lines: Vec<String> = vec!["abcdefghij".into(); 4];
        let ctxs = fence_states(&lines);
        let (display, has_scrollbar, text_width) = wrap_for_viewport(&lines, &ctxs, 10, 3);
        assert!(has_scrollbar);
        assert_eq!(text_width, 8);
        assert_eq!(display.len(), 8);
    }

    #[test]
    fn wrap_for_viewport_keeps_full_width_when_it_fits() {
        let lines: Vec<String> = vec!["abcdefghij".into(); 2];
        let ctxs = fence_states(&lines);
        let (display, has_scrollbar, text_width) = wrap_for_viewport(&lines, &ctxs, 10, 10);
        assert!(!has_scrollbar);
        assert_eq!(text_width, 10);
        assert_eq!(display.len(), 2);
    }

    #[test]
    fn wrapping_counts_display_lines_for_scroll_ceiling() {
        // One 2000-char logical line wraps into many display lines. The render-fed
        // ceiling + wrapped length count DISPLAY lines — a single unwrapped logical
        // line would have left `detail_max_scroll` at 0 (nothing to scroll).
        // Render the same instance (not `render_at`, which clones) so the
        // interior-mutability feedback cells are observable afterwards.
        let mut app = detail_app_transcript(vec!["x".repeat(2000)], 1);
        app.size = (80, 24);
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| {
            render_frame(&mut app, frame);
        }).unwrap();
        let wrapped = app.detail_wrapped_len.get();
        assert!(wrapped > 1, "the long line wrapped into many display lines");
        assert!(
            app.detail_max_scroll.get() > 0,
            "wrapping opened scroll room a single logical line would not have"
        );
        assert!(app.detail_max_scroll.get() < wrapped, "ceiling stays below the wrapped total");
    }

    // ---- text selection ----------------------------------------------------

    use crate::app::{DetailPoint, DetailSelection};

    fn sel(a: (usize, usize), b: (usize, usize)) -> DetailSelection {
        DetailSelection {
            anchor: DetailPoint { line: a.0, cell: a.1 },
            cursor: DetailPoint { line: b.0, cell: b.1 },
        }
    }

    #[test]
    fn extract_selection_single_line_inclusive() {
        let lines = vec!["hello world".to_string()];
        assert_eq!(extract_selection(&lines, &sel((0, 0), (0, 4))), "hello");
        // Reversed anchor/cursor orders the same.
        assert_eq!(extract_selection(&lines, &sel((0, 4), (0, 0))), "hello");
    }

    #[test]
    fn extract_selection_spans_multiple_lines_with_newlines() {
        let lines = vec![
            "first line".to_string(),
            "middle".to_string(),
            "last one".to_string(),
        ];
        // From cell 6 on line 0 → cell 3 on line 2: "line" + whole middle + "last".
        let got = extract_selection(&lines, &sel((0, 6), (2, 3)));
        assert_eq!(got, "line\nmiddle\nlast");
    }

    #[test]
    fn extract_selection_multiwidth_and_empty_line() {
        // A CJK line (each char 2 cells) plus an empty line in the range.
        let lines = vec!["中文字".to_string(), String::new(), "tail".to_string()];
        // line0 cell2..end (字文... actually cells: 中[0,1] 文[2,3] 字[4,5]) → from
        // cell 2 = "文字"; empty middle → ""; line2 to cell1 = "ta".
        let got = extract_selection(&lines, &sel((0, 2), (2, 1)));
        assert_eq!(got, "文字\n\nta");
    }

    #[test]
    fn extract_selection_clamps_shrunk_content() {
        // A selection referencing lines past a shrunk transcript slices safely.
        let lines = vec!["only".to_string()];
        assert_eq!(extract_selection(&lines, &sel((0, 0), (9, 99))), "only");
        assert_eq!(extract_selection(&[], &sel((0, 0), (0, 3))), "");
    }

    #[test]
    fn patch_line_cols_highlights_only_the_selected_columns() {
        let p = Palette::default();
        let selection = p.selection();
        // A single plain span "hello world"; highlight cells [0,4] = "hello".
        let line = Line::from(vec![Span::raw("hello world")]);
        let out = patch_line_cols(&line, 0, 4, selection);
        let parts: Vec<(String, Style)> =
            out.spans.iter().map(|s| (s.content.to_string(), s.style)).collect();
        assert_eq!(parts[0].0, "hello");
        assert_eq!(parts[0].1, Style::default().patch(selection));
        // The remainder keeps its (plain) style.
        let rest: String = parts[1..].iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(rest, " world");
        assert!(parts[1..].iter().all(|(_, st)| *st == Style::default()));
    }

    #[test]
    fn patch_line_cols_to_end_of_line_with_max_sentinel() {
        let p = Palette::default();
        let selection = p.selection();
        let line = Line::from(vec![Span::raw("abcde")]);
        let out = patch_line_cols(&line, 2, usize::MAX, selection);
        // Cells 0..1 plain, 2..end selected.
        let sel_text: String = out
            .spans
            .iter()
            .filter(|s| s.style == Style::default().patch(selection))
            .map(|s| s.content.to_string())
            .collect();
        assert_eq!(sel_text, "cde");
    }

    #[test]
    fn out_of_range_sub_tab_clamps_into_range() {
        // sub_tab 9 on a Run context clamps to the last valid index (3 = info),
        // NOT the report the `_` fall-through would hit with an unclamped index.
        // Info always renders live-task sections (no meta needed), so the
        // section headers prove the clamp landed on info — distinct from the
        // report placeholder the unclamped `_` fall-through would show.
        let (terminal, _hits) = render_at(&detail_app(9), 80, 24);
        let body = terminal.backend().to_string();
        assert!(body.contains("Run"), "clamped to the info sub-tab (Run section)");
        assert!(body.contains("Timing"), "info Timing section present");
        assert!(!body.contains("(no report yet)"), "clamped index is not the report fall-through");
    }

    /// Queued task with no run dir yet: the info tab still shows identity/
    /// status/created from the live task (not an empty placeholder).
    #[test]
    fn snapshot_detail_run_info_queued_no_meta() {
        use crate::ipc::types::TaskStatus;
        let mut app = fixture_app();
        app.now_epoch_s = crate::selectors::parse_iso_epoch_s("2026-07-09T12:05:03.000Z");
        if let Some(snap) = app.snapshot.as_mut() {
            let mut t = snap.tasks[0].clone();
            t.status = TaskStatus::Queued;
            t.definition = Some("pr-fix-ci-conflicts".to_string());
            t.created = "2026-07-09T12:00:00.000Z".to_string();
            t.model = Some(crate::ipc::types::ModelRef::One("claude-opus-4-8".to_string()));
            t.finished_at = None;
            t.started_at = None;
            snap.tasks = vec![t];
            snap.archived_recent = vec![];
            snap.running = vec![];
        }
        // No run_files at all — the scheduling/queued case.
        app.run_files = None;
        let mut ui = TabUiState::default();
        ui.focus = PaneId::Detail;
        ui.last_list_pane = ListPane::Queue;
        ui.sub_tab[DetailKind::Run as usize] = 3; // info
        app.ui_by_tab.insert("acme".to_string(), ui);

        let (terminal, _hits) = render_at(&app, 80, 28);
        let body = terminal.backend().to_string();
        for header in ["Run", "Timing", "Details"] {
            assert!(body.contains(header), "{header} section present while scheduling");
        }
        assert!(body.contains("pr-fix-ci-conflicts"), "definition from live task");
        assert!(body.contains("queued"), "status from live task");
        assert!(!body.contains("(no run recorded yet)"), "no empty placeholder");
        assert!(!body.contains("Config"), "no Config without a def snapshot");
        insta::assert_snapshot!("detail_run_info_queued_no_meta", terminal.backend());
    }
