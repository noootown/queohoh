import { describe, expect, it } from "vitest";
import { claudeAdapter } from "../claude.js";

describe("claudeAdapter", () => {
	it("builds the exact legacy argv (inline prompt, model, resume)", () => {
		expect(
			claudeAdapter.buildArgs({ prompt: "hi", model: "claude-opus-4-8", resumeSessionId: "s1" }),
		).toEqual([
			"-p", "hi",
			"--output-format", "stream-json",
			"--verbose",
			"--model", "claude-opus-4-8",
			"--resume", "s1",
		]);
	});

	it("omits --resume when no session and appends extraArgs after", () => {
		expect(claudeAdapter.buildArgs({ prompt: "hi", model: "m", extraArgs: ["--foo"] })).toEqual([
			"-p", "hi", "--output-format", "stream-json", "--verbose", "--model", "m", "--foo",
		]);
	});

	it("appends the system prompt via --append-system-prompt before extraArgs", () => {
		expect(
			claudeAdapter.buildArgs({ prompt: "hi", model: "m", systemPrompt: "SP" }),
		).toContain("--append-system-prompt");
	});

	it("parses session_id and result usage from a result event", () => {
		expect(
			claudeAdapter.parseEvent({
				type: "result", session_id: "abc", result: "done",
				total_cost_usd: 0.5, num_turns: 3, duration_ms: 2000,
			}),
		).toEqual({
			sessionId: "abc",
			result: {
				text: "done", costUsd: 0.5, turns: 3, durationMs: 2000,
				inputTokens: null, outputTokens: null,
			},
		});
	});

	it("parses input/output token counts from the result event's usage object", () => {
		const p = claudeAdapter.parseEvent({
			type: "result", session_id: "abc", result: "done",
			usage: { input_tokens: 111_234, output_tokens: 4_567, cache_read_input_tokens: 128 },
		});
		expect(p.result?.inputTokens).toBe(111_234);
		expect(p.result?.outputTokens).toBe(4_567);
	});

	it("renders assistant text to markdown via transcriptMd", () => {
		const p = claudeAdapter.parseEvent({
			type: "assistant", message: { content: [{ type: "text", text: "hello" }] },
		});
		expect(p.transcriptMd).toContain("hello");
	});

	it("classifies session-limit, out-of-budget, and passes real failures through", () => {
		expect(
			claudeAdapter.classifyUnavailable({ exitCode: 1, stderr: "", resultText: "You've hit your session limit" }),
		).toBe("session limit");
		expect(
			claudeAdapter.classifyUnavailable({ exitCode: 1, stderr: "", resultText: "Your credit balance is too low" }),
		).toBe("out of budget");
		expect(
			claudeAdapter.classifyUnavailable({ exitCode: 2, stderr: "boom", resultText: "TypeError" }),
		).toBeNull();
	});

	it("supports resume and uses no prompt file", () => {
		expect(claudeAdapter.supportsResume).toBe(true);
		expect(claudeAdapter.promptFileSuffix).toBeNull();
	});
});
