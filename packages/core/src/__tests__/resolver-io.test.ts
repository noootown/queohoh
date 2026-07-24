import { describe, expect, it } from "vitest";
import type { Exec } from "../resolver-io.js";
import { createResolverIO, parseWorktreePorcelain } from "../resolver-io.js";

const PORCELAIN = [
	"worktree /Users/me/ws/platform",
	"HEAD abc123",
	"branch refs/heads/main",
	"",
	"worktree /Users/me/ws/platform-worktrees/JUS-1423",
	"HEAD def456",
	"branch refs/heads/JUS-1423-fix-auth",
	"",
	"worktree /Users/me/ws/platform-worktrees/detached",
	"HEAD 999999",
	"detached",
	"",
].join("\n");

describe("parseWorktreePorcelain", () => {
	it("parses name/path/branch and skips detached", () => {
		expect(parseWorktreePorcelain(PORCELAIN)).toEqual([
			{ name: "platform", path: "/Users/me/ws/platform", branch: "main" },
			{
				name: "JUS-1423",
				path: "/Users/me/ws/platform-worktrees/JUS-1423",
				branch: "JUS-1423-fix-auth",
			},
		]);
	});

	it("returns [] for empty output", () => {
		expect(parseWorktreePorcelain("")).toEqual([]);
	});
});

function fakeExec(
	responses: Record<string, { stdout: string; exitCode: number }>,
): Exec & { calls: string[] } {
	const calls: string[] = [];
	return Object.assign(
		async (command: string, args: string[]) => {
			const key = [command, ...args].join(" ");
			calls.push(key);
			return responses[key] ?? { stdout: "", exitCode: 1 };
		},
		{ calls },
	);
}

