use std::collections::HashMap;

use crate::ipc::types::ArgSpec;
use crate::selectors::WorktreeRow;

/// First `[A-Za-z]+-\d+` token, uppercased; `None` when the branch carries no
/// ticket-shaped token. Hand-rolled scan (no regex dep): greedy letter run,
/// a single `-`, then one-or-more digits.
pub fn extract_ticket(branch: &str) -> Option<String> {
    let b = branch.as_bytes();
    let n = b.len();
    let mut i = 0;
    while i < n {
        if b[i].is_ascii_alphabetic() {
            let start = i;
            while i < n && b[i].is_ascii_alphabetic() {
                i += 1;
            }
            if i < n && b[i] == b'-' {
                let mut j = i + 1;
                while j < n && b[j].is_ascii_digit() {
                    j += 1;
                }
                if j > i + 1 {
                    return Some(branch[start..j].to_ascii_uppercase());
                }
            }
            // letters not followed by `-<digits>`: keep scanning from `i`.
        } else {
            i += 1;
        }
    }
    None
}

/// Arg values implied by a worktree branch: `source`/`branch` are the branch,
/// `ticket` the extracted token (key omitted when absent so a def default
/// wins). Empty branch → empty map.
pub fn context_arg_values(branch: &str) -> HashMap<String, String> {
    let mut values = HashMap::new();
    if branch.is_empty() {
        return values;
    }
    values.insert("source".to_string(), branch.to_string());
    values.insert("branch".to_string(), branch.to_string());
    if let Some(ticket) = extract_ticket(branch) {
        values.insert("ticket".to_string(), ticket);
    }
    values
}

/// Validate a branch name for the create-worktree modal: `Some(message)` when
/// not git-ref-safe, `None` when acceptable. Order: non-empty, no whitespace,
/// no `..`, no leading `-`/`/`, no trailing `.lock`, printable ASCII only.
/// Ported from `packages/tui/src/branch.ts` (`validateBranch`); the message
/// order is load-bearing (first failing check surfaces).
pub fn validate_branch(name: &str) -> Option<String> {
    if name.is_empty() {
        return Some("branch name required".into());
    }
    if name.chars().any(|c| c.is_whitespace()) {
        return Some("no whitespace allowed".into());
    }
    if name.contains("..") {
        return Some("no '..' allowed".into());
    }
    if name.starts_with('-') || name.starts_with('/') {
        return Some("cannot start with '-' or '/'".into());
    }
    if name.ends_with(".lock") {
        return Some("cannot end with '.lock'".into());
    }
    if name.chars().any(|c| !('\u{20}'..='\u{7e}').contains(&c)) {
        return Some("printable ASCII only".into());
    }
    None
}

/// Prefill from the selected worktree row: only a real worktree row with a
/// branch other than main/master contributes (session/branchless/main rows
/// borrow nothing).
fn ambient_context_arg_values(row: Option<&WorktreeRow>) -> HashMap<String, String> {
    let Some(row) = row else { return HashMap::new() };
    if row.is_session || row.branch.is_empty() || row.branch == "main" || row.branch == "master" {
        return HashMap::new();
    }
    context_arg_values(&row.branch)
}

/// Every real worktree's branch in row order, minus session/branchless rows
/// and the primary checkout (main/master is never a sensible source).
fn branch_candidates(rows: &[WorktreeRow]) -> Vec<String> {
    rows.iter()
        .filter(|r| !r.is_session && !r.branch.is_empty() && r.branch != "main" && r.branch != "master")
        .map(|r| r.branch.clone())
        .collect()
}

