import type {
	BuildArgsInput,
	ParsedEvent,
	ProviderAdapter,
	UnavailableInput,
} from "./types.js";

// Schema live-verified against grok 0.2.101 on 2026-07-16: the streaming-json
// stream carries token-level `thought`/`text` deltas and a single terminal
// `end` event; it emits NO tool-call events (a tool-using run showed only
// thought/text/end, with num_turns proving the tool ran) and no full-text
// result, so the runner accumulates the text deltas into the final result.

/** Matches quota/subscription failures from the grok CLI (rate limit, out of
 * credits, unauthorized, etc.) — these mean the provider is temporarily or
 * structurally unusable, not that the run itself failed on its merits. The
 * error wording is still unverified against a live failure, so this stays
 * permissive: a false negative just falls back to the generic `exit code N`
 * reason, never a crash. */
const UNAVAILABLE_RE =
	/\b401\b|\b429\b|unauthorized|unauthenticated|invalid api key|rate limit|quota|subscription|too many requests/i;

export const grokAdapter: ProviderAdapter = {
	name: "grok",
	defaultBin: "grok",
	supportsResume: true,
	promptFileSuffix: ".grok.txt",
	buildArgs({
		model,
		resumeSessionId,
		systemPrompt,
		promptFilePath,
		mode,
	}: BuildArgsInput): string[] {
		if (!promptFilePath) {
			throw new Error(
				"grokAdapter.buildArgs requires promptFilePath — the runner always " +
					"provides one for a prompt-file adapter (promptFileSuffix !== null); " +
					"a missing path here means the runner/adapter wiring is broken.",
			);
		}
		// --always-approve: agent-mode queohoh runs are autonomous, so tool
		// executions must auto-approve. Discuss turns are read-only review, so
		// we omit it (default/`agent` keeps it for back-compat). --resume reuses
		// the same session id (no --fork-session), so finalizeRun's recordFork
		// no-ops. --rules appends to the system prompt.
		const approve = mode === "discuss" ? [] : ["--always-approve"];
		return [
			"--prompt-file",
			promptFilePath,
			"--output-format",
			"streaming-json",
			...approve,
			"--model",
			model,
			...(resumeSessionId ? ["--resume", resumeSessionId] : []),
			...(systemPrompt ? ["--rules", systemPrompt] : []),
		];
	},
	parseEvent(event: Record<string, unknown>): ParsedEvent {
		const out: ParsedEvent = {};
		const type = event.type as string | undefined;

		if (type === "thought" && typeof event.data === "string") {
			out.thinkingDelta = event.data;
		}
		if (type === "text" && typeof event.data === "string") {
			out.textDelta = event.data;
		}

		if (type === "end") {
			if (typeof event.sessionId === "string") out.sessionId = event.sessionId;
			const usage = event.usage as Record<string, unknown> | undefined;
			// No cost or duration field in the stream (both stay null, best-effort
			// per the ParsedEvent contract); text is accumulated from the deltas by
			// the runner, so the result text here is empty. Token counts DO live in
			// `usage` even though cost doesn't — grok reports usage with no priced
			// cost, so tokens are the only usage signal this adapter can surface.
			out.result = {
				text: "",
				costUsd: null,
				turns: typeof event.num_turns === "number" ? event.num_turns : null,
				durationMs: null,
				inputTokens:
					typeof usage?.input_tokens === "number" ? usage.input_tokens : null,
				outputTokens:
					typeof usage?.output_tokens === "number" ? usage.output_tokens : null,
			};
		}

		return out;
	},
	classifyUnavailable({ stderr, resultText }: UnavailableInput): string | null {
		if (UNAVAILABLE_RE.test(stderr) || UNAVAILABLE_RE.test(resultText)) {
			return "provider unavailable";
		}
		return null;
	},
};
