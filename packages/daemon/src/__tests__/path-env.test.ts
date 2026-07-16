import type { Exec } from "@queohoh/core";
import { describe, expect, it, vi } from "vitest";
import {
	GH_MISSING_WARNING,
	mergePathEntries,
	normalizeDaemonPath,
	probeGh,
} from "../path-env.js";

describe("mergePathEntries", () => {
	it("appends login-shell entries not already present, existing first", () => {
		// The daemon's inherited PATH (`current`) may carry deliberate overrides,
		// so it keeps precedence — new entries are only appended after it.
		expect(
			mergePathEntries("/usr/bin:/bin", "/opt/homebrew/bin:/usr/bin"),
		).toBe("/usr/bin:/bin:/opt/homebrew/bin");
	});

	it("dedups entries within and across both inputs (first occurrence wins)", () => {
		expect(mergePathEntries("/a:/b:/a", "/b:/c:/c")).toBe("/a:/b:/c");
	});

	it("returns login-shell entries when current PATH is empty", () => {
		expect(mergePathEntries("", "/opt/homebrew/bin:/usr/bin")).toBe(
			"/opt/homebrew/bin:/usr/bin",
		);
	});

	it("returns current PATH unchanged when login shell is empty", () => {
		expect(mergePathEntries("/usr/bin:/bin", "")).toBe("/usr/bin:/bin");
	});

	it("strips a trailing newline from the bash output", () => {
		// `echo "$PATH"` appends a newline; the last entry would otherwise carry it.
		expect(mergePathEntries("/usr/bin", "/opt/homebrew/bin\n")).toBe(
			"/usr/bin:/opt/homebrew/bin",
		);
	});

	it("drops empty segments (leading/trailing/doubled colons)", () => {
		expect(mergePathEntries("/usr/bin:", ":/opt/homebrew/bin::")).toBe(
			"/usr/bin:/opt/homebrew/bin",
		);
	});

	it("returns empty string when both inputs are empty", () => {
		expect(mergePathEntries("", "")).toBe("");
	});
});

/** An Exec stub that records its calls and returns a fixed result. */
function recordingExec(
	result: { stdout: string; exitCode: number },
	calls: { command: string; args: string[]; cwd: string }[],
): Exec {
	return async (command, args, opts) => {
		calls.push({ command, args, cwd: opts.cwd });
		return result;
	};
}

describe("normalizeDaemonPath", () => {
	it("resolves the login-shell PATH via `/bin/bash -lc` and merges it into env.PATH", async () => {
		const calls: { command: string; args: string[]; cwd: string }[] = [];
		const exec = recordingExec(
			{ stdout: "/opt/homebrew/bin:/usr/bin\n", exitCode: 0 },
			calls,
		);
		const env = { PATH: "/usr/bin:/bin" };

		await normalizeDaemonPath(exec, { env });

		expect(calls).toHaveLength(1);
		expect(calls[0]?.command).toBe("/bin/bash");
		expect(calls[0]?.args).toEqual(["-lc", 'echo "$PATH"']);
		expect(env.PATH).toBe("/usr/bin:/bin:/opt/homebrew/bin");
	});

	it("leaves PATH untouched when the bash call exits non-zero", async () => {
		const calls: { command: string; args: string[]; cwd: string }[] = [];
		const exec = recordingExec({ stdout: "", exitCode: 1 }, calls);
		const env = { PATH: "/usr/bin:/bin" };

		await normalizeDaemonPath(exec, { env });

		expect(env.PATH).toBe("/usr/bin:/bin");
	});

	it("leaves PATH untouched when the bash call throws", async () => {
		const exec: Exec = async () => {
			throw new Error("spawn failed");
		};
		const env = { PATH: "/usr/bin:/bin" };

		await normalizeDaemonPath(exec, { env });

		expect(env.PATH).toBe("/usr/bin:/bin");
	});
});

describe("probeGh", () => {
	it("returns true and does not warn when `gh --version` succeeds", async () => {
		const calls: { command: string; args: string[]; cwd: string }[] = [];
		const exec = recordingExec(
			{ stdout: "gh version 2.0\n", exitCode: 0 },
			calls,
		);
		const warn = vi.fn();

		const ok = await probeGh(exec, { warn });

		expect(ok).toBe(true);
		expect(warn).not.toHaveBeenCalled();
		expect(calls[0]?.command).toBe("gh");
		expect(calls[0]?.args).toEqual(["--version"]);
	});

	it("returns false and warns once when `gh --version` exits non-zero", async () => {
		const calls: { command: string; args: string[]; cwd: string }[] = [];
		const exec = recordingExec({ stdout: "", exitCode: 127 }, calls);
		const warn = vi.fn();

		const ok = await probeGh(exec, { warn });

		expect(ok).toBe(false);
		expect(warn).toHaveBeenCalledTimes(1);
		expect(warn).toHaveBeenCalledWith(GH_MISSING_WARNING);
	});

	it("returns false and warns when the probe throws", async () => {
		const exec: Exec = async () => {
			throw new Error("gh not found");
		};
		const warn = vi.fn();

		const ok = await probeGh(exec, { warn });

		expect(ok).toBe(false);
		expect(warn).toHaveBeenCalledTimes(1);
	});
});
