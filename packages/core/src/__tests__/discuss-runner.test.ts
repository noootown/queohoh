import { mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it, vi } from "vitest";
import { runDiscussTurn } from "../discuss/runner.js";
import { DiscussStore } from "../discuss/store.js";
import { grokAdapter } from "../providers/grok.js";
import type { ProviderAdapter } from "../providers/types.js";
import type { ExecuteRunOptions, RunResult } from "../runner.js";
import { SessionLineageStore } from "../session-lineage.js";

function makeWorktree(): string {
	const wt = mkdtempSync(join(tmpdir(), "discuss-wt-"));
	mkdirSync(wt, { recursive: true });
	return wt;
}

function emptyRunResult(overrides: Partial<RunResult> = {}): RunResult {
	return {
		exitCode: 0,
		timedOut: false,
		signal: null,
		sessionId: null,
		resultText: "",
		stderr: "",
		usage: {
			costUsd: null,
			turns: null,
			durationMs: null,
			inputTokens: null,
			outputTokens: null,
		},
		...overrides,
	};
}

describe("grok discuss mode argv", () => {
	it("discuss mode omits --always-approve", () => {
		const args = grokAdapter.buildArgs({
			prompt: "p",
			model: "grok-4.5",
			promptFilePath: "/tmp/p.grok.txt",
			mode: "discuss",
		});
		expect(args).not.toContain("--always-approve");
	});

	it("agent mode still has --always-approve", () => {
		const args = grokAdapter.buildArgs({
			prompt: "p",
			model: "grok-4.5",
			promptFilePath: "/tmp/p.grok.txt",
			mode: "agent",
		});
		expect(args).toContain("--always-approve");
	});

	it("default (no mode) keeps --always-approve for agent back-compat", () => {
		const args = grokAdapter.buildArgs({
			prompt: "p",
			model: "grok-4.5",
			promptFilePath: "/tmp/p.grok.txt",
		});
		expect(args).toContain("--always-approve");
	});
});