describe("createResolverIO", () => {
	it("listWorktrees shells to git", async () => {
		const exec = fakeExec({
			"git worktree list --porcelain": { stdout: PORCELAIN, exitCode: 0 },
		});
		const io = createResolverIO(exec);
		const list = await io.listWorktrees("/repo");
		expect(list.map((w) => w.name)).toEqual(["platform", "JUS-1423"]);
	});

	it("listWorktrees throws on non-zero exit so the engine keeps last-known list", async () => {
		const exec = fakeExec({
			"git worktree list --porcelain": { stdout: "", exitCode: 128 },
		});
		const io = createResolverIO(exec);
		await expect(io.listWorktrees("/repo")).rejects.toThrow(
			/git worktree list failed/,
		);
	});

	it("prBranch returns headRefName on success, null on failure", async () => {
		const exec = fakeExec({
			"gh pr view 1423 --json headRefName": {
				stdout: '{"headRefName":"JUS-1423-fix-auth"}',
				exitCode: 0,
			},
		});
		const io = createResolverIO(exec);
		expect(await io.prBranch("/repo", 1423)).toBe("JUS-1423-fix-auth");
		expect(await io.prBranch("/repo", 9999)).toBeNull();
	});

	it("spawnWorktree runs wt then finds the new worktree", async () => {
		const before = PORCELAIN;
		const after = `${PORCELAIN}worktree /Users/me/ws/platform-worktrees/JUS-77\nHEAD aaa\nbranch refs/heads/JUS-77\n\n`;
		let wtRan = false;
		const exec: Exec = async (command, args) => {
			const key = [command, ...args].join(" ");
			if (key === "git worktree list --porcelain") {
				// The new worktree only appears after `wt switch` has run.
				return { stdout: wtRan ? after : before, exitCode: 0 };
			}
			if (key === "wt switch -c JUS-77") {
				wtRan = true;
				return { stdout: "", exitCode: 0 };
			}
			return { stdout: "", exitCode: 1 };
		};
		const io = createResolverIO(exec);
		const spawned = await io.spawnWorktree("/repo", "JUS-77");
		expect(spawned.name).toBe("JUS-77");
	});

	it("spawnWorktree with a branch fetches + tracks it and switches WITHOUT -c", async () => {
		const branch = "dependabot/npm_and_yarn/npm-0846159061";
		const name = "dependabot-npm_and_yarn-npm-0846159061";
		const after = `${PORCELAIN}worktree /Users/me/ws/platform-worktrees/${name}\nHEAD aaa\nbranch refs/heads/${branch}\n\n`;
		let wtRan = false;
		const calls: string[] = [];
		const exec: Exec = async (command, args) => {
			const key = [command, ...args].join(" ");
			calls.push(key);
			if (key === "git worktree list --porcelain") {
				return { stdout: wtRan ? after : PORCELAIN, exitCode: 0 };
			}
			if (key === `wt switch ${branch}`) {
				wtRan = true;
				return { stdout: "", exitCode: 0 };
			}
			// fetch + branch --track succeed silently.
			return { stdout: "", exitCode: 0 };
		};
		const io = createResolverIO(exec);
		const spawned = await io.spawnWorktree("/repo", name, branch);
		expect(spawned.branch).toBe(branch);
		expect(calls).toContain(`git fetch origin ${branch}`);
		expect(calls).toContain(`git branch --track ${branch} origin/${branch}`);
		expect(calls.some((c) => c.startsWith("wt switch -c"))).toBe(false);
	});

	it("spawnWorktree throws when wt fails", async () => {
		const exec = fakeExec({
			"git worktree list --porcelain": { stdout: PORCELAIN, exitCode: 0 },
		});
		const io = createResolverIO(exec);
		await expect(io.spawnWorktree("/repo", "JUS-77")).rejects.toThrow(
			/failed to spawn worktree/,
		);
	});

	it("removeWorktree force-cleans then removes then deletes the branch", async () => {
		const records: { key: string; cwd: string }[] = [];
		const exec: Exec = async (command, args, opts) => {
			records.push({ key: [command, ...args].join(" "), cwd: opts.cwd });
			return { stdout: "", exitCode: 0 };
		};
		const io = createResolverIO(exec);
		await io.removeWorktree("/repo", {
			name: "JUS-77",
			path: "/wt/JUS-77",
			branch: "JUS-77-fix",
		});
		expect(records).toEqual([
			{ key: "git reset --hard HEAD", cwd: "/wt/JUS-77" },
			{ key: "git clean -fd", cwd: "/wt/JUS-77" },
			{ key: "wt remove JUS-77-fix --yes", cwd: "/repo" },
			{ key: "git branch -D JUS-77-fix", cwd: "/repo" },
		]);
	});

	it("removeWorktree throws and skips branch -D when wt remove fails", async () => {
		const keys: string[] = [];
		const exec: Exec = async (command, args) => {
			const key = [command, ...args].join(" ");
			keys.push(key);
			// Only the `wt remove` step fails.
			return { stdout: "", exitCode: command === "wt" ? 1 : 0 };
		};
		const io = createResolverIO(exec);
		await expect(
			io.removeWorktree("/repo", {
				name: "JUS-77",
				path: "/wt/JUS-77",
				branch: "JUS-77-fix",
			}),
		).rejects.toThrow(/failed to remove worktree: JUS-77/);
		expect(keys).not.toContain("git branch -D JUS-77-fix");
	});

	it("removeWorktree tolerates reset/clean failures and still runs wt remove", async () => {
		const keys: string[] = [];
		const exec: Exec = async (command, args) => {
			const key = [command, ...args].join(" ");
			keys.push(key);
			// reset + clean fail; wt remove + branch -D succeed.
			return {
				stdout: "",
				exitCode: command === "git" && args[0] !== "branch" ? 1 : 0,
			};
		};
		const io = createResolverIO(exec);
		await io.removeWorktree("/repo", {
			name: "JUS-77",
			path: "/wt/JUS-77",
			branch: "JUS-77-fix",
		});
		expect(keys).toContain("wt remove JUS-77-fix --yes");
		expect(keys).toContain("git branch -D JUS-77-fix");
	});
});
