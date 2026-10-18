import type { TaskInstance } from "@queohoh/core";
import { REPO_SENTINEL } from "@queohoh/core";

/**
 * Name forms that all mean the same worktree directory: full `repo.branch`,
 * stripped display name, and the caller's request string. Used when cancelling
 * tasks that target a worktree being removed.
 */
export function worktreeNameAliases(
	repo: string,
	canonicalName: string,
	requestName: string,
): Set<string> {
	const names = new Set<string>([canonicalName, requestName]);
	const prefix = `${repo}.`;
	if (canonicalName.startsWith(prefix)) {
		names.add(canonicalName.slice(prefix.length));
	} else {
		names.add(`${prefix}${canonicalName}`);
	}
	if (requestName && requestName !== canonicalName) {
		if (requestName.startsWith(prefix)) {
			names.add(requestName.slice(prefix.length));
		} else {
			names.add(`${prefix}${requestName}`);
		}
	}
	return names;
}

/**
 * Whether a task is bound to one of the worktree names (resolved
 * `target.worktree`, or an unresolved `worktree:<name>` ref that would land
 * there). Does not use `laneKey` — a shared definition lane on another
 * worktree must not be cancelled when this worktree is removed.
 */
export function taskTargetsWorktreeNames(
	task: TaskInstance,
	repo: string,
	names: ReadonlySet<string>,
): boolean {
	if (task.target.repo !== repo) return false;
	const wt = task.target.worktree;
	if (wt !== null && wt !== REPO_SENTINEL && names.has(wt)) return true;
	// Unresolved yet: ref still points at a worktree name that is going away.
	if (wt === null) {
		const m = /^worktree:(.+)$/.exec(task.target.ref);
		if (m?.[1] && names.has(m[1])) return true;
	}
	return false;
}
