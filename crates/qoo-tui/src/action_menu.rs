//! Single-target action menus. Ports `packages/tui/src/action-menu.ts`
//! `buildActions` — one menu per list pane (queue / tasks / worktrees / session).
//! The menu shape is stable per context (inapplicable rows are disabled with a
//! reason, never hidden) so rows never jump as status changes. Each row also
//! carries a one-sentence `description` shown in the lazyvim-style right pane.
//! Bulk menus (Task 16) mirror the same shape with `(<eligible> of <total>)`
//! counts. The `RunNamedDef` variant carries repo/name so the tasks-pane "Run"
//! row can dispatch (or open the args form) without re-resolving the def.

use crate::ipc::types::{DefinitionSummary, TaskInstance, TaskStatus};
use crate::selectors::{QueueRow, WorktreeRow, WtState};

/// What a chosen menu row does. `execute_menu_action` (app.rs) maps each variant
/// to a mode transition or an RPC dispatch. Variants are only ever added.
#[derive(Debug, Clone)]
pub enum MenuAction {
    Rerun { id: String },
    Skip { id: String },
    /// Assign a worktree to a needs-input task → opens `Mode::WorktreeInput`.
    AssignWorktree { id: String },
    /// New adhoc task on this worktree, fresh session → opens `Mode::AddTask`.
    TaskFresh { worktree: Option<String> },
    /// New adhoc task on this worktree, main session → opens `Mode::AddTask`.
    TaskMain { worktree: Option<String> },
    /// Run this named definition (repo/name already known) — tasks-pane "Run".
    RunNamedDef { repo: String, name: String },
    OpenTmux { path: String },
    RemoveWorktree { repo: String, name: String, branch: String },
    // --- Bulk actions (Task 16). Targets are frozen at menu-open time: the
    // eligible ids/names are captured here so a snapshot push that reshuffles
    // rows mid-menu can never retarget the dispatch. ---
    /// Rerun each eligible queue task (failed / needs-input).
    BulkRerun { ids: Vec<String> },
    /// Skip each eligible queue task (failed / needs-input / done).
    BulkSkip { ids: Vec<String> },
    /// Run each zero-arg definition on this repo.
    BulkRunDefs { repo: String, names: Vec<String> },
    /// Remove each non-busy worktree (routes through `Mode::ConfirmBulkRemove`).
    BulkRemove { repo: String, names: Vec<String> },
}

/// One menu row: display `label`, an optional `disabled` reason (renders dimmed +
/// inert when `Some`), a one-sentence `description` shown in the right pane, and
/// the `action` fired on Enter/click.
#[derive(Debug, Clone)]
pub struct ActionItem {
    pub label: String,
    pub disabled: Option<String>,
    pub description: String,
    pub action: MenuAction,
}

fn item(
    label: &str,
    applicable: bool,
    reason: &str,
    description: &str,
    action: MenuAction,
) -> ActionItem {
    ActionItem {
        label: label.to_string(),
        disabled: if applicable { None } else { Some(reason.to_string()) },
        description: description.to_string(),
        action,
    }
}

/// Filtered indices into `items` whose label case-insensitively contains `query`
/// (empty query matches everything). Mirrors `selectors::filter_rows` so the
/// action-menu search bar and the pane filters share one matching semantics.
pub fn filter_items(items: &[ActionItem], query: &str) -> Vec<usize> {
    crate::selectors::filter_rows(items, query, |it| it.label.clone())
}

fn status_kebab(s: TaskStatus) -> &'static str {
    match s {
        TaskStatus::Queued => "queued",
        TaskStatus::NeedsInput => "needs-input",
        TaskStatus::Running => "running",
        TaskStatus::Done => "done",
        TaskStatus::Failed => "failed",
        TaskStatus::Unknown => "unknown",
    }
}

const RERUN_DESC: &str = "Re-queue this task and run it again.";
const SKIP_DESC: &str = "Mark this task as skipped; it will not run.";
const ASSIGN_DESC: &str = "Assign a worktree to this needs-input task, then re-queue it.";
const TASK_FRESH_DESC: &str = "Queue a new adhoc task on this worktree in a fresh session.";
const TASK_MAIN_DESC: &str = "Queue a new adhoc task that resumes this worktree's main session.";
const OPEN_TMUX_DESC: &str = "Open this worktree in a new tmux window.";
const REMOVE_DESC: &str =
    "Remove this worktree and delete its local branch (asks for confirmation).";