describe("runDiscussTurn", () => {
	it("appends user+assistant transcript, records lineage tip on new session", async () => {
		const discussDir = mkdtempSync(join(tmpdir(), "discuss-"));
		const lineagePath = join(
			mkdtempSync(join(tmpdir(), "lineage-")),
			"session-lineage.json",
		);
		const wt = makeWorktree();
		const store = new DiscussStore(discussDir);
		const lineage = new SessionLineageStore(lineagePath);
		const meta = store.ensure(wt, "grok");
		const turnId = "turn-1";

		const executeRun = vi.fn(
			async (_adapter: ProviderAdapter, opts: ExecuteRunOptions) => {
				// Simulate provider writing the turn-local transcript body.
				writeFileSync(opts.transcriptPath, "Assistant says hello.");
				expect(opts.systemPrompt).toBe("SYS");
				expect(opts.mode).toBe("discuss");
				expect(opts.resumeSessionId).toBeUndefined();
				// Turn-local paths live under turns/<turnId>/
				expect(opts.eventsPath).toContain(join("turns", turnId));
				expect(opts.transcriptPath).toContain(join("turns", turnId));
				return emptyRunResult({ sessionId: "prov-sess-1", exitCode: 0 });
			},
		);

		const result = await runDiscussTurn({
			store,
			lineage,
			sessionId: meta.sessionId,
			turnId,
			prompt: "What does this do?",
			systemPrompt: "SYS",
			model: "grok-4.5",
			provider: "grok",
			cwd: wt,
			timeoutMs: 5000,
			redact: (s) => s,
			executeRun,
		});

		expect(result.exitCode).toBe(0);
		expect(result.sessionId).toBe("prov-sess-1");
		expect(executeRun).toHaveBeenCalledOnce();

		const sessionTranscript = readFileSync(
			store.transcriptPath(meta.sessionId),
			"utf-8",
		);
		expect(sessionTranscript).toContain("### User\n\nWhat does this do?");
		expect(sessionTranscript).toContain("### Assistant\n\n");
		expect(sessionTranscript).toContain("Assistant says hello.");

		// First turn: no prior root → set lineageRoot to provider session id.
		expect(store.get(meta.sessionId)?.lineageRoot).toBe("prov-sess-1");
		expect(lineage.providerOf("prov-sess-1")).toBe("grok");
		// No resume → no fork; tip of root is itself.
		expect(lineage.tip("prov-sess-1")).toBe("prov-sess-1");
	});

	it("resumes lineage tip and records fork on subsequent turns", async () => {
		const discussDir = mkdtempSync(join(tmpdir(), "discuss-"));
		const lineagePath = join(
			mkdtempSync(join(tmpdir(), "lineage-")),
			"session-lineage.json",
		);
		const wt = makeWorktree();
		const store = new DiscussStore(discussDir);
		const lineage = new SessionLineageStore(lineagePath);
		const meta = store.ensure(wt, "claude");
		store.setLineageRoot(meta.sessionId, "prov-root");
		lineage.recordProvider("prov-root", "claude");
		// Prior fork so tip is ahead of root.
		lineage.recordFork("prov-root", "prov-child");

		const executeRun = vi.fn(
			async (_adapter: ProviderAdapter, opts: ExecuteRunOptions) => {
				writeFileSync(opts.transcriptPath, "Follow-up answer.");
				expect(opts.resumeSessionId).toBe("prov-child"); // tip, not root
				return emptyRunResult({ sessionId: "prov-grand", exitCode: 0 });
			},
		);

		const result = await runDiscussTurn({
			store,
			lineage,
			sessionId: meta.sessionId,
			turnId: "turn-2",
			prompt: "And this?",
			systemPrompt: "SYS",
			model: "claude-opus-4-8",
			provider: "claude",
			cwd: wt,
			timeoutMs: 5000,
			redact: (s) => s,
			executeRun,
		});

		expect(result.sessionId).toBe("prov-grand");
		// Root stays; tip advances via fork chain.
		expect(store.get(meta.sessionId)?.lineageRoot).toBe("prov-root");
		expect(lineage.tip("prov-root")).toBe("prov-grand");
		expect(lineage.providerOf("prov-grand")).toBe("claude");

		const sessionTranscript = readFileSync(
			store.transcriptPath(meta.sessionId),
			"utf-8",
		);
		expect(sessionTranscript).toContain("### User\n\nAnd this?");
		expect(sessionTranscript).toContain("Follow-up answer.");
	});

	it("returns error when discuss session is missing", async () => {
		const discussDir = mkdtempSync(join(tmpdir(), "discuss-"));
		const lineagePath = join(
			mkdtempSync(join(tmpdir(), "lineage-")),
			"session-lineage.json",
		);
		const store = new DiscussStore(discussDir);
		const lineage = new SessionLineageStore(lineagePath);
		const executeRun = vi.fn();

		const result = await runDiscussTurn({
			store,
			lineage,
			sessionId: "does-not-exist",
			turnId: "t",
			prompt: "p",
			systemPrompt: "s",
			model: "m",
			provider: "grok",
			cwd: makeWorktree(),
			timeoutMs: 1000,
			redact: (s) => s,
			executeRun,
		});

		expect(result.exitCode).not.toBe(0);
		expect(result.sessionId).toBeNull();
		expect(result.error).toMatch(/not found|missing/i);
		expect(executeRun).not.toHaveBeenCalled();
	});

	it("returns error for unknown provider", async () => {
		const discussDir = mkdtempSync(join(tmpdir(), "discuss-"));
		const lineagePath = join(
			mkdtempSync(join(tmpdir(), "lineage-")),
			"session-lineage.json",
		);
		const wt = makeWorktree();
		const store = new DiscussStore(discussDir);
		const lineage = new SessionLineageStore(lineagePath);
		const meta = store.ensure(wt, "unknown-provider");
		const executeRun = vi.fn();

		const result = await runDiscussTurn({
			store,
			lineage,
			sessionId: meta.sessionId,
			turnId: "t",
			prompt: "p",
			systemPrompt: "s",
			model: "m",
			provider: "unknown-provider",
			cwd: wt,
			timeoutMs: 1000,
			redact: (s) => s,
			executeRun,
		});

		expect(result.exitCode).not.toBe(0);
		expect(result.sessionId).toBeNull();
		expect(result.error).toMatch(/unknown provider|no adapter/i);
		expect(executeRun).not.toHaveBeenCalled();
	});
});
