import type { DiscussAnchor } from "./types.js";

/**
 * Read-only system constraints for discuss turns.
 * Passed via adapter `--append-system-prompt` / `--rules` so the model
 * stays advisory; scheduling fixes and PR posts goes through qoo, not here.
 */
export const DISCUSS_SYSTEM_PROMPT = `You are a code-review assistant in a read-only discuss session.
You may read and reason about code, explain behavior, and draft text (including draft PR comments).
Do not edit files, run mutating shell commands, commit, push, or post to GitHub.
The human will schedule fixes and PR comments through a separate queue (qoo).`;

/**
 * Build the turn prompt body (and separate system string) for one discuss message.
 * `fullPrompt` is what goes to the CLI as the turn prompt body; `systemPrompt`
 * is the read-only system string for the adapter.
 */
export function buildDiscussTurnPrompt(input: {
	userPrompt: string;
	anchor?: DiscussAnchor;
}): { systemPrompt: string; userMessage: string; fullPrompt: string } {
	const parts: string[] = [];
	if (input.anchor) {
		parts.push(
			`## Context anchor\n- file: \`${input.anchor.path}\`\n- side: ${input.anchor.side}\n- line: ${input.anchor.line}`,
		);
		if (input.anchor.snippet?.trim()) {
			parts.push("```\n" + input.anchor.snippet.trim() + "\n```");
		}
	}
	parts.push("## Reviewer question\n" + input.userPrompt.trim());
	const userMessage = parts.join("\n\n");
	return {
		systemPrompt: DISCUSS_SYSTEM_PROMPT,
		userMessage,
		fullPrompt: userMessage,
	};
}
