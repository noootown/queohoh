use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use crate::chain::{effective_model_head, model_ref_display};
use crate::ipc::types::{
    ArgSpec, CatalogEntry, DefaultModels, DefinitionSummary, StateSnapshot, TaskInstance,
    TaskStatus,
};

/// Resolution context for the TASKS Model column: catalog + enabled providers +
/// default_models + active_provider. Layout and render share the same ctx so
/// the column width tracks the **effective head** under the active provider
/// (one label via [`crate::chain::effective_model_head`] +
/// [`crate::chain::model_ref_display`]), not the authored yaml list and not the
/// full fallback chain. The detail config pane still shows the full chain
/// separately. Built once per frame from `App` settings/snapshot (see
/// [`ModelResolveOwned`] / `App::model_resolve_owned`).
#[derive(Debug, Clone, Copy)]
pub struct ModelResolveCtx<'a> {
    pub catalog: &'a [CatalogEntry],
    /// Names of providers currently enabled (absent / disabled → dropped from
    /// the chain; an empty slice treats every provider as disabled).
    pub enabled_providers: &'a [String],
    pub default_models: &'a DefaultModels,
    pub active_provider: &'a str,
}

impl ModelResolveCtx<'_> {
    fn enabled_refs(&self) -> Vec<&str> {
        self.enabled_providers.iter().map(String::as_str).collect()
    }
}

/// Owned bundle of the pieces [`ModelResolveCtx`] borrows — built once per
/// frame so layout + line closures share one resolution source without
/// lifetime entanglement with `App`.
#[derive(Debug, Clone, Default)]
pub struct ModelResolveOwned {
    pub catalog: Vec<CatalogEntry>,
    pub enabled_providers: Vec<String>,
    pub default_models: DefaultModels,
    pub active_provider: String,
}

