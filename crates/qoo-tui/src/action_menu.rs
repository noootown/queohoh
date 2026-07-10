//! Single-target action menus. Ports `packages/tui/src/action-menu.ts`
//! `buildActions` — one menu per list pane (queue / tasks / worktrees / session).
//! The menu shape is stable per context (inapplicable rows are disabled with a
//! reason, never hidden) so rows never jump as status changes. Bulk menus land
//! in Task 16; the M3 stub variants (`RunNamedDef`/`RunDef`/`CreateWorktree`/
//! `SquashMerge`) carry enough context for their replacing task to dispatch.

use crate::ipc::types::{DefinitionSummary, TaskInstance, TaskStatus};
use crate::selectors::{QueueRow, WorktreeRow, WtState};

/// What a chosen menu row does. `execute_menu_action` (app.rs) maps each variant
/// to a mode transition, an RPC dispatch, or (for M3 stubs) a status line naming
/// the replacing task. Variants are only ever added.
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
    /// Run a task definition on this worktree (def not yet chosen) — Task 18 picker.
    RunDef { worktree: Option<String>, branch: Option<String> },
    /// Run this named definition (repo/name already known) — Task 19 args form.
    RunNamedDef { repo: String, name: String },
    OpenTmux { path: String },
    RemoveWorktree { repo: String, name: String, branch: String },
    /// Create a new worktree — Task 21.
    CreateWorktree,
    /// Squash-merge this worktree's branch into a target — Task 21.
    SquashMerge { branch: String },
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
/// inert when `Some`), and the `action` fired on Enter/click.
#[derive(Debug, Clone)]
pub struct ActionItem {
    pub label: String,
    pub disabled: Option<String>,
    pub action: MenuAction,
}

