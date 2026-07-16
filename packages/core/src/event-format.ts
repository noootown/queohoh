export function formatEventToMarkdown(
	event: Record<string, unknown>,
): string | null {
	if ((event.type as string) !== "assistant") return null;
	const msg = event.message as Record<string, unknown> | undefined;
	const content = msg?.content as Array<Record<string, unknown>> | undefined;
	if (!content) return null;

	const parts: string[] = [];
	for (const block of content) {
		if (block.type === "thinking" && block.thinking) {
			parts.push("### Thinking");
			parts.push(String(block.thinking));
			parts.push("");
		}
		if (block.type === "text" && block.text) {
			parts.push(String(block.text));
			parts.push("");
		}
		if (block.type === "tool_use") {
			const name = block.name as string;
			const input = (block.input as Record<string, unknown>) ?? {};
			parts.push(`### Tool: ${name}`);
			const filePath = input.file_path as string | undefined;
			if (name === "Bash" && input.command) {
				parts.push("```bash");
				parts.push(String(input.command));
				parts.push("```");
			} else if (["Edit", "Read", "Write"].includes(name) && filePath) {
				parts.push(`File: \`${filePath}\``);
			} else if (name === "Grep" && input.pattern) {
				parts.push(`Pattern: \`${input.pattern}\``);
			} else {
				parts.push("```json");
				parts.push(JSON.stringify(input, null, 2).slice(0, 500));
				parts.push("```");
			}
			parts.push("");
		}
	}
	return parts.length > 0 ? parts.join("\n") : null;
}
