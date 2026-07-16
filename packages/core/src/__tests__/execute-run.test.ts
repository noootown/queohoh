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
		expect(res.usage).toEqual({ costUsd: 0.01, turns: 1, durationMs: 42 });
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

	// grok's streaming-json carries no full-text result: the runner accumulates
	// `thought`/`text` deltas and flushes one transcript section on the `end`
	// event. Driven through the real grokAdapter with a fake bin emitting the
	// live-verified event shape.
	it("accumulates grok token deltas into the result text and flushes one transcript section", async () => {
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
		expect(res.usage).toEqual({ costUsd: null, turns: 1, durationMs: null });
		// One flush, mirroring formatEventToMarkdown's shape.
		expect(readFileSync(transcriptPath, "utf-8")).toBe(
			"### Thinking\nAnswer with pong.\n\npong\n\n",
		);
	});
});