/// Single-target queue menu. Shape is stable per status (disabled rows keep
/// their slot); archived rows disable everything with reason "archived".
pub fn queue_menu(row: &QueueRow, full: &TaskInstance) -> (String, Vec<ActionItem>) {
    let title = row.summary.clone();
    let id = full.id.clone();
    if row.archived {
        return (
            title,
            vec![
                item("Rerun", false, "archived", RERUN_DESC, MenuAction::Rerun { id: id.clone() }),
                item("Skip", false, "archived", SKIP_DESC, MenuAction::Skip { id: id.clone() }),
                item("Assign worktree", false, "archived", ASSIGN_DESC, MenuAction::AssignWorktree { id }),
            ],
        );
    }
    let s = full.status;
    let k = status_kebab(s);
    let rerun_ok = matches!(s, TaskStatus::Failed | TaskStatus::NeedsInput);
    let skip_ok = matches!(s, TaskStatus::Failed | TaskStatus::NeedsInput | TaskStatus::Done);
    let assign_ok = matches!(s, TaskStatus::NeedsInput);
    (
        title,
        vec![
            item("Rerun", rerun_ok, &format!("cannot rerun a {k} task"), RERUN_DESC, MenuAction::Rerun { id: id.clone() }),
            item("Skip", skip_ok, &format!("cannot skip a {k} task"), SKIP_DESC, MenuAction::Skip { id: id.clone() }),
            item("Assign worktree", assign_ok, "only for needs-input tasks", ASSIGN_DESC, MenuAction::AssignWorktree { id }),
        ],
    )
}

/// Single-target tasks menu: one "Run" row → the named-def run. The row's
/// description prefers the def's own one-liner, falling back to a generic hint.
pub fn tasks_menu(def: &DefinitionSummary) -> (String, Vec<ActionItem>) {
    let description = def
        .description
        .clone()
        .unwrap_or_else(|| "Run this task definition.".into());
    (
        def.name.clone(),
        vec![ActionItem {
            label: "Run".into(),
            disabled: None,
            description,
            action: MenuAction::RunNamedDef { repo: def.repo.clone(), name: def.name.clone() },
        }],
    )
}

/// Single-target worktree menu (or session menu when the row is an interactive
/// session). `repo` is the active project — needed for `RemoveWorktree`.
pub fn worktree_menu(repo: &str, row: &WorktreeRow, inside_tmux: bool) -> (String, Vec<ActionItem>) {
    if row.is_session {
        return (
            row.name.clone(),
            vec![item(
                "Open in tmux window",
                inside_tmux,
                "not inside tmux",
                OPEN_TMUX_DESC,
                MenuAction::OpenTmux { path: row.path.clone() },
            )],
        );
    }
    let busy = matches!(row.state, WtState::Busy);
    (
        row.name.clone(),
        vec![
            ActionItem {
                label: "New task (fresh session)".into(),
                disabled: None,
                description: TASK_FRESH_DESC.into(),
                action: MenuAction::TaskFresh { worktree: Some(row.raw_name.clone()) },
            },
            ActionItem {
                label: "New task (main session)".into(),
                disabled: None,
                description: TASK_MAIN_DESC.into(),
                action: MenuAction::TaskMain { worktree: Some(row.raw_name.clone()) },
            },
            item(
                "Open in tmux window",
                inside_tmux,
                "not inside tmux",
                OPEN_TMUX_DESC,
                MenuAction::OpenTmux { path: row.path.clone() },
            ),
            item(
                "Remove worktree",
                !busy,
                "a task is running here",
                REMOVE_DESC,
                MenuAction::RemoveWorktree {
                    repo: repo.to_string(),
                    name: row.raw_name.clone(),
                    branch: row.branch.clone(),
                },
            ),
        ],
    )
}

