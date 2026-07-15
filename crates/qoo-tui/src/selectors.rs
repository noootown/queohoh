use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use crate::ipc::types::{ArgSpec, DefinitionSummary, StateSnapshot, TaskInstance, TaskStatus};

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
    /// worktree name only (the `<repo>:` lane prefix dropped)
    pub worktree: String,
    /// task definition name; None for ad-hoc prompts
    pub def_name: Option<String>,
    pub summary: String,
    pub detail: String,
    /// creation epoch seconds (parsed from the daemon ISO timestamp)
    pub created_epoch_s: u64,
    pub archived: bool,
    /// task status — drives the ACTIVE-section status ordering and the
    /// active/finished section split (see [`queue_rows`]).
    pub status: TaskStatus,
    /// task priority string (`high`/`normal`/`low`) — the second ACTIVE-section
    /// sort key after status.
    pub priority: String,
    /// completion epoch seconds (parsed from the daemon `finishedAt`), `None`
    /// until the task finishes / on an old daemon — the FINISHED-section sort key.
    pub finished_epoch_s: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WtState {
    #[default]
    Free,
    Busy,
    You,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorktreeRow {
    /// display name (`<repo>.` prefix stripped) — never an identifier
    pub name: String,
    /// untouched worktree identifier used for every daemon action
    pub raw_name: String,
    pub path: String,
    /// "" for session rows (no real worktree)
    pub branch: String,
    pub state: WtState,
    pub queued: usize,
    pub is_session: bool,
    /// creation epoch of the task RUNNING on this lane, if any — the pane formats
    /// the live timer via `elapsed_label` against `now`.
    pub running_elapsed: Option<u64>,
    /// display name of the head-of-lane queued task (def name, else clipped
    /// prompt); paired with `queued > 0`.
    pub next_name: Option<String>,
    /// whether `next_name` is a definition name (colors it mauve vs a prompt fg).
    pub next_is_def: bool,
    /// most recent finished (non-running/non-queued) lane task, newest by id:
    /// (status glyph, display name, creation epoch for the relative-age label,
    /// whether the name is a def name — colors it mauve vs a prompt fg).
    pub last: Option<(char, String, u64, bool)>,
    /// git enrichment passthrough from the daemon (all `None` on an old daemon):
    /// uncommitted changes, last-commit epoch, last-commit author name + email.
    /// The author name/email feed the WORKTREES "mine-first" sort.
    pub dirty: Option<bool>,
    pub last_commit_epoch: Option<u64>,
    pub last_commit_author: Option<String>,
    pub last_commit_author_email: Option<String>,
    /// short hash + open PR number passthrough from the daemon (all `None` on an
    /// old daemon); surfaced in the worktree detail info tab.
    pub last_commit_hash: Option<String>,
    pub pr_number: Option<u64>,
    /// Web URL of the open PR (paired with `pr_number`; `None` on an old daemon
    /// or when there is no open PR). Drives the clickable `#<n>` link in the
    /// detail info tab and the WORKTREES PR column — a click opens it.
    pub pr_url: Option<String>,
    /// True when the daemon flagged this worktree as protected from deletion.
    /// Drives the 🔒 marker and gates the remove action. Session rows default
    /// `false` (never removable anyway).
    pub protected: bool,
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

/// The worker's exact `reason` string (`worker.ts`) for a run that hit its
/// configured timeout — matched verbatim against `task.error` so the queue/
/// worktree panes can pick the distinct timeout glyph without a run-detail
/// fetch (the snapshot already carries `error`).
const TIMED_OUT_REASON: &str = "timed out";
/// The worker's exact `reason` string (`worker.ts`'s `SESSION_LIMIT_RE` match)
/// for a run whose result text reported Claude's own session/usage limit.
const SESSION_LIMIT_REASON: &str = "session limit";
/// The worker's exact `reason` string (`worker.ts`'s `OUT_OF_BUDGET_RE` match)
/// for a run whose result text reported Anthropic's credit-balance/out-of-credits
/// billing error — distinct from a session limit (that resets on a timer; this
/// needs an account top-up before a rerun succeeds).
const OUT_OF_BUDGET_REASON: &str = "out of budget";

// Status glyphs MUST match the `GLYPH_*` consts in `view::theme` char-for-char —
// `theme::glyph_style` colors a row by matching this char. (Kept as literals to
// avoid a data-layer → view dependency; a test asserts the two stay in sync.)
// `Failed` is sub-classified by `task.error`'s exact text into two distinct
// red glyphs (session-limit, timed-out) so those cases read apart from a
// generic worker failure at a glance in the QUEUE pane and the WORKTREES
// pane's "Last Task" column — both render from this same snapshot data, no
// extra run-detail fetch needed.
fn status_glyph(task: &TaskInstance) -> char {
    match task.status {
        TaskStatus::Running => '▶',
        TaskStatus::Queued => '○',
        TaskStatus::NeedsInput => '‼',
        TaskStatus::Done => '●',
        TaskStatus::Failed => match task.error.as_deref() {
            Some(SESSION_LIMIT_REASON) => '⊠',
            Some(TIMED_OUT_REASON) => '⧗',
            Some(OUT_OF_BUDGET_REASON) => '¤',
            _ => '✗',
        },
        TaskStatus::Cancelled => '⊘',
        TaskStatus::Skipped => '⊝',
        TaskStatus::VerifyFailed => '⊗', // circled ✕ — the done-condition disagreed
        TaskStatus::Unknown => '·', // no TS counterpart (old-daemon statuses only)
    }
}

/// `repo:worktree-or-ref` with the redundant `<repo>.` display prefix stripped.
fn lane_label(task: &TaskInstance) -> String {
    let lane = task.target.worktree.as_deref().unwrap_or(&task.target.git_ref);
    format!("{}:{}", task.target.repo, strip_repo_prefix(lane, &task.target.repo))
}

/// ACTIVE-section status priority: running first, then needs-input, then queued.
/// Finished statuses share the max rank (they never sort in the active section).
fn status_active_rank(status: TaskStatus) -> u8 {
    match status {
        TaskStatus::Running => 0,
        TaskStatus::NeedsInput => 1,
        TaskStatus::Queued => 2,
        _ => 3,
    }
}

/// Priority sort rank: high first, then normal, then low (an unknown priority
/// string sorts with `normal`).
fn priority_rank(priority: &str) -> u8 {
    match priority {
        "high" => 0,
        "low" => 2,
        _ => 1, // "normal" and any unrecognized value
    }
}

/// Whether a queue row belongs to the FINISHED section: any terminal/unknown
/// status, or an archived row (archived rows are always finished). Cancelled and
/// Skipped are terminal — the daemon stamps `finishedAt` for both, so they sort
/// by completion in the finished section like Done/Failed (NOT in ACTIVE, which
/// is why they must be listed here explicitly and not fall through to `false`).
pub fn queue_row_finished(row: &QueueRow) -> bool {
    row.archived
        || matches!(
            row.status,
            TaskStatus::Done
                | TaskStatus::Failed
                | TaskStatus::VerifyFailed
                | TaskStatus::Cancelled
                | TaskStatus::Skipped
                | TaskStatus::Unknown
        )
}

/// Real-row index AFTER which the ACTIVE/FINISHED divider is drawn, or `None`
/// when either section is empty (no divider on a single-section queue). The rows
/// are already partitioned active-before-finished by [`queue_rows`], so this is
/// simply "one before the first finished row" — provided an active row precedes
/// it. Operates on the FINAL (post-filter) row list so filtering a whole section
/// away drops the divider with it.
pub fn queue_divider_after(rows: &[QueueRow]) -> Option<usize> {
    match rows.iter().position(queue_row_finished) {
        Some(first_finished) if first_finished > 0 => Some(first_finished - 1),
        _ => None,
    }
}

/// Comparator ordering the queue rows into two sections. ACTIVE
/// (running/needs-input/queued) sorts first — by status, then priority, then id
/// (ULID, stable). FINISHED (done/failed/cancelled/skipped/unknown + the archived
/// tail) sorts after
/// — by completion timestamp DESCENDING (most recently finished first); a row
/// without `finished_epoch_s` falls back to its id, newest first.
fn queue_sort(a: &QueueRow, b: &QueueRow) -> std::cmp::Ordering {
    let (fa, fb) = (queue_row_finished(a), queue_row_finished(b));
    fa.cmp(&fb).then_with(|| {
        if fa {
            // Both finished: newest completion first, then newest id first.
            b.finished_epoch_s
                .unwrap_or(0)
                .cmp(&a.finished_epoch_s.unwrap_or(0))
                .then_with(|| b.task_id.cmp(&a.task_id))
        } else {
            // Both active: status, then priority, then id ascending (stable).
            status_active_rank(a.status)
                .cmp(&status_active_rank(b.status))
                .then_with(|| priority_rank(&a.priority).cmp(&priority_rank(&b.priority)))
                .then_with(|| a.task_id.cmp(&b.task_id))
        }
    })
}

/// Count of currently-running tasks (from the daemon's authoritative `running`
/// id set) that belong to `project`. The concurrency cap is enforced PER PROJECT,
/// so the tabbar shows this against the cap rather than dividing the global
/// running total by the per-project number — that conflated two scopes and made
/// N tasks spread across N projects read as a saturated global cap (e.g. "3/3").
pub fn running_count_for(snapshot: &StateSnapshot, project: &str) -> usize {
    let running: HashSet<&str> = snapshot.running.iter().map(String::as_str).collect();
    snapshot
        .tasks
        .iter()
        .filter(|t| t.target.repo == project && running.contains(t.id.as_str()))
        .count()
}

/// Scheduled-plus-running task count for a project — the tabbar chip's `(n)`
/// suffix. Queued and Running rows count (work the scheduler will do or is
/// doing); needs-input and terminal rows do not.
pub fn active_count_for(snapshot: &StateSnapshot, project: &str) -> usize {
    snapshot
        .tasks
        .iter()
        .filter(|t| {
            t.target.repo == project
                && matches!(t.status, TaskStatus::Queued | TaskStatus::Running)
        })
        .count()
}

pub fn queue_rows(snapshot: &StateSnapshot, project: &str, now_epoch_s: u64) -> Vec<QueueRow> {
    // Live rows plus the last 10 archived rows (dimmed by the view via
    // `archived: true`), then sorted into an ACTIVE section (running/needs-input/
    // queued) followed by a FINISHED section (done/failed/cancelled/skipped/
    // unknown + archived).
    // The per-lane queue position (`#N in lane`) is computed in snapshot order
    // (creation order) BEFORE the display sort so it reflects execution order,
    // not the re-sorted display position.
    let mut queued_position: HashMap<String, usize> = HashMap::new();
    let mut rows: Vec<QueueRow> = Vec::new();
    for task in snapshot.tasks.iter().filter(|t| t.target.repo == project) {
        // The detail column is a live-progress hint only: elapsed for Running,
        // queue position for Queued. Done/Failed/NeedsInput/Unknown carry NOTHING
        // — the ✓/✗/? glyph already says the outcome and the full error/status
        // lives in the DETAIL pane; a trailing "done"/"exit code 1" duplicated it.
        let detail = match task.status {
            TaskStatus::Running => elapsed_label(run_start_epoch_s(task), now_epoch_s),
            TaskStatus::Queued => {
                let lane = lane_label(task);
                let position = queued_position.get(&lane).copied().unwrap_or(0) + 1;
                queued_position.insert(lane, position);
                format!("#{position} in lane")
            }
            _ => String::new(),
        };
        rows.push(QueueRow {
            task_id: task.id.clone(),
            glyph: status_glyph(task),
            running: task.status == TaskStatus::Running,
            worktree: strip_repo_prefix(
                task.target.worktree.as_deref().unwrap_or(&task.target.git_ref),
                &task.target.repo,
            )
            .to_string(),
            def_name: task.definition.as_deref().map(def_display_name),
            summary: prompt_summary(&task.prompt),
            detail,
            created_epoch_s: parse_iso_epoch_s(&task.created),
            archived: false,
            status: task.status,
            priority: task.priority.clone(),
            finished_epoch_s: task.finished_at.as_deref().map(parse_iso_epoch_s),
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
            glyph: status_glyph(task),
            running: false,
            worktree: strip_repo_prefix(
                task.target.worktree.as_deref().unwrap_or(&task.target.git_ref),
                &task.target.repo,
            )
            .to_string(),
            def_name: task.definition.as_deref().map(def_display_name),
            summary: prompt_summary(&task.prompt),
            // Archived rows carry no detail text (the dimming + glyph convey state).
            detail: String::new(),
            created_epoch_s: parse_iso_epoch_s(&task.created),
            archived: true,
            status: task.status,
            priority: task.priority.clone(),
            finished_epoch_s: task.finished_at.as_deref().map(parse_iso_epoch_s),
        });
    }
    // Sort into the ACTIVE then FINISHED sections (sort_by is stable; the id
    // tiebreak makes every comparison total regardless).
    rows.sort_by(queue_sort);
    rows
}

/// Display name for a task's definition: the daemon qualifies project-scoped
/// defs as `repo/name` (e.g. `platform/pr-ready`), but the scope carries no
/// meaning in the queue — show only the final segment.
pub fn def_display_name(definition: &str) -> String {
    definition.rsplit('/').next().unwrap_or(definition).to_string()
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
        // A failed done-condition reads as a failed lane, same as a worker failure.
        Some(t)
            if matches!(t.status, TaskStatus::Failed | TaskStatus::VerifyFailed) =>
        {
            WtState::Failed
        }
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

/// Run-start epoch of a task RUNNING on `lane` (the first in snapshot order), or
/// None. Drives the worktree row's live `⏱` timer — anchored on `started_at` so a
/// re-run's clock restarts from the re-run (see [`run_start_epoch_s`]).
fn running_elapsed_on_lane(snapshot: &StateSnapshot, lane: &str) -> Option<u64> {
    snapshot
        .tasks
        .iter()
        .find(|t| task_lane(t).as_deref() == Some(lane) && t.status == TaskStatus::Running)
        .map(run_start_epoch_s)
}

/// Display name of the head-of-lane queued task (first in snapshot order): the
/// def's short name, else the prompt clipped to `NEXT_NAME_CAP`. The bool is
/// whether the name came from a definition (drives mauve vs fg coloring).
fn next_queued_name_on_lane(snapshot: &StateSnapshot, lane: &str) -> Option<(String, bool)> {
    snapshot
        .tasks
        .iter()
        .find(|t| task_lane(t).as_deref() == Some(lane) && t.status == TaskStatus::Queued)
        .map(|t| lane_task_display_name(t, NEXT_NAME_CAP))
}

/// The lane's most recent FINISHED task (anything not running/queued) across both
/// the live and archived lists, newest by id (ULIDs sort chronologically):
/// (status glyph, display name, creation epoch).
fn last_finished_on_lane(snapshot: &StateSnapshot, lane: &str) -> Option<(char, String, u64, bool)> {
    snapshot
        .tasks
        .iter()
        .chain(snapshot.archived_recent.iter())
        .filter(|t| {
            task_lane(t).as_deref() == Some(lane)
                && !matches!(t.status, TaskStatus::Running | TaskStatus::Queued)
        })
        .max_by(|a, b| a.id.cmp(&b.id))
        .map(|t| {
            // Uncapped at derivation (prompt_summary already bounds it at 240):
            // the last-task cell is the pane's FILL column, so the render clips
            // to whatever width the row actually has — a 16-char pre-clip here
            // left the wide fill rendering blank padding (the exact complaint
            // that made this column the fill).
            let (name, is_def) = lane_task_display_name(t, usize::MAX);
            (status_glyph(t), name, parse_iso_epoch_s(&t.created), is_def)
        })
}

/// Display tuple for one lane task in the worktree detail's task list:
/// `(status glyph, name, name-is-def, creation epoch)`. Mirrors
/// [`last_finished_on_lane`]'s tuple but for a task of any status — the detail
/// pane lists every task on the lane, not just the last finished one.
pub(crate) fn lane_task_display(task: &TaskInstance) -> (char, String, bool, u64) {
    let (name, is_def) = lane_task_display_name(task, usize::MAX);
    (status_glyph(task), name, is_def, parse_iso_epoch_s(&task.created))
}

/// The "Live" cell for a lane task in the worktree detail list, mirroring the
/// QUEUE pane's live slot: `⏱ <elapsed>` for a running task, `#N in lane` for a
/// queued task, empty for any other status. `queue_pos` is the task's 1-based
/// position among the lane's queued tasks in creation order — the caller counts
/// queued tasks as it walks the (creation-ordered) list; it is only read for a
/// Queued task.
pub(crate) fn lane_task_live(task: &TaskInstance, now_epoch_s: u64, queue_pos: usize) -> String {
    match task.status {
        TaskStatus::Running => elapsed_label(run_start_epoch_s(task), now_epoch_s),
        TaskStatus::Queued => format!("#{queue_pos} in lane"),
        _ => String::new(),
    }
}

/// ACTIVE-vs-finished ordering rank for the worktree detail task list: running
/// first, then needs-input, then queued, then everything finished. Finished
/// tasks then sort newest-first at the call site (by id, descending).
pub(crate) fn lane_task_order_rank(status: TaskStatus) -> u8 {
    status_active_rank(status)
}

/// Whether a worktree row is "mine": the active project's `github_id`
/// (case-insensitive) is a SUBSTRING of the last-commit author email OR the
/// author name. `None`/empty `github_id` → always `false` (the mine-first sort
/// tier becomes a no-op). Substring on both fields per docs/setup.md (its example
/// `Ian Chiu <noootown@gmail.com>` matches `noootown` in the email and `Ian`/
/// `Chiu` in the name).
fn worktree_is_mine(row: &WorktreeRow, github_id: Option<&str>) -> bool {
    let Some(id) = github_id.filter(|s| !s.is_empty()) else {
        return false;
    };
    let id = id.to_lowercase();
    let contains = |field: &Option<String>| {
        field.as_deref().is_some_and(|v| v.to_lowercase().contains(&id))
    };
    contains(&row.last_commit_author_email) || contains(&row.last_commit_author)
}

/// The lane's most recent task-activity epoch: the later of the RUNNING task's
/// created epoch (`running_elapsed`) and the last FINISHED task's creation epoch
/// (the `last` tuple's epoch). `0` when the lane has had no task activity.
fn worktree_last_run_epoch(row: &WorktreeRow) -> u64 {
    let running = row.running_elapsed.unwrap_or(0);
    let finished = row.last.as_ref().map(|(_, _, epoch, _)| *epoch).unwrap_or(0);
    running.max(finished)
}

/// Three-level WORKTREES row ordering: (1) MINE first, (2) LAST-RUN time
/// descending (newest task activity), (3) LAST-COMMIT descending. Equal keys keep
/// their input order — the caller must use a STABLE sort. Pure over the row (+ the
/// project's `github_id`) so the ordering is unit-testable in isolation. Session
/// rows never pass through this comparator (they are appended after the sort).
fn cmp_worktree_rows(a: &WorktreeRow, b: &WorktreeRow, github_id: Option<&str>) -> Ordering {
    worktree_is_mine(b, github_id)
        .cmp(&worktree_is_mine(a, github_id)) // mine (true) sorts first
        .then_with(|| worktree_last_run_epoch(b).cmp(&worktree_last_run_epoch(a))) // newest run first
        .then_with(|| {
            b.last_commit_epoch
                .unwrap_or(0)
                .cmp(&a.last_commit_epoch.unwrap_or(0)) // newest commit first
        })
}

/// A lane task's short label plus whether it came from a definition: its def's
/// display name (`true`), else the first prompt line clipped to `cap` chars
/// (`false`). Shared by the head-of-lane and last-finished columns; the bool
/// drives mauve (def) vs fg (prompt) coloring in the worktree row.
fn lane_task_display_name(task: &TaskInstance, cap: usize) -> (String, bool) {
    match task.definition.as_deref() {
        Some(def) => (def_display_name(def), true),
        None => (clip(&prompt_summary(&task.prompt), cap), false),
    }
}

/// Clip width for the head-of-lane (`next:`) task name in the worktree pane —
/// its column is a fixed 30-cell slot, so the name is trimmed to keep the
/// `next:` lead visible. The last-finished cell is deliberately NOT
/// pre-clipped (it is the pane's FILL column; the render clips to the row).
pub const NEXT_NAME_CAP: usize = 24;

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

pub fn worktree_rows(snapshot: &StateSnapshot, project: &str) -> Vec<WorktreeRow> {
    let empty: Vec<crate::ipc::types::WorktreeInfo> = Vec::new();
    let worktrees = snapshot.worktrees.get(project).unwrap_or(&empty);
    // The active project's optional author identity, matched against each row's
    // last-commit author for the mine-first tier (absent → that tier no-ops).
    let github_id = snapshot
        .projects
        .iter()
        .find(|p| p.name == project)
        .and_then(|p| p.github_id.as_deref());
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
                queued: queued_on_lane(snapshot, &lane),
                is_session: false,
                running_elapsed: running_elapsed_on_lane(snapshot, &lane),
                next_name: next_queued_name_on_lane(snapshot, &lane).map(|(n, _)| n),
                next_is_def: next_queued_name_on_lane(snapshot, &lane)
                    .map(|(_, d)| d)
                    .unwrap_or(false),
                last: last_finished_on_lane(snapshot, &lane),
                dirty: wt.dirty,
                last_commit_epoch: wt.last_commit_epoch,
                last_commit_author: wt.last_commit_author.clone(),
                last_commit_author_email: wt.last_commit_author_email.clone(),
                last_commit_hash: wt.last_commit_hash.clone(),
                pr_number: wt.pr_number,
                pr_url: wt.pr_url.clone(),
                protected: wt.protected,
            }
        })
        .collect();

    // Three-level order (see `cmp_worktree_rows`): mine first, then most-recent
    // task activity, then most-recent commit. `sort_by` is STABLE, so equal-key
    // rows keep their daemon-emitted order (the tiebreak). Session rows (below)
    // keep their append-at-end placement — they never enter this ordering.
    rows.sort_by(|a, b| cmp_worktree_rows(a, b, github_id));

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
            queued: 0,
            is_session: true,
            // A session is not a real worktree: no lane tasks, no git enrichment.
            ..Default::default()
        });
    }
    rows
}

