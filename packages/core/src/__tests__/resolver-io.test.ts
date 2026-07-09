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

	it("spawnWorktree throws when wt fails", async () => {
		const exec = fakeExec({
			"git worktree list --porcelain": { stdout: PORCELAIN, exitCode: 0 },
		});
		const io = createResolverIO(exec);
		await expect(io.spawnWorktree("/repo", "JUS-77")).rejects.toThrow(
			/failed to spawn worktree/,
		);
	});
});