/// Overlay worktree context onto a def's args for an ambient (TASKS-pane) run:
/// an arg named `source`/`branch` with no declared options becomes a dropdown
/// of the repo's worktree branches; `initial` prefills from the selected row.
/// Nothing is fixed — submission stays positional and the daemon never sees the
/// injected options.
pub fn ambient_run_args(
    args: &[ArgSpec],
    worktrees: &[WorktreeRow],
    selected: Option<&WorktreeRow>,
) -> (Vec<ArgSpec>, HashMap<String, String>) {
    let candidates = branch_candidates(worktrees);
    let out = args
        .iter()
        .map(|arg| {
            let named = arg.name == "source" || arg.name == "branch";
            let has_options = arg.options.as_ref().is_some_and(|o| !o.is_empty());
            if !named || has_options || candidates.is_empty() {
                arg.clone()
            } else {
                ArgSpec { options: Some(candidates.clone()), ..arg.clone() }
            }
        })
        .collect();
    (out, ambient_context_arg_values(selected))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::types::ArgSpec;
    use crate::selectors::{WorktreeRow, WtState};
    use std::collections::HashMap;

    fn map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn extract_ticket_cases() {
        assert_eq!(extract_ticket("JUS-1008").as_deref(), Some("JUS-1008"));
        assert_eq!(extract_ticket("jus-1008-fix-thing").as_deref(), Some("JUS-1008"));
        assert_eq!(extract_ticket("jus-1008").as_deref(), Some("JUS-1008"));
        assert_eq!(extract_ticket("main"), None);
        assert_eq!(extract_ticket("feature/no-number"), None);
        assert_eq!(extract_ticket(""), None);
        assert_eq!(extract_ticket("jus-1008-then-abc-42").as_deref(), Some("JUS-1008"));
    }

    #[test]
    fn validate_branch_table() {
        assert_eq!(validate_branch("feature-x"), None);
        assert_eq!(validate_branch("JUS-1423/fix-auth"), None);
        assert!(validate_branch("").unwrap().contains("required"));
        assert!(validate_branch("fix login").unwrap().contains("whitespace"));
        assert!(validate_branch("fix\tlogin").unwrap().contains("whitespace"));
        assert!(validate_branch("fix..auth").unwrap().contains(".."));
        assert!(validate_branch("-fix").unwrap().contains("start"));
        assert!(validate_branch("/fix").unwrap().contains("start"));
        assert!(validate_branch("fix.lock").unwrap().contains(".lock"));
        assert!(validate_branch("fix\u{1}").unwrap().contains("printable ASCII"));
        assert!(validate_branch("fïx").unwrap().contains("printable ASCII"));
    }

    #[test]
    fn context_arg_values_cases() {
        assert_eq!(
            context_arg_values("jus-1008-fix-thing"),
            map(&[("source", "jus-1008-fix-thing"), ("branch", "jus-1008-fix-thing"), ("ticket", "JUS-1008")])
        );
        assert_eq!(
            context_arg_values("feature/no-number"),
            map(&[("source", "feature/no-number"), ("branch", "feature/no-number")])
        );
        assert!(context_arg_values("").is_empty());
    }

    fn wt(name: &str, branch: &str, is_session: bool) -> WorktreeRow {
        WorktreeRow {
            name: name.to_string(),
            raw_name: name.to_string(),
            path: format!("/wt/{name}"),
            branch: branch.to_string(),
            state: WtState::Free,
            has_main_session: false,
            queued: 0,
            is_session,
            ..Default::default()
        }
    }
    fn arg(name: &str) -> ArgSpec {
        ArgSpec { name: name.to_string(), default: None, options: None, description: None }
    }
    fn rows() -> Vec<WorktreeRow> {
        vec![
            wt("a", "jus-1-a", false),
            wt("main", "main", false),
            wt("b", "feat-b", false),
            wt("sess", "", true),
        ]
    }

    #[test]
    fn ambient_context_arg_values_cases() {
        assert_eq!(
            ambient_context_arg_values(Some(&wt("a", "jus-1008-fix", false))),
            map(&[("source", "jus-1008-fix"), ("branch", "jus-1008-fix"), ("ticket", "JUS-1008")])
        );
        assert!(ambient_context_arg_values(Some(&wt("s", "", true))).is_empty()); // session row
        assert!(ambient_context_arg_values(Some(&wt("x", "", false))).is_empty()); // branchless
        assert!(ambient_context_arg_values(Some(&wt("m", "main", false))).is_empty());
        assert!(ambient_context_arg_values(Some(&wt("m", "master", false))).is_empty());
        assert!(ambient_context_arg_values(None).is_empty());
    }

    #[test]
    fn ambient_run_args_injects_source_dropdown_excluding_main_and_sessions() {
        let r = rows();
        let (args, _) = ambient_run_args(
            &[arg("source"), ArgSpec { default: Some("main".into()), ..arg("target") }],
            &r,
            Some(&r[0]),
        );
        assert_eq!(args[0].name, "source");
        assert_eq!(args[0].options.as_deref(), Some(&["jus-1-a".to_string(), "feat-b".to_string()][..]));
        assert_eq!(args[1].name, "target");
        assert_eq!(args[1].options, None);
        assert_eq!(args[1].default.as_deref(), Some("main"));
    }

    #[test]
    fn ambient_run_args_injects_for_branch_and_prefills_initial() {
        let r = rows();
        let (args, _) = ambient_run_args(&[arg("branch")], &r, Some(&r[0]));
        assert_eq!(args[0].options.as_deref(), Some(&["jus-1-a".to_string(), "feat-b".to_string()][..]));
        let (_, initial) = ambient_run_args(&[arg("source")], &r, Some(&r[0]));
        assert_eq!(initial, map(&[("source", "jus-1-a"), ("branch", "jus-1-a"), ("ticket", "JUS-1")]));
    }

    #[test]
    fn ambient_run_args_leaves_declared_options_and_freetext_untouched() {
        let r = rows();
        let declared = ArgSpec { options: Some(vec!["x".into(), "y".into()]), ..arg("source") };
        let (args, _) = ambient_run_args(std::slice::from_ref(&declared), &r, Some(&r[0]));
        assert_eq!(args[0], declared);

        let only_main = vec![wt("main", "main", false)];
        let (args, initial) = ambient_run_args(&[arg("source")], &only_main, Some(&only_main[0]));
        assert_eq!(args[0], arg("source")); // no options injected
        assert!(initial.is_empty());

        let (args, _) = ambient_run_args(&[arg("pr")], &r, Some(&r[0]));
        assert_eq!(args[0], arg("pr")); // non source/branch untouched
    }
}
