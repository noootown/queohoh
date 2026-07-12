use crate::app::{DetailKind, ListPane, Selection};
use crate::ipc::types::{DefinitionSummary, StateSnapshot, TaskInstance};
use crate::selectors::{QueueRow, WorktreeRow, lane_key};

#[derive(Debug, Clone, PartialEq)]
pub enum DetailContext {
    Run { task: TaskInstance },
    Definition { repo: String, name: String },
    Worktree { row: WorktreeRow, lane_tasks: Vec<TaskInstance> },
    Empty,
}

impl DetailContext {
    pub fn kind(&self) -> DetailKind {
        match self {
            DetailContext::Run { .. } => DetailKind::Run,
            DetailContext::Definition { .. } => DetailKind::Definition,
            DetailContext::Worktree { .. } => DetailKind::Worktree,
            DetailContext::Empty => DetailKind::Empty,
        }
    }
}

const RUN_TABS: &[&str] = &["report", "transcript", "prompt", "info"];
const DEF_TABS: &[&str] = &["prompt", "config"];
const WT_TABS: &[&str] = &["info"];
const NO_TABS: &[&str] = &[];

pub fn sub_tab_names(kind: DetailKind) -> &'static [&'static str] {
    match kind {
        DetailKind::Run => RUN_TABS,
        DetailKind::Definition => DEF_TABS,
        DetailKind::Worktree => WT_TABS,
        DetailKind::Empty => NO_TABS,
    }
}

pub fn clamp_sub_tab(idx: usize, kind: DetailKind) -> usize {
    let count = sub_tab_names(kind).len();
    if count == 0 {
        return 0;
    }
    idx.min(count - 1)
}

pub fn bottom_anchored(kind: DetailKind, sub_tab: usize) -> bool {
    // Only the transcript tail-anchors; it now sits at index 1 (report is first).
    matches!(kind, DetailKind::Run) && sub_tab == 1
}

/// `(start, end_exclusive)` slice into `total` lines for a `height`-tall window
/// shifted `offset` from its anchor (`bottom` = tail-anchored, else head).
pub fn window_lines(total: usize, height: usize, offset: usize, bottom: bool) -> (usize, usize) {
    if height == 0 {
        return (0, 0);
    }
    if total <= height {
        return (0, total);
    }
    let max_offset = total - height;
    let offset = offset.min(max_offset);
    if bottom {
        let end = total - offset;
        (end - height, end)
    } else {
        (offset, offset + height)
    }
}

/// Derive the detail context from the last-focused list pane and its selection.
pub fn derive_context(
    snapshot: &StateSnapshot,
    project: &str,
    last: ListPane,
    queue: &[QueueRow],
    wt: &[WorktreeRow],
    defs: &[DefinitionSummary],
    sel: &[Selection; 3],
) -> DetailContext {
    match last {
        ListPane::Queue => {
            let Some(row) = queue.get(sel[0].cursor) else {
                return DetailContext::Empty;
            };
            let task = snapshot
                .tasks
                .iter()
                .chain(snapshot.archived_recent.iter())
                .find(|t| t.id == row.task_id)
                .cloned();
            match task {
                Some(task) => DetailContext::Run { task },
                None => DetailContext::Empty,
            }
        }
        ListPane::Tasks => match defs.get(sel[1].cursor) {
            Some(def) => DetailContext::Definition { repo: def.repo.clone(), name: def.name.clone() },
            None => DetailContext::Empty,
        },
        ListPane::Worktrees => {
            let Some(row) = wt.get(sel[2].cursor) else {
                return DetailContext::Empty;
            };
            let lane = lane_key(project, &row.raw_name);
            let mut lane_tasks: Vec<TaskInstance> = snapshot
                .tasks
                .iter()
                .chain(snapshot.archived_recent.iter())
                .filter(|t| {
                    t.target
                        .worktree
                        .as_deref()
                        .map(|w| lane_key(&t.target.repo, w) == lane)
                        .unwrap_or(false)
                })
                .cloned()
                .collect();
            // Display order: running, then needs-input, then queued, then
            // everything finished — finished newest-first (ids are ULID-like, so
            // a descending id sort is newest-first). Active tiers keep ascending
            // id (stable, oldest-first) within their rank.
            lane_tasks.sort_by(|a, b| {
                let (ra, rb) = (
                    crate::selectors::lane_task_order_rank(a.status),
                    crate::selectors::lane_task_order_rank(b.status),
                );
                ra.cmp(&rb).then_with(|| {
                    if ra >= 3 { b.id.cmp(&a.id) } else { a.id.cmp(&b.id) }
                })
            });
            DetailContext::Worktree { row: row.clone(), lane_tasks }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::DetailKind;

    #[test]
    fn sub_tab_names_per_kind() {
        assert_eq!(sub_tab_names(DetailKind::Run), &["report", "transcript", "prompt", "info"]);
        assert_eq!(sub_tab_names(DetailKind::Definition), &["prompt", "config"]);
        assert_eq!(sub_tab_names(DetailKind::Worktree), &["info"]);
        assert_eq!(sub_tab_names(DetailKind::Empty), &[] as &[&str]);
    }

    #[test]
    fn clamp_sub_tab_into_range() {
        assert_eq!(clamp_sub_tab(0, DetailKind::Run), 0);
        assert_eq!(clamp_sub_tab(3, DetailKind::Run), 3);
        assert_eq!(clamp_sub_tab(5, DetailKind::Run), 3); // clamps to `info` (last)
        assert_eq!(clamp_sub_tab(3, DetailKind::Definition), 1);
        assert_eq!(clamp_sub_tab(1, DetailKind::Worktree), 0);
        assert_eq!(clamp_sub_tab(0, DetailKind::Empty), 0);
        assert_eq!(clamp_sub_tab(4, DetailKind::Empty), 0);
    }

    #[test]
    fn bottom_anchored_only_run_transcript() {
        // Transcript (index 1) is the only tail-anchored view.
        assert!(bottom_anchored(DetailKind::Run, 1));
        assert!(!bottom_anchored(DetailKind::Run, 0)); // report
        assert!(!bottom_anchored(DetailKind::Run, 2)); // prompt
        assert!(!bottom_anchored(DetailKind::Run, 3)); // info
        assert!(!bottom_anchored(DetailKind::Definition, 0));
        assert!(!bottom_anchored(DetailKind::Worktree, 0));
        assert!(!bottom_anchored(DetailKind::Empty, 0));
    }

    // window_lines returns (start, end_exclusive). 5 lines "a".."e".
    #[test]
    fn window_all_when_fits() {
        assert_eq!(window_lines(5, 10, 0, false), (0, 5));
        assert_eq!(window_lines(5, 10, 3, true), (0, 5));
    }
    #[test]
    fn window_zero_height() {
        assert_eq!(window_lines(5, 0, 0, false), (0, 0));
    }
    #[test]
    fn window_top_default_and_offset() {
        assert_eq!(window_lines(5, 2, 0, false), (0, 2)); // a,b
        assert_eq!(window_lines(5, 2, 2, false), (2, 4)); // c,d
    }
    #[test]
    fn window_bottom_default_and_offset() {
        assert_eq!(window_lines(5, 2, 0, true), (3, 5)); // d,e
        assert_eq!(window_lines(5, 2, 1, true), (2, 4)); // c,d
    }
    #[test]
    fn window_clamps_offset() {
        assert_eq!(window_lines(5, 2, 99, false), (3, 5)); // d,e
        assert_eq!(window_lines(5, 2, 99, true), (0, 2)); // a,b
    }
}