/// Minimum rows any expanded left pane keeps (border + title-on-border + two
/// content rows). With the title now embedded in the top border, four rows still
/// leaves two rows of content.
pub const PANE_MIN_H: u16 = 4;

/// Rows a collapsed pane occupies: just the title border row and the bottom
/// border. No content, no scrollbar.
pub const COLLAPSED_H: u16 = 2;

/// Heights of the three stacked left panes. Pure and re-clamped every frame so
/// session drag overrides never violate the invariants: every EXPANDED pane
/// ≥ `PANE_MIN_H`, every COLLAPSED pane is pinned to `COLLAPSED_H`, and the three
/// sum to exactly `body_height` (the last expanded pane — or, when all three are
/// collapsed, the last pane — absorbs the remainder).
///
/// With no pane collapsed and both overrides `None` this reproduces the historic
/// 2:1:1 default exactly (so default snapshots never move). `queue_h`/`tasks_h`
/// overrides are the requested heights from a divider drag; they are clamped, not
/// trusted. `collapsed` is `[queue, tasks, worktrees]`.
pub fn pane_layout(
    body_height: u16,
    queue_h: Option<u16>,
    tasks_h: Option<u16>,
    collapsed: [bool; 3],
) -> PaneLayout {
    const MIN: u16 = PANE_MIN_H;
    const COL: u16 = COLLAPSED_H;

    // No pane collapsed → the historic formula, byte-for-byte (default snapshots
    // and every legacy override test stay pinned).
    if collapsed == [false, false, false] {
        // Below room for three floors nobody can be satisfied; hand each the floor
        // and let the view's Length constraints clamp the overflow. Matches the old
        // default, which also produced (MIN,MIN,MIN) for any body ≤ 3·MIN.
        if body_height <= 3 * MIN {
            return PaneLayout { queue_h: MIN, tasks_h: MIN, worktrees_h: MIN };
        }
        // Default 2:1:1 heights, used for whichever override is absent.
        let def_tasks = std::cmp::max(MIN, body_height / 4);
        let def_queue = std::cmp::max(MIN, body_height.saturating_sub(2 * def_tasks));
        // Clamp queue into [MIN, body − 2·MIN] (leaves a floor each for tasks +
        // worktrees), then tasks into [MIN, body − queue − MIN] (leaves a floor for
        // worktrees). worktrees takes whatever is left — always ≥ MIN by construction.
        let q = queue_h.unwrap_or(def_queue).clamp(MIN, body_height - 2 * MIN);
        let t = tasks_h.unwrap_or(def_tasks).clamp(MIN, body_height - q - MIN);
        let w = body_height - q - t;
        return PaneLayout { queue_h: q, tasks_h: t, worktrees_h: w };
    }

    // Collapse-aware allocation. Collapsed panes are pinned to COL rows; the
    // expanded panes share what remains, each ≥ MIN, and the three heights sum to
    // exactly body_height.
    let ncol = collapsed.iter().filter(|&&c| c).count() as u16;
    let mut h = [0u16; 3];
    for (i, &c) in collapsed.iter().enumerate() {
        if c {
            h[i] = COL;
        }
    }
    let avail = body_height.saturating_sub(ncol * COL);
    let expanded: Vec<usize> = (0..3).filter(|&i| !collapsed[i]).collect();
    match expanded.as_slice() {
        // All three collapsed: no content pane. The leftover becomes a blank
        // filler region folded into the last pane's allocation (the collapsed bar
        // still renders only COL rows at its top, leaving the rest blank).
        [] => h[2] = h[2].saturating_add(avail),
        // One expanded pane takes everything left.
        [a] => h[*a] = std::cmp::max(MIN, avail),
        // Two expanded panes split `avail`: the upper honors its override (or an
        // even split), the lower absorbs the remainder. The upper index is always
        // 0 or 1, so it always has an override field; worktrees (index 2) is only
        // ever the lower of a pair.
        [a, b] => {
            let ov = match *a {
                0 => queue_h,
                1 => tasks_h,
                _ => None,
            };
            let hi = avail.saturating_sub(MIN).max(MIN);
            let ha = ov.unwrap_or(avail / 2).clamp(MIN, hi);
            h[*a] = ha;
            h[*b] = avail.saturating_sub(ha);
        }
        _ => unreachable!("at least one pane is collapsed in this branch"),
    }
    PaneLayout { queue_h: h[0], tasks_h: h[1], worktrees_h: h[2] }
}

