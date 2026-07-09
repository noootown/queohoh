import { describe, expect, it } from "vitest";
import { discoverItems } from "../discovery.js";
import type { Exec } from "../resolver-io.js";

function execReturning(stdout: string, exitCode = 0): Exec {
	return async () => ({ stdout, exitCode });
}

describe("discoverItems", () => {
	it("parses a JSON array and stringifies values", async () => {
		const exec = execReturning('[{"number": 1423, "title": "fix auth"}]');
		const items = await discoverItems("gh pr list --json number,title", exec, {
			cwd: "/repo",
		});
		expect(items).toEqual([{ number: "1423", title: "fix auth" }]);
	});

	it("throws on nonzero exit", async () => {
		const exec = execReturning("", 1);
		await expect(discoverItems("boom", exec, { cwd: "/repo" })).rejects.toThrow(
			"discovery command failed (exit 1)",
		);
	});

	it("throws on non-array JSON", async () => {
		const exec = execReturning('{"not": "array"}');
		await expect(discoverItems("x", exec, { cwd: "/repo" })).rejects.toThrow(
			"discovery command must return a JSON array",
		);
	});

	it("passes the command through bash -lc with cwd", async () => {
		let seen: { command: string; args: string[]; cwd: string } | null = null;
		const exec: Exec = async (command, args, opts) => {
			seen = { command, args, cwd: opts.cwd };
			return { stdout: "[]", exitCode: 0 };
		};
		await discoverItems("echo '[]'", exec, { cwd: "/repo" });
		expect(seen).toEqual({
			command: "/bin/bash",
			args: ["-lc", "echo '[]'"],
			cwd: "/repo",
		});
	});
});