impl ModelResolveOwned {
    pub fn ctx(&self) -> ModelResolveCtx<'_> {
        ModelResolveCtx {
            catalog: &self.catalog,
            enabled_providers: &self.enabled_providers,
            default_models: &self.default_models,
            active_provider: &self.active_provider,
        }
    }
}

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
    /// Static live-column text for non-running rows (`#N in lane` for Queued;
    /// empty otherwise). Running elapsed is NOT baked here — see
    /// [`Self::running_elapsed`] so a rows cache can survive Tick without
    /// freezing the timer (pane formats `elapsed_label` at paint, like worktrees).
    /// When [`Self::not_before_epoch_s`] is still in the future the pane paints
    /// a countdown instead of this string.
    pub detail: String,
    /// Start epoch of the CURRENT run when `running` (see [`run_start_epoch_s`]);
    /// `None` for non-running rows. The QUEUE Live column formats the timer
    /// against wall-clock `now` at draw time.
    pub running_elapsed: Option<u64>,
    /// Epoch seconds of a future `notBefore` (QUEUE `[d]efer`). Only set for
    /// statuses that can still be deferred (`Queued`, and `Running` mid
    /// defer-stop). `None` for terminal/cancelled rows even if a stale stamp
    /// remains on the wire — Live must stay empty there. Also `None` when
    /// unset / past / old daemon. Painted as a countdown (`⧗ 4h32m`) while
    /// still in the future; falls back to [`Self::detail`] once due.
    pub not_before_epoch_s: Option<u64>,
    /// Scheduler lane key for Queued rows (see [`scheduler_lane_key`]) — ACTIVE
    /// ready-queue sort groups by this, then [`Self::lane_position`]. Empty for
    /// non-queued rows.
    pub lane_key: String,
    /// 1-based `#N in lane` position for Queued rows; `None` otherwise. Sort
    /// key within a lane group (ASC).
    pub lane_position: Option<usize>,
    /// creation epoch seconds (parsed from the daemon ISO timestamp)
    pub created_epoch_s: u64,
    pub archived: bool,
    /// task status — drives the ACTIVE-section status ordering and the
    /// active/finished section split (see [`queue_rows`]).
    pub status: TaskStatus,
    /// task priority string (`high`/`normal`/`low`) — still on the wire/row for
    /// detail; ACTIVE queue sort no longer keys on it (running → ready lane →
    /// deferred).
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
    /// Worktree HEAD is an ancestor of the project's default branch — committed
    /// work merged back (`None` = unknown / old daemon / the default-branch
    /// checkout itself). Drives the `↣` front-column marker.
    pub merged: Option<bool>,
    /// Worktree's PR is APPROVED (daemon `approved`, from gh's reviewDecision).
    /// `Some(true)` = approved, `Some(false)` = a PR exists but isn't approved,
    /// `None` = unknown / no PR / old daemon. Drives the green approved marker,
    /// which shares the `↣` merged slot but yields to it (see `wt_merge_marker`).
    pub approved: Option<bool>,
    /// PR has the `ready-for-review` label (daemon `readyForReview`). Shares the
    /// merge-marker front slot as `◎`; yields to merge and approve.
    pub ready_for_review: Option<bool>,
    /// PR has the `WIP` label (daemon `wip`). Shares the merge-marker front slot
    /// as `✎`; lowest priority (merge > approve > ready-for-review > WIP).
    pub wip: Option<bool>,
    pub last_commit_epoch: Option<u64>,
    pub last_commit_author: Option<String>,
    pub last_commit_author_email: Option<String>,
    /// PR author display name from the daemon (its `prAuthor`) — who OPENED the
    /// PR. Wins over `last_commit_author` in the Author column (`wt_author_text`)
    /// because a squash-merged branch's local HEAD author is an automation merge
    /// commit, not the PR author. `None` on an old daemon or when there is no PR.
    pub pr_author: Option<String>,
    /// short hash + open PR number passthrough from the daemon (all `None` on an
    /// old daemon); surfaced in the worktree detail info tab.
    pub last_commit_hash: Option<String>,
    pub pr_number: Option<u64>,
    /// Web URL of the open PR (paired with `pr_number`; `None` on an old daemon
    /// or when there is no open PR). Drives the clickable `#<n>` link in the
    /// detail info tab and the WORKTREES PR column — a click opens it.
    pub pr_url: Option<String>,
    /// PR base branch (`gh` `baseRefName`); drives goto's `juice --base`.
    /// `None` when no PR / old daemon → TUI falls back to `origin/main`.
    pub pr_base: Option<String>,
    /// True when the daemon flagged this worktree as protected from deletion.
    /// Drives the `⛨` front-column marker and gates the remove action. Session
    /// rows default `false` (never removable anyway).
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
/// The worker's exact `reason` string for a run that failed because its
/// configured provider/model (a non-claude adapter — Task 9/11's codex/grok
/// providers) was unavailable — disabled in settings, missing credentials, or
/// otherwise unable to run. Distinct from a session limit or out-of-budget
/// failure: those are Claude-account states that clear on their own (a timer
/// reset or a top-up); this needs the provider itself fixed (re-enabled,
/// re-authenticated) before a rerun can succeed.
const PROVIDER_UNAVAILABLE_REASON: &str = "provider unavailable";

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
            Some(SESSION_LIMIT_REASON) => '$',
            Some(TIMED_OUT_REASON) => '⧗',
            Some(OUT_OF_BUDGET_REASON) => '$',
            Some(PROVIDER_UNAVAILABLE_REASON) => '⊟',
            _ => '✗',
        },
        TaskStatus::Cancelled => '⊘',
        TaskStatus::Skipped => '⊝',
        TaskStatus::VerifyFailed => '⊗', // circled ✕ — the done-condition disagreed
        TaskStatus::Unknown => '·', // no TS counterpart (old-daemon statuses only)
    }
}

