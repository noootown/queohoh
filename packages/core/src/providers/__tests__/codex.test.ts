import { describe, expect, it } from "vitest";
import { codexAdapter } from "../codex.js";

describe("codexAdapter", () => {
	it("builds `exec --json --model <m>` argv with the prompt last", () => {
		expect(codexAdapter.buildArgs({ prompt: "do it", model: "gpt-5.6-terra" })).toEqual([
			"exec", "--json", "--skip-git-repo-check", "--model", "gpt-5.6-terra", "do it",
		]);
	});
	it("adds `resume <id>` when resuming", () => {
		expect(codexAdapter.buildArgs({ prompt: "p", model: "m", resumeSessionId: "t1" })).toContain("resume");
	});
	it("parses the thread id as sessionId", () => {
		expect(codexAdapter.parseEvent({ type: "thread.started", thread_id: "th_1" }).sessionId).toBe("th_1");
	});
	it("parses assistant message into transcriptMd", () => {
		const p = codexAdapter.parseEvent({ type: "item.completed", item: { type: "assistant_message", text: "hi" } });
		expect(p.transcriptMd).toContain("hi");
	});
	it("parses usage on turn.completed", () => {
		const p = codexAdapter.parseEvent({ type: "turn.completed", usage: { input_tokens: 1, output_tokens: 2 } });
		expect(p.result?.turns).toBe(1);
		// costUsd stays null (codex usage isn't priced here) but the token counts
		// themselves still come through — unpriced ≠ uncounted.
		expect(p.result?.costUsd).toBeNull();
		expect(p.result?.inputTokens).toBe(1);
		expect(p.result?.outputTokens).toBe(2);
	});

	it("leaves inputTokens/outputTokens null when turn.completed carries no usage object", () => {
		const p = codexAdapter.parseEvent({ type: "turn.completed" });
		expect(p.result?.inputTokens).toBeNull();
		expect(p.result?.outputTokens).toBeNull();
	});
	it("classifies auth/quota failures as unavailable", () => {
		expect(codexAdapter.classifyUnavailable({ exitCode: 1, stderr: "401 Unauthorized", resultText: "" })).toBe("provider unavailable");
		expect(codexAdapter.classifyUnavailable({ exitCode: 1, stderr: "", resultText: "rate limit exceeded" })).toBe("provider unavailable");
		expect(codexAdapter.classifyUnavailable({ exitCode: 3, stderr: "syntax error", resultText: "" })).toBeNull();
	});
});