/// Clamp a requested left-column width so both sides stay usable: left keeps
/// `MIN_LEFT`, DETAIL keeps `MIN_RIGHT`. The `.max(MIN_LEFT)` on the ceiling keeps
/// the range non-empty (so `clamp` never panics) even at the 60-col minimum.
pub fn clamp_left_cols(total_width: u16, want: u16) -> u16 {
    const MIN_LEFT: u16 = 24;
    const MIN_RIGHT: u16 = 30;
    let hi = total_width.saturating_sub(MIN_RIGHT).max(MIN_LEFT);
    want.clamp(MIN_LEFT, hi)
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

/// The pane's border title: the base plus a `· N selected` suffix when the pane
/// holds a BULK selection. `selected` is the union count (range ∪ marks) the
/// caller resolved via `view::selected_positions`; `bulk` is
/// `view::is_bulk_selection`. Both are passed in rather than derived here: a
/// mark-aware count needs the pane's rows, which this pure helper doesn't see.
/// The `/filter` + cursor decoration lives in the inline hint row (see
/// `view::panes`), so it is not part of the title.
///
/// `bulk` and `selected` can disagree: a mark is `bulk` by presence even when
/// it resolves to no visible row (e.g. filtered out by search), in which case
/// `selected` is 0. The suffix only renders when `selected > 0` — "· 0
/// selected" would be nonsensical, and the status line already explains any
/// blocked action.
pub fn pane_title(base: &str, selected: usize, bulk: bool) -> String {
    if bulk && selected > 0 {
        format!("{base} · {selected} selected")
    } else {
        base.to_string()
    }
}

/// The QUEUE pane's title-bar summary: outstanding work at a glance —
/// `N queued · N running` (counts over the pane's rows, so an active filter
/// summarizes what is shown).
pub fn queue_pane_summary(rows: &[QueueRow]) -> String {
    let queued = rows.iter().filter(|r| r.status == TaskStatus::Queued).count();
    let running = rows.iter().filter(|r| r.running).count();
    format!("{queued} queued · {running} running")
}

/// The TASKS pane's title-bar summary: the definition count — `N tasks`.
pub fn tasks_pane_summary(defs: &[DefinitionSummary]) -> String {
    let n = defs.len();
    if n == 1 { "1 task".to_string() } else { format!("{n} tasks") }
}

/// The WORKTREES pane's title-bar summary: `N busy · N total`. Busy = a task is
/// running on the lane; total counts real worktrees only (session rows are not
/// worktrees).
pub fn wt_pane_summary(rows: &[WorktreeRow]) -> String {
    let busy = rows.iter().filter(|r| r.running_elapsed.is_some()).count();
    let total = rows.iter().filter(|r| !r.is_session).count();
    format!("{busy} busy · {total} total")
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

/// Sentinel worktree name for a task targeting the project's primary checkout
/// (mirror of core's `REPO_SENTINEL`). Never a real worktree — display it as
/// the bare repo name, matching how the primary checkout appears elsewhere.
pub const REPO_SENTINEL: &str = "@repo";

pub fn strip_repo_prefix<'a>(worktree: &'a str, repo: &'a str) -> &'a str {
    if worktree == REPO_SENTINEL {
        return repo;
    }
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

/// First non-blank line of the prompt, trimmed, clipped to ≤240 chars with `…`.
/// The generous cap only bounds pathological one-line prompts — the queue's
/// summary column does the real width-fitting per frame, so the summary can
/// flex across however much row the pane has.
pub fn prompt_summary(prompt: &str) -> String {
    let line = prompt
        .lines()
        .find(|l| !l.trim().is_empty())
        .map(str::trim)
        .unwrap_or("");
    let chars: Vec<char> = line.chars().collect();
    if chars.len() <= 240 {
        return line.to_string();
    }
    let mut out: String = chars[..239].iter().collect();
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

/// Epoch (seconds) the task's CURRENT run started: its `started_at` (re-stamped
/// on every (re-)run when the worker flips it to `running`), falling back to
/// `created` when absent — a task that never ran, or an old daemon that omits the
/// field. Anchoring the live `⏱` timer here means a re-run's clock restarts from
/// the re-run, not the original creation — so a re-run doesn't inherit hours of
/// phantom elapsed and read as if it were about to hit the 3h wall-clock ceiling.
fn run_start_epoch_s(task: &TaskInstance) -> u64 {
    parse_iso_epoch_s(task.started_at.as_deref().unwrap_or(&task.created))
}

/// Parse a daemon ISO-8601 UTC timestamp ("YYYY-MM-DDTHH:MM:SS[.mmm]Z") into
/// epoch seconds. No date crate: Howard Hinnant's days-from-civil algorithm.
pub fn parse_iso_epoch_s(iso: &str) -> u64 {
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

/// Inverse of `days_from_civil`: (year, month, day) for a days-since-epoch count
/// (Howard Hinnant's `civil_from_days`).
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// "MM/DD HH:MM" in local time. `utc_offset_s` is injected so tests are
/// deterministic (production passes the real local offset).
pub fn absolute_local_label(created_epoch_s: u64, utc_offset_s: i32) -> String {
    let local = created_epoch_s as i64 + utc_offset_s as i64;
    let days = local.div_euclid(86_400);
    let secs = local.rem_euclid(86_400);
    let (_, m, d) = civil_from_days(days);
    let hh = secs / 3600;
    let mm = (secs % 3600) / 60;
    format!("{m:02}/{d:02} {hh:02}:{mm:02}")
}

/// "just now" / "5m ago" / "1h ago" / "2d ago".
pub fn relative_age_label(created_epoch_s: u64, now_epoch_s: u64) -> String {
    let delta = now_epoch_s.saturating_sub(created_epoch_s);
    if delta < 60 {
        "just now".to_string()
    } else if delta < 3600 {
        format!("{}m ago", delta / 60)
    } else if delta < 86_400 {
        format!("{}h ago", delta / 3600)
    } else {
        format!("{}d ago", delta / 86_400)
    }
}

// ---- cron humanizer (pure) -----------------------------------------------------

/// `HH:MM` on a 12-hour clock with an `am`/`pm` suffix; the `:MM` is dropped when
/// the minute is zero. e.g. `(30, 13) → "1:30pm"`, `(0, 9) → "9am"`, `(0, 0) → "12am"`.
fn fmt_time(min: u32, hour: u32) -> String {
    let ampm = if hour < 12 { "am" } else { "pm" };
    let h12 = match hour % 12 {
        0 => 12,
        h => h,
    };
    if min == 0 {
        format!("{h12}{ampm}")
    } else {
        format!("{h12}:{min:02}{ampm}")
    }
}

/// Abbreviated weekday for a cron day-of-week number (0 or 7 == Sunday).
fn day_name(d: u32) -> &'static str {
    match d % 7 {
        0 => "Sun",
        1 => "Mon",
        2 => "Tue",
        3 => "Wed",
        4 => "Thu",
        5 => "Fri",
        _ => "Sat",
    }
}

/// `1 → "1st"`, `2 → "2nd"`, `3 → "3rd"`, `11..=13 → "…th"`, else `"…th"`.
fn ordinal(n: u32) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

/// A cron field made only of the characters a standard schedule uses.
fn is_cron_field(f: &str) -> bool {
    !f.is_empty()
        && f.chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '*' | '/' | ',' | '-'))
}

/// Parse a field that must be a single integer in `[lo, hi]`; `None` otherwise
/// (a range/step/list is not a single value).
fn single(f: &str, lo: u32, hi: u32) -> Option<u32> {
    let v: u32 = f.parse().ok()?;
    (lo..=hi).contains(&v).then_some(v)
}

/// The `N` of a `*/N` step field, or `None` when the field isn't a step.
fn step(f: &str) -> Option<u32> {
    f.strip_prefix("*/").and_then(|s| s.parse().ok())
}

/// A comma list whose members are exactly {Sat, Sun} (any 0/6/7 spelling).
fn is_weekend(dow: &str) -> bool {
    let mut days: Vec<u32> = dow
        .split(',')
        .filter_map(|s| s.parse::<u32>().ok())
        .map(|d| d % 7)
        .collect();
    days.sort_unstable();
    days.dedup();
    days == [0, 6]
}

/// Best-effort humanization of the five parsed cron fields. Returns `None` when
/// the pattern is a valid cron shape we don't confidently phrase — `cron_human`
/// then falls back to the raw expression rather than dropping it.
fn humanize_fields(f: &[&str; 5]) -> Option<String> {
    let [m, h, dom, mon, dow] = *f;
    let all_dmw = dom == "*" && mon == "*" && dow == "*";

    // Frequency tiers: minute/hour carry a step or the top-of-hour marker. A
    // tuple match keeps the arms flat (nested ifs here would be collapsible).
    if all_dmw {
        match (step(m), step(h), m, h) {
            (Some(n), _, _, "*") => return Some(format!("Every {n}m")),
            (_, _, "0", "*") => return Some("Hourly".to_string()),
            (_, Some(n), "0", _) => return Some(format!("Every {n}h")),
            _ => {}
        }
    }

    // Time-of-day tiers need a concrete minute + hour.
    let time = fmt_time(single(m, 0, 59)?, single(h, 0, 23)?);

    if dom == "*" && mon == "*" {
        if dow == "*" {
            return Some(format!("Everyday {time}"));
        }
        if dow == "1-5" {
            return Some(format!("Weekdays {time}"));
        }
        if is_weekend(dow) {
            return Some(format!("Weekends {time}"));
        }
        if let Some(d) = single(dow, 0, 7) {
            return Some(format!("{} {time}", day_name(d)));
        }
        return None; // an unhandled day-of-week list → raw fallback
    }

    if mon == "*" && dow == "*" {
        return single(dom, 1, 31).map(|d| format!("Monthly {} {time}", ordinal(d)));
    }

    None
}

/// Turn a standard 5-field cron expression into a short human phrase for the
/// TASKS schedule column. Best-effort: recognized patterns get a friendly phrase
/// (`"30 13 * * *" → "Everyday 1:30pm"`), any other valid-shaped cron falls back
/// to the raw expression, and empty/non-cron input returns `None` (showing
/// nothing beats noise). See the unit tests for the full tier table.
pub fn cron_human(expr: &str) -> Option<String> {
    let expr = expr.trim();
    if expr.is_empty() {
        return None;
    }
    let fields: Vec<&str> = expr.split_whitespace().collect();
    if fields.len() != 5 || !fields.iter().all(|f| is_cron_field(f)) {
        return None; // not a standard 5-field cron
    }
    let five = [fields[0], fields[1], fields[2], fields[3], fields[4]];
    Some(humanize_fields(&five).unwrap_or_else(|| expr.to_string()))
}

// ---- column layout (pure, per-frame, computed from the VISIBLE rows) ----------
//
// Every content glyph the list rows use (▶ ✓ ✗ ○ ? · ⛓ ⏱ ◆ ●) measures one
// terminal cell, so column widths can be reasoned about in chars. Truncation is
// char-based (never byte slicing) so unicode text can't panic.

/// Char count of `s` (== cell width for the row content we render).
fn cw(s: &str) -> usize {
    s.chars().count()
}

/// Clip `s` to `width` chars, appending `…` when truncated (mirrors
/// `prompt_summary`). `width == 0` yields the empty string.
pub fn clip(s: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= width {
        return s.to_string();
    }
    if width == 1 {
        return "…".to_string();
    }
    let mut out: String = chars[..width - 1].iter().collect();
    out.push('…');
    out
}

/// Left-align `s` in a `width`-char field: clip if too long, right-pad with
/// spaces if short. The result is always exactly `width` chars.
pub fn pad_clip(s: &str, width: usize) -> String {
    let mut out = clip(s, width);
    let n = out.chars().count();
    if n < width {
        out.extend(std::iter::repeat_n(' ', width - n));
    }
    out
}

/// Largest char-width among `values`, capped at `cap` (0 if the iterator is
/// empty). Used to size the name/worktree/def columns to the widest visible cell.
fn capped_max<'a>(values: impl Iterator<Item = &'a str>, cap: usize) -> usize {
    values.map(cw).max().unwrap_or(0).min(cap)
}

pub const WORKTREE_CAP: usize = 28;
pub const DEF_CAP: usize = 20;
pub const NAME_CAP: usize = 48;
/// Max width of the humanized schedule text in the TASKS pane. A raw-cron
/// fallback longer than this is clipped with `…` rather than blowing out the
/// row.
pub const SCHED_CAP: usize = 20;
/// Max width of the model cell in the TASKS pane (the `claude-` prefix is
/// stripped first, so real values are short: `sonnet`, `opus`, `fable-5`,
/// `opus-4-8`). Clipped with `…` if somehow longer.
pub const MODEL_CAP: usize = 20;
pub const SUMMARY_MIN: usize = 10;
/// Gutter between adjacent field columns (glyph/chain markers keep single
/// spaces; the field columns get a wider gap so they read as columns).
pub const COL_GAP: usize = 2;
/// Fixed width of the absolute timestamp column (`MM/DD HH:MM`).
pub const TIMESTAMP_W: usize = 11;

// ---- FIXED reserved widths for the metadata/marker/time/live columns --------
//
// Column PRESENCE is a function of the pane width (and, for capability columns,
// whole-pane data availability) — never of an individual row's data. A row that
// lacks a value renders blanks (`pad_clip("", W)`) in its reserved cell, so a
// timer appearing or a wide value scrolling in never shifts any other column.
// Values fit the realistic max label under `cw`.

/// Live timer column (`⏱ 99h59m`: ⏱ + space + up-to-2-digit hours).
pub const TIMER_W: usize = 8;
/// Relative-age column (`relative_age_label` max is `just now` = 8).
pub const AGE_W: usize = 8;
/// Last-commit author column (fixed reserved width; longer names clip with `…`).
pub const AUTHOR_W: usize = 14;
/// Last-commit relative-age column (`relative_age_label` max `just now`).
pub const COMMIT_AGE_W: usize = 8;
/// Open-PR column (`#<n>`): a fixed reserved width like author/commit-age.
/// Sized for a 5-digit PR number plus the `#` (`#12345`); longer numbers clip.
pub const PR_W: usize = 6;
/// Shared QUEUE live slot: `⏱ 99h59m` (8) or `#N in lane` (`#9 in lane` = 10);
/// `#10 in lane` and beyond clip.
pub const QUEUE_LIVE_W: usize = 10;
/// Fill floor for the worktrees `next:` column: the minimum width that
/// keeps it "present" (below this the ladder drops it before dirty/live). It is
/// a flex column — this is only its minimum, not a reserved fixed width.
/// Fixed reserved width of the worktrees `next: <name>` column (always a
/// candidate, blank when nothing is queued — data-independent so a queued task
/// appearing never shifts columns). The queued count is NOT shown here — the
/// leading indicator digit already carries it. Fits "next: " plus a ~24-char
/// name; longer names clip with `…`.
const WT_QUEUED_W: usize = 30;
/// Floor of the worktrees last-task FILL column (the pane's flex column — it
/// absorbs remaining width like the queue pane's summary, per user request).
const WT_LAST_MIN: usize = 12;

/// Resolved per-frame column widths for the QUEUE pane. A width of `0` (or
/// `false`) means the column is omitted for this frame; `summary_w` is the flex
/// remainder. Computed from the windowed (visible) rows so alignment tracks what
/// is actually on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueueColLayout {
    pub worktree_w: usize,
    pub def_w: usize,
    pub summary_w: usize,
    pub show_timestamp: bool,
    /// `AGE_W` when the relative-age column is kept, else 0 (fixed width).
    pub age_w: usize,
    /// `QUEUE_LIVE_W` when the trailing live slot is kept, else 0 (fixed width).
    /// Renders `row.detail` — `⏱ <elapsed>` (running) or `#N in lane` (queued).
    pub live_w: usize,
}

