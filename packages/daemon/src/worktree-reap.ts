import { realpathSync } from "node:fs";
import type { Exec, TaskInstance, WorktreeInfo } from "@queohoh/core";
import { parseRef } from "@queohoh/core";
import { worktreeNameAliases } from "./worktree-names.js";

/** Fresh deletion preflight, independent of display enrichment and its TTL.
 * Only a successful exact OPEN query returning [] proves absence; merged PR
 * data, malformed output, missing gh, and all IO failures retain the tree. */
export async function freshCleanupFacts(
	exec: Exec,
	repoPath: string,
	wt: WorktreeInfo,
): Promise<{ lastCommitMs: number } | null> {
	try {
		const pr = await exec(
			"gh",
			[
				"pr",
				"list",
				"--head",
				wt.branch,
				"--state",
				"open",
				"--json",
				"number",
				"--limit",
				"1",
			],
			{ cwd: repoPath },
		);
		if (pr.exitCode !== 0) return null;
		const open: unknown = JSON.parse(pr.stdout);
		if (!Array.isArray(open) || open.length !== 0) return null;
		const status = await exec(
			"git",
			[
				"-C",
				wt.path,
				"status",
				"--porcelain=v1",
				"--untracked-files=all",
				"--ignored=matching",
			],
			{ cwd: wt.path },
		);
		if (status.exitCode !== 0 || status.stdout.trim() !== "") return null;
		const commit = await exec(
			"git",
			["-C", wt.path, "log", "-1", "--format=%ct"],
			{ cwd: wt.path },
		);
		if (commit.exitCode !== 0 || !/^\d+$/.test(commit.stdout.trim()))
			return null;
		const lastCommitMs = Number(commit.stdout.trim()) * 1000;
		if (!Number.isSafeInteger(lastCommitMs) || lastCommitMs <= 0) return null;
		return { lastCommitMs };
	} catch {
		return null;
	}
}

/** Every finite name that findWorktree can resolve to this checkout. Directory
 * suffixes and the real branch matter with custom worktree path templates. */
export function cleanupWorktreeNames(
	repo: string,
	wt: WorktreeInfo,
): Set<string> {
	const names = worktreeNameAliases(repo, wt.name, wt.name);
	names.add(wt.branch);
	names.add(wt.branch.replace(/\//g, "-"));
	for (let i = wt.name.indexOf("."); i >= 0; i = wt.name.indexOf(".", i + 1)) {
		names.add(wt.name.slice(i + 1));
	}
	names.delete("");
	return names;
}

/** null = an unresolved ref whose destination cannot safely be ruled out
 * without async IO; live/favorite instances conservatively protect the repo. */
export function cleanupTaskMatch(
	task: TaskInstance,
	repo: string,
	names: ReadonlySet<string>,
): boolean | null {
	if (task.target.repo !== repo) return false;
	if (task.target.worktree !== null) return names.has(task.target.worktree);
	try {
		const ref = parseRef(task.target.ref);
		if (ref.kind === "worktree") return names.has(ref.name);
		if (ref.kind === "ticket") return names.has(ref.id);
		if (ref.kind === "repo") return false;
		return null;
	} catch {
		return null;
	}
}

/** Project labels are distinct task namespaces, but git ownership is scoped to
 * a common directory. Ambiguous registrations cannot safely reserve a lane:
 * conservatively disable cleanup there, including on any identity query error. */
export async function cleanupUnambiguousRepos(
	projects: readonly { name: string; path: string }[],
	exec: Exec,
): Promise<Set<string>> {
	if (projects.length <= 1)
		return new Set(projects.map((project) => project.name));
	try {
		const identities = await Promise.all(
			projects.map(async (project) => {
				const result = await exec(
					"git",
					[
						"-C",
						project.path,
						"rev-parse",
						"--path-format=absolute",
						"--git-common-dir",
					],
					{ cwd: project.path },
				);
				if (result.exitCode !== 0 || !result.stdout.trim())
					throw new Error("unknown repository identity");
				return {
					name: project.name,
					common: realpathSync(result.stdout.trim()),
				};
			}),
		);
		return new Set(
			identities
				.filter(
					(identity) =>
						identities.filter((other) => other.common === identity.common)
							.length === 1,
				)
				.map((identity) => identity.name),
		);
	} catch {
		return new Set();
	}
}
