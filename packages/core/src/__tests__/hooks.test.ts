import { describe, expect, it } from "vitest";
import { execHook } from "../hooks.js";
import type { Exec } from "../resolver-io.js";

describe("execHook", () => {
	it("runs the command through bash -lc in cwd", async () => {
		let seen: unknown;
		const exec: Exec = async (command, args, opts) => {
			seen = { command, args, cwd: opts.cwd };
			return { stdout: "", exitCode: 0 };
		};
		await execHook("mise run setup", exec, { cwd: "/wt" });
		expect(seen).toEqual({
			command: "/bin/bash",
			args: ["-lc", "mise run setup"],
			cwd: "/wt",
		});
	});

	it("throws on nonzero exit", async () => {
		const exec: Exec = async () => ({ stdout: "", exitCode: 3 });
		await expect(execHook("boom", exec, { cwd: "/wt" })).rejects.toThrow(
			"hook failed (exit 3): boom",
		);
	});
});