/// Fit the QUEUE columns into `avail` inner cells. The identity/content columns
/// (glyph, optional ⛓ chain, worktree, def) are sized to the widest visible value
/// (capped so the summary keeps room); the summary flexes into what remains. The
/// trailing timestamp / age / live columns have FIXED reserved widths (never
/// sized from row data) — their PRESENCE is decided purely by the width ladder,
/// so a row gaining a timer or a wider value never shifts any column. When space
/// is tight the trailing columns degrade in a fixed order — timestamp, then age,
/// then live — so the summary keeps at least `SUMMARY_MIN` cells; only if that
/// still isn't enough does def drop and then worktree shrink.
pub fn queue_col_layout(rows: &[QueueRow], avail: usize, _now_epoch_s: u64) -> QueueColLayout {
    let worktree_w = capped_max(rows.iter().map(|r| r.worktree.as_str()), WORKTREE_CAP);
    let mut def_w = capped_max(rows.iter().filter_map(|r| r.def_name.as_deref()), DEF_CAP);

    // Non-flex prefix width: glyph(1) + worktree(+gutter) + def(+gutter) + the
    // gutter before the summary. The summary itself is the remainder.
    let prefix = |worktree_w: usize, def_w: usize| -> usize {
        1 + if worktree_w > 0 { COL_GAP + worktree_w } else { 0 }
            + if def_w > 0 { COL_GAP + def_w } else { 0 }
            + COL_GAP
    };
    // Summary width given the current column choices (may be negative → too tight).
    // Trailing columns are fixed-width: timestamp=TIMESTAMP_W, age=AGE_W,
    // live=QUEUE_LIVE_W — each present as a bool.
    let summary_of =
        |worktree_w: usize, def_w: usize, show_ts: bool, age_w: usize, live_w: usize| -> isize {
            let mut used = prefix(worktree_w, def_w) as isize;
            if show_ts {
                used += (COL_GAP + TIMESTAMP_W) as isize;
            }
            if age_w > 0 {
                used += (COL_GAP + age_w) as isize;
            }
            if live_w > 0 {
                used += (COL_GAP + live_w) as isize;
            }
            avail as isize - used
        };

    let min = SUMMARY_MIN as isize;
    let mut show_timestamp = true;
    let mut age_w = AGE_W;
    let mut live_w = QUEUE_LIVE_W;
    let mut worktree_w = worktree_w;

    // Trailing columns degrade first: timestamp, then age, then live.
    if summary_of(worktree_w, def_w, show_timestamp, age_w, live_w) < min {
        show_timestamp = false;
    }
    if summary_of(worktree_w, def_w, show_timestamp, age_w, live_w) < min {
        age_w = 0;
    }
    if summary_of(worktree_w, def_w, show_timestamp, age_w, live_w) < min {
        live_w = 0;
    }
    // Still cramped → drop def, then shrink worktree toward the summary floor.
    if summary_of(worktree_w, def_w, show_timestamp, age_w, live_w) < min {
        def_w = 0;
    }
    let s = summary_of(worktree_w, def_w, show_timestamp, age_w, live_w);
    if s < min && worktree_w > 0 {
        worktree_w = worktree_w.saturating_sub((min - s) as usize);
    }
    let summary_w = summary_of(worktree_w, def_w, show_timestamp, age_w, live_w).max(0) as usize;

    QueueColLayout { worktree_w, def_w, summary_w, show_timestamp, age_w, live_w }
}