/// Pre-resolved bulk selection: eligibility is computed once by the caller at
/// menu-open time so the ids/names are frozen into the returned actions. The
/// label counts read `<eligible> of <total>` where `total` is the whole range.
pub enum BulkSelection {
    Queue { rerun_ids: Vec<String>, skip_ids: Vec<String>, total: usize },
    Tasks { repo: String, run_names: Vec<String>, total: usize },
    Worktrees { repo: String, remove_names: Vec<String>, total: usize },
}

fn bulk_item(verb: &str, eligible: usize, total: usize, description: &str, action: MenuAction) -> ActionItem {
    ActionItem {
        label: format!("{verb} ({eligible} of {total})"),
        disabled: if eligible > 0 { None } else { Some("no eligible rows".into()) },
        description: description.to_string(),
        action,
    }
}

const BULK_RERUN_DESC: &str = "Re-queue each eligible task in the selection.";
const BULK_SKIP_DESC: &str = "Skip each eligible task in the selection.";
const BULK_RUN_DESC: &str = "Run each zero-arg task definition in the selection.";
const BULK_REMOVE_DESC: &str =
    "Remove each non-busy worktree in the selection (asks for confirmation).";

/// Build the bulk menu for a pre-resolved selection. Title is `"<total> selected"`;
/// each row's count is `(<eligible> of <total>)`, disabled with "no eligible rows"
/// when nothing in the range qualifies.
pub fn bulk_menu(sel: BulkSelection) -> (String, Vec<ActionItem>) {
    match sel {
        BulkSelection::Queue { rerun_ids, skip_ids, total } => (
            format!("{total} selected"),
            vec![
                bulk_item("Rerun", rerun_ids.len(), total, BULK_RERUN_DESC, MenuAction::BulkRerun { ids: rerun_ids }),
                bulk_item("Skip", skip_ids.len(), total, BULK_SKIP_DESC, MenuAction::BulkSkip { ids: skip_ids }),
            ],
        ),
        BulkSelection::Tasks { repo, run_names, total } => (
            format!("{total} selected"),
            vec![bulk_item("Run", run_names.len(), total, BULK_RUN_DESC, MenuAction::BulkRunDefs { repo, names: run_names })],
        ),
        BulkSelection::Worktrees { repo, remove_names, total } => (
            format!("{total} selected"),
            vec![bulk_item(
                "Remove worktrees",
                remove_names.len(),
                total,
                BULK_REMOVE_DESC,
                MenuAction::BulkRemove { repo, names: remove_names },
            )],
        ),
    }
}

#[cfg(test)]
mod filter_tests {
    use super::*;

    fn sample() -> Vec<ActionItem> {
        vec![
            item("Rerun", true, "", RERUN_DESC, MenuAction::Rerun { id: "t".into() }),
            item("Skip", true, "", SKIP_DESC, MenuAction::Skip { id: "t".into() }),
            item("Assign worktree", true, "", ASSIGN_DESC, MenuAction::AssignWorktree { id: "t".into() }),
        ]
    }

    #[test]
    fn empty_query_matches_all() {
        let items = sample();
        assert_eq!(filter_items(&items, ""), vec![0, 1, 2]);
    }

    #[test]
    fn matching_is_case_insensitive_substring() {
        let items = sample();
        // "ki" matches only "Skip".
        assert_eq!(filter_items(&items, "ki"), vec![1]);
        // Case-insensitive: "ASSIGN" matches "Assign worktree".
        assert_eq!(filter_items(&items, "ASSIGN"), vec![2]);
        // "r" matches Rerun and worktree (assign WORKTREE has an 'r').
        assert_eq!(filter_items(&items, "r"), vec![0, 2]);
        // No match → empty.
        assert!(filter_items(&items, "zzz").is_empty());
    }
}

#[cfg(test)]
mod bulk_builder_tests {
    use super::*;

    fn labels(items: &[ActionItem]) -> Vec<String> { items.iter().map(|i| i.label.clone()).collect() }

