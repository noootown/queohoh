import { describe, expect, it } from "vitest";
import type { WorktreeInfo } from "../resolver.js";
import {
	createResolverIO,
	type Exec,
	parseExistingDirPath,
	type ResolverFs,
} from "../resolver-io.js";

/** Provider exit 0 is not proof the original path is gone. Wait for deletion,
 * then require manual inspection of any leftover; never recursively clear it. */

const wt: WorktreeInfo = {
	name: "platform.feature-x",
	path: "/wt/platform.feature-x",
	branch: "feature/x",
} as WorktreeInfo;

/** Records argv and lets each command's exit code be scripted. */
function recordingExec(worktreeListOutput = ""): {
	exec: Exec;
	calls: string[][];
} {
	const calls: string[][] = [];
	const exec: Exec = async (command, args) => {
		calls.push([command, ...args]);
		if (command === "git" && args[0] === "worktree" && args[1] === "list") {
			return { stdout: worktreeListOutput, stderr: "", exitCode: 0 };
		}
		return { stdout: "", stderr: "", exitCode: 0 };
	};
	return { exec, calls };
}

/** An fs seam whose `exists` answers true for the first `trueFor` probes. */
function scriptedFs(trueFor: number): ResolverFs & { removed: string[] } {
	let seen = 0;
	const removed: string[] = [];
	return {
		removed,
		async exists() {
			seen += 1;
			return seen <= trueFor;
		},
		async remove(path) {
			removed.push(path);
		},
	};
}

const fast = { settleAttempts: 3, settleIntervalMs: 1 };

describe("removeWorktree", () => {
	it("does not pre-clean ignored data before a removal that could fail", async () => {
		// Explicit cleanup must not broaden to ignored data as a preparatory step.
		const { exec, calls } = recordingExec();
		await createResolverIO(exec, scriptedFs(0), fast).removeWorktree(
			"/repo",
			wt,
		);
		const clean = calls.find((c) => c[0] === "git" && c[1] === "clean");
		expect(clean).toEqual(["git", "clean", "-fd"]);
	});

	it("returns as soon as the tree is actually gone, without forcing", async () => {
		const { exec } = recordingExec();
		const fs = scriptedFs(0); // gone on the first probe
		await createResolverIO(exec, fs, fast).removeWorktree("/repo", wt);
		expect(fs.removed).toEqual([]);
	});

	it("waits for the background delete rather than trusting exit 0", async () => {
		const { exec } = recordingExec();
		const fs = scriptedFs(2); // lingers for two probes, then vanishes
		await createResolverIO(exec, fs, fast).removeWorktree("/repo", wt);
		// It went on its own within the window — nothing forced.
		expect(fs.removed).toEqual([]);
	});

	it("preserves unregistered leftovers when the background delete never lands", async () => {
		// Missing registration cannot prove this path is still provider output.
		const { exec } = recordingExec(
			"worktree /repo\nHEAD abc\nbranch refs/heads/main\n",
		);
		const fs = scriptedFs(99); // never disappears
		await expect(
			createResolverIO(exec, fs, fast).removeWorktree("/repo", wt),
		).rejects.toThrow(/manual inspection/);
		expect(fs.removed).toEqual([]);
	});

	it("refuses to delete a path git still lists as a worktree", async () => {
		// wt claimed success but the registration survives — a contract violation.
		// Deleting here could destroy a live checkout, so surface it instead.
		const listing = `worktree ${wt.path}\nHEAD abc\nbranch refs/heads/feature/x\n`;
		const { exec } = recordingExec(listing);
		const fs = scriptedFs(99);
		await expect(
			createResolverIO(exec, fs, fast).removeWorktree("/repo", wt),
		).rejects.toThrow(/still registered/);
		expect(fs.removed).toEqual([]);
	});

	it("treats an unreadable worktree listing as 'still registered', never as safe", async () => {
		const failing: Exec = async (command, args) => {
			if (command === "git" && args[0] === "worktree" && args[1] === "list") {
				return { stdout: "", stderr: "git busy", exitCode: 1 };
			}
			return { stdout: "", stderr: "", exitCode: 0 };
		};
		const fs = scriptedFs(99);
		await expect(
			createResolverIO(failing, fs, fast).removeWorktree("/repo", wt),
		).rejects.toThrow(/still registered/);
		expect(fs.removed).toEqual([]);
	});

	it("still throws when wt remove itself fails, before any settling", async () => {
		const failing: Exec = async (command) =>
			command === "wt"
				? { stdout: "", stderr: "boom", exitCode: 1 }
				: { stdout: "", stderr: "", exitCode: 0 };
		const fs = scriptedFs(99);
		await expect(
			createResolverIO(failing, fs, fast).removeWorktree("/repo", wt),
		).rejects.toThrow(/failed to remove worktree/);
		expect(fs.removed).toEqual([]);
	});
});

