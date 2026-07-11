use std::io::SeekFrom;
use std::path::Path;

use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

/// Fixed tail window: read at most this many bytes from the end of transcript.md.
const TAIL_WINDOW: u64 = 262_144;

/// Contents of a run's on-disk files. `PartialEq` so content-identical re-reads
/// can be dropped before becoming events (Task 10's poll loop).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RunFiles {
    pub transcript_tail: Vec<String>,
    pub report: Vec<String>,
    /// Claude session id for this run, read from `data.json` (`session_id`,
    /// written by the daemon's run store at finish). `None` until the run has
    /// recorded one; the queue "Resume" action resumes this session in a new
    /// tmux pane and falls back to the task's `resume_session_id` when absent.
    pub session_id: Option<String>,
    /// Absolute worktree path this run executed in (`data.json` →
    /// `resolved_worktree`), used as the tmux pane's working directory for
    /// "Resume". `None` when the run record has no path yet.
    pub worktree_path: Option<String>,
}

/// Read `<runs_dir>/<task_id>/{report.md, transcript.md}`. Report is read fully
/// and split into lines; transcript is tail-read: stat the length, seek to
/// `max(0, len − 256KiB)`, read to end, drop the first (partial) line when the
/// seek started mid-file, keep the last `tail_lines` lines. Missing or
/// unreadable files yield empty vecs — never an error (parity: run dirs appear
/// lazily as the worker writes).
pub async fn read_run_files(runs_dir: &Path, task_id: &str, tail_lines: usize) -> RunFiles {
    let dir = runs_dir.join(task_id);
    let report = match fs::read_to_string(dir.join("report.md")).await {
        Ok(s) => s.split('\n').map(str::to_string).collect(),
        Err(_) => Vec::new(),
    };
    let transcript_tail = read_tail(&dir.join("transcript.md"), tail_lines)
        .await
        .unwrap_or_default();
    let (session_id, worktree_path) = read_run_meta(&dir.join("data.json")).await;
    RunFiles { transcript_tail, report, session_id, worktree_path }
}

/// Parse `session_id` + `resolved_worktree` out of a run's `data.json`. Missing
/// or malformed file → `(None, None)` (parity with the other lazy reads: run
/// metadata appears only after the worker records it). A blank string is treated
/// as absent so the "Resume" action doesn't offer an unusable target.
async fn read_run_meta(path: &Path) -> (Option<String>, Option<String>) {
    let Ok(text) = fs::read_to_string(path).await else {
        return (None, None);
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return (None, None);
    };
    let field = |key: &str| {
        json.get(key)
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .filter(|s| !s.is_empty())
    };
    (field("session_id"), field("resolved_worktree"))
}

async fn read_tail(path: &Path, tail_lines: usize) -> std::io::Result<Vec<String>> {
    // Clamp at the source (parity with the TS slice(-0) guard): 0 would keep
    // everything instead of one line.
    let tail_lines = tail_lines.max(1);
    let mut file = fs::File::open(path).await?;
    let len = file.metadata().await?.len();
    if len == 0 {
        return Ok(Vec::new());
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
    Ok(lines[keep_from..].iter().map(|s| s.to_string()).collect())
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
            r#"{"session_id":"sess-abc","resolved_worktree":"/repos/acme.feature","outcome":"done"}"#,
        )
        .unwrap();
        let out = read_run_files(&runs, "01SESS", 25).await;
        assert_eq!(out.session_id.as_deref(), Some("sess-abc"));
        assert_eq!(out.worktree_path.as_deref(), Some("/repos/acme.feature"));
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
    async fn empty_transcript_yields_empty() {
        let runs = setup("01EMPTY", Some(""), None);
        let out = read_run_files(&runs, "01EMPTY", 25).await;
        assert!(out.transcript_tail.is_empty());
    }

    #[tokio::test]
    async fn large_transcript_tail_correct_and_partial_line_dropped() {
        // Push well past the 256 KiB window, then 25 known tail lines. The seek
        // lands mid-line inside the padding; the torn first line must be dropped.
        let padding: Vec<String> = (0..8000)
            .map(|i| format!("padding line {i} {}", "x".repeat(32)))
            .collect();
        let tail: Vec<String> = (0..25).map(|i| format!("tail {i}")).collect();
        let content = [padding.clone(), tail.clone()].concat().join("\n");
        assert!(content.len() > 262_144);
        let runs = setup("01BIG", Some(&content), None);
        let out = read_run_files(&runs, "01BIG", 25).await;
        assert_eq!(out.transcript_tail, tail);
        // Torn-line check: nothing in the tail starts mid-word garbage.
        assert!(out.transcript_tail.iter().all(|l| l.starts_with("tail ")));
    }
}