/// Minimum name column the worktree detail lane-task rows keep before a trailing
/// column is dropped to make room.
const LANE_NAME_MIN: usize = 6;

/// Resolved column widths for one worktree-detail lane-task row: the flex `Task`
/// name (`name_w`) after the `<glyph> ` prefix, then the fixed trailing columns
/// `Created` (`TIMESTAMP_W`), `Age` (`AGE_W`), `Live` (`QUEUE_LIVE_W`) — the same
/// widths and `COL_GAP` gutters the QUEUE pane uses. A width of `0` omits that
/// column. Shared by the row and header stylers so the two align cell-for-cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LaneTaskCols {
    pub name_w: usize,
    pub created_w: usize,
    pub age_w: usize,
    pub live_w: usize,
}

/// Fit a lane-task row into `width` cells. The trailing columns are fixed-width
/// (never sized from row data) and degrade in a fixed order — Live, then Created,
/// then Age — so the `Task` name keeps at least [`LANE_NAME_MIN`] cells; the name
/// is the flex remainder. Pure over `width` (the ideal unit-test target).
pub(crate) fn lane_task_cols(width: usize) -> LaneTaskCols {
    const PREFIX: usize = 2; // `<glyph> ` (glyph + one space)
    let mut created_w = TIMESTAMP_W;
    let mut age_w = AGE_W;
    let mut live_w = QUEUE_LIVE_W;
    let trailing = |c: usize, a: usize, l: usize| {
        (if c > 0 { COL_GAP + c } else { 0 })
            + (if a > 0 { COL_GAP + a } else { 0 })
            + (if l > 0 { COL_GAP + l } else { 0 })
    };
    // Drop trailing columns (live → created → age) until the name floor fits.
    for op in 0..3 {
        if PREFIX + LANE_NAME_MIN + trailing(created_w, age_w, live_w) <= width {
            break;
        }
        match op {
            0 => live_w = 0,
            1 => created_w = 0,
            _ => age_w = 0,
        }
    }
    let name_w = width.saturating_sub(PREFIX + trailing(created_w, age_w, live_w));
    LaneTaskCols { name_w, created_w, age_w, live_w }
}

/// Resolved column widths for the TASKS pane: `name | model | description |
/// schedule`. `name_w`/`model_w` are content-capped columns; `desc_w` is the
/// FILL (the remainder, like the queue pane's summary — prose gets the slack),
/// 0 when no visible def has a description or the pane is too narrow to spare
/// any. `model_w` sits right after the name (user request: the model matters
/// more than anything else on the row), pane-gated (reserved only while some
/// visible def carries a model, blank on a def without one so the columns never
/// slide). The args column was dropped from the row entirely (user request —
/// args still show in the def picker rows and the detail config tab). The
/// schedule stays the trailing capped column (`sched_w` sizes the humanized
/// cron — see [`def_sched_text`]). Narrow-pane drop order:
/// the desc FILL shrinks to 0 first, then the model column drops, then `name_w`
/// shrinks last; the schedule column is always kept. A width of 0 means that
/// column is omitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DefColLayout {
    pub name_w: usize,
    pub desc_w: usize,
    pub model_w: usize,
    pub sched_w: usize,
    /// Front `⌕` discovery-marker slot — 2 cells, glyph + separator — reserved
    /// pane-wide when any visible def has discovery; 0 otherwise. Mirrors the
    /// worktree `±` dirty slot (`WtColLayout::dirty_w`).
    pub marker_w: usize,
}

/// The description cell text for a def ("" when it has none). Prose, rendered in
/// plain fg and filling the remaining pane width (truncated with `…` when tight).
pub fn def_desc_text(def: &DefinitionSummary) -> String {
    def.description.clone().unwrap_or_default()
}

/// The model cell text for a def ("" when the summary has no model, e.g. an old
/// daemon). The `claude-` prefix is stripped so the column stays narrow
/// (`claude-fable-5` → `fable-5`); short aliases like `sonnet`/`opus` pass
/// through unchanged.
pub fn def_model_text(def: &DefinitionSummary) -> String {
    match def.model.as_deref() {
        Some(m) if !m.is_empty() => m.strip_prefix("claude-").unwrap_or(m).to_string(),
        _ => String::new(),
    }
}

