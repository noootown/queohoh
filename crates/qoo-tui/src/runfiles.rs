use std::io::SeekFrom;
use std::path::Path;

use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use crate::ipc::types::TaskDefinition;

/// Fixed tail window: read at most this many bytes from the end of transcript.md.
/// Bumped from 256 KiB → 2 MiB so long agent runs still scroll back usefully
/// without loading multi‑MB full files every poll.
const TAIL_WINDOW: u64 = 2 * 1024 * 1024;

/// Contents of a run's on-disk files. `PartialEq` so content-identical re-reads
/// can be dropped before becoming events (Task 10's poll loop).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RunFiles {
    pub transcript_tail: Vec<String>,
    /// Whether the kept `transcript_tail` begins INSIDE a ```` ``` ```` code
    /// fence — true when the tail window's cut landed mid-fence (the opener
    /// scrolled out). The transcript styler seeds [`crate::markup::fence_states_from`]
    /// with this so mid-fence tails don't invert prose/code styling. `false`
    /// when the transcript is missing/empty, and (a known limitation) when the
    /// file exceeds the `TAIL_WINDOW` read window the unseen prefix's parity is
    /// unknowable so we assume outside-fence — same as before this flag existed.
    pub transcript_starts_in_fence: bool,
    pub report: Vec<String>,
    /// Claude session id for this run, read from `data.json` (`session_id`,
    /// written by the daemon's run store at finish). `None` until the run has
    /// recorded one; the queue "Resume" action resumes this session in a new
    /// tmux tab and falls back to the task's `resume_session_id` when absent.
    pub session_id: Option<String>,
    /// Absolute worktree path this run executed in (`data.json` →
    /// `resolved_worktree_path`), used as the tmux window's working directory
    /// for "Resume". `None` when the run record has no path yet.
    pub worktree_path: Option<String>,
    /// Parsed `data.json` metadata for the detail pane's `info` sub-tab. `None`
    /// until the run has a parseable `data.json` — run dirs appear lazily, so a
    /// missing/malformed file is absence, never an error.
    pub meta: Option<RunMeta>,
}

/// The subset of a run's `data.json` the `info` sub-tab renders: flat run facts
/// (start/finish stamps, outcome, exit, usage, model, worktree) plus the def
/// snapshot for the Config section. Every field is optional — `data.json` is
/// written at start and merged at finish, so a still-running run has no finish
/// fields yet. `PartialEq` so the poll loop drops content-identical re-reads.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RunMeta {
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub outcome: Option<String>,
    pub reason: Option<String>,
    pub exit_code: Option<i64>,
    pub timed_out: bool,
    pub session_id: Option<String>,
    pub model: Option<String>,
    /// Adapter name that executed this run (`data.json` → `provider`, e.g.
    /// `grok`). Distinct from `model`, which the daemon writes as the concrete
    /// CLI id without a slash (`grok-4.5`). Queue resume prefers this over
    /// parsing a provider segment from `model` / the task's `provider/label`
    /// ref so a non-claude run doesn't fall through to `claude`.
    pub provider: Option<String>,
    pub resolved_worktree: Option<String>,
    /// Absolute checkout path the run executed in (`data.json` →
    /// `resolved_worktree_path`). Distinct from `resolved_worktree` (the bare
    /// name); this is what "Resume" hands to tmux `-c`.
    pub resolved_worktree_path: Option<String>,
    pub cost_usd: Option<f64>,
    pub turns: Option<i64>,
    pub duration_ms: Option<u64>,
    /// Input/output token counts from the provider's own usage reporting
    /// (`data.json` → `usage.inputTokens`/`usage.outputTokens`). Populated
    /// independently of `cost_usd` — a provider (grok) can report tokens with
    /// no priced cost, so the detail pane's `tokens` row fills in where `cost`
    /// shows a dash. `None` for a run whose provider/event carried no usage
    /// object, or an old run record predating this field (adoption-safe:
    /// absent key on the parsed JSON just yields `None`, same as every other
    /// optional field here).
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    /// The def snapshot recorded at run start (`null`/absent for an adhoc run).
    pub definition: Option<TaskDefinition>,
}

