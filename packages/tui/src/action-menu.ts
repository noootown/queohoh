import type { TaskStatus } from "@queohoh/core";

export type ActionId =
	| "rerun"
	| "skip"
	| "assign-worktree"
	| "run"
	| "task-fresh"
	| "task-main"
	| "run-def"
	| "tmux-open"
	| "remove-worktree";

export interface ActionItem {
	id: ActionId;
	/** menu row text; a trailing "…" signals a follow-up modal opens */
	label: string;
	/** reason the action is inapplicable; renders dimmed + inert when set */
	disabled?: string;
}

export type ActionContext =
	| { kind: "queue"; status: TaskStatus; archived: boolean }
	| { kind: "task" }
	| { kind: "worktree"; busy: boolean; insideTmux: boolean }
	| { kind: "session"; insideTmux: boolean };

function item(
	id: ActionId,
	label: string,
	applicable: boolean,
	reason: string,
): ActionItem {
	return applicable ? { id, label } : { id, label, disabled: reason };
}

/**
 * Menu rows for the targeted item. The menu shape is stable per context kind —
 * inapplicable actions are disabled (with a reason), never hidden — so the
 * rows don't jump around as task status changes. Future project-wise actions
 * concatenate onto the returned list.
 */
export function buildActions(context: ActionContext): ActionItem[] {
	switch (context.kind) {
		case "queue": {
			if (context.archived) {
				return [
					{ id: "rerun", label: "Rerun", disabled: "archived" },
					{ id: "skip", label: "Skip", disabled: "archived" },
					{
						id: "assign-worktree",
						label: "Assign worktree…",
						disabled: "archived",
					},
				];
			}
			const { status } = context;
			return [
				item(
					"rerun",
					"Rerun",
					status === "failed" || status === "needs-input",
					`cannot rerun a ${status} task`,
				),
				item(
					"skip",
					"Skip",
					status === "failed" || status === "needs-input" || status === "done",
					`cannot skip a ${status} task`,
				),
				item(
					"assign-worktree",
					"Assign worktree…",
					status === "needs-input",
					"only for needs-input tasks",
				),
			];
		}
		case "task":
			return [{ id: "run", label: "Run" }];
		case "worktree":
			return [
				{ id: "task-fresh", label: "New task (fresh session)…" },
				{ id: "task-main", label: "New task (main session)…" },
				{ id: "run-def", label: "Run task definition…" },
				item(
					"tmux-open",
					"Open in tmux window",
					context.insideTmux,
					"not inside tmux",
				),
				item(
					"remove-worktree",
					"Remove worktree…",
					!context.busy,
					"a task is running here",
				),
			];
		case "session":
			return [
				item(
					"tmux-open",
					"Open in tmux window",
					context.insideTmux,
					"not inside tmux",
				),
			];
	}
}