fn item(label: &str, applicable: bool, reason: &str, action: MenuAction) -> ActionItem {
    ActionItem {
        label: label.to_string(),
        disabled: if applicable { None } else { Some(reason.to_string()) },
        action,
    }
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

/// Single-target queue menu. Shape is stable per status (disabled rows keep
/// their slot); archived rows disable everything with reason "archived".
pub fn queue_menu(row: &QueueRow, full: &TaskInstance) -> (String, Vec<ActionItem>) {
    let title = row.summary.clone();
    let id = full.id.clone();
    if row.archived {
        return (
            title,
            vec![
                item("Rerun", false, "archived", MenuAction::Rerun { id: id.clone() }),
                item("Skip", false, "archived", MenuAction::Skip { id: id.clone() }),
                item("Assign worktree…", false, "archived", MenuAction::AssignWorktree { id }),
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
            item("Rerun", rerun_ok, &format!("cannot rerun a {k} task"), MenuAction::Rerun { id: id.clone() }),
            item("Skip", skip_ok, &format!("cannot skip a {k} task"), MenuAction::Skip { id: id.clone() }),
            item("Assign worktree…", assign_ok, "only for needs-input tasks", MenuAction::AssignWorktree { id }),
        ],
    )
}

/// Single-target tasks menu: one "Run" row → the named-def run (the args form is
/// M3, Task 19; the action carries repo/name so that task can dispatch).
pub fn tasks_menu(def: &DefinitionSummary) -> (String, Vec<ActionItem>) {
    (
        def.name.clone(),
        vec![ActionItem {
            label: "Run".into(),
            disabled: None,
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
                MenuAction::OpenTmux { path: row.path.clone() },
            )],
        );
    }
    let busy = matches!(row.state, WtState::Busy);
    let has_branch = !row.branch.is_empty();
    let branch_opt = if has_branch { Some(row.branch.clone()) } else { None };
    let squash_reason = if busy { "a task is running here" } else { "worktree has no branch" };
    (
        row.name.clone(),
        vec![
            ActionItem {
                label: "New task (fresh session)…".into(),
                disabled: None,
                action: MenuAction::TaskFresh { worktree: Some(row.raw_name.clone()) },
            },
            ActionItem {
                label: "New task (main session)…".into(),
                disabled: None,
                action: MenuAction::TaskMain { worktree: Some(row.raw_name.clone()) },
            },
            ActionItem {
                label: "Run task definition…".into(),
                disabled: None,
                action: MenuAction::RunDef { worktree: Some(row.raw_name.clone()), branch: branch_opt.clone() },
            },
            item(
                "Open in tmux window",
                inside_tmux,
                "not inside tmux",
                MenuAction::OpenTmux { path: row.path.clone() },
            ),
            item(
                "Squash merge into…",
                !busy && has_branch,
                squash_reason,
                MenuAction::SquashMerge { branch: row.branch.clone() },
            ),
            item(
                "Remove worktree…",
                !busy,
                "a task is running here",
                MenuAction::RemoveWorktree {
                    repo: repo.to_string(),
                    name: row.raw_name.clone(),
                    branch: row.branch.clone(),
                },
            ),
            ActionItem {
                label: "Create worktree…".into(),
                disabled: None,
                action: MenuAction::CreateWorktree,
            },
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

fn bulk_item(verb: &str, eligible: usize, total: usize, action: MenuAction) -> ActionItem {
    ActionItem {
        label: format!("{verb} ({eligible} of {total})"),
        disabled: if eligible > 0 { None } else { Some("no eligible rows".into()) },
        action,
    }
}

/// Build the bulk menu for a pre-resolved selection. Title is `"<total> selected"`;
/// each row's count is `(<eligible> of <total>)`, disabled with "no eligible rows"
/// when nothing in the range qualifies.
pub fn bulk_menu(sel: BulkSelection) -> (String, Vec<ActionItem>) {
    match sel {
        BulkSelection::Queue { rerun_ids, skip_ids, total } => (
            format!("{total} selected"),
            vec![
                bulk_item("Rerun", rerun_ids.len(), total, MenuAction::BulkRerun { ids: rerun_ids }),
                bulk_item("Skip", skip_ids.len(), total, MenuAction::BulkSkip { ids: skip_ids }),
            ],
        ),
        BulkSelection::Tasks { repo, run_names, total } => (
            format!("{total} selected"),
            vec![bulk_item("Run", run_names.len(), total, MenuAction::BulkRunDefs { repo, names: run_names })],
        ),
        BulkSelection::Worktrees { repo, remove_names, total } => (
            format!("{total} selected"),
            vec![bulk_item(
                "Remove worktrees…",
                remove_names.len(),
                total,
                MenuAction::BulkRemove { repo, names: remove_names },
            )],
        ),
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
        assert_eq!(labels(&items), ["Remove worktrees… (2 of 4)"]);
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
        assert_eq!(labels(&items), ["Rerun", "Skip", "Assign worktree…"]);
        assert!(enabled(&items).is_empty()); // running: nothing enabled

        assert_eq!(enabled(&queue_menu(&qrow(false), &task(TaskStatus::Failed)).1), ["Rerun", "Skip"]);
        assert_eq!(
            enabled(&queue_menu(&qrow(false), &task(TaskStatus::NeedsInput)).1),
            ["Rerun", "Skip", "Assign worktree…"]
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
    fn tasks_menu_offers_run() {
        let mut d = DefinitionSummary::default();
        d.repo = "platform".into();
        d.name = "pr-ready".into();
        let (title, items) = tasks_menu(&d);
        assert_eq!(title, "pr-ready");
        assert_eq!(labels(&items), ["Run"]);
        assert!(matches!(items[0].action, MenuAction::RunNamedDef { .. }));
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
                "New task (fresh session)…",
                "New task (main session)…",
                "Run task definition…",
                "Open in tmux window",
                "Squash merge into…",
                "Remove worktree…",
                "Create worktree…",
            ]
        );
        assert_eq!(enabled(&items), labels(&items));
    }

    #[test]
    fn worktree_menu_busy_disables_remove_and_squash_create_stays() {
        let (_t, items) = worktree_menu("platform", &wrow(WtState::Busy, "wt-a", false), true);
        let by = |lbl: &str| items.iter().find(|i| i.label == lbl).unwrap();
        assert_eq!(by("Remove worktree…").disabled.as_deref(), Some("a task is running here"));
        assert_eq!(by("Squash merge into…").disabled.as_deref(), Some("a task is running here"));
        assert_eq!(by("Create worktree…").disabled, None);
    }

    #[test]
    fn worktree_menu_branchless_disables_only_squash() {
        let (_t, items) = worktree_menu("platform", &wrow(WtState::Free, "", false), true);
        let by = |lbl: &str| items.iter().find(|i| i.label == lbl).unwrap();
        assert_eq!(by("Squash merge into…").disabled.as_deref(), Some("worktree has no branch"));
        assert_eq!(by("Remove worktree…").disabled, None);
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
