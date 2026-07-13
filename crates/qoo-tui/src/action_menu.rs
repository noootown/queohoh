//! Single-target and bulk action menus. The menu shape is stable per context
//! (inapplicable rows are disabled with a reason, never hidden) so rows never
//! jump as status changes. Each row also carries a one-sentence `description`
//! shown in the lazyvim-style right pane. Bulk menus mirror the same shape with
//! `(<eligible> of <total>)` counts.
//!
//! The QUEUE menu holds exactly ONE action — **Resume** (open the task's Claude
//! session in a new tmux tab). Its old verbs became title-bar chips/keys
//! instead: `r` re-queues (see `App::requeue_selected`) and `x` cancels
//! (skip/stop; see `App::cancel_selected`). The tasks pane has no single-target
//! menu: Enter on a tasks row runs the highlighted definition directly
//! (`App::run_selected_task_def`). The WORKTREES pane likewise dropped its
//! single-target menu — its `r`/`g`/`x` hotkeys (new task / goto tmux / remove)
//! act on the selected row directly (see `App::new_task_on_worktree` etc.).

use crate::selectors::QueueRow;

/// What a chosen menu row does. `execute_menu_action` (app) maps each variant to
/// a mode transition or an RPC/tmux dispatch. Variants are only ever added.
#[derive(Debug, Clone)]
pub enum MenuAction {
    /// Resume the task's Claude session in a new tmux tab (window) rooted at `path`
    /// (`tmux new-window -c <path> 'claude --resume <session_id>'`).
    Resume { path: String, session_id: String },
    // --- Bulk actions. Targets are frozen at menu-open time: the eligible
    // ids/names are captured here so a snapshot push that reshuffles rows
    // mid-menu can never retarget the dispatch. (Queue has no bulk menu — its
    // `r`/`x` chips carry the bulk verbs.) ---
    /// Run each zero-arg definition on this repo.
    BulkRunDefs { repo: String, names: Vec<String> },
    /// Remove each non-busy worktree (routes through `Mode::Confirm`).
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

const RESUME_DESC: &str = "Resume this task's Claude session in a new tmux tab.";

/// Single-target queue menu: exactly one row, **Resume**. Disabled (with the
/// most specific reason) when not inside tmux, when the run has recorded no
/// Claude session id yet, or when no worktree path resolves — otherwise it fires
/// [`MenuAction::Resume`]. `session_id`/`worktree_path` are resolved by the
/// caller (run record, falling back to the task's `resume_session_id`).
pub fn queue_menu(
    row: &QueueRow,
    session_id: Option<&str>,
    worktree_path: Option<&str>,
    inside_tmux: bool,
) -> (String, Vec<ActionItem>) {
    let title = row.summary.clone();
    // Reason precedence: environment first (tmux), then the two data gaps.
    let disabled_placeholder =
        || MenuAction::Resume { path: String::new(), session_id: String::new() };
    let (applicable, reason, action) = match (inside_tmux, session_id, worktree_path) {
        (false, _, _) => (false, "not inside tmux", disabled_placeholder()),
        (true, None, _) => (false, "no session yet (task never ran)", disabled_placeholder()),
        (true, Some(_), None) => (false, "no worktree for this task", disabled_placeholder()),
        (true, Some(sid), Some(path)) => (
            true,
            "",
            MenuAction::Resume { path: path.to_string(), session_id: sid.to_string() },
        ),
    };
    (title, vec![item("Resume", applicable, reason, RESUME_DESC, action)])
}

/// Pre-resolved bulk selection: eligibility is computed once by the caller at
/// menu-open time so the ids/names are frozen into the returned actions. The
/// label counts read `<eligible> of <total>` where `total` is the whole range.
/// (Queue has no bulk menu — `r`/`x` chips carry its bulk verbs.)
pub enum BulkSelection {
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

const BULK_RUN_DESC: &str = "Run each zero-arg task definition in the selection.";
const BULK_REMOVE_DESC: &str =
    "Remove each non-busy worktree in the selection (asks for confirmation).";

/// Build the bulk menu for a pre-resolved selection. Title is `"<total> selected"`;
/// each row's count is `(<eligible> of <total>)`, disabled with "no eligible rows"
/// when nothing in the range qualifies.
pub fn bulk_menu(sel: BulkSelection) -> (String, Vec<ActionItem>) {
    match sel {
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
            item("Resume", true, "", RESUME_DESC, MenuAction::Resume { path: "p".into(), session_id: "s".into() }),
            item("Run defs", true, "", BULK_RUN_DESC, MenuAction::BulkRunDefs { repo: "r".into(), names: vec![] }),
            item("Remove worktrees", true, "", BULK_REMOVE_DESC, MenuAction::BulkRemove { repo: "r".into(), names: vec![] }),
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
        // "sum" matches only "Resume".
        assert_eq!(filter_items(&items, "sum"), vec![0]);
        // Case-insensitive: "REMOVE" matches "Remove worktree".
        assert_eq!(filter_items(&items, "REMOVE"), vec![2]);
        // No match → empty.
        assert!(filter_items(&items, "zzz").is_empty());
    }

    #[test]
    fn resume_desc_opens_a_tab_not_a_pane() {
        // Resume opens a new tmux window (tab), not a split pane.
        assert!(RESUME_DESC.contains("tab"));
        assert!(!RESUME_DESC.contains("pane"));
    }
}

#[cfg(test)]
mod bulk_builder_tests {
    use super::*;