    #[test]
    fn bulk_queue_rerun_and_skip_with_counts() {
        let (title, items) = bulk_menu(BulkSelection::Queue {
            rerun_ids: vec!["a".into(), "b".into()],
            skip_ids: vec!["a".into(), "b".into(), "c".into()],
            total: 5,
        });
        assert_eq!(title, "5 selected");
        assert_eq!(labels(&items), ["Rerun (2 of 5)", "Skip (3 of 5)"]);
        assert!(items.iter().all(|i| i.disabled.is_none()));
        // Frozen ids live inside the action.
        assert!(matches!(&items[0].action, MenuAction::BulkRerun { ids } if ids == &["a".to_string(), "b".to_string()]));
        assert!(matches!(&items[1].action, MenuAction::BulkSkip { ids } if ids.len() == 3));
    }

    #[test]
    fn bulk_queue_zero_eligible_disables() {
        let (_t, items) = bulk_menu(BulkSelection::Queue { rerun_ids: vec![], skip_ids: vec!["a".into()], total: 4 });
        assert_eq!(items[0].label, "Rerun (0 of 4)");
        assert_eq!(items[0].disabled.as_deref(), Some("no eligible rows"));
        assert_eq!(items[1].disabled, None);
    }

    #[test]
    fn bulk_tasks_run_only() {
        let (title, items) = bulk_menu(BulkSelection::Tasks { repo: "platform".into(), run_names: vec!["lint".into()], total: 3 });
        assert_eq!(title, "3 selected");
        assert_eq!(labels(&items), ["Run (1 of 3)"]);
        assert!(matches!(&items[0].action, MenuAction::BulkRunDefs { repo, names } if repo == "platform" && names == &["lint".to_string()]));
    }

    #[test]
    fn bulk_worktrees_remove_only() {
        let (_t, items) = bulk_menu(BulkSelection::Worktrees { repo: "platform".into(), remove_names: vec!["wt-a".into(), "wt-b".into()], total: 4 });
        assert_eq!(labels(&items), ["Remove worktrees (2 of 4)"]);
        assert!(matches!(&items[0].action, MenuAction::BulkRemove { repo, names } if repo == "platform" && names.len() == 2));
    }
}

#[cfg(test)]
mod builder_tests {
    use super::*;
    use crate::ipc::types::{TaskInstance, TaskStatus};
    use crate::selectors::{QueueRow, WorktreeRow, WtState};

    fn qrow(archived: bool) -> QueueRow {
        QueueRow {
            task_id: "t1".into(),
            glyph: '?',
            running: false,
            main_session: false,
            worktree: "main".into(),
            def_name: None,
            summary: "do the thing".into(),
            detail: String::new(),
            created_epoch_s: 0,
            archived,
            status: TaskStatus::NeedsInput,
            priority: "normal".into(),
            finished_epoch_s: None,
        }
    }
    fn task(status: TaskStatus) -> TaskInstance {
        let mut t = TaskInstance::default();
        t.id = "t1".into();
        t.status = status;
        t
    }
    fn labels(items: &[ActionItem]) -> Vec<&str> {
        items.iter().map(|i| i.label.as_str()).collect()
    }
    fn enabled(items: &[ActionItem]) -> Vec<&str> {
        items.iter().filter(|i| i.disabled.is_none()).map(|i| i.label.as_str()).collect()
    }

    #[test]
    fn queue_stable_order_and_status_gating() {
        // Stable order regardless of status.
        let (title, items) = queue_menu(&qrow(false), &task(TaskStatus::Running));
        assert_eq!(title, "do the thing");
        assert_eq!(labels(&items), ["Rerun", "Skip", "Assign worktree"]);
        assert!(enabled(&items).is_empty()); // running: nothing enabled

        assert_eq!(enabled(&queue_menu(&qrow(false), &task(TaskStatus::Failed)).1), ["Rerun", "Skip"]);
        assert_eq!(
            enabled(&queue_menu(&qrow(false), &task(TaskStatus::NeedsInput)).1),
            ["Rerun", "Skip", "Assign worktree"]
        );
        assert_eq!(enabled(&queue_menu(&qrow(false), &task(TaskStatus::Done)).1), ["Skip"]);
        assert!(enabled(&queue_menu(&qrow(false), &task(TaskStatus::Queued)).1).is_empty());
    }

