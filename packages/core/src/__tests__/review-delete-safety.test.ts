import { existsSync, mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import {
	createResolverIO,
	type Exec,
	parseExistingDirPath,
} from "../resolver-io.js";

function refusal(path: string): Exec {
	return async (command, args) =>
		command === "wt"
			? {
					stdout: "",
					stderr: `✗ Directory already exists: ${path}`,
					exitCode: 1,
				}
			: {
					stdout:
						args[0] === "worktree"
							? "worktree /repo\nHEAD abc\nbranch refs/heads/main\n"
							: "",
					exitCode: 0,
				};
}
describe("review deletion safety", () => {
	it("preserves a non-git directory that predates this spawn attempt", async () => {
		const base = mkdtempSync(join(tmpdir(), "qoo-safe-repro-"));
		const target = join(base, "platform.feature");
		mkdirSync(target);
		const file = join(target, "valuable-user-notes.txt");
		writeFileSync(file, "not an abandoned worktree");
		await expect(
			createResolverIO(refusal(target)).spawnWorktree(
				"/repo",
				"feature",
				"feature",
			),
		).rejects.toThrow();
		expect(existsSync(file)).toBe(true);
	});
	it("preserves the complete directory path containing spaces", () => {
		expect(
			parseExistingDirPath(
				"✗ Directory already exists: /tmp/My Projects/platform.feature",
			),
		).toBe("/tmp/My Projects/platform.feature");
	});
	it("does not delete a sibling due to path truncation at a space", async () => {
		const base = mkdtempSync(join(tmpdir(), "qoo-space-repro-"));
		const unrelated = join(base, "My");
		mkdirSync(unrelated);
		const file = join(unrelated, "valuable.txt");
		writeFileSync(file, "unrelated sibling");
		const target = join(base, "My Projects", "platform.feature");
		await expect(
			createResolverIO(refusal(target)).spawnWorktree(
				"/repo",
				"feature",
				"feature",
			),
		).rejects.toThrow();
		expect(existsSync(file)).toBe(true);
	});
});