    fn labels(items: &[ActionItem]) -> Vec<String> { items.iter().map(|i| i.label.clone()).collect() }

    #[test]
    fn bulk_tasks_run_only() {
        let (title, items) = bulk_menu(BulkSelection::Tasks { repo: "platform".into(), run_names: vec!["lint".into()], total: 3 });
        assert_eq!(title, "3 selected");
        assert_eq!(labels(&items), ["Run (1 of 3)"]);
        assert!(matches!(&items[0].action, MenuAction::BulkRunDefs { repo, names } if repo == "platform" && names == &["lint".to_string()]));
    }

    #[test]
    fn bulk_tasks_zero_eligible_disables() {
        let (_t, items) = bulk_menu(BulkSelection::Tasks { repo: "platform".into(), run_names: vec![], total: 4 });
        assert_eq!(items[0].label, "Run (0 of 4)");
        assert_eq!(items[0].disabled.as_deref(), Some("no eligible rows"));
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
    use crate::ipc::types::TaskStatus;
    use crate::selectors::QueueRow;

    fn qrow(archived: bool) -> QueueRow {
        QueueRow {
            task_id: "t1".into(),
            glyph: '?',
            running: false,
            worktree: "main".into(),
            def_name: None,
            summary: "do the thing".into(),
            detail: String::new(),
            created_epoch_s: 0,
            archived,
            status: TaskStatus::Done,
            priority: "normal".into(),
            finished_epoch_s: None,
        }
    }
    fn labels(items: &[ActionItem]) -> Vec<&str> {
        items.iter().map(|i| i.label.as_str()).collect()
    }
    fn enabled(items: &[ActionItem]) -> Vec<&str> {
        items.iter().filter(|i| i.disabled.is_none()).map(|i| i.label.as_str()).collect()
    }

    #[test]
    fn queue_menu_is_single_resume_row() {
        let (title, items) = queue_menu(&qrow(false), Some("sess-1"), Some("/wt/a"), true);
        assert_eq!(title, "do the thing");
        assert_eq!(labels(&items), ["Resume"]);
        assert_eq!(enabled(&items), ["Resume"]);
        assert!(matches!(&items[0].action, MenuAction::Resume { path, session_id } if path == "/wt/a" && session_id == "sess-1"));
        assert_eq!(items[0].description, RESUME_DESC);
    }

    #[test]
    fn queue_resume_disabled_reason_precedence() {
        // tmux first, then session, then worktree path.
        let outside = queue_menu(&qrow(false), Some("s"), Some("/p"), false).1;
        assert_eq!(outside[0].disabled.as_deref(), Some("not inside tmux"));
        let no_session = queue_menu(&qrow(false), None, Some("/p"), true).1;
        assert_eq!(no_session[0].disabled.as_deref(), Some("no session yet (task never ran)"));
        let no_path = queue_menu(&qrow(false), Some("s"), None, true).1;
        assert_eq!(no_path[0].disabled.as_deref(), Some("no worktree for this task"));
    }
}
