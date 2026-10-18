import { mkdtempSync, rmSync, symlinkSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, expect, it } from "vitest";
import {
	createResolverIO,
	defaultResolverFs,
	type Exec,
} from "../resolver-io.js";

const roots: string[] = [];
afterEach(() => {
	for (const root of roots.splice(0))
		rmSync(root, { recursive: true, force: true });
});

it("refuses to delete the branch when filesystem errors leave path removal unknown", async () => {
	const root = mkdtempSync(join(tmpdir(), "qoo-fs-error-"));
	roots.push(root);
	symlinkSync("b", join(root, "a"));
	symlinkSync("a", join(root, "b"));
	// lstat resolves ancestors: this is ELOOP, not a missing checkout.
	const path = join(root, "a", "checkout");
	const calls: string[][] = [];
	const exec: Exec = async (command, args) => {
		calls.push([command, ...args]);
		return { stdout: "", exitCode: 0 };
	};
	await expect(
		createResolverIO(exec).removeWorktree("/repo", {
			name: "feature",
			branch: "feature",
			path,
		}),
	).rejects.toMatchObject({ code: "ELOOP" });
	expect(calls.some((call) => call[0] === "git" && call[1] === "branch")).toBe(
		false,
	);
});

it("still recognizes a genuinely missing path", async () => {
	const root = mkdtempSync(join(tmpdir(), "qoo-fs-absent-"));
	roots.push(root);
	await expect(defaultResolverFs.exists(join(root, "absent"))).resolves.toBe(
		false,
	);
});