/** A failed provider command must never authorize deletion of its error path. */
describe("spawnWorktree leftover safety", () => {
	const HUSK = "/wt/platform.feature-x";
	const REFUSAL = `✗ Directory already exists: ${HUSK}`;
	const LISTING = (withWt: boolean) =>
		withWt
			? `worktree /repo\nHEAD a\nbranch refs/heads/main\n\nworktree ${HUSK}\nHEAD b\nbranch refs/heads/feature/x\n`
			: "worktree /repo\nHEAD a\nbranch refs/heads/main\n";

	/** wt refuses until the husk is cleared, then succeeds. */
	function healingExec(fs: { present: boolean }) {
		const calls: string[] = [];
		const exec: Exec = async (command, args) => {
			const key = [command, ...args].join(" ");
			calls.push(key);
			if (command === "git" && args[0] === "worktree" && args[1] === "list") {
				return { stdout: LISTING(!fs.present), stderr: "", exitCode: 0 };
			}
			if (command === "wt" && args[0] === "switch") {
				return fs.present
					? { stdout: "", stderr: REFUSAL, exitCode: 1 }
					: { stdout: "", stderr: "", exitCode: 0 };
			}
			return { stdout: "", stderr: "", exitCode: 0 };
		};
		return { exec, calls };
	}

	it("preserves an unregistered directory rather than guessing it is a husk", async () => {
		const state = { present: true };
		const { exec, calls } = healingExec(state);
		const removed: string[] = [];
		const fsio: ResolverFs = {
			async exists(p) {
				return p === HUSK && state.present;
			},
			async remove(p) {
				removed.push(p);
				state.present = false;
			},
		};
		await expect(
			createResolverIO(exec, fsio, fast).spawnWorktree(
				"/repo",
				"platform.feature-x",
				"feature/x",
			),
		).rejects.toThrow(/failed to spawn worktree/);
		expect(state.present).toBe(true);
		expect(removed).toEqual([]);
		expect(calls.filter((c) => c.startsWith("wt switch"))).toHaveLength(1);
	});

	it("never deletes a path that still has a .git (a live worktree)", async () => {
		const state = { present: true };
		const { exec } = healingExec(state);
		const removed: string[] = [];
		const fsio: ResolverFs = {
			async exists() {
				return true; // both the dir AND its .git exist
			},
			async remove(p) {
				removed.push(p);
			},
		};
		await expect(
			createResolverIO(exec, fsio, fast).spawnWorktree(
				"/repo",
				"platform.feature-x",
				"feature/x",
			),
		).rejects.toThrow(/failed to spawn worktree/);
		expect(removed).toEqual([]);
	});

	it("leaves the filesystem alone when wt failed for any other reason", async () => {
		const exec: Exec = async (command, args) => {
			if (command === "git" && args[0] === "worktree" && args[1] === "list") {
				return { stdout: LISTING(false), stderr: "", exitCode: 0 };
			}
			if (command === "wt") {
				return { stdout: "", stderr: "✗ mise sync failed", exitCode: 1 };
			}
			return { stdout: "", stderr: "", exitCode: 0 };
		};
		const removed: string[] = [];
		const fsio: ResolverFs = {
			async exists() {
				return true;
			},
			async remove(p) {
				removed.push(p);
			},
		};
		await expect(
			createResolverIO(exec, fsio, fast).spawnWorktree(
				"/repo",
				"platform.feature-x",
				"feature/x",
			),
		).rejects.toThrow(/mise sync failed/);
		expect(removed).toEqual([]);
	});
});

describe("parseExistingDirPath", () => {
	it("extracts and home-expands the path wt refused", () => {
		expect(
			parseExistingDirPath(
				"✗ Directory already exists: ~/dev/platform.feature-x",
				"/home/op",
			),
		).toBe("/home/op/dev/platform.feature-x");
	});

	it("passes an absolute path through unchanged", () => {
		expect(
			parseExistingDirPath("✗ Directory already exists: /wt/thing", "/home/k"),
		).toBe("/wt/thing");
	});

	it("returns null for any other failure, so no path is ever guessed", () => {
		expect(parseExistingDirPath("✗ mise sync failed", "/home/k")).toBeNull();
		expect(parseExistingDirPath("", "/home/k")).toBeNull();
	});

	it.each([
		"✗ Directory already exists: relative/path",
		"✗ Directory already exists: /tmp/../valuable",
		"✗ Directory already exists: /tmp/valuable\u001b[0m",
		"✗ Directory already exists: /tmp/valuable\nwrapped-path",
		"✗ Directory already exists: /tmp/first\n✗ Directory already exists: /tmp/second",
	])("refuses ambiguous or malformed diagnostic %j", (output) => {
		expect(parseExistingDirPath(output)).toBeNull();
	});
});
