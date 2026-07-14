import { mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { makeRedactor } from "../redact.js";
import { executeClaude, formatEventToMarkdown } from "../runner.js";

const FAKE = join(
	dirname(fileURLToPath(import.meta.url)),
	"fixtures",
	"fake-claude.mjs",
);

function paths() {
	const dir = mkdtempSync(join(tmpdir(), "qo-run-"));
	return {
		dir,
		eventsPath: join(dir, "events.jsonl"),
		transcriptPath: join(dir, "transcript.md"),
	};
}

const passthrough = makeRedactor(new Map());

describe("executeClaude", () => {
	it("captures result, usage, session id, and writes events + transcript", async () => {
		const { dir, eventsPath, transcriptPath } = paths();
		const result = await executeClaude({
			prompt: "do the thing",
			model: "opus",
			cwd: dir,
			timeoutMs: 30_000,
			claudeBin: FAKE,
			eventsPath,
			transcriptPath,
			redact: makeRedactor(new Map([["TOKEN_VALUE_XYZ", "MY_TOKEN"]])),
		});
		expect(result.exitCode).toBe(0);
		expect(result.timedOut).toBe(false);
		expect(result.signal).toBeNull();
		expect(result.sessionId).toBe("sess-123");
		expect(result.resultText).toBe("All done.");
		expect(result.usage).toEqual({ costUsd: 0.42, turns: 3, durationMs: 1234 });

		const events = readFileSync(eventsPath, "utf-8").trim().split("\n");
		expect(events).toHaveLength(3);
		expect(events[1]).toContain("[REDACTED:MY_TOKEN]");
		expect(events[1]).not.toContain("TOKEN_VALUE_XYZ");

		const transcript = readFileSync(transcriptPath, "utf-8");
		expect(transcript).toContain("### Tool: Bash");
		expect(transcript).toContain("echo hi");
		expect(transcript).toContain("[REDACTED:MY_TOKEN]");
	});

	it("times out a hung process", async () => {
		const { eventsPath, transcriptPath, dir } = paths();
		const result = await executeClaude({
			prompt: "hang",
			model: "opus",
			cwd: dir,
			timeoutMs: 1500,
			claudeBin: FAKE,
			claudeArgs: [],
			env: { FAKE_CLAUDE_MODE: "hang" },
			eventsPath,
			transcriptPath,
			redact: passthrough,
		});
		expect(result.timedOut).toBe(true);
		// The timeout path kills the process group with SIGTERM; the close event
		// surfaces that signal.
		expect(result.signal).toBe("SIGTERM");
	}, 15_000);

	it("kills via the idle timer when the stream goes silent, well before the ceiling", async () => {
		const { eventsPath, transcriptPath, dir } = paths();
		const start = Date.now();
		const result = await executeClaude({
			prompt: "hang",
			model: "opus",
			cwd: dir,
			// Ceiling is generous; the idle window (short) must be the one that
			// actually fires.
			timeoutMs: 10_000,
			idleTimeoutMs: 300,
			claudeBin: FAKE,
			env: { FAKE_CLAUDE_MODE: "hang" },
			eventsPath,
			transcriptPath,
			redact: passthrough,
		});
		const elapsed = Date.now() - start;
		expect(result.timedOut).toBe(true);
		expect(result.signal).toBe("SIGTERM");
		// Killed near the idle window, nowhere near the 10s ceiling.
		expect(elapsed).toBeLessThan(5000);
	}, 15_000);

	it("ceiling fires even when the stream stays continuously active (idle never elapses)", async () => {
		const { eventsPath, transcriptPath, dir } = paths();
		const start = Date.now();
		const result = await executeClaude({
			prompt: "trickle",
			model: "opus",
			cwd: dir,
			// Ceiling is shorter than how long the trickle would need to go idle;
			// the idle timer (reset every ~100ms by the fixture) must never fire.
			timeoutMs: 1200,
			idleTimeoutMs: 400,
			claudeBin: FAKE,
			env: { FAKE_CLAUDE_MODE: "trickle" },
			eventsPath,
			transcriptPath,
			redact: passthrough,
		});
		const elapsed = Date.now() - start;
		expect(result.timedOut).toBe(true);
		expect(result.signal).toBe("SIGTERM");
		// Proves the idle timer (400ms) did NOT kill it early — the ceiling
		// (1200ms) had to fire instead.
		expect(elapsed).toBeGreaterThanOrEqual(1000);
	}, 15_000);

	it("reports nonzero exit with stderr", async () => {
		const { eventsPath, transcriptPath, dir } = paths();
		const result = await executeClaude({
			prompt: "x",
			model: "opus",
			cwd: dir,
			timeoutMs: 10_000,
			claudeBin: FAKE,
			env: { FAKE_CLAUDE_MODE: "crash" },
			eventsPath,
			transcriptPath,
			redact: passthrough,
		});
		expect(result.exitCode).toBe(2);
		expect(result.signal).toBeNull();
		expect(result.stderr).toContain("boom");
	});

	it("resolves (never rejects) when run files cannot be initialized", async () => {
		const { dir, transcriptPath } = paths();
		const eventsPath = join(dir, "nonexistent-subdir", "events.jsonl");
		const result = await executeClaude({
			prompt: "x",
			model: "opus",
			cwd: dir,
			timeoutMs: 5_000,
			claudeBin: FAKE,
			eventsPath,
			transcriptPath,
			redact: passthrough,
		});
		expect(result.exitCode).toBe(1);
		expect(result.timedOut).toBe(false);
		expect(result.sessionId).toBeNull();
		expect(result.stderr).toContain("Failed to initialize run files");
	});

	it("includes --resume <id> after --model when resumeSessionId is set", async () => {
		const { dir, eventsPath, transcriptPath } = paths();
		const argvOut = join(dir, "argv.json");
		await executeClaude({
			prompt: "x",
			model: "opus",
			cwd: dir,
			timeoutMs: 30_000,
			claudeBin: FAKE,
			resumeSessionId: "abc",
			env: { FAKE_CLAUDE_ARGV_OUT: argvOut },
			eventsPath,
			transcriptPath,
			redact: passthrough,
		});
		const argv = JSON.parse(readFileSync(argvOut, "utf-8")) as string[];
		const resumeIdx = argv.indexOf("--resume");
		expect(resumeIdx).toBeGreaterThanOrEqual(0);
		expect(argv[resumeIdx + 1]).toBe("abc");
		// --resume is inserted after --model <model>
		expect(argv.indexOf("--model")).toBeLessThan(resumeIdx);
	});

	it("omits --resume when resumeSessionId is unset", async () => {
		const { dir, eventsPath, transcriptPath } = paths();
		const argvOut = join(dir, "argv.json");
		await executeClaude({
			prompt: "x",
			model: "opus",
			cwd: dir,
			timeoutMs: 30_000,
			claudeBin: FAKE,
			env: { FAKE_CLAUDE_ARGV_OUT: argvOut },
			eventsPath,
			transcriptPath,
			redact: passthrough,
		});
		const argv = JSON.parse(readFileSync(argvOut, "utf-8")) as string[];
		expect(argv).not.toContain("--resume");
	});

	it("resolves (never rejects) when the binary is missing", async () => {
		const { eventsPath, transcriptPath, dir } = paths();
		const result = await executeClaude({
			prompt: "x",
			model: "opus",
			cwd: dir,
			timeoutMs: 5_000,
			claudeBin: "/nonexistent/claude-bin",
			eventsPath,
			transcriptPath,
			redact: passthrough,
		});
		expect(result.exitCode).toBe(1);
		expect(result.stderr).toContain("Failed to spawn");
	});
});

describe("formatEventToMarkdown", () => {
	it("formats assistant text, thinking, and tool_use blocks", () => {
		const md = formatEventToMarkdown({
			type: "assistant",
			message: {
				content: [
					{ type: "thinking", thinking: "hmm" },
					{ type: "text", text: "hello" },
					{ type: "tool_use", name: "Edit", input: { file_path: "/a.ts" } },
				],
			},
		});
		expect(md).toContain("### Thinking");
		expect(md).toContain("hello");
		expect(md).toContain("### Tool: Edit");
		expect(md).toContain("File: `/a.ts`");
	});

	it("returns null for non-assistant events", () => {
		expect(formatEventToMarkdown({ type: "system" })).toBeNull();
	});
});
