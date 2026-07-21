import {
	chmodSync,
	existsSync,
	mkdtempSync,
	readFileSync,
	writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { grokAdapter, type ProviderAdapter } from "../providers/index.js";
import { executeRun } from "../runner.js";

function fakeBin(dir: string, body: string): string {
	const p = join(dir, "fake-cli.js");
	writeFileSync(p, `#!/usr/bin/env node\n${body}`);
	chmodSync(p, 0o755);
	return p;
}

function paths() {
	const dir = mkdtempSync(join(tmpdir(), "qoo-run-"));
	return {
		dir,
		eventsPath: join(dir, "e.jsonl"),
		transcriptPath: join(dir, "t.md"),
	};
}

/** Poll `check` until it returns true or `timeoutMs` elapses. Used instead of
 * a fixed sleep to observe transcript.md mid-run without guessing exactly how
 * long process spawn + the fake bin's own delay will take. */
async function waitFor(
	check: () => boolean,
	timeoutMs: number,
	intervalMs = 15,
): Promise<boolean> {
	const start = Date.now();
	while (Date.now() - start < timeoutMs) {
		if (check()) return true;
		await new Promise((r) => setTimeout(r, intervalMs));
	}
	return check();
}

// Contract under test: argv comes from `adapter.buildArgs(...)` first, then
// `opts.claudeArgs` trailing — identical to today's claude ordering. The fake
// CLI echoes its full argv back so the ordering (and the adapter-produced
// usage numbers) can be asserted without special-casing this adapter in
// runner.ts.
const echoAdapter: ProviderAdapter = {
	name: "echo",
	defaultBin: "node",
	supportsResume: false,
	promptFileSuffix: null,
	buildArgs: ({ prompt }) => [prompt, "from-build-args"],
	parseEvent: (e) =>
		e.type === "result"
			? {
					sessionId: "sess-echo",
					result: {
						text: String(e.text),
						costUsd: 0.01,
						turns: 1,
						durationMs: 42,
						inputTokens: 100,
						outputTokens: 20,
					},
				}
			: {},
	classifyUnavailable: () => null,
};

const promptFileAdapter: ProviderAdapter = {
	name: "prompt-file-echo",
	defaultBin: "node",
	supportsResume: false,
	promptFileSuffix: ".txt",
	buildArgs: ({ promptFilePath }) => [promptFilePath ?? ""],
	parseEvent: (e) =>
		e.type === "result"
			? {
					result: {
						text: String(e.text),
						costUsd: null,
						turns: null,
						durationMs: null,
						inputTokens: null,
						outputTokens: null,
					},
				}
			: {},
	classifyUnavailable: () => null,
};

describe("executeRun", () => {
	it("dispatches argv from adapter.buildArgs then opts.claudeArgs trailing, and captures the parsed result/usage", async () => {
		const { dir, eventsPath, transcriptPath } = paths();
		const script = fakeBin(
			dir,
			`const argv = process.argv.slice(2);
process.stdout.write(JSON.stringify({ type: "result", text: argv.join(",") }) + "\\n");
`,
		);
		const res = await executeRun(echoAdapter, {
			prompt: "hello",
			model: "m",
			cwd: dir,
			timeoutMs: 5000,
			claudeBin: script,
			claudeArgs: ["from-claude-args"],
			eventsPath,
			transcriptPath,
			redact: (s) => s,
		});
		expect(res.exitCode).toBe(0);
		expect(res.resultText).toBe("hello,from-build-args,from-claude-args");
		expect(res.sessionId).toBe("sess-echo");
		expect(res.usage).toEqual({
			costUsd: 0.01,
			turns: 1,
			durationMs: 42,
			inputTokens: 100,
			outputTokens: 20,
		});
	});

	it("writes the prompt to a temp file for prompt-file adapters, threads its path through buildArgs, and removes it after the run settles", async () => {
		const { dir, eventsPath, transcriptPath } = paths();
		const script = fakeBin(
			dir,
			`const fs = require("node:fs");
const promptFilePath = process.argv[2];
const contents = fs.readFileSync(promptFilePath, "utf-8");
process.stdout.write(JSON.stringify({ type: "result", text: contents }) + "\\n");
`,
		);
		const res = await executeRun(promptFileAdapter, {
			prompt: "the prompt body",
			model: "m",
			cwd: dir,
			timeoutMs: 5000,
			claudeBin: script,
			eventsPath,
			transcriptPath,
			redact: (s) => s,
		});
		expect(res.exitCode).toBe(0);
		expect(res.resultText).toBe("the prompt body");
		expect(existsSync(join(dir, "prompt.txt"))).toBe(false);
	});

	it("inserts a space when grok thought deltas join sentence.Next without one", async () => {
		// Live bug: grok emits `"comments."` then `"Python"` as separate thought
		// tokens; naive concat produced `comments.Python` in the TUI transcript.
		const { dir, eventsPath, transcriptPath } = paths();
		const script = fakeBin(
			dir,
			`const emit = (o) => process.stdout.write(JSON.stringify(o) + "\\n");
emit({ type: "thought", data: "reviews, and comments." });
emit({ type: "thought", data: "Python f-string issue." });
emit({ type: "thought", data: "The first query failed." });
emit({ type: "text", data: "done." });
emit({ type: "text", data: "Really." });
emit({ type: "end", stopReason: "EndTurn", sessionId: "gsess-glue", num_turns: 1 });
`,
		);
		const res = await executeRun(grokAdapter, {
			prompt: "ping",
			model: "grok-4.5",
			cwd: dir,
			timeoutMs: 5000,
			claudeBin: script,
			eventsPath,
			transcriptPath,
			redact: (s) => s,
		});
		expect(res.exitCode).toBe(0);
		const md = readFileSync(transcriptPath, "utf-8");
		expect(md).toContain(
			"reviews, and comments. Python f-string issue. The first query failed.",
		);
		expect(md).toContain("done. Really.");
		expect(md).not.toMatch(/comments\.Python/);
		expect(md).not.toMatch(/issue\.The/);
		expect(md).not.toMatch(/done\.Really/);
		// resultText is the text section only (glued the same way)
		expect(res.resultText).toBe("done. Really.");
	});

	// grok's streaming-json carries no full-text result: the runner accumulates
	// `thought`/`text` deltas into resultText, AND streams them into
	// transcript.md line-by-line as they arrive (not just once at the end).
	// Driven through the real grokAdapter with a fake bin emitting the
	// live-verified event shape.
	it("accumulates grok token deltas into the result text and streams the same content into transcript.md", async () => {
		const { dir, eventsPath, transcriptPath } = paths();
		const script = fakeBin(
			dir,
			`const emit = (o) => process.stdout.write(JSON.stringify(o) + "\\n");
emit({ type: "thought", data: "Answer " });
emit({ type: "thought", data: "with pong." });
emit({ type: "text", data: "po" });
emit({ type: "text", data: "ng" });
emit({ type: "end", stopReason: "EndTurn", sessionId: "gsess-1", num_turns: 1, usage: { output_tokens: 2 } });
`,
		);
		const res = await executeRun(grokAdapter, {
			prompt: "ping",
			model: "grok-4.5",
			cwd: dir,
			timeoutMs: 5000,
			claudeBin: script,
			eventsPath,
			transcriptPath,
			redact: (s) => s,
		});
		expect(res.exitCode).toBe(0);
		expect(res.resultText).toBe("pong");
		expect(res.sessionId).toBe("gsess-1");
		expect(res.usage).toEqual({
			costUsd: null,
			turns: 1,
			durationMs: null,
			// The fixture's `end` event carries `usage.output_tokens` with no
			// `input_tokens` — proves the two sides are read independently, not
			// as an all-or-nothing pair.
			inputTokens: null,
			outputTokens: 2,
		});
		// This particular delta shape (no embedded newlines until the section
		// switch/end) happens to land byte-identical to the old one-shot flush,
		// mirroring formatEventToMarkdown's shape.
		expect(readFileSync(transcriptPath, "utf-8")).toBe(
			"### Thinking\nAnswer with pong.\n\npong\n\n",
		);
	});

	// Live-view proof: a complete line (delta containing "\n") must land in
	// transcript.md WHILE the run is still going, not only after the terminal
	// `end` event. The fake bin pauses before the final text/end so the test
	// can observe the file mid-run; a `finished` flag (rather than a guessed
	// sleep duration) proves the observation really happened before the run
	// settled, independent of process-spawn jitter.
	it("streams a complete line into transcript.md before the run ends, then flushes the remainder at the end", async () => {
		const { dir, eventsPath, transcriptPath } = paths();
		const script = fakeBin(
			dir,
			`const emit = (o) => process.stdout.write(JSON.stringify(o) + "\\n");
emit({ type: "thought", data: "First thought.\\n" });
setTimeout(() => {
  emit({ type: "text", data: "final answer" });
  emit({ type: "end", stopReason: "EndTurn", sessionId: "gsess-live", num_turns: 1 });
}, 400);
`,
		);
		let finished = false;
		const runPromise = executeRun(grokAdapter, {
			prompt: "ping",
			model: "grok-4.5",
			cwd: dir,
			timeoutMs: 5000,
			claudeBin: script,
			eventsPath,
			transcriptPath,
			redact: (s) => s,
		});
		runPromise.then(() => {
			finished = true;
		});

		const landed = await waitFor(
			() => readFileSync(transcriptPath, "utf-8") !== "",
			350,
		);
		expect(landed).toBe(true);
		expect(finished).toBe(false); // proves this was observed mid-run, not after
		const midRun = readFileSync(transcriptPath, "utf-8");
		expect(midRun).toBe("### Thinking\nFirst thought.\n");

		const res = await runPromise;
		expect(res.exitCode).toBe(0);
		expect(res.resultText).toBe("final answer");

		const final = readFileSync(transcriptPath, "utf-8");
		// The mid-run content is untouched, and the trailing partial line (no
		// newline in the source delta) plus the closing blank line land at `end`.
		expect(final.startsWith(midRun)).toBe(true);
		expect(final).toContain("final answer");
	});

	// Redaction safety: a secret split across two token deltas but landing
	// within the SAME line must never appear unredacted in transcript.md, even
	// transiently mid-run — proving the line-buffered flush never redacts a
	// still-open partial line (it simply never flushes it until the line
	// completes).
	it("redacts a secret that arrives split across two deltas within one line", async () => {
		const { dir, eventsPath, transcriptPath } = paths();
		const script = fakeBin(
			dir,
			`const emit = (o) => process.stdout.write(JSON.stringify(o) + "\\n");
emit({ type: "text", data: "before SECRET_" });
setTimeout(() => {
  emit({ type: "text", data: "TOKEN_ABC after\\n" });
  emit({ type: "end", stopReason: "EndTurn", sessionId: "gsess-sec", num_turns: 1 });
}, 400);
`,
		);
		const redact = (s: string) =>
			s.replaceAll("SECRET_TOKEN_ABC", "[REDACTED:X]");
		let finished = false;
		const runPromise = executeRun(grokAdapter, {
			prompt: "ping",
			model: "grok-4.5",
			cwd: dir,
			timeoutMs: 5000,
			claudeBin: script,
			eventsPath,
			transcriptPath,
			redact,
		});
		runPromise.then(() => {
			finished = true;
		});

		// Mid-run: the first half of the secret has arrived but the line isn't
		// complete yet, so nothing should have been flushed — neither the raw
		// secret nor a premature (and therefore impossible) redaction. Poll for
		// a while to prove this holds throughout the pre-end window, not just at
		// one sampled instant.
		await waitFor(() => finished, 250);
		expect(finished).toBe(false);
		expect(readFileSync(transcriptPath, "utf-8")).toBe("");

		await runPromise;
		const final = readFileSync(transcriptPath, "utf-8");
		expect(final).toContain("[REDACTED:X]");
		expect(final).not.toContain("SECRET_TOKEN_ABC");
	});
});
