import { describe, expect, it } from "vitest";
import type { Exec } from "../resolver-io.js";
import { createResolverIO, parseWorktreePorcelain } from "../resolver-io.js";

const PORCELAIN = [
	"worktree /Users/me/ws/platform",
	"HEAD abc123",
	"branch refs/heads/main",
	"",
	"worktree /Users/me/ws/platform-worktrees/TICK-1423",
	"HEAD def456",
	"branch refs/heads/TICK-1423-fix-auth",
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
				name: "TICK-1423",
				path: "/Users/me/ws/platform-worktrees/TICK-1423",
				branch: "TICK-1423-fix-auth",
			},
		]);
	});

	it("returns [] for empty output", () => {
		expect(parseWorktreePorcelain("")).toEqual([]);
	});
});

function fakeExec(
	responses: Record<
		string,
		{ stdout: string; stderr?: string; exitCode: number }
	>,
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
		expect(list.map((w) => w.name)).toEqual(["platform", "TICK-1423"]);
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
				stdout: '{"headRefName":"TICK-1423-fix-auth"}',
				exitCode: 0,
			},
		});
		const io = createResolverIO(exec);
		expect(await io.prBranch("/repo", 1423)).toBe("TICK-1423-fix-auth");
		expect(await io.prBranch("/repo", 9999)).toBeNull();
	});

	it("spawnWorktree runs wt then finds the new worktree", async () => {
		const before = PORCELAIN;
		const after = `${PORCELAIN}worktree /Users/me/ws/platform-worktrees/TICK-77\nHEAD aaa\nbranch refs/heads/TICK-77\n\n`;
		let wtRan = false;
		const exec: Exec = async (command, args) => {
			const key = [command, ...args].join(" ");
			if (key === "git worktree list --porcelain") {
				// The new worktree only appears after `wt switch` has run.
				return { stdout: wtRan ? after : before, exitCode: 0 };
			}
			if (key === "wt switch --yes --no-cd -c TICK-77") {
				wtRan = true;
				return { stdout: "", exitCode: 0 };
			}
			return { stdout: "", exitCode: 1 };
		};
		const io = createResolverIO(exec);
		const spawned = await io.spawnWorktree("/repo", "TICK-77");
		expect(spawned.name).toBe("TICK-77");
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
			if (key === `wt switch --yes --no-cd ${branch}`) {
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
		// Must not create a NEW branch (-c); --no-cd is fine.
		expect(calls.some((c) => /\bswitch\b.*\s-c(\s|$)/.test(c))).toBe(false);
	});

	it("spawnWorktree throws when wt fails and no worktree appeared", async () => {
		const exec = fakeExec({
			"git worktree list --porcelain": { stdout: PORCELAIN, exitCode: 0 },
			"wt switch --yes --no-cd -c TICK-77": {
				stdout: "",
				stderr: "hook boom",
				exitCode: 1,
			},
			// reopen retry also fails
			"wt switch --yes --no-cd TICK-77": {
				stdout: "",
				stderr: "still broken",
				exitCode: 1,
			},
		});
		const io = createResolverIO(exec);
		await expect(io.spawnWorktree("/repo", "TICK-77")).rejects.toThrow(
			/failed to spawn worktree: TICK-77:.*hook boom/,
		);
	});

	it("spawnWorktree adopts worktree when wt exits non-zero after create", async () => {
		// post-create hooks failed but the worktree directory exists
		const after = `${PORCELAIN}worktree /Users/me/ws/platform.JUS-1995\nHEAD aaa\nbranch refs/heads/JUS-1995\n\n`;
		let wtRan = false;
		const exec: Exec = async (command, args) => {
			const key = [command, ...args].join(" ");
			if (key === "git worktree list --porcelain") {
				return { stdout: wtRan ? after : PORCELAIN, exitCode: 0 };
			}
			if (key === "wt switch --yes --no-cd -c JUS-1995") {
				wtRan = true;
				return { stdout: "", stderr: "mise sync failed", exitCode: 1 };
			}
			return { stdout: "", exitCode: 1 };
		};
		const io = createResolverIO(exec);
		const spawned = await io.spawnWorktree("/repo", "JUS-1995");
		expect(spawned).toEqual({
			name: "platform.JUS-1995",
			path: "/Users/me/ws/platform.JUS-1995",
			branch: "JUS-1995",
		});
	});

	it("spawnWorktree retries without -c when branch already exists", async () => {
		const after = `${PORCELAIN}worktree /Users/me/ws/platform.JUS-1995\nHEAD aaa\nbranch refs/heads/JUS-1995\n\n`;
		const calls: string[] = [];
		let listN = 0;
		const exec: Exec = async (command, args) => {
			const key = [command, ...args].join(" ");
			calls.push(key);
			if (key === "git worktree list --porcelain") {
				listN += 1;
				// after reopen succeeds, list sees the worktree
				return {
					stdout: listN >= 1 && calls.includes("wt switch --yes --no-cd JUS-1995")
						? after
						: PORCELAIN,
					exitCode: 0,
				};
			}
			if (key === "wt switch --yes --no-cd -c JUS-1995") {
				return {
					stdout: "",
					stderr: "✗ Branch JUS-1995 already exists",
					exitCode: 1,
				};
			}
			if (key === "wt switch --yes --no-cd JUS-1995") {
				return { stdout: "", exitCode: 0 };
			}
			return { stdout: "", exitCode: 1 };
		};
		const io = createResolverIO(exec);
		const spawned = await io.spawnWorktree("/repo", "JUS-1995");
		expect(spawned.branch).toBe("JUS-1995");
		expect(calls).toContain("wt switch --yes --no-cd -c JUS-1995");
		expect(calls).toContain("wt switch --yes --no-cd JUS-1995");
	});

	it("removeWorktree force-cleans then removes then deletes the branch", async () => {
		const records: { key: string; cwd: string }[] = [];
		const exec: Exec = async (command, args, opts) => {
			records.push({ key: [command, ...args].join(" "), cwd: opts.cwd });
			return { stdout: "", exitCode: 0 };
		};
		const io = createResolverIO(exec);
		await io.removeWorktree("/repo", {
			name: "TICK-77",
			path: "/wt/TICK-77",
			branch: "TICK-77-fix",
		});
		expect(records).toEqual([
			{ key: "git reset --hard HEAD", cwd: "/wt/TICK-77" },
			{ key: "git clean -fd", cwd: "/wt/TICK-77" },
			{ key: "wt remove --yes TICK-77-fix", cwd: "/repo" },
			{ key: "git branch -D TICK-77-fix", cwd: "/repo" },
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
				name: "TICK-77",
				path: "/wt/TICK-77",
				branch: "TICK-77-fix",
			}),
		).rejects.toThrow(/failed to remove worktree: TICK-77/);
		expect(keys).not.toContain("git branch -D TICK-77-fix");
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
			name: "TICK-77",
			path: "/wt/TICK-77",
			branch: "TICK-77-fix",
		});
		expect(keys).toContain("wt remove --yes TICK-77-fix");
		expect(keys).toContain("git branch -D TICK-77-fix");
	});
});