/// Trailing schedule-cell text for a def row: the humanized cron schedule, or
/// empty when the def has none. Single source for BOTH the layout width
/// ([`def_col_layout`]) and the rendered cell ([`crate::view::panes`]). The
/// `⌕` discovery marker lives in the row's front marker slot (`marker_w`),
/// not here.
pub fn def_sched_text(def: &DefinitionSummary) -> String {
    def.cron.as_deref().and_then(cron_human).unwrap_or_default()
}

pub fn def_col_layout(rows: &[DefinitionSummary], avail: usize) -> DefColLayout {
    let name_w0 = capped_max(rows.iter().map(|d| d.name.as_str()), NAME_CAP);
    let sched_w = rows.iter().map(|d| cw(&def_sched_text(d))).max().unwrap_or(0).min(SCHED_CAP);
    // Trailing schedule column footprint (right-pinned by the desc fill): the
    // humanized cron (see `def_sched_text` — layout and render share it).
    // Blank for a def with none.
    let has_sched = sched_w > 0;
    let sched_col = if has_sched { sched_w } else { 0 };
    // Front `⌕` discovery-marker slot: 2 cells (glyph + separator), reserved
    // pane-wide when any visible def has discovery — mirrors the worktree `±`
    // dirty slot. It sits before the name with no COL_GAP of its own (the slot
    // already embeds its separator space).
    let marker_w = if rows.iter().any(|d| d.has_discovery) { 2 } else { 0 };
    // The desc FILL is present only when some visible def actually has a
    // description (else the schedule keeps its today-position, no layout shift).
    let has_desc = rows.iter().any(|d| d.description.as_deref().is_some_and(|s| !s.is_empty()));
    // Model column: fixed, pane-gated on whole-pane data (widest model cell, 0
    // pane-wide when no visible def carries a model — e.g. an old daemon).
    let model_w0 = rows.iter().map(|d| cw(&def_model_text(d))).max().unwrap_or(0).min(MODEL_CAP);

    // Cells used by the fixed (non-fill) columns for a given name/model width.
    let used_wo_desc = |name_w: usize, model_w: usize| -> usize {
        marker_w
            + name_w
            + if model_w > 0 { COL_GAP + model_w } else { 0 }
            + if sched_col > 0 { COL_GAP + sched_col } else { 0 }
    };
    // Reclaim when even the fixed columns overflow: drop model, then shrink
    // name. (The desc fill has already implicitly shrunk to 0 — it is only ever
    // the leftover remainder below.)
    let mut model_w = model_w0;
    if used_wo_desc(name_w0, model_w) > avail {
        model_w = 0;
    }
    let mut name_w = name_w0;
    let u = used_wo_desc(name_w, model_w);
    if u > avail {
        name_w = name_w.saturating_sub(u - avail);
    }
    // Description is the FILL: the remainder after name/model/schedule and its
    // leading gutter. Zero when absent or when nothing is left to give it.
    let desc_w = if has_desc {
        avail.saturating_sub(used_wo_desc(name_w, model_w) + COL_GAP)
    } else {
        0
    };

    DefColLayout { name_w, desc_w, model_w, sched_w, marker_w }
}

/// The last-commit author cell text, or None when the daemon didn't supply it
/// (an old daemon, or a worktree whose `git log` failed) — the whole column is
/// then omitted pane-wide.
pub fn wt_author_text(row: &WorktreeRow) -> Option<String> {
    row.last_commit_author.clone()
}

/// Resolved per-frame column widths for the WORKTREES pane. A width of `0` means
/// the column is omitted this frame.
///
/// Columns, left→right (identity → content → time → live):
///   `● ± name` (anchor; the `±` dirty marker is a single-cell front slot after
///   the dot, per user request),
///   last-finished (FILL), PR `#<n>` (fixed `PR_W`), last-commit author
///   (fixed `AUTHOR_W`), last-commit age (fixed `COMMIT_AGE_W`),
///   `next: <name>` (fixed `WT_QUEUED_W`), live `⏱` (fixed `TIMER_W`,
///   right-pinned by the fill). The PR column sits immediately LEFT of the
///   author (between the fill and author) so the open-PR chip reads before the
///   who·when pair; the author sits right before the commit-age so the pair
///   reads `koshea  3d ago` = who · when.
///
/// The marker/time columns (`dirty`, `pr`, `queued`, `author`, `commit_age`,
/// `elapsed`) are FIXED widths — never sized from row data — so a row gaining a
/// value never shifts any column; `name_w` stays content-capped and `last_w` is
/// the FILL column (absorbs the remaining width, like the queue pane's summary
/// — per user request the last task's description gets the slack). The front
/// `±` marker slot and the live timer are ALWAYS reserved when the ladder
/// keeps them (per user request — data-gated slots made the name column shift
/// as scroll/data changed); pr/queued/author/commit-age stay pane-gated
/// (reserved only while some visible row carries the value).
/// Degradation drop priority (first dropped first): commit-age → author → PR
/// → queued·next → dirty → last-finished → live; only after all of those drop
/// does `name_w` shrink. PR outlives author/commit-age dropping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WtColLayout {
    pub name_w: usize,
    pub dirty_w: usize,
    pub elapsed_w: usize,
    pub queued_w: usize,
    pub last_w: usize,
    /// PR `#<n>` column (`PR_W` when some visible row has an open PR, else 0).
    /// Positioned between the last-task fill and the author column.
    pub pr_w: usize,
    pub author_w: usize,
    pub commit_age_w: usize,
}

impl WtColLayout {
    /// Char offset from the row start to the first cell of the PR `#<n>` value —
    /// the SINGLE source of truth shared by `worktree_line` (which lays the cell
    /// out) and `render_rows` (which registers the click rect), so the two can
    /// never drift. Mirrors the span widths `worktree_line` pushes before the PR
    /// cell: the anchor (`● ` + the `±` front slot + the name), then the
    /// last-task FILL when present, then the `COL_GAP` before the PR column.
    /// Meaningless when `pr_w == 0` (the column is absent); callers gate on that.
    pub fn pr_col_x(&self) -> usize {
        let anchor = 2 + if self.dirty_w > 0 { 2 } else { 0 } + self.name_w;
        let after_last = if self.last_w > 0 { COL_GAP + self.last_w } else { 0 };
        anchor + after_last + COL_GAP
    }
}