    #[test]
    fn queue_archived_all_disabled_with_reason() {
        let (_t, items) = queue_menu(&qrow(true), &task(TaskStatus::Done));
        assert!(items.iter().all(|i| i.disabled.as_deref() == Some("archived")));
    }

    #[test]
    fn queue_disabled_reasons_name_the_status() {
        let (_t, items) = queue_menu(&qrow(false), &task(TaskStatus::Running));
        assert_eq!(items[0].disabled.as_deref(), Some("cannot rerun a running task"));
        assert_eq!(items[1].disabled.as_deref(), Some("cannot skip a running task"));
        assert_eq!(items[2].disabled.as_deref(), Some("only for needs-input tasks"));
    }

    #[test]
    fn queue_rows_carry_descriptions() {
        let (_t, items) = queue_menu(&qrow(false), &task(TaskStatus::NeedsInput));
        assert_eq!(items[0].description, RERUN_DESC);
        assert_eq!(items[2].description, ASSIGN_DESC);
    }

    #[test]
    fn tasks_menu_offers_run() {
        let mut d = DefinitionSummary::default();
        d.repo = "platform".into();
        d.name = "pr-ready".into();
        let (title, items) = tasks_menu(&d);
        assert_eq!(title, "pr-ready");
        assert_eq!(labels(&items), ["Run"]);
        assert!(matches!(items[0].action, MenuAction::RunNamedDef { .. }));
        // No def description → generic fallback.
        assert_eq!(items[0].description, "Run this task definition.");
    }

    #[test]
    fn tasks_menu_run_uses_def_description_when_present() {
        let mut d = DefinitionSummary::default();
        d.repo = "platform".into();
        d.name = "pr-ready".into();
        d.description = Some("Flip WIP → ready and assign reviewers.".into());
        let (_t, items) = tasks_menu(&d);
        assert_eq!(items[0].description, "Flip WIP → ready and assign reviewers.");
    }

    fn wrow(state: WtState, branch: &str, is_session: bool) -> WorktreeRow {
        WorktreeRow {
            name: "wt-a".into(),
            raw_name: "platform.wt-a".into(),
            path: "/wt/wt-a".into(),
            branch: branch.into(),
            state,
            has_main_session: false,
            queued: 0,
            is_session,
            ..Default::default()
        }
    }

    #[test]
    fn worktree_menu_order_and_all_enabled() {
        let (title, items) = worktree_menu("platform", &wrow(WtState::Free, "wt-a", false), true);
        assert_eq!(title, "wt-a");
        assert_eq!(
            labels(&items),
            [
                "New task (fresh session)",
                "New task (main session)",
                "Open in tmux window",
                "Remove worktree",
            ]
        );
        assert_eq!(enabled(&items), labels(&items));
    }

    #[test]
    fn worktree_menu_busy_disables_remove() {
        let (_t, items) = worktree_menu("platform", &wrow(WtState::Busy, "wt-a", false), true);
        let by = |lbl: &str| items.iter().find(|i| i.label == lbl).unwrap();
        assert_eq!(by("Remove worktree").disabled.as_deref(), Some("a task is running here"));
        // New-task rows stay enabled while busy.
        assert_eq!(by("New task (fresh session)").disabled, None);
    }

    #[test]
    fn worktree_menu_branchless_still_allows_remove() {
        // With squash-merge gone, a branchless free worktree has every row enabled.
        let (_t, items) = worktree_menu("platform", &wrow(WtState::Free, "", false), true);
        assert_eq!(enabled(&items).len(), items.len());
    }

    #[test]
    fn worktree_menu_outside_tmux_disables_open() {
        let (_t, items) = worktree_menu("platform", &wrow(WtState::Free, "wt-a", false), false);
        let by = |lbl: &str| items.iter().find(|i| i.label == lbl).unwrap();
        assert_eq!(by("Open in tmux window").disabled.as_deref(), Some("not inside tmux"));
    }

    #[test]
    fn session_row_offers_only_tmux_open() {
        let (title, items) = worktree_menu("platform", &wrow(WtState::You, "", true), true);
        assert_eq!(title, "wt-a");
        assert_eq!(labels(&items), ["Open in tmux window"]);
        assert_eq!(enabled(&items), ["Open in tmux window"]);
    }
}