/// Scheduler lane key for queue-position display — mirrors core `laneKey`
/// (`packages/core/src/task.ts`):
///
/// - worktree unresolved → bucket by override or git ref (display-only; the
///   scheduler treats unresolved as null and routes them to resolve)
/// - `task.lane` set → `repo:<lane>` (definition override; serializes across
///   worktrees, e.g. `platform:testing1-stack`)
/// - else → `repo:<worktree>` (default per-worktree concurrency)
///
/// This is the **one** key the scheduler uses for serialization, so `#N in lane`
/// already aggregates worktree + override control: whatever would block a start
/// on that key shares one position counter. Per-project max is a separate cap
/// and is not part of lane position.
fn scheduler_lane_key(task: &TaskInstance) -> String {
    if let Some(override_lane) = task.lane.as_deref().filter(|s| !s.is_empty()) {
        // Override applies even when worktree is still resolving so testing1-stack
        // waiters already share one counter before their worktrees land.
        return format!("{}:{}", task.target.repo, override_lane);
    }
    let wt = task
        .target
        .worktree
        .as_deref()
        .unwrap_or(task.target.git_ref.as_str());
    format!("{}:{}", task.target.repo, wt)
}

/// ACTIVE-section coarse rank for lane-detail task lists (and as a fallback):
/// running first, then needs-input, then queued. Finished statuses share the
/// max rank. QUEUE pane ACTIVE sort uses [`active_queue_bucket`] instead so
/// ready vs deferred queued split into separate bands.
fn status_active_rank(status: TaskStatus) -> u8 {
    match status {
        TaskStatus::Running => 0,
        TaskStatus::NeedsInput => 1,
        TaskStatus::Queued => 2,
        _ => 3,
    }
}

