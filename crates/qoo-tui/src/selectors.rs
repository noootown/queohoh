use std::collections::{HashMap, HashSet};

use crate::app::Selection;
use crate::ipc::types::{ArgSpec, StateSnapshot, TaskInstance, TaskStatus};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabInfo {
    pub name: String,
    /// repo seen in tasks/archivedRecent but absent from config projects
    pub synthetic: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueRow {
    pub task_id: String,
    pub glyph: char,
    /// drives the animated throbber in place of the static ▶
    pub running: bool,
    /// ⛓ marker: task resumes the lane's main session
    pub main_session: bool,
    pub lane: String,
    pub summary: String,
    pub detail: String,
    pub archived: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WtState {
    Free,
    Busy,
    You,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeRow {
    /// display name (`<repo>.` prefix stripped) — never an identifier
    pub name: String,
    /// untouched worktree identifier used for every daemon action
    pub raw_name: String,
    pub path: String,
    /// "" for session rows (no real worktree)
    pub branch: String,
    pub state: WtState,
    pub has_main_session: bool,
    pub queued: usize,
    pub is_session: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneLayout {
    pub queue_h: u16,
    pub tasks_h: u16,
    pub worktrees_h: u16,
}

pub fn build_tabs(snapshot: &StateSnapshot) -> Vec<TabInfo> {
    let configured: HashSet<&str> =
        snapshot.projects.iter().map(|p| p.name.as_str()).collect();
    let mut tabs: Vec<TabInfo> = snapshot
        .projects
        .iter()
        .map(|p| TabInfo { name: p.name.clone(), synthetic: false })
        .collect();
    // Repos seen in tasks/archived but absent from config → synthetic tabs,
    // sorted alphabetically after the configured ones.
    let mut orphans: Vec<String> = Vec::new();
    for task in snapshot.tasks.iter().chain(snapshot.archived_recent.iter()) {
        let repo = &task.target.repo;
        if !configured.contains(repo.as_str()) && !orphans.contains(repo) {
            orphans.push(repo.clone());
        }
    }
    orphans.sort();
    for name in orphans {
        tabs.push(TabInfo { name, synthetic: true });
    }
    tabs
}

fn status_glyph(status: TaskStatus) -> char {
    match status {
        TaskStatus::Running => '▶',
        TaskStatus::Queued => '○',
        TaskStatus::NeedsInput => '?',
        TaskStatus::Done => '✓',
        TaskStatus::Failed => '✗',
        TaskStatus::Unknown => '·', // no TS counterpart (old-daemon statuses only)
    }
}

fn status_str(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Queued => "queued",
        TaskStatus::NeedsInput => "needs-input",
        TaskStatus::Running => "running",
        TaskStatus::Done => "done",
        TaskStatus::Failed => "failed",
        TaskStatus::Unknown => "unknown",
    }
}

/// `repo:worktree-or-ref` with the redundant `<repo>.` display prefix stripped.
fn lane_label(task: &TaskInstance) -> String {
    let lane = task.target.worktree.as_deref().unwrap_or(&task.target.git_ref);
    format!("{}:{}", task.target.repo, strip_repo_prefix(lane, &task.target.repo))
}

pub fn queue_rows(snapshot: &StateSnapshot, project: &str, now_epoch_s: u64) -> Vec<QueueRow> {
    // Live rows in snapshot order (the daemon stores by creation), then the
    // last 10 archived rows, dimmed by the view via `archived: true`.
    let mut queued_position: HashMap<String, usize> = HashMap::new();
    let mut rows: Vec<QueueRow> = Vec::new();
    for task in snapshot.tasks.iter().filter(|t| t.target.repo == project) {
        let detail = match task.status {
            TaskStatus::Running => {
                elapsed_label(parse_iso_epoch_s(&task.created), now_epoch_s)
            }
            TaskStatus::Queued => {
                let lane = lane_label(task);
                let position = queued_position.get(&lane).copied().unwrap_or(0) + 1;
                queued_position.insert(lane, position);
                format!("#{position} in lane")
            }
            TaskStatus::NeedsInput | TaskStatus::Failed => task
                .error
                .clone()
                .unwrap_or_else(|| status_str(task.status).to_string()),
            TaskStatus::Done => "done".to_string(),
            TaskStatus::Unknown => status_str(task.status).to_string(),
        };
        rows.push(QueueRow {
            task_id: task.id.clone(),
            glyph: status_glyph(task.status),
            running: task.status == TaskStatus::Running,
            main_session: task.session == "main",
            lane: lane_label(task),
            summary: prompt_summary(&task.prompt),
            detail,
            archived: false,
        });
    }
    let archived: Vec<&TaskInstance> = snapshot
        .archived_recent
        .iter()
        .filter(|t| t.target.repo == project)
        .collect();
    let start = archived.len().saturating_sub(10);
    for task in &archived[start..] {
        rows.push(QueueRow {
            task_id: task.id.clone(),
            glyph: status_glyph(task.status),
            running: false,
            main_session: task.session == "main",
            lane: lane_label(task),
            summary: prompt_summary(&task.prompt),
            detail: "archived".to_string(),
            archived: true,
        });
    }
    rows
}

/// `repo:worktree` from a task's target; None while the worktree is unresolved
/// (mirror of core's laneKey — raw identifiers, no display stripping).
fn task_lane(task: &TaskInstance) -> Option<String> {
    task.target
        .worktree
        .as_ref()
        .map(|wt| format!("{}:{}", task.target.repo, wt))
}

fn worktree_state(snapshot: &StateSnapshot, lane: &str) -> WtState {
    let on_lane: Vec<&TaskInstance> = snapshot
        .tasks
        .iter()
        .filter(|t| task_lane(t).as_deref() == Some(lane))
        .collect();
    if on_lane.iter().any(|t| t.status == TaskStatus::Running) {
        return WtState::Busy;
    }
    // newest by id — ULIDs sort chronologically
    match on_lane.iter().max_by(|a, b| a.id.cmp(&b.id)) {
        Some(t) if t.status == TaskStatus::Failed => WtState::Failed,
        _ => WtState::Free,
    }
}

fn queued_on_lane(snapshot: &StateSnapshot, lane: &str) -> usize {
    snapshot
        .tasks
        .iter()
        .filter(|t| task_lane(t).as_deref() == Some(lane) && t.status == TaskStatus::Queued)
        .count()
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

pub fn worktree_rows(snapshot: &StateSnapshot, project: &str) -> Vec<WorktreeRow> {
    let empty: Vec<crate::ipc::types::WorktreeInfo> = Vec::new();
    let worktrees = snapshot.worktrees.get(project).unwrap_or(&empty);
    let mut rows: Vec<WorktreeRow> = worktrees
        .iter()
        .map(|wt| {
            let lane = lane_key(project, &wt.name);
            WorktreeRow {
                name: strip_repo_prefix(&wt.name, project).to_string(),
                raw_name: wt.name.clone(),
                path: wt.path.clone(),
                branch: wt.branch.clone(),
                state: worktree_state(snapshot, &lane),
                has_main_session: snapshot.main_sessions.contains_key(&lane),
                queued: queued_on_lane(snapshot, &lane),
                is_session: false,
            }
        })
        .collect();

    // One "You" row per interactive session whose cwd is inside a project
    // worktree (exact path or path + "/" prefix — never a sibling).
    for session in &snapshot.sessions {
        if session.kind != "interactive" {
            continue;
        }
        let Some(cwd) = session.cwd.as_deref() else { continue };
        let inside = worktrees
            .iter()
            .any(|wt| cwd == wt.path || cwd.starts_with(&format!("{}/", wt.path)));
        if !inside {
            continue;
        }
        // A session is not a real worktree: rawName mirrors the display name and
        // is never dispatched to the daemon as a worktree identifier.
        let display = strip_repo_prefix(basename(cwd), project).to_string();
        rows.push(WorktreeRow {
            name: display.clone(),
            raw_name: display,
            path: cwd.to_string(),
            branch: String::new(),
            state: WtState::You,
            has_main_session: false,
            queued: 0,
            is_session: true,
        });
    }
    rows
}

pub fn pane_layout(body_height: u16) -> PaneLayout {
    // queue : tasks : worktrees ≈ 2:1:1, explicit heights (no flex-grow) so a
    // pane never balloons past its capped content. Row capacity per pane is
    // height − 3 (border + title chrome), computed by the view.
    let list_h = std::cmp::max(4, body_height / 4);
    let queue_h = std::cmp::max(4, body_height.saturating_sub(2 * list_h));
    PaneLayout { queue_h, tasks_h: list_h, worktrees_h: list_h }
}

/// Cursor-centered scroll window: half-open `(start, end)` slice indices of the
/// visible rows (`start` is the TS `offset`).
pub fn window_rows(len: usize, cursor: usize, capacity: usize) -> (usize, usize) {
    if capacity == 0 || len == 0 {
        return (0, 0);
    }
    if len <= capacity {
        return (0, len);
    }
    let clamped = cursor.min(len - 1);
    let start = clamped.saturating_sub(capacity / 2).min(len - capacity);
    (start, start + capacity)
}

pub fn pane_title(base: &str, sel: &Selection, filter: &str, searching: bool) -> String {
    let selected = match sel.anchor {
        Some(anchor) => anchor.abs_diff(sel.cursor) + 1,
        None => 1,
    };
    let title = if selected > 1 {
        format!("{base} · {selected} selected")
    } else {
        base.to_string()
    };
    if !searching && filter.is_empty() {
        return title;
    }
    let cursor = if searching { "█" } else { "" };
    format!("{title} /{filter}{cursor}")
}

/// Indices of rows whose text matches the filter (case-insensitive substring;
/// empty filter matches everything).
pub fn filter_rows<T>(rows: &[T], filter: &str, text_of: impl Fn(&T) -> String) -> Vec<usize> {
    if filter.is_empty() {
        return (0..rows.len()).collect();
    }
    let needle = filter.to_lowercase();
    rows.iter()
        .enumerate()
        .filter(|(_, row)| text_of(row).to_lowercase().contains(&needle))
        .map(|(i, _)| i)
        .collect()
}

/// "pr, mode=ready, review=auto" — `name` for required args, `name=default` otherwise.
pub fn arg_summary(args: &[ArgSpec]) -> String {
    args.iter()
        .map(|a| match &a.default {
            Some(d) => format!("{}={}", a.name, d),
            None => a.name.clone(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn strip_repo_prefix<'a>(worktree: &'a str, repo: &str) -> &'a str {
    match worktree.strip_prefix(repo) {
        Some(rest) => match rest.strip_prefix('.') {
            Some(stripped) => stripped,
            None => worktree, // bare repo name or shared prefix without the dot
        },
        None => worktree,
    }
}

pub fn lane_key(repo: &str, worktree: &str) -> String {
    format!("{repo}:{worktree}")
}

/// First non-blank line of the prompt, trimmed, clipped to ≤60 chars with `…`.
pub fn prompt_summary(prompt: &str) -> String {
    let line = prompt
        .lines()
        .find(|l| !l.trim().is_empty())
        .map(str::trim)
        .unwrap_or("");
    let chars: Vec<char> = line.chars().collect();
    if chars.len() <= 60 {
        return line.to_string();
    }
    let mut out: String = chars[..59].iter().collect();
    out.push('…');
    out
}

/// "⏱ 47s" / "⏱ 5m03s" (zero-padded seconds) / "⏱ 1h02m" (zero-padded minutes).
pub fn elapsed_label(created_epoch_s: u64, now_epoch_s: u64) -> String {
    let total = now_epoch_s.saturating_sub(created_epoch_s);
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;
    if hours > 0 {
        format!("⏱ {hours}h{minutes:02}m")
    } else if minutes > 0 {
        format!("⏱ {minutes}m{seconds:02}s")
    } else {
        format!("⏱ {seconds}s")
    }
}

/// Parse a daemon ISO-8601 UTC timestamp ("YYYY-MM-DDTHH:MM:SS[.mmm]Z") into
/// epoch seconds. No date crate: Howard Hinnant's days-from-civil algorithm.
fn parse_iso_epoch_s(iso: &str) -> u64 {
    if iso.len() < 19 {
        return 0;
    }
    let num = |s: &str| s.parse::<i64>().unwrap_or(0);
    let (y, m, d) = (num(&iso[0..4]), num(&iso[5..7]), num(&iso[8..10]));
    let (hh, mm, ss) = (num(&iso[11..13]), num(&iso[14..16]), num(&iso[17..19]));
    let secs = days_from_civil(y, m, d) * 86_400 + hh * 3600 + mm * 60 + ss;
    if secs < 0 { 0 } else { secs as u64 }
}

/// Days since 1970-01-01 for a proleptic-Gregorian civil date
/// (Howard Hinnant's `days_from_civil`).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::types::{Project, SessionEntry, TaskTarget, WorktreeInfo};

    // ---- fixtures (mirror __tests__/helpers.ts makeTask/makeSnapshot/makeSession) ----

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
            source: "tui".into(),
            ephemeral_worktree: false,
            error: None,
            session: "fresh".into(),
            resume_session_id: None,
            model: None,
            prompt: "fix the flaky test\nmore context\n".into(),
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
        WorktreeInfo { name: name.into(), path: path.into(), branch: branch.into() }
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
        names.iter().map(|n| Project { name: n.to_string() }).collect()
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
        let rows = queue_rows(&s, "platform", now());
        assert_eq!(
            rows.iter().map(|r| r.task_id.as_str()).collect::<Vec<_>>(),
            vec!["01TASKAAA000000000000000000", "01TASKCCC000000000000000000"]
        );
        assert_eq!(rows.iter().map(|r| r.archived).collect::<Vec<_>>(), vec![false, true]);
    }

    #[test]
    fn queue_rows_detail_running_elapsed_queued_position_failed_error() {
        let running = task_on(TaskStatus::Running, "t1", "platform", Some("wt-a"));
        let q1 = task_on(TaskStatus::Queued, "t2", "platform", Some("wt-a"));
        let q2 = task_on(TaskStatus::Queued, "t3", "platform", Some("wt-a"));
        let mut failed = task_on(TaskStatus::Failed, "t4", "platform", Some("wt-a"));
        failed.error = Some("tree left dirty".into());
        let done = task_on(TaskStatus::Done, "t5", "platform", Some("wt-a"));
        let rows = queue_rows(&snap(vec![running, q1, q2, failed, done], vec![]), "platform", now());
        assert_eq!(rows[0].detail, "⏱ 3m12s");
        assert_eq!(rows[1].detail, "#1 in lane");
        assert_eq!(rows[2].detail, "#2 in lane");
        assert_eq!(rows[3].detail, "tree left dirty");
        assert_eq!(rows[4].detail, "done");
        assert_eq!(rows[0].lane, "platform:wt-a");
        assert!(rows[0].running && !rows[1].running);
        assert_eq!(
            rows.iter().map(|r| r.glyph).collect::<Vec<_>>(),
            vec!['▶', '○', '○', '✗', '✓']
        );
    }

    #[test]
    fn queue_rows_needs_input_without_error_falls_back_to_status_word() {
        let ni = task_on(TaskStatus::NeedsInput, "t1", "platform", Some("wt-a"));
        let rows = queue_rows(&snap(vec![ni], vec![]), "platform", now());
        assert_eq!(rows[0].detail, "needs-input");
        assert_eq!(rows[0].glyph, '?');
    }

    #[test]
    fn queue_rows_use_ref_as_lane_when_worktree_unresolved_and_append_archived() {
        let mut pending = task_on(TaskStatus::Queued, "t1", "platform", None);
        pending.target.git_ref = "pr:257".into();
        let old = task_on(TaskStatus::Done, "t0", "platform", Some("wt-a"));
        let rows = queue_rows(&snap(vec![pending], vec![old]), "platform", now());
        assert_eq!(rows[0].lane, "platform:pr:257");
        assert!(rows[1].archived);
        assert_eq!(rows[1].detail, "archived");
    }

    #[test]
    fn queue_rows_cap_archived_at_last_10() {
        let archived: Vec<TaskInstance> = (0..15)
            .map(|i| task_on(TaskStatus::Done, &format!("t{i:02}"), "platform", Some("wt-a")))
            .collect();
        let rows = queue_rows(&snap(vec![], archived), "platform", now());
        assert_eq!(rows.len(), 10);
        assert_eq!(rows[0].task_id, "t05"); // last 10 → t05..t14
    }

    #[test]
    fn queue_rows_mark_main_session_tasks_live_and_archived() {
        let mut main_task = task_on(TaskStatus::Running, "t1", "platform", Some("wt-a"));
        main_task.session = "main".into();
        let fresh = task_on(TaskStatus::Queued, "t2", "platform", Some("wt-a"));
        let mut archived_main = task_on(TaskStatus::Done, "t3", "platform", Some("wt-a"));
        archived_main.session = "main".into();
        let rows = queue_rows(&snap(vec![main_task, fresh], vec![archived_main]), "platform", now());
        assert!(rows[0].main_session);
        assert!(!rows[1].main_session);
        assert!(rows[2].main_session);
    }

    #[test]
    fn queue_rows_strip_repo_prefix_in_lane() {
        let running = task_on(
            TaskStatus::Running,
            "t1",
            "platform",
            Some("platform.dedup-dependabot-run"),
        );
        let rows = queue_rows(&snap(vec![running], vec![]), "platform", now());
        assert_eq!(rows[0].lane, "platform:dedup-dependabot-run");
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
    fn prompt_summary_first_non_blank_line_clipped_at_60() {
        assert_eq!(prompt_summary("\n\nfix the thing\nrest"), "fix the thing");
        assert_eq!(prompt_summary(""), "");
        let long = "a".repeat(70);
        let expected = format!("{}…", "a".repeat(59));
        assert_eq!(prompt_summary(&long), expected);
        assert_eq!(prompt_summary(&"a".repeat(60)), "a".repeat(60)); // exactly 60 fits
    }

    #[test]
    fn strip_repo_prefix_cases() {
        assert_eq!(strip_repo_prefix("platform.dedup-dependabot-run", "platform"), "dedup-dependabot-run");
        assert_eq!(strip_repo_prefix("platform", "platform"), "platform"); // bare repo kept
        assert_eq!(strip_repo_prefix("wt-a", "platform"), "wt-a"); // unprefixed kept
    }

    #[test]
    fn lane_key_joins_repo_and_worktree() {
        assert_eq!(lane_key("platform", "wt-a"), "platform:wt-a");
    }

    #[test]
    fn arg_summary_names_and_defaults() {
        let args = vec![
            ArgSpec { name: "pr".into(), default: None, options: None, description: None },
            ArgSpec { name: "mode".into(), default: Some("ready".into()), options: None, description: None },
            ArgSpec { name: "review".into(), default: Some("auto".into()), options: None, description: None },
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
                    branch: "feat/a".into(), state: WtState::Free, has_main_session: false,
                    queued: 0, is_session: false,
                },
                WorktreeRow {
                    name: "wt-b".into(), raw_name: "wt-b".into(), path: "/wt/wt-b".into(),
                    branch: "feat/b".into(), state: WtState::Free, has_main_session: false,
                    queued: 0, is_session: false,
                },
                WorktreeRow {
                    name: "wt-c".into(), raw_name: "wt-c".into(), path: "/wt/wt-c".into(),
                    branch: "feat/c".into(), state: WtState::Free, has_main_session: false,
                    queued: 0, is_session: false,
                },
            ]
        );
    }

    #[test]
    fn worktree_flags_main_session_lanes() {
        let mut s = snap(vec![], vec![]);
        s.worktrees = platform_worktrees();
        s.main_sessions = HashMap::from([("platform:wt-b".to_string(), "sess-main".to_string())]);
        let rows = worktree_rows(&s, "platform");
        assert!(!rows.iter().find(|r| r.name == "wt-a").unwrap().has_main_session);
        assert!(rows.iter().find(|r| r.name == "wt-b").unwrap().has_main_session);
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
        assert_eq!(
            rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            vec!["platform", "dedup-dependabot-run"]
        );
        assert_eq!(
            rows.iter().map(|r| r.raw_name.as_str()).collect::<Vec<_>>(),
            vec!["platform", "platform.dedup-dependabot-run"]
        );
        assert_eq!(rows[1].path, "/wt/platform.dedup-dependabot-run");
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

    // ---- pane_layout (mirrors computePaneLayout) ----

    #[test]
    fn pane_layout_sums_exactly_to_body_height() {
        for body in [13u16, 20, 38, 50, 77] {
            let l = pane_layout(body);
            assert_eq!(l.queue_h + l.tasks_h + l.worktrees_h, body, "body={body}");
        }
    }

    #[test]
    fn pane_layout_gives_queue_half_and_lists_quarter_each() {
        let l = pane_layout(38);
        assert_eq!(l.tasks_h, 9);
        assert_eq!(l.worktrees_h, 9);
        assert_eq!(l.queue_h, 20);
    }

    #[test]
    fn pane_layout_keeps_minimums_for_tiny_body() {
        let l = pane_layout(1);
        assert!(l.tasks_h >= 4);
        assert!(l.worktrees_h >= 4);
        assert!(l.queue_h >= 4);
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
    fn pane_title_variants() {
        let single = Selection { cursor: 0, anchor: None };
        assert_eq!(pane_title("QUEUE", &single, "", false), "QUEUE");
        assert_eq!(pane_title("QUEUE", &single, "foo", false), "QUEUE /foo");
        assert_eq!(pane_title("QUEUE", &single, "fo", true), "QUEUE /fo█");
        assert_eq!(pane_title("QUEUE", &single, "", true), "QUEUE /█");
    }

    #[test]
    fn pane_title_selection_count() {
        let three = Selection { cursor: 4, anchor: Some(2) }; // rows 2..=4
        assert_eq!(pane_title("WORKTREES", &three, "", false), "WORKTREES · 3 selected");
        let two = Selection { cursor: 1, anchor: Some(2) };
        assert_eq!(pane_title("WORKTREES", &two, "tmp", false), "WORKTREES · 2 selected /tmp");
        let anchored_single = Selection { cursor: 3, anchor: Some(3) };
        assert_eq!(pane_title("QUEUE", &anchored_single, "", false), "QUEUE");
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
}
