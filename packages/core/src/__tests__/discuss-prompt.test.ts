import { describe, expect, it } from "vitest";
import { buildDiscussTurnPrompt, DISCUSS_SYSTEM_PROMPT } from "../discuss/prompt.js";

describe("buildDiscussTurnPrompt", () => {
	it("includes read-only system constraints", () => {
		expect(DISCUSS_SYSTEM_PROMPT).toMatch(/do not edit/i);
		expect(DISCUSS_SYSTEM_PROMPT).toMatch(/github/i);
	});

	it("embeds anchor and user prompt", () => {
		const { fullPrompt } = buildDiscussTurnPrompt({
			userPrompt: "what does this do?",
			anchor: {
				path: "src/a.rs",
				side: "new",
				line: 42,
				snippet: "let x = 1;",
			},
		});
		expect(fullPrompt).toContain("src/a.rs");
		expect(fullPrompt).toContain("42");
		expect(fullPrompt).toContain("let x = 1;");
		expect(fullPrompt).toContain("what does this do?");
	});
});
