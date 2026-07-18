#!/usr/bin/env node
// Fake `claude` for runner tests. Behavior selected by FAKE_CLAUDE_MODE env var.
import { writeFileSync } from "node:fs";

const mode = process.env.FAKE_CLAUDE_MODE ?? "ok";

// Capture spawn argv when requested, so tests can assert flag construction.
const argvOut = process.env.FAKE_CLAUDE_ARGV_OUT;
if (argvOut) {
	writeFileSync(argvOut, JSON.stringify(process.argv.slice(2)));
}

const emit = (obj) => process.stdout.write(`${JSON.stringify(obj)}\n`);

if (mode === "ok") {
	emit({ type: "system", session_id: "sess-123" });
	emit({
		type: "assistant",
		message: {
			content: [
				{ type: "text", text: "Working on it with TOKEN_VALUE_XYZ" },
				{ type: "tool_use", name: "Bash", input: { command: "echo hi" } },
			],
		},
	});
	emit({
		type: "result",
		result: "All done.",
		total_cost_usd: 0.42,
		num_turns: 3,
		duration_ms: 1234,
		usage: { input_tokens: 111234, output_tokens: 4567 },
	});
	process.exit(0);
} else if (mode === "hang") {
	emit({ type: "system", session_id: "sess-hang" });
	// Never exits — runner must SIGTERM the group.
	setInterval(() => {}, 1000);
} else if (mode === "trickle") {
	// Emits a keepalive event on a short fixed cadence and never exits — used to
	// prove the idle timer keeps getting reset by live stream activity, so a
	// kill (if one lands) must come from the ceiling timeout, not idle.
	emit({ type: "system", session_id: "sess-trickle" });
	setInterval(() => {
		emit({ type: "system", session_id: "sess-trickle" });
	}, 100);
} else if (mode === "crash") {
	process.stderr.write("boom\n");
	process.exit(2);
}