/// Read `<runs_dir>/<task_id>/{report.md, transcript.md}`. Report is read fully
/// and split into lines; transcript is tail-read: stat the length, seek to
/// `max(0, len − TAIL_WINDOW)`, read to end, drop the first (partial) line when
/// the seek started mid-file, keep the last `tail_lines` lines. Missing or
/// unreadable files yield empty vecs — never an error (parity: run dirs appear
/// lazily as the worker writes).
pub async fn read_run_files(runs_dir: &Path, task_id: &str, tail_lines: usize) -> RunFiles {
    let dir = runs_dir.join(task_id);
    // Both files can embed captured command output (test runners) carrying
    // ANSI escapes, \r spinner overwrites, and tabs — sanitize per line so the
    // cell renderer never sees them (see sanitize_display_line).
    let report = match fs::read_to_string(dir.join("report.md")).await {
        Ok(s) => s.split('\n').map(crate::markup::sanitize_display_line).collect(),
        Err(_) => Vec::new(),
    };
    let (transcript_tail, transcript_starts_in_fence) =
        read_tail(&dir.join("transcript.md"), tail_lines).await.unwrap_or_default();
    let meta = read_run_meta(&dir.join("data.json")).await;
    // `session_id`/`worktree_path` are the "Resume" action's convenience copies,
    // derived from the same parse (blank strings already dropped as absent so
    // Resume never offers an unusable target).
    let session_id = meta.as_ref().and_then(|m| m.session_id.clone());
    let worktree_path = meta.as_ref().and_then(|m| m.resolved_worktree_path.clone());
    RunFiles { transcript_tail, transcript_starts_in_fence, report, session_id, worktree_path, meta }
}

/// Parse a run's `data.json` into [`RunMeta`]. Mixed key casing is deliberate:
/// the daemon's run store writes the flat run facts snake_case (`resolved_worktree`,
/// `exit_code`, …) but nests the TS `usage`/`definition` objects camelCase, so
/// each field is plucked by its own on-disk key rather than via one `rename_all`.
/// Missing/malformed file or a non-object payload → `None` (parity with the other
/// lazy reads); blank strings are dropped so a still-empty field reads as absent.
async fn read_run_meta(path: &Path) -> Option<RunMeta> {
    let text = fs::read_to_string(path).await.ok()?;
    let json = serde_json::from_str::<serde_json::Value>(&text).ok()?;
    if !json.is_object() {
        return None;
    }
    let s = |key: &str| {
        json.get(key).and_then(|v| v.as_str()).map(str::to_string).filter(|v| !v.is_empty())
    };
    let usage = |key: &str| json.get("usage").and_then(|u| u.get(key));
    // Only a real object deserializes; a `null`/absent definition (adhoc run)
    // stays `None` rather than erroring `from_value`.
    let definition = json
        .get("definition")
        .filter(|d| d.is_object())
        .and_then(|d| serde_json::from_value::<TaskDefinition>(d.clone()).ok());
    Some(RunMeta {
        started_at: s("started_at"),
        finished_at: s("finished_at"),
        outcome: s("outcome"),
        reason: s("reason"),
        exit_code: json.get("exit_code").and_then(|v| v.as_i64()),
        timed_out: json.get("timed_out").and_then(|v| v.as_bool()).unwrap_or(false),
        session_id: s("session_id"),
        model: s("model"),
        provider: s("provider"),
        resolved_worktree: s("resolved_worktree"),
        resolved_worktree_path: s("resolved_worktree_path"),
        cost_usd: usage("costUsd").and_then(|v| v.as_f64()),
        turns: usage("turns").and_then(|v| v.as_i64()),
        duration_ms: usage("durationMs").and_then(|v| v.as_u64()),
        input_tokens: usage("inputTokens").and_then(|v| v.as_u64()),
        output_tokens: usage("outputTokens").and_then(|v| v.as_u64()),
        definition,
    })
}