/// ACTIVE QUEUE bands (user-requested order):
/// 0 running · 1 needs-input · 2 ready `#N in lane` · 3 deferred (future notBefore).
fn active_queue_bucket(row: &QueueRow) -> u8 {
    match row.status {
        TaskStatus::Running => 0,
        TaskStatus::NeedsInput => 1,
        TaskStatus::Queued if row.not_before_epoch_s.is_some() => 3,
        TaskStatus::Queued => 2,
        _ => 4,
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

/// Comparator ordering the queue rows into two sections.
///
/// **ACTIVE** (running / needs-input / ready-queued / deferred-queued) first:
/// 1. **Running** — newest run start first (`running_elapsed` desc, then id desc).
/// 2. **Needs-input** — id ascending (stable; rare band).
/// 3. **Ready queued** (`#N in lane`, no future notBefore) — group by scheduler
///    lane key ASC, then `#N` ASC, then id ASC.
/// 4. **Deferred** (queued with `notBefore`) — wake time earlier→later
///    (`not_before_epoch_s` ASC), then id ASC.
///
/// **FINISHED** (done/failed/cancelled/skipped/unknown + archived) after:
/// live before dimmed ARCHIVED, then completion DESC, then id DESC.
fn queue_sort(a: &QueueRow, b: &QueueRow) -> std::cmp::Ordering {
    let (fa, fb) = (queue_row_finished(a), queue_row_finished(b));
    fa.cmp(&fb).then_with(|| {
        if fa {
            // Both finished: live before archived, then newest completion
            // first, then newest id first.
            a.archived
                .cmp(&b.archived)
                .then_with(|| {
                    b.finished_epoch_s.unwrap_or(0).cmp(&a.finished_epoch_s.unwrap_or(0))
                })
                .then_with(|| b.task_id.cmp(&a.task_id))
        } else {
            let ba = active_queue_bucket(a);
            let bb = active_queue_bucket(b);
            ba.cmp(&bb).then_with(|| match ba {
                0 => {
                    // Running: newest start at top.
                    b.running_elapsed
                        .unwrap_or(0)
                        .cmp(&a.running_elapsed.unwrap_or(0))
                        .then_with(|| b.task_id.cmp(&a.task_id))
                }
                2 => {
                    // Ready `#N in lane`: group by lane, position ASC.
                    a.lane_key
                        .cmp(&b.lane_key)
                        .then_with(|| {
                            a.lane_position
                                .unwrap_or(0)
                                .cmp(&b.lane_position.unwrap_or(0))
                        })
                        .then_with(|| a.task_id.cmp(&b.task_id))
                }
                3 => {
                    // Deferred: earlier wake first.
                    a.not_before_epoch_s
                        .unwrap_or(0)
                        .cmp(&b.not_before_epoch_s.unwrap_or(0))
                        .then_with(|| a.task_id.cmp(&b.task_id))
                }
                // needs-input (1) and unknown active (4): stable id.
                _ => a.task_id.cmp(&b.task_id),
            })
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

/// Derive the pre-filter QUEUE rows for `project`. Elapsed is NOT formatted
/// here: running rows carry a start epoch ([`QueueRow::running_elapsed`]) and
/// the pane paints `elapsed_label` against wall-clock now — so App's rows cache
/// need not rebuild on Tick (worktree rows already worked that way).
pub fn queue_rows(snapshot: &StateSnapshot, project: &str) -> Vec<QueueRow> {
    // Live rows plus ALL project-filtered archived rows (dimmed by the view via
    // `archived: true`; archived rows whose worktree was deleted are hidden
    // outright — see the filter below). Full history is intentional (user wants
    // the archive visible, not a recent tail). Sorted into an ACTIVE section
    // (running/needs-input/queued) followed by a FINISHED section
    // (done/failed/cancelled/skipped/unknown + archived).
    // The per-lane queue position (`#N in lane`) is computed in snapshot order
    // (creation order) BEFORE the display sort so it reflects execution order,
    // not the re-sorted display position. Lane key = scheduler key
    // (`scheduler_lane_key`), so definition `lane:` overrides (testing1-stack)
    // share one counter across worktrees — matching who actually waits on whom.
    let mut queued_position: HashMap<String, usize> = HashMap::new();
    let mut rows: Vec<QueueRow> = Vec::new();
    for task in snapshot.tasks.iter().filter(|t| t.target.repo == project) {
        // Live-progress: start epoch for Running (formatted at paint), queue
        // position for Queued. Done/Failed/NeedsInput/Unknown carry NOTHING —
        // the ✓/✗/? glyph already says the outcome and the full error/status
        // lives in the DETAIL pane; a trailing "done"/"exit code 1" duplicated it.
        let (detail, running_elapsed, lane_key, lane_position) = match task.status {
            TaskStatus::Running => (String::new(), Some(run_start_epoch_s(task)), String::new(), None),
            TaskStatus::Queued => {
                let lane = scheduler_lane_key(task);
                let position = queued_position.get(&lane).copied().unwrap_or(0) + 1;
                queued_position.insert(lane.clone(), position);
                (
                    format!("#{position} in lane"),
                    None,
                    lane,
                    Some(position),
                )
            }
            _ => (String::new(), None, String::new(), None),
        };
        // Future notBefore only for statuses still live for defer purposes.
        // Terminal/cancelled rows must never paint a schedule stamp even when a
        // stale not_before remains on disk (cancel should clear it in the
        // daemon; this is defense in depth). Running rows may carry a stamped
        // notBefore mid-defer-stop; the paint path prefers the running timer
        // until the kill settles.
        let not_before_epoch_s = match task.status {
            TaskStatus::Queued | TaskStatus::Running => task
                .not_before
                .as_deref()
                .map(parse_iso_epoch_s)
                .filter(|&e| e > 0),
            _ => None,
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
            running_elapsed,
            not_before_epoch_s,
            lane_key,
            lane_position,
            created_epoch_s: parse_iso_epoch_s(&task.created),
            archived: false,
            status: task.status,
            priority: task.priority.clone(),
            finished_epoch_s: task.finished_at.as_deref().map(parse_iso_epoch_s),
        });
    }
    // Archived rows whose spawned worktree has since been DELETED are hidden
    // entirely, not dimmed (user request: deleting the worktree is the "I'm
    // done with this" signal — the daemon archives the task on deletion, and a
    // grayed leftover row is just clutter). The check is against the repo's
    // worktree listing in the snapshot; a repo with NO listing (`None` — old
    // daemon, or the cache hasn't populated) hides nothing, mirroring the
    // daemon sweep's cold-cache guard. `null`/`@repo` targets have no worktree
    // to delete and keep the dimmed display, as do age-swept rows whose
    // worktree still exists. No display cap: every surviving archived row for
    // this project is shown (daemon also sends the full archive list).
    let repo_worktrees = snapshot.worktrees.get(project);
    for task in snapshot
        .archived_recent
        .iter()
        .filter(|t| t.target.repo == project)
        .filter(|t| match (t.target.worktree.as_deref(), repo_worktrees) {
            (Some(wt), Some(list)) if wt != REPO_SENTINEL => list.iter().any(|w| w.name == wt),
            _ => true,
        })
    {
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
            running_elapsed: None,
            not_before_epoch_s: None,
            lane_key: String::new(),
            lane_position: None,
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

/// Per-lane aggregate pre-computed once by [`build_lane_index`] — everything
/// [`worktree_rows`] needs to answer, in O(1), the same five questions the
/// legacy `worktree_state`/`queued_on_lane`/`running_elapsed_on_lane`/
/// `next_queued_name_on_lane`/`last_finished_on_lane` each re-scanned
/// `snapshot.tasks` (and `last_finished_on_lane` also `archived_recent`) to
/// answer, once PER WORKTREE. Borrows into the snapshot rather than cloning —
/// this is a hot per-frame path.
#[derive(Debug, Default)]
struct LaneAgg<'a> {
    /// Any LIVE task on the lane is Running — feeds `worktree_state`'s Busy
    /// branch (an archived task is never Running, so this is live-only by
    /// construction, matching the legacy helper's live-only filter).
    has_running: bool,
    /// Newest-by-id LIVE task (ULIDs sort chronologically); ties keep the LAST
    /// one seen while walking `snapshot.tasks`, mirroring `Iterator::max_by`'s
    /// "last of equal maxima wins". Its status feeds `worktree_state`'s
    /// Failed-vs-Free branch once nothing on the lane is running.
    newest_live: Option<&'a TaskInstance>,
    /// Count of LIVE Queued tasks on the lane — `queued_on_lane`.
    queued_count: usize,
    /// FIRST live Running task in `snapshot.tasks` order (NOT the newest) —
    /// `running_elapsed_on_lane` always reads the head-of-lane runner.
    first_running: Option<&'a TaskInstance>,
    /// FIRST live Queued task in `snapshot.tasks` order — feeds
    /// `next_queued_name_on_lane`.
    first_queued: Option<&'a TaskInstance>,
    /// Newest-by-id task across LIVE+ARCHIVED whose status is neither Running
    /// nor Queued — feeds `last_finished_on_lane`. Built by walking
    /// `snapshot.tasks` then `snapshot.archived_recent`, in that order, with
    /// the same last-of-equal-maxima tiebreak as `newest_live`; together the
    /// two passes reproduce
    /// `tasks.iter().chain(archived_recent.iter()).max_by(id)` exactly.
    newest_finished: Option<&'a TaskInstance>,
}

/// Builds every lane's [`LaneAgg`] in one forward pass over `snapshot.tasks`
/// (live) and one over `snapshot.archived_recent`, replacing the five O(W×T)
/// per-lane re-scans the old `worktree_rows` ran for every worktree with
/// O(T+A) total work up front — the `W` worktrees then do O(1) hashmap
/// lookups. A lane with zero tasks anywhere legitimately has no entry (the
/// caller treats a miss as "no tasks on this lane", the same outcome the
/// legacy per-lane scans produced when nothing matched).
///
/// "First" fields (`first_running`/`first_queued`) are set only once — the
/// FIRST write wins — to preserve the legacy helpers' `.find()` (first-in-
/// snapshot-order) semantics. "Newest" fields (`newest_live`/
/// `newest_finished`) replace on `>=` on every candidate, preserving
/// `Iterator::max_by`'s "last of equal maxima wins" semantics across the full
/// live-then-archived scan (mirroring the legacy `.max_by(id)` calls exactly).
fn build_lane_index<'a>(snapshot: &'a StateSnapshot) -> HashMap<String, LaneAgg<'a>> {
    let mut index: HashMap<String, LaneAgg<'a>> = HashMap::new();

    for task in &snapshot.tasks {
        let Some(lane) = task_lane(task) else { continue };
        let agg = index.entry(lane).or_default();
        match task.status {
            TaskStatus::Running => {
                agg.has_running = true;
                if agg.first_running.is_none() {
                    agg.first_running = Some(task);
                }
            }
            TaskStatus::Queued => {
                agg.queued_count += 1;
                if agg.first_queued.is_none() {
                    agg.first_queued = Some(task);
                }
            }
            _ => {}
        }
        if agg.newest_live.is_none_or(|cur| task.id >= cur.id) {
            agg.newest_live = Some(task);
        }
        if !matches!(task.status, TaskStatus::Running | TaskStatus::Queued)
            && agg.newest_finished.is_none_or(|cur| task.id >= cur.id)
        {
            agg.newest_finished = Some(task);
        }
    }

    for task in &snapshot.archived_recent {
        // Archived tasks are never Running/Queued in practice, but the guard
        // mirrors the legacy filter exactly regardless of what the archive holds.
        if matches!(task.status, TaskStatus::Running | TaskStatus::Queued) {
            continue;
        }
        let Some(lane) = task_lane(task) else { continue };
        let agg = index.entry(lane).or_default();
        if agg.newest_finished.is_none_or(|cur| task.id >= cur.id) {
            agg.newest_finished = Some(task);
        }
    }

    index
}

/// Projects a lane's aggregate into the same `WtState` [`worktree_state`]
/// returns; a lane miss (`None`) is the same "no tasks on this lane" outcome
/// the legacy helper produced from an empty filtered vec.
fn lane_state(agg: Option<&LaneAgg>) -> WtState {
    let Some(agg) = agg else { return WtState::Free };
    if agg.has_running {
        return WtState::Busy;
    }
    match agg.newest_live {
        Some(t) if matches!(t.status, TaskStatus::Failed | TaskStatus::VerifyFailed) => {
            WtState::Failed
        }
        _ => WtState::Free,
    }
}

/// Mirrors [`queued_on_lane`] over a [`LaneAgg`].
fn lane_queued(agg: Option<&LaneAgg>) -> usize {
    agg.map_or(0, |a| a.queued_count)
}

/// Mirrors [`running_elapsed_on_lane`] over a [`LaneAgg`].
fn lane_running_elapsed(agg: Option<&LaneAgg>) -> Option<u64> {
    agg.and_then(|a| a.first_running).map(run_start_epoch_s)
}

/// Mirrors [`next_queued_name_on_lane`] over a [`LaneAgg`].
fn lane_next_queued(agg: Option<&LaneAgg>) -> Option<(String, bool)> {
    agg.and_then(|a| a.first_queued).map(|t| lane_task_display_name(t, NEXT_NAME_CAP))
}

/// Mirrors [`last_finished_on_lane`] over a [`LaneAgg`].
fn lane_last_finished(agg: Option<&LaneAgg>) -> Option<(char, String, u64, bool)> {
    agg.and_then(|a| a.newest_finished).map(|t| {
        let (name, is_def) = lane_task_display_name(t, usize::MAX);
        (status_glyph(t), name, parse_iso_epoch_s(&t.created), is_def)
    })
}

// The five per-lane helpers below are kept only as the differential-test
// oracle for `build_lane_index` (see `lane_index_matches_legacy_per_lane_helpers`)
// — `worktree_rows` now goes through the single-pass index above instead of
// calling these once per worktree. `#[cfg(test)]` keeps them out of the
// release build so they don't trip `dead_code` now that nothing outside tests
// calls them.
#[cfg(test)]
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

#[cfg(test)]
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
#[cfg(test)]
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
#[cfg(test)]
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
#[cfg(test)]
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
/// QUEUE pane's live slot: `⏱ <elapsed>` for a running task, `⧗ <remaining>`
/// countdown for a deferred queued task (future `notBefore`), `#N in lane` for
/// an eligible queued task, empty for any other status. `queue_pos` is the
/// task's 1-based position among the lane's queued tasks in creation order —
/// the caller counts queued tasks as it walks the (creation-ordered) list; it
/// is only read for a non-deferred Queued task. `tz_offset_s` is unused (kept
/// so call sites stay stable after the countdown↔stamp experiment).
pub(crate) fn lane_task_live(
    task: &TaskInstance,
    now_epoch_s: u64,
    queue_pos: usize,
    _tz_offset_s: i32,
) -> String {
    match task.status {
        TaskStatus::Running => elapsed_label(run_start_epoch_s(task), now_epoch_s),
        TaskStatus::Queued => {
            if let Some(until) = task.not_before.as_deref().map(parse_iso_epoch_s) {
                if until > now_epoch_s {
                    return remaining_label(until, now_epoch_s);
                }
            }
            format!("#{queue_pos} in lane")
        }
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

/// Clip width for the head-of-lane (`→`) task name in the worktree pane —
/// its column is a fixed 30-cell slot, so the name is trimmed to keep the
/// `next:` lead visible. The last-finished cell is deliberately NOT
/// pre-clipped (it is the pane's FILL column; the render clips to the row).
pub const NEXT_NAME_CAP: usize = 24;

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Build WORKTREES pane rows for `project`. Lane-derived fields (state, queued,
/// running timer, next queued name, last finished) come from a single
/// [`build_lane_index`] pass over the snapshot — O(T+A+W) instead of the old
/// O(W×T) five-scan-per-worktree path. The next-queued name is projected once
/// into both `next_name` and `next_is_def` (the double helper call used to walk
/// the task list twice for the same cell).
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
    // One pass over live + archived tasks; each worktree then O(1)-looks up its
    // lane. Built outside the map so W worktrees share a single index.
    let lanes = build_lane_index(snapshot);
    let mut rows: Vec<WorktreeRow> = worktrees
        .iter()
        .map(|wt| {
            let lane = lane_key(project, &wt.name);
            let agg = lanes.get(&lane);
            // One next-queued projection — both the clipped name and the is-def
            // flag come from the same head-of-lane Queued task (the double
            // helper call used to re-walk the task list for each field).
            let (next_name, next_is_def) = match lane_next_queued(agg) {
                Some((n, d)) => (Some(n), d),
                None => (None, false),
            };
            WorktreeRow {
                name: strip_repo_prefix(&wt.name, project).to_string(),
                raw_name: wt.name.clone(),
                path: wt.path.clone(),
                branch: wt.branch.clone(),
                state: lane_state(agg),
                queued: lane_queued(agg),
                is_session: false,
                running_elapsed: lane_running_elapsed(agg),
                next_name,
                next_is_def,
                last: lane_last_finished(agg),
                dirty: wt.dirty,
                merged: wt.merged,
                approved: wt.approved,
                ready_for_review: wt.ready_for_review,
                wip: wt.wip,
                last_commit_epoch: wt.last_commit_epoch,
                last_commit_author: wt.last_commit_author.clone(),
                last_commit_author_email: wt.last_commit_author_email.clone(),
                last_commit_hash: wt.last_commit_hash.clone(),
                pr_number: wt.pr_number,
                pr_url: wt.pr_url.clone(),
                pr_base: wt.pr_base.clone(),
                pr_author: wt.pr_author.clone(),
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