/// Fit the WORKTREES columns into `avail` inner cells (see [`WtColLayout`] for the
/// column order, fixed-width model, and drop priority). The live `⏱` column and
/// the last-task fill are always candidates; the dirty/pr/queued/author/
/// commit-age columns stay gated on whole-pane data availability.
pub fn wt_col_layout(rows: &[WorktreeRow], avail: usize) -> WtColLayout {
    let name_w0 = capped_max(rows.iter().map(|r| r.name.as_str()), NAME_CAP);
    // Fixed marker/time widths. The `±` front slot and the live timer are
    // statically reserved (blank when a row has no value); queued/author/
    // commit-age stay gated on whole-pane data availability.
    // STATICALLY reserved (user request): gating this slot on visible-row data
    // made the name column shift whenever a dirty flag flipped or scrolling
    // changed which rows were visible. The width ladder may still drop it under
    // width pressure (geometry-driven, not data-driven).
    let dirty_w0 = if rows.is_empty() { 0 } else { 1 };
    let elapsed_w0 = TIMER_W; // live timer: always reserved when the ladder keeps it
    // Queued·next is pane-gated like dirty/author/commit-age: its fixed slot is
    // reserved only while some visible row actually has a queued task. Always
    // reserving it burned 30 blank cells that the last-task FILL should be
    // stretching into (user feedback); rows shift only on the slot's first-ever
    // pane-wide appearance, same accepted tradeoff as the others.
    let queued_w0 = if rows.iter().any(|r| r.queued > 0) { WT_QUEUED_W } else { 0 };
    let author_w0 = if rows.iter().any(|r| wt_author_text(r).is_some()) { AUTHOR_W } else { 0 };
    let commit_w0 = if rows.iter().any(|r| r.last_commit_epoch.is_some()) { COMMIT_AGE_W } else { 0 };
    // PR is pane-gated like author/commit-age: reserved only while some visible
    // row carries an open PR number. It survives author/commit-age dropping (it
    // drops third, after them) — an open PR is the more actionable signal.
    let pr_w0 = if rows.iter().any(|r| r.pr_number.is_some()) { PR_W } else { 0 };

    // Anchor width: `● ` (dot + space) + the `± ` (dirty) front marker — a single
    // cell + space when present — then the name. The marker sits up front per
    // user request, not as a mid-row column.
    let anchor = |name_w: usize, dirty: bool| 2 + if dirty { 2 } else { 0 } + name_w;
    // Used cells for a set of column widths and whether the last-task FILL is
    // reserved (at its `WT_LAST_MIN` floor — the actual fill absorbs the slack).
    // cols = [queued, author, commit]; `pr` is the fixed PR column and `elapsed`
    // the trailing fixed live column (both position-independent in the total).
    let used = |name_w: usize, dirty: bool, cols: [usize; 3], pr_w: usize, elapsed_w: usize, last: bool| -> usize {
        let mut u = anchor(name_w, dirty);
        for w in cols {
            if w > 0 {
                u += COL_GAP + w;
            }
        }
        if pr_w > 0 {
            u += COL_GAP + pr_w;
        }
        if last {
            u += COL_GAP + WT_LAST_MIN;
        }
        if elapsed_w > 0 {
            u += COL_GAP + elapsed_w;
        }
        u
    };

    // Degrade in drop order: commit → author → pr → queued → dirty → last →
    // elapsed. cols = [queued(0), author(1), commit(2)]; PR drops after author
    // (so it outlives the who·when pair) but before queued·next.
    let mut cols = [queued_w0, author_w0, commit_w0];
    let mut pr_w = pr_w0;
    let mut dirty = dirty_w0 > 0;
    let mut elapsed_w = elapsed_w0;
    let mut last = true;
    #[derive(Clone, Copy)]
    enum Drop {
        Col(usize),
        Pr,
        Dirty,
        Last,
        Elapsed,
    }
    for op in
        [Drop::Col(2), Drop::Col(1), Drop::Pr, Drop::Col(0), Drop::Dirty, Drop::Last, Drop::Elapsed]
    {
        if used(name_w0, dirty, cols, pr_w, elapsed_w, last) <= avail {
            break;
        }
        match op {
            Drop::Col(i) => cols[i] = 0,
            Drop::Pr => pr_w = 0,
            Drop::Dirty => dirty = false,
            Drop::Last => last = false,
            Drop::Elapsed => elapsed_w = 0,
        }
    }
    // Still too wide with only `● ± name` left → shrink the name column.
    let mut name_w = name_w0;
    let u = used(name_w, dirty, cols, pr_w, elapsed_w, last);
    if u > avail {
        name_w = name_w.saturating_sub(u - avail);
    }
    // The last-task column is the FILL: the remainder after every reserved column
    // (≥ WT_LAST_MIN by construction, 0 when dropped). Its width is data-
    // independent — a lane finishing its first task changes only its own cell,
    // never any other column's offset — and the trailing live timer stays
    // right-pinned at the row edge.
    let last_w = if last {
        let base = used(name_w, dirty, cols, pr_w, elapsed_w, false);
        avail.saturating_sub(base + COL_GAP)
    } else {
        0
    };

    WtColLayout {
        name_w,
        dirty_w: if dirty { 1 } else { 0 },
        elapsed_w,
        queued_w: cols[0],
        last_w,
        pr_w,
        author_w: cols[1],
        commit_age_w: cols[2],
    }
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
        let rows = queue_rows(&s, "platform", now());
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
        let rows = queue_rows(&snap(vec![running, q1, q2, failed, done], vec![]), "platform", now());
        // ACTIVE section (running → queued) then FINISHED section (done/failed,
        // ordered by completion; here both lack finishedAt so id-desc wins →
        // t5(done) before t4(failed)). The `#N in lane` position is still computed
        // in creation order, so q1/q2 keep #1/#2 regardless of the display sort.
        // Only live-progress states carry detail text; Failed/Done are empty.
        assert_eq!(rows[0].detail, "⏱ 3m12s"); // running
        assert_eq!(rows[1].detail, "#1 in lane"); // q1
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
    fn queue_rows_sub_classifies_failed_by_error_reason() {
        // `worker.ts` stamps the exact strings "timed out" / "session limit" /
        // "out of budget" into `task.error` for those failure modes; any other
        // reason (or none) stays the generic `✗`. All special cases are still red
        // (see `theme::glyph_style`) — only the glyph differs, so they read apart
        // at a glance without a color that could be confused for a different
        // severity.
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
        let rows = queue_rows(
            &snap(
                vec![timed_out, session_limit, generic, no_reason, out_of_budget],
                vec![],
            ),
            "platform",
            now(),
        );
        let glyph_for = |id: &str| rows.iter().find(|r| r.task_id == id).unwrap().glyph;
        assert_eq!(glyph_for("t1"), '⧗');
        assert_eq!(glyph_for("t2"), '⊠');
        assert_eq!(glyph_for("t3"), '✗');
        assert_eq!(glyph_for("t4"), '✗');
        assert_eq!(glyph_for("t5"), '¤');
    }

    #[test]
    fn queue_rows_needs_input_has_no_detail_text() {
        let ni = task_on(TaskStatus::NeedsInput, "t1", "platform", Some("wt-a"));
        let rows = queue_rows(&snap(vec![ni], vec![]), "platform", now());
        assert_eq!(rows[0].detail, ""); // the ‼ glyph carries the state
        assert_eq!(rows[0].glyph, '‼');
    }

    #[test]
    fn queue_rows_use_ref_as_lane_when_worktree_unresolved_and_append_archived() {
        let mut pending = task_on(TaskStatus::Queued, "t1", "platform", None);
        pending.target.git_ref = "pr:257".into();
        let old = task_on(TaskStatus::Done, "t0", "platform", Some("wt-a"));
        let rows = queue_rows(&snap(vec![pending], vec![old]), "platform", now());
        assert_eq!(rows[0].worktree, "pr:257");
        assert!(rows[1].archived);
        assert_eq!(rows[1].detail, ""); // archived rows carry no detail text
    }

    #[test]
    fn queue_rows_cap_archived_at_last_10() {
        let archived: Vec<TaskInstance> = (0..15)
            .map(|i| task_on(TaskStatus::Done, &format!("t{i:02}"), "platform", Some("wt-a")))
            .collect();
        let rows = queue_rows(&snap(vec![], archived), "platform", now());
        assert_eq!(rows.len(), 10);
        // Cap keeps the last 10 archived (t05..t14); with no finishedAt the
        // FINISHED section falls back to id-desc, so the newest (t14) leads.
        assert_eq!(rows[0].task_id, "t14");
        assert_eq!(rows[9].task_id, "t05");
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
        assert_eq!(rows[0].worktree, "dedup-dependabot-run");
    }

    #[test]
    fn queue_rows_display_repo_sentinel_as_repo_name() {
        let live = task_on(TaskStatus::Running, "t1", "platform", Some(REPO_SENTINEL));
        let archived = task_on(TaskStatus::Done, "t2", "platform", Some(REPO_SENTINEL));
        let rows = queue_rows(&snap(vec![live], vec![archived]), "platform", now());
        assert_eq!(rows[0].worktree, "platform");
        assert_eq!(rows[1].worktree, "platform");
    }

    #[test]
    fn queue_rows_carry_def_name_and_created_epoch() {
        let mut t = task_on(TaskStatus::Running, "t1", "platform", Some("wt-a"));
        t.definition = Some("squash-merge".into());
        t.created = "2026-07-08T10:00:00.000Z".into();
        let adhoc = task_on(TaskStatus::Queued, "t2", "platform", Some("wt-a"));
        let rows = queue_rows(&snap(vec![t, adhoc], vec![]), "platform", now());
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
        let rows = queue_rows(&snap(vec![t], vec![]), "platform", now());
        assert_eq!(rows[0].def_name, Some("pr-ready".into()));
        assert_eq!(def_display_name("pr-ready"), "pr-ready");
        assert_eq!(def_display_name("platform/pr-ready"), "pr-ready");
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
    fn queue_active_section_orders_by_status_then_priority_then_id() {
        // Scrambled input; expect running → needs-input → queued(high,normal,low),
        // with id as the final (stable) tiebreak inside the queued run.
        let rows = queue_rows(
            &snap(
                vec![
                    qtask(TaskStatus::Queued, "01Q_LOW", "low", None),
                    qtask(TaskStatus::Running, "01RUNNING", "normal", None),
                    qtask(TaskStatus::Queued, "01Q_NORMAL", "normal", None),
                    qtask(TaskStatus::NeedsInput, "01NEEDS", "normal", None),
                    qtask(TaskStatus::Queued, "01Q_HIGH", "high", None),
                ],
                vec![],
            ),
            "platform",
            now(),
        );
        assert_eq!(
            rows.iter().map(|r| r.task_id.as_str()).collect::<Vec<_>>(),
            vec!["01RUNNING", "01NEEDS", "01Q_HIGH", "01Q_NORMAL", "01Q_LOW"]
        );
        // None of the active rows are in the FINISHED section, so no divider.
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
            now(),
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
            now(),
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
            now(),
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
            now(),
        );
        assert_eq!(queue_divider_after(&active), None);
        let finished = queue_rows(
            &snap(vec![qtask(TaskStatus::Done, "01D", "normal", None)], vec![]),
            "platform",
            now(),
        );
        assert_eq!(queue_divider_after(&finished), None);
    }

    #[test]
    fn queue_archived_rows_join_the_finished_section_sorted_with_live() {
        // A live failed task (finished 12:00) and an archived done task (finished
        // 13:00) both land in the FINISHED section, ordered by completion — the
        // newer archived row leads even though it lives in a different list.
        let rows = queue_rows(
            &snap(
                vec![
                    qtask(TaskStatus::Running, "01RUN", "normal", None),
                    qtask(TaskStatus::Failed, "01FAIL", "normal", Some("2026-07-09T12:00:00.000Z")),
                ],
                vec![qtask(TaskStatus::Done, "01ARCH", "normal", Some("2026-07-09T13:00:00.000Z"))],
            ),
            "platform",
            now(),
        );
        assert_eq!(
            rows.iter().map(|r| r.task_id.as_str()).collect::<Vec<_>>(),
            vec!["01RUN", "01ARCH", "01FAIL"]
        );
        assert!(rows[1].archived && !rows[2].archived);
        assert_eq!(queue_divider_after(&rows), Some(0)); // after the single active row
    }

    // ---- lane_task_live / lane_task_cols (worktree detail lane list) ----

    #[test]
    fn lane_task_live_running_queued_and_terminal() {
        // Running → elapsed against `now` (created default is 3m12s before now).
        let running = task_on(TaskStatus::Running, "t1", "platform", Some("wt-a"));
        assert_eq!(lane_task_live(&running, now(), 0), "⏱ 3m12s");
        // A re-run stamps `started_at` LATER than `created`: the timer anchors on
        // the re-run (47s ago), not the original creation — so it never inherits
        // the phantom elapsed that would race it to the 3h ceiling.
        let mut rerun = running.clone();
        rerun.started_at = Some("2026-07-08T10:02:25.000Z".into()); // now - 47s
        assert_eq!(lane_task_live(&rerun, now(), 0), "⏱ 47s");
        // Queued → `#N in lane` using the caller-supplied 1-based position (the
        // elapsed clock is never consulted for a queued row).
        let queued = task_on(TaskStatus::Queued, "t2", "platform", Some("wt-a"));
        assert_eq!(lane_task_live(&queued, now(), 1), "#1 in lane");
        assert_eq!(lane_task_live(&queued, now(), 3), "#3 in lane");
        // Every terminal / non-live status → empty (the glyph carries the state).
        for status in
            [TaskStatus::Done, TaskStatus::Failed, TaskStatus::Cancelled, TaskStatus::NeedsInput]
        {
            let t = task_on(status, "t3", "platform", Some("wt-a"));
            assert_eq!(lane_task_live(&t, now(), 0), "", "{status:?} has no live text");
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
        let l = def_col_layout(&defs, 80);
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
        assert_eq!(b.last.as_ref().unwrap().0, '⊠');

        let mut out_of_budget =
            task_on(TaskStatus::Failed, "01D00000000000000000000001", "platform", Some("wt-a"));
        out_of_budget.error = Some("out of budget".into());
        let mut s = snap(vec![out_of_budget], vec![]);
        s.worktrees = platform_worktrees();
        let rows = worktree_rows(&s, "platform");
        let c = rows.iter().find(|r| r.name == "wt-a").unwrap();
        assert_eq!(c.last.as_ref().unwrap().0, '¤');
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
        // Fixed widths: dirty=1, last-min=12, author=AUTHOR_W(14), commit=8,
        // live=8; anchor = `● ± name` = 2+2+9 = 13. Full reserved =
        // anchor(13) + queued(2+30) + author(2+14) + commit(2+8) + last-min(2+12)
        // + live(2+8) = 95.
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
            last_commit_author: Some("koshea".into()), // author column fixed AUTHOR_W
            last_commit_epoch: Some(now() - 3 * 86_400), // commit-age fixed COMMIT_AGE_W
            ..Default::default()
        };
        let rows = [row];
        // (dirty, live, queued, last, author, commit) presence tuple.
        let present = |a: usize| {
            let l = wt_col_layout(&rows, a);
            (l.dirty_w > 0, l.elapsed_w > 0, l.queued_w > 0, l.last_w > 0, l.author_w > 0, l.commit_age_w > 0)
        };
        // Wide: everything shown, name at full width. All reserved widths sum to
        // 95; at 120 the last-task FILL absorbs the slack.
        assert_eq!(present(95), (true, true, true, true, true, true));
        assert_eq!(wt_col_layout(&rows, 120).name_w, 9);
        assert_eq!(wt_col_layout(&rows, 120).last_w, 37, "last-task fill absorbs the slack");
        assert_eq!(wt_col_layout(&rows, 120).queued_w, 30, "queued is a fixed slot");
        // Drop in ladder order: commit → author → queued → dirty → last →
        // live. The last-task fill outlives queued/dirty (it is the summary-
        // equivalent), the live timer is the last optional to go, and only after
        // everything drops does the name column shrink.
        assert_eq!(present(94), (true, true, true, true, true, false)); // commit dropped (< 95)
        assert_eq!(present(84), (true, true, true, true, false, false)); // author dropped (< 85)
        assert_eq!(present(68), (true, true, false, true, false, false)); // queued dropped (< 69)
        assert_eq!(present(36), (false, true, false, true, false, false)); // dirty dropped (< 37)
        assert_eq!(present(34), (false, true, false, false, false, false)); // last dropped (< 35)
        // Only ⏱ + `● name` remain; the timer is the last optional to go.
        assert_eq!(present(20), (false, false, false, false, false, false)); // live dropped (< 21)
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
    fn queue_col_layout_stable_when_a_row_gains_a_timer() {
        // Two row sets identical except one row goes finished (empty detail) →
        // running (detail = "⏱ 5m03s"). The live/age/timestamp columns are FIXED
        // reserved widths (never data-sized), so the layout is byte-identical.
        let finished = |detail: &str, running: bool, glyph: char| QueueRow {
            task_id: "t".into(),
            glyph,
            running,
            worktree: "feature".into(),
            def_name: Some("squash-merge".into()),
            summary: "implement the widget cache".into(),
            detail: detail.into(),
            created_epoch_s: 0,
            archived: false,
            status: if running { TaskStatus::Running } else { TaskStatus::Done },
            priority: "normal".into(),
            finished_epoch_s: None,
        };
        let before = vec![finished("", false, '✓'), qrow("main", None, "flaky migration")];
        let after = vec![finished("⏱ 5m03s", true, '▶'), qrow("main", None, "flaky migration")];
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
        // Pair 1: one row gains a running timer. The live column is always a
        // candidate (fixed TIMER_W), so the layout is unchanged.
        let with_timer = WorktreeRow { running_elapsed: Some(now() - 100), ..base.clone() };
        assert_eq!(
            wt_col_layout(std::slice::from_ref(&base), 120),
            wt_col_layout(std::slice::from_ref(&with_timer), 120)
        );
        // Pair 2: the queued slot is pane-gated (reserved while ANY visible row
        // has a queued task — always reserving its fixed WT_QUEUED_W burned 30
        // blank cells the fill should stretch into). So: with one row already
        // queued, ANOTHER row gaining a queued task changes nothing.
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
        // No discovery anywhere → no slot.
        assert_eq!(def_col_layout(&[plain.clone()], 80).marker_w, 0);
        // Any discovery def visible → 2-cell slot (glyph + separator), pane-wide.
        let l = def_col_layout(&[plain, disc], 80).marker_w;
        assert_eq!(l, 2);
    }

    #[test]
    fn def_col_layout_sizes_and_caps_schedule_column() {
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
        let l = def_col_layout(&defs, 120);
        assert_eq!(l.sched_w, 15); // "Everyday 1:30pm" (cron-only; marker lives in the front slot)
        assert_eq!(l.marker_w, 2, "pr-review has_discovery reserves the pane-wide front slot");

        // A raw-cron fallback longer than SCHED_CAP is clamped to the cap.
        let long = vec![DefinitionSummary {
            name: "x".into(),
            cron: Some("15 10 5 6 2".into()), // unphrased → 11-char raw... still ≤ cap
            ..Default::default()
        }];
        assert_eq!(def_col_layout(&long, 120).sched_w, 11);
        let huge = vec![DefinitionSummary {
            name: "x".into(),
            cron: Some("1,2,3,4,5,6,7,8 10 5 6 2".into()), // long raw fallback
            ..Default::default()
        }];
        assert_eq!(def_col_layout(&huge, 120).sched_w, SCHED_CAP);
    }

    #[test]
    fn def_col_layout_description_fills_then_degrades() {
        // name="pr-review"(9), cron→"Everyday 1:30pm"(15 cells), plus a 2-cell
        // front discovery-marker slot (pr-review has_discovery), description
        // present. Schedule footprint = 15; marker footprint = 2 (no extra
        // COL_GAP of its own — it embeds its own separator space), so the total
        // fixed-column budget is unchanged from when the marker lived inside
        // `sched_w`.
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
        let wide = def_col_layout(&defs, 120);
        assert_eq!((wide.name_w, wide.sched_w, wide.marker_w), (9, 15, 2));
        assert_eq!(wide.desc_w, 90, "description is the fill remainder");
        // Tighter: the desc fill shrinks toward 0 first (name/schedule/marker kept).
        let mid = def_col_layout(&defs, 40);
        assert_eq!((mid.name_w, mid.sched_w, mid.marker_w), (9, 15, 2));
        assert_eq!(mid.desc_w, 10, "fill absorbs only what's left: 40 - 28 - 2");
        // Very narrow: name shrinks next (28 > 20 → shrink by 8 → name_w 1), but
        // schedule and marker stay. 9 - (28 - 20) = 1.
        let tiny = def_col_layout(&defs, 20);
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
        assert_eq!(def_col_layout(&no_desc, 120).desc_w, 0);
    }

    #[test]
    fn def_model_text_strips_claude_prefix_and_handles_absence() {
        let stripped = DefinitionSummary { model: Some("claude-fable-5".into()), ..Default::default() };
        assert_eq!(def_model_text(&stripped), "fable-5");
        // Short aliases pass through unchanged.
        let alias = DefinitionSummary { model: Some("sonnet".into()), ..Default::default() };
        assert_eq!(def_model_text(&alias), "sonnet");
        // Absent (old daemon) and empty both render blank.
        assert_eq!(def_model_text(&DefinitionSummary::default()), "");
        let empty = DefinitionSummary { model: Some(String::new()), ..Default::default() };
        assert_eq!(def_model_text(&empty), "");
    }

    #[test]
    fn def_col_layout_model_sizes_and_degrades_before_name() {
        // name="pr-review"(9), model "claude-fable-5"→"fable-5"(7),
        // cron→"Everyday 1:30pm"(15 cells), plus a 2-cell front discovery-marker
        // slot (pr-review has_discovery), description present. Schedule
        // footprint = 15; marker footprint = 2.
        let defs = vec![
            DefinitionSummary {
                name: "pr-review".into(),
                model: Some("claude-fable-5".into()),
                cron: Some("30 13 * * *".into()),
                has_discovery: true,
                description: Some("Review an open PR end to end.".into()),
                ..Default::default()
            },
            DefinitionSummary { name: "lint".into(), model: Some("sonnet".into()), ..Default::default() },
        ];
        // Wide: model sized to the widest cell (7), desc is the fill remainder.
        // used_wo_desc = marker(2) + name(9) + (2+7) + (2+15) = 37; desc = 120 - 37 - 2 = 81.
        let wide = def_col_layout(&defs, 120);
        assert_eq!((wide.name_w, wide.model_w, wide.sched_w, wide.marker_w), (9, 7, 15, 2));
        assert_eq!(wide.desc_w, 81, "description is the fill remainder after the model column");
        // Tighter: name+model+schedule+marker (37) still fit in 40; the fill takes the rest.
        let mid = def_col_layout(&defs, 40);
        assert_eq!(mid.model_w, 7, "model kept while the fixed columns fit");
        assert_eq!(mid.desc_w, 1, "fill absorbs only what's left: 40 - 37 - 2");
        // Narrower: the model column drops (before the name shrinks).
        // used_wo_desc without model = marker(2) + name(9) + (2+15) = 28.
        let narrow = def_col_layout(&defs, 30);
        assert_eq!((narrow.model_w, narrow.desc_w), (0, 0));
        assert_eq!(narrow.name_w, 9, "name still fits; schedule/marker kept");
        assert_eq!(narrow.marker_w, 2, "marker slot survives model drop");
        // No model anywhere → the model column is omitted pane-wide even when wide.
        let no_model = vec![DefinitionSummary { name: "lint".into(), ..Default::default() }];
        assert_eq!(def_col_layout(&no_model, 120).model_w, 0);
    }
}