/// Returns the kept tail lines and whether that tail begins inside a ``` fence
/// (odd count of ``` markers among the lines trimmed off the FRONT). When the
/// file exceeds `TAIL_WINDOW` the unseen prefix's parity is unknowable, so a
/// fence opened before the window reads as outside-fence — an accepted, rare gap.
async fn read_tail(path: &Path, tail_lines: usize) -> std::io::Result<(Vec<String>, bool)> {
    // Clamp at the source (parity with the TS slice(-0) guard): 0 would keep
    // everything instead of one line.
    let tail_lines = tail_lines.max(1);
    let mut file = fs::File::open(path).await?;
    let len = file.metadata().await?.len();
    if len == 0 {
        return Ok((Vec::new(), false));
    }
    let start = len.saturating_sub(TAIL_WINDOW);
    if start > 0 {
        file.seek(SeekFrom::Start(start)).await?;
    }
    let mut buf = Vec::with_capacity((len - start) as usize);
    file.read_to_end(&mut buf).await?;
    let text = String::from_utf8_lossy(&buf);
    let mut lines: Vec<&str> = text.split('\n').collect();
    // A mid-file seek almost certainly landed inside a line: the first split
    // element is a torn fragment — drop it (keep at least one line).
    if start > 0 && lines.len() > 1 {
        lines.remove(0);
    }
    let keep_from = lines.len().saturating_sub(tail_lines);
    // Odd parity of ``` markers in the trimmed-off prefix → the kept tail's
    // first line sits inside a fence whose opener is no longer in the window.
    let starts_in_fence =
        lines[..keep_from].iter().filter(|l| l.trim_start().starts_with("```")).count() % 2 == 1;
    // Reports/transcripts embed captured command output (test runners) that can
    // carry ANSI escapes, \r spinner overwrites, and tabs — sanitize per line
    // so the cell renderer never sees them (see sanitize_display_line).
    let tail = lines[keep_from..].iter().map(|s| crate::markup::sanitize_display_line(s)).collect();
    Ok((tail, starts_in_fence))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn setup(task_id: &str, transcript: Option<&str>, report: Option<&str>) -> PathBuf {
        let runs = tempfile::tempdir().unwrap().keep();
        let dir = runs.join(task_id);
        std::fs::create_dir_all(&dir).unwrap();
        if let Some(t) = transcript {
            std::fs::write(dir.join("transcript.md"), t).unwrap();
        }
        if let Some(r) = report {
            std::fs::write(dir.join("report.md"), r).unwrap();
        }
        runs
    }

    #[tokio::test]
    async fn reads_report_and_last_25_lines() {
        let lines: Vec<String> = (0..40).map(|i| format!("line {i}")).collect();
        let runs = setup("01TASK", Some(&lines.join("\n")), Some("# Result\nok\n"));
        let out = read_run_files(&runs, "01TASK", 25).await;
        assert_eq!(out.report[0], "# Result");
        assert_eq!(out.transcript_tail.len(), 25);
        assert_eq!(out.transcript_tail[24], "line 39");
    }

    #[tokio::test]
    async fn sanitizes_ansi_and_cr_in_read_lines() {
        // A report whose Verify block captured raw vitest output: ANSI SGR
        // codes and \r spinner overwrites must never reach the renderer.
        let report = "## Verify\n\u{1b}[90mstderr\u{1b}[2m | api.test.ts\u{1b}[22m > ok\nspin\rdone\n";
        let runs = setup("01ANSI", Some("t\u{1b}[31mred\u{1b}[39m"), Some(report));
        let out = read_run_files(&runs, "01ANSI", 25).await;
        assert_eq!(out.report[1], "stderr | api.test.ts > ok");
        assert_eq!(out.report[2], "done");
        assert_eq!(out.transcript_tail[0], "tred");
    }

    #[tokio::test]
    async fn honors_tail_lines() {
        let lines: Vec<String> = (0..200).map(|i| format!("line {i}")).collect();
        let runs = setup("01TAIL", Some(&lines.join("\n")), None);
        let out = read_run_files(&runs, "01TAIL", 100).await;
        assert_eq!(out.transcript_tail.len(), 100);
        assert_eq!(out.transcript_tail[99], "line 199");
        assert!(out.report.is_empty());
    }

    #[tokio::test]
    async fn clamps_tail_lines_below_1() {
        let lines: Vec<String> = (0..10).map(|i| format!("line {i}")).collect();
        let runs = setup("01ZERO", Some(&lines.join("\n")), None);
        let out = read_run_files(&runs, "01ZERO", 0).await;
        assert_eq!(out.transcript_tail, vec!["line 9".to_string()]);
    }

    #[tokio::test]
    async fn missing_dir_yields_empty() {
        let runs = tempfile::tempdir().unwrap().keep();
        let out = read_run_files(&runs, "01NOPE", 25).await;
        assert!(out.report.is_empty());
        assert!(out.transcript_tail.is_empty());
    }

    #[tokio::test]
    async fn reads_session_id_and_worktree_from_data_json() {
        // The daemon's run record (`data.json`) carries `session_id` (written at
        // finish) and `resolved_worktree`; both surface on `RunFiles` for the
        // queue "Resume" action.
        let runs = setup("01SESS", Some("hi"), None);
        std::fs::write(
            runs.join("01SESS").join("data.json"),
            r#"{"session_id":"sess-abc","resolved_worktree":"acme.feature","resolved_worktree_path":"/repos/acme.feature","outcome":"done"}"#,
        )
        .unwrap();
        let out = read_run_files(&runs, "01SESS", 25).await;
        assert_eq!(out.session_id.as_deref(), Some("sess-abc"));
        // `worktree_path` is the ABSOLUTE path (from `resolved_worktree_path`),
        // not the bare `resolved_worktree` name — Resume feeds it to tmux `-c`.
        assert_eq!(out.worktree_path.as_deref(), Some("/repos/acme.feature"));
        assert_eq!(out.meta.as_ref().unwrap().resolved_worktree.as_deref(), Some("acme.feature"));
    }

    #[tokio::test]
    async fn missing_or_blank_run_meta_is_none() {
        // No data.json → None; and blank strings are treated as absent so Resume
        // never offers an unusable target.
        let runs = setup("01NOMETA", Some("hi"), None);
        let out = read_run_files(&runs, "01NOMETA", 25).await;
        assert_eq!(out.session_id, None);
        assert_eq!(out.worktree_path, None);

        let runs2 = setup("01BLANK", Some("hi"), None);
        std::fs::write(
            runs2.join("01BLANK").join("data.json"),
            r#"{"session_id":"","resolved_worktree":""}"#,
        )
        .unwrap();
        let out2 = read_run_files(&runs2, "01BLANK", 25).await;
        assert_eq!(out2.session_id, None);
        assert_eq!(out2.worktree_path, None);
    }

    #[tokio::test]
    async fn parses_full_run_meta() {
        // A finished run's `data.json`: snake_case flat facts, camelCase `usage`
        // sub-fields, and a camelCase TS `definition` snapshot — all surface on
        // `RunMeta` for the info sub-tab.
        let runs = setup("01META", Some("hi"), None);
        std::fs::write(
            runs.join("01META").join("data.json"),
            r#"{
              "started_at": "2026-07-09T12:00:05.000Z",
              "finished_at": "2026-07-09T12:03:20.000Z",
              "outcome": "done",
              "reason": null,
              "exit_code": 0,
              "timed_out": false,
              "session_id": "sess-abc",
              "model": "claude-opus-4-8",
              "resolved_worktree": "/repos/acme.feature",
              "usage": {"costUsd": 0.42, "turns": 37, "durationMs": 195000},
              "definition": {"name": "squash-merge", "repo": "acme", "dedup": "none",
                             "worktree": "auto", "timeoutMs": 1800000, "priority": "normal",
                             "cron": "30 13 * * *", "description": "Squash-merge the branch."}
            }"#,
        )
        .unwrap();
        let m = read_run_files(&runs, "01META", 25).await.meta.unwrap();
        assert_eq!(m.started_at.as_deref(), Some("2026-07-09T12:00:05.000Z"));
        assert_eq!(m.finished_at.as_deref(), Some("2026-07-09T12:03:20.000Z"));
        assert_eq!(m.outcome.as_deref(), Some("done"));
        assert_eq!(m.reason, None); // JSON null → absent
        assert_eq!(m.exit_code, Some(0));
        assert!(!m.timed_out);
        assert_eq!(m.model.as_deref(), Some("claude-opus-4-8"));
        assert_eq!(m.provider, None); // absent key → None
        assert_eq!(m.cost_usd, Some(0.42));
        assert_eq!(m.turns, Some(37));
        assert_eq!(m.duration_ms, Some(195_000));
        // Old-shaped `usage` object (predates the tokens fields) → None, never
        // an error: adoption-safe for a run record written before this feature.
        assert_eq!(m.input_tokens, None);
        assert_eq!(m.output_tokens, None);
        let def = m.definition.unwrap();
        assert_eq!(def.name, "squash-merge");
        assert_eq!(def.timeout_ms, 1_800_000);
        assert_eq!(def.cron.as_deref(), Some("30 13 * * *"));
        assert_eq!(def.description.as_deref(), Some("Squash-merge the branch."));
    }

    #[tokio::test]
    async fn parses_token_usage_when_present() {
        // A run whose provider reported token usage (e.g. grok, which has no
        // priced cost) — `usage.inputTokens`/`usage.outputTokens` surface on
        // `RunMeta` alongside (not instead of) the existing cost/turns fields.
        let runs = setup("01TOK", Some("hi"), None);
        std::fs::write(
            runs.join("01TOK").join("data.json"),
            r#"{"usage": {"costUsd": null, "turns": 2, "durationMs": null,
                          "inputTokens": 199057, "outputTokens": 22341}}"#,
        )
        .unwrap();
        let m = read_run_files(&runs, "01TOK", 25).await.meta.unwrap();
        assert_eq!(m.cost_usd, None);
        assert_eq!(m.input_tokens, Some(199_057));
        assert_eq!(m.output_tokens, Some(22_341));
    }

    #[tokio::test]
    async fn parses_provider_alongside_bare_model_id() {
        // Daemon writes concrete CLI ids without slash + a separate provider
        // adapter name — both must surface so queue resume can prefer provider.
        let runs = setup("01PROV", Some("hi"), None);
        std::fs::write(
            runs.join("01PROV").join("data.json"),
            r#"{"model":"grok-4.5","provider":"grok","session_id":"sess-g"}"#,
        )
        .unwrap();
        let m = read_run_files(&runs, "01PROV", 25).await.meta.unwrap();
        assert_eq!(m.model.as_deref(), Some("grok-4.5"));
        assert_eq!(m.provider.as_deref(), Some("grok"));
    }

    #[tokio::test]
    async fn adhoc_and_malformed_meta() {
        // A running adhoc run: `data.json` exists with start facts but a `null`
        // definition (adhoc) and no finish/usage yet — meta is Some, def is None,
        // finish fields absent.
        let runs = setup("01ADHOC", Some("hi"), None);
        std::fs::write(
            runs.join("01ADHOC").join("data.json"),
            r#"{"started_at": "2026-07-09T12:00:05.000Z", "definition": null,
                "resolved_worktree": "/repos/acme.feature"}"#,
        )
        .unwrap();
        let m = read_run_files(&runs, "01ADHOC", 25).await.meta.unwrap();
        assert!(m.definition.is_none());
        assert_eq!(m.started_at.as_deref(), Some("2026-07-09T12:00:05.000Z"));
        assert_eq!(m.finished_at, None);
        assert_eq!(m.exit_code, None);

        // Malformed JSON → no meta at all (parity with the other lazy reads).
        let runs2 = setup("01BAD", Some("hi"), None);
        std::fs::write(runs2.join("01BAD").join("data.json"), "{not json").unwrap();
        assert!(read_run_files(&runs2, "01BAD", 25).await.meta.is_none());
    }

    #[tokio::test]
    async fn empty_transcript_yields_empty() {
        let runs = setup("01EMPTY", Some(""), None);
        let out = read_run_files(&runs, "01EMPTY", 25).await;
        assert!(out.transcript_tail.is_empty());
        assert!(!out.transcript_starts_in_fence);
    }

    #[tokio::test]
    async fn tail_starting_mid_fence_flags_starts_in_fence() {
        // The kept tail begins after a ```bash opener whose closer scrolled out
        // of the window → odd parity in the trimmed-off prefix → starts_in_fence.
        let transcript = ["intro", "```bash", "a", "b", "c", "d", "e"].join("\n");
        let runs = setup("01MIDFENCE", Some(&transcript), None);
        let out = read_run_files(&runs, "01MIDFENCE", 3).await;
        assert_eq!(out.transcript_tail, vec!["c", "d", "e"]);
        assert!(out.transcript_starts_in_fence);
    }

    #[tokio::test]
    async fn tail_starting_outside_a_balanced_fence_is_false() {
        // The trimmed-off prefix opens AND closes the fence (even parity), so the
        // kept tail is outside any fence.
        let transcript = ["```bash", "a", "```", "x", "y", "z"].join("\n");
        let runs = setup("01BALANCED", Some(&transcript), None);
        let out = read_run_files(&runs, "01BALANCED", 3).await;
        assert_eq!(out.transcript_tail, vec!["x", "y", "z"]);
        assert!(!out.transcript_starts_in_fence);
    }

    #[tokio::test]
    async fn missing_transcript_is_not_in_fence() {
        let runs = tempfile::tempdir().unwrap().keep();
        let out = read_run_files(&runs, "01NONE", 25).await;
        assert!(out.transcript_tail.is_empty());
        assert!(!out.transcript_starts_in_fence);
    }

    #[tokio::test]
    async fn large_transcript_tail_correct_and_partial_line_dropped() {
        // Push well past TAIL_WINDOW (2 MiB), then 25 known tail lines. The seek
        // lands mid-line inside the padding; the torn first line must be dropped.
        let padding: Vec<String> = (0..40_000)
            .map(|i| format!("padding line {i} {}", "x".repeat(48)))
            .collect();
        let tail: Vec<String> = (0..25).map(|i| format!("tail {i}")).collect();
        let content = [padding.clone(), tail.clone()].concat().join("\n");
        assert!(content.len() as u64 > TAIL_WINDOW);
        let runs = setup("01BIG", Some(&content), None);
        let out = read_run_files(&runs, "01BIG", 25).await;
        assert_eq!(out.transcript_tail, tail);
        // Torn-line check: nothing in the tail starts mid-word garbage.
        assert!(out.transcript_tail.iter().all(|l| l.starts_with("tail ")));
    }
}
